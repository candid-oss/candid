//! After processing, new recording parts are re-encoded from WAV to AAC with the system encoder (data model v1
//! §9.4: `.m4a`, mono, 48 kbps, the part's own sample rate). Encoding and verification run without the store lock;
//! the store swaps the original only if the part is still idle, and deletes the WAV.
use crate::router::{lock, SharedStore};
use std::time::Duration;

/// Quiet time after the last import or job before a part counts as processed.
pub const GRACE: Duration = Duration::from_secs(10 * 60);
const PER_PASS: usize = 8;

/// One pass at a time. When it runs is decided by the background loop (`background.rs`: once at start, then once
/// `GRACE` after media or jobs last changed, on the store clock); this only runs a pass and says when parts left
/// waiting out their grace period become due, so the loop can sleep until then.
pub(crate) struct Compactor {
    /// Whether some WAV may still be compacted. Until the first pass we do not know, so it starts true. When it is
    /// false only a new audio original can bring work, so the loop watches the media tables alone, not every job.
    pub waiting: bool,
}

impl Default for Compactor {
    fn default() -> Self {
        Compactor { waiting: true }
    }
}

impl Compactor {
    /// Runs one pass; returns the store-clock time a further pass is due, if any.
    pub fn run(&mut self, store: &SharedStore) -> Option<i64> {
        let done = pass(store, GRACE, PER_PASS);
        let s = lock(store);
        let (next, waiting) = s.media_compaction_next_ms(GRACE.as_millis() as i64).unwrap_or((None, true));
        self.waiting = waiting;
        if done >= PER_PASS {
            return Some(s.clock.now_ms());
        }
        next
    }
}

/// One pass; returns how many originals were compacted. Errors leave the WAV in place for a later pass.
pub fn pass(store: &SharedStore, grace: Duration, limit: usize) -> usize {
    let candidates = match lock(store).media_compaction_candidates(grace.as_millis() as i64, limit) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let mut done = 0;
    for candidate in candidates {
        match encode(&candidate.path) {
            Ok((dir, encoded)) => {
                let verified = verify(&candidate.path, &encoded, candidate.sample_count, candidate.sample_rate);
                if verified.is_ok() && lock(store).media_compact_original(&candidate, &encoded).unwrap_or(false) {
                    done += 1;
                }
                let _ = std::fs::remove_dir_all(dir);
            }
            Err(_) => continue,
        }
    }
    done
}

#[cfg(target_os = "macos")]
fn encode(source: &std::path::Path) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = std::env::temp_dir().join(format!("candid-compact-{}", candid_store::uuid_v4()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    let output = dir.join("part.m4a");
    let status = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "m4af", "-d", "aac", "-b", &candid_store::media_compaction::COMPACT_AUDIO_BITRATE.to_string(), "-c", "1"])
        .arg(source)
        .arg(&output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !status.is_ok_and(|s| s.success()) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("audio_compaction_failed".into());
    }
    Ok((dir, output))
}

#[cfg(not(target_os = "macos"))]
fn encode(_: &std::path::Path) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    Err("audio_compaction_unavailable".into())
}

/// The AAC must decode to exactly the WAV's frames at the WAV's rate (recording parts carry sample coordinates).
fn verify(wav: &std::path::Path, aac: &std::path::Path, count: Option<u64>, rate: Option<u32>) -> Result<(), String> {
    let source = crate::audio_chunks::AudioChunks::open(wav)?;
    if count.is_some_and(|c| c != source.frames) || rate.is_some_and(|r| r != source.rate) {
        return Err("part_bounds_mismatch".into());
    }
    let (frames, encoded_rate) = afinfo(aac)?;
    if frames != source.frames || encoded_rate != source.rate {
        return Err("compacted_frames_mismatch".into());
    }
    Ok(())
}

/// `audio N valid frames + …` and the stream's sample rate, from the system's `afinfo`.
#[cfg(target_os = "macos")]
fn afinfo(path: &std::path::Path) -> Result<(u64, u32), String> {
    let out = std::process::Command::new("/usr/bin/afinfo").arg(path).env("LC_ALL", "C").output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("afinfo_failed".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let frames = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("audio ").and_then(|r| r.split_once(" valid frames")).and_then(|(n, _)| n.trim().parse().ok()));
    let rate = text.lines().find_map(|l| {
        let l = l.trim().strip_prefix("Data format:")?;
        let hz = l.split(',').find(|p| p.trim().ends_with("Hz"))?;
        hz.trim().trim_end_matches("Hz").trim().parse().ok()
    });
    match (frames, rate) {
        (Some(f), Some(r)) => Ok((f, r)),
        _ => Err("afinfo_unreadable".into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn afinfo(_: &std::path::Path) -> Result<(u64, u32), String> {
    Err("audio_compaction_unavailable".into())
}
