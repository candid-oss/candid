//! Background thread for the one-time conversion of originals stored before data model v1 §9: JPEG screenshots
//! become WebP (decoding and encoding run without the store lock; only the swap takes it), and WAV recording parts
//! are queued for the AAC compaction the workflow thread already runs. Idle once nothing is left; a later check picks
//! up anything an older App version still sends as JPEG.
use crate::router::{lock, SharedStore};
use candid_store::library_conversion::{jpeg_to_webp, read_original};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const BATCH: usize = 64;
/// Between images, so capture and the App stay responsive on a busy machine.
const PAUSE: Duration = Duration::from_millis(5);
const IDLE_CHECK: Duration = Duration::from_secs(30 * 60);

/// Converts one batch on `workers` threads (decode and encode without the store lock; each swap takes it briefly).
/// Returns how many screenshots were replaced and whether more remain.
pub fn screenshot_pass(store: &SharedStore, limit: usize, workers: usize, pause: Duration, stop: &AtomicBool) -> (usize, bool) {
    let Ok(batch) = lock(store).jpeg_screenshot_candidates(limit) else { return (0, false) };
    let full = batch.len() == limit;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            scope.spawn(|| {
                crate::scheduling::utility();
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= batch.len() || stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let shot = &batch[i];
                    // An unreadable or missing JPEG stays as it is; the integrity check reports missing files.
                    if let Ok((webp, w, h)) = read_original(&shot.path).map_err(|e| e.to_string()).and_then(|jpeg| jpeg_to_webp(&jpeg)) {
                        if lock(store).replace_screenshot_with_webp(shot, &webp, w, h).unwrap_or(false) {
                            done.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    if !pause.is_zero() {
                        std::thread::sleep(pause);
                    }
                }
            });
        }
    });
    let done = done.load(Ordering::SeqCst);
    // A batch where nothing could be converted would otherwise be retried forever.
    (done, full && done > 0 && !stop.load(Ordering::SeqCst))
}

/// Half the cores, at least one: fast, while capture and the App keep the rest.
pub fn default_workers() -> usize {
    std::thread::available_parallelism().map(|n| (n.get() / 2).max(1)).unwrap_or(1)
}

pub(crate) fn start(store: SharedStore, stop: Arc<AtomicBool>) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("candid-convert".into()).spawn(move || {
        crate::scheduling::background();
        let _ = lock(&store).queue_legacy_wav_compaction();
        while !stop.load(Ordering::SeqCst) {
            let (_, more) = screenshot_pass(&store, BATCH, default_workers(), PAUSE, &stop);
            if !more {
                // Wait in short slices: a stop's unpark can be consumed by the scoped workers' own parking.
                let until = std::time::Instant::now() + IDLE_CHECK;
                while !stop.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                    std::thread::park_timeout(Duration::from_secs(1));
                }
            }
        }
    })
}
