//! Runs the one-time conversion on a COPY of a library (path as the first argument) and reports sizes and timing.
//! Never point it at a live library.
use candid_service::library_conversion::{default_workers, screenshot_pass};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("library copy path"));
    let store = candid_store::TimelineStore::open(&root, "conversion-trial", true, candid_store::Clock::default()).unwrap();
    let before = store.jpeg_screenshot_backlog().unwrap();
    let shared = Arc::new(Mutex::new(store));
    let stop = AtomicBool::new(false);
    let start = Instant::now();
    let workers = default_workers();
    let mut total = 0;
    loop {
        let (n, more) = screenshot_pass(&shared, 256, workers, Duration::ZERO, &stop);
        total += n;
        if !more { break; }
    }
    let s = shared.lock().unwrap();
    let wav = s.queue_legacy_wav_compaction().unwrap();
    println!("workers {workers}; converted {total} of {} JPEG ({} MB) in {:.0} s", before.0, before.1 / 1_000_000, start.elapsed().as_secs_f64());
    println!("status {}", s.conversion_status().unwrap());
    println!("wav parts queued {wav}");
    println!("integrity ok {}", s.integrity_check(false).unwrap().ok);
}
