//! One-time conversion of derived JSON originals stored plain before compression at ingest (data model v1 §9.5).
//! Runs on the background worker in short slices: candidates are listed under the store lock, read, verified and
//! compressed without it, and swapped in one transaction per chunk. Resumable (a converted blob is no longer a
//! candidate) and idempotent; a blob that fails (missing or mismatched file) is skipped until the next start.
use crate::router::{lock, SharedStore};
use candid_store::json_blobs::{compress_derived_json, JsonConversionStep};
use std::time::{Duration, Instant};

const CHUNK: usize = 400;
/// Work per tick, so the worker's other duties keep their pace while the conversion runs.
const SLICE: Duration = Duration::from_millis(1000);

#[derive(Default)]
pub(crate) struct Converter {
    after: String,
    finished: bool,
    total: JsonConversionStep,
}

impl Converter {
    pub fn tick(&mut self, store: &SharedStore) {
        if self.finished {
            return;
        }
        let started = Instant::now();
        while started.elapsed() < SLICE {
            match step(store, &self.after, CHUNK) {
                Ok((s, Some(last))) => {
                    self.after = last;
                    self.total.converted += s.converted;
                    self.total.skipped += s.skipped;
                    self.total.bytes_before += s.bytes_before;
                    self.total.bytes_after += s.bytes_after;
                }
                Ok((_, None)) | Err(_) => {
                    self.finished = true;
                    return;
                }
            }
        }
    }
}

/// One chunk after `after`; returns the step and the last hash looked at (None when nothing is left).
pub fn step(store: &SharedStore, after: &str, limit: usize) -> candid_store::Result<(JsonConversionStep, Option<String>)> {
    let (root, candidates) = {
        let s = lock(store);
        (s.root.clone(), s.derived_json_candidates(after, limit)?)
    };
    let Some(last) = candidates.last().map(|c| c.sha256.clone()) else {
        return Ok((JsonConversionStep::default(), None));
    };
    let mut done = Vec::with_capacity(candidates.len());
    let mut failed = 0;
    for c in &candidates {
        match compress_derived_json(&root, c) {
            Ok(d) => done.push(d),
            Err(_) => failed += 1,
        }
    }
    let mut s = lock(store).swap_derived_json(&done)?;
    s.skipped += failed;
    Ok((s, Some(last)))
}

/// Runs the whole conversion now (tests, tools).
pub fn run_all(store: &SharedStore) -> candid_store::Result<JsonConversionStep> {
    let mut total = JsonConversionStep::default();
    let mut after = String::new();
    while let (s, Some(last)) = step(store, &after, CHUNK)? {
        after = last;
        total.converted += s.converted;
        total.skipped += s.skipped;
        total.bytes_before += s.bytes_before;
        total.bytes_after += s.bytes_after;
    }
    Ok(total)
}
