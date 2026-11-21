//! Screenshot storage budget (data model v1 §6, §8): over the configured limit the library steps old screenshot
//! originals down the fidelity ladder to "只留文字" instead of stopping capture. The screenshot event stays with its
//! OCR text, AX evidence (the `derived` blob) and metadata; only the `original` image goes, oldest first, never one
//! from the last seven days. Screenshots whose words are already kept (OCR or AX evidence) go before those without,
//! so an image is dropped before it was read only when nothing else frees enough. Blob removal follows the deletion
//! order of `delete.rs` (rows in one transaction, files after commit). What was freed is counted in `store_meta`
//! and reported in `AppStats`.
use crate::db::{int, placeholders, text, RowExt, P};
use crate::{Result, TimelineError, TimelineStore};
use serde::{Deserialize, Serialize};
use std::fs;

/// Screenshots this recent always keep their image.
pub const SCREENSHOT_KEEP_RECENT_MS: i64 = 7 * 86_400_000;
const META_FREED_BYTES: &str = "screenshot_budget.freed_bytes";
const META_FREED_SCREENSHOTS: &str = "screenshot_budget.freed_screenshots";
const BATCH: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotSpaceRequest {
    /// Desktop originals may use at most this many bytes afterwards (the App passes ~90% of its limit).
    pub target_bytes: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotSpaceReport {
    /// Bytes and screenshots this call stepped down to text only.
    pub freed_bytes: i64,
    pub screenshots: i64,
    /// Desktop originals' bytes afterwards (the same measure as `AppStats::storage_bytes`).
    pub storage_bytes: i64,
    /// False when every eligible original went and usage is still above the target (the rest is under 7 days old).
    pub reached_target: bool,
    /// Running totals since the library started counting.
    pub total_freed_bytes: i64,
    pub total_screenshots: i64,
}

impl TimelineStore {
    /// Bytes and screenshots stepped down to text only so far, from `store_meta`.
    pub fn screenshot_space_freed(&self) -> Result<(i64, i64)> {
        let read = |k| -> Result<i64> { Ok(self.meta_value(k)?.and_then(|v| v.parse().ok()).unwrap_or(0)) };
        Ok((read(META_FREED_BYTES)?, read(META_FREED_SCREENSHOTS)?))
    }

    /// Drops screenshot originals, oldest first, until desktop originals use at most `target_bytes`.
    pub fn free_screenshot_space(&self, req: &ScreenshotSpaceRequest) -> Result<ScreenshotSpaceReport> {
        if req.target_bytes < 0 {
            return Err(TimelineError::InvalidInput("target_bytes".into()));
        }
        let mut usage = self.app_stats()?.storage_bytes;
        let mut report = ScreenshotSpaceReport::default();
        let sources = self.desktop_sources()?;
        let cutoff = self.clock.now_ms() - SCREENSHOT_KEEP_RECENT_MS;
        while usage > req.target_bytes && !sources.is_empty() {
            let mut p: Vec<P> = sources.iter().map(text).collect();
            p.push(int(cutoff));
            p.push(int(BATCH as i64));
            // Words kept first (OCR child or AX evidence), then oldest first.
            let candidates = self.db.query(
                &format!(
                    "SELECT e.event_id FROM events e WHERE e.source_id IN ({}) AND e.kind='screen.shot' AND e.start_ms<?
                       AND EXISTS (SELECT 1 FROM event_blobs eb WHERE eb.event_id=e.event_id AND eb.role='original')
                     ORDER BY (EXISTS (SELECT 1 FROM events c WHERE c.parent_event_id=e.event_id AND c.kind='image.text')
                               OR EXISTS (SELECT 1 FROM event_blobs d WHERE d.event_id=e.event_id AND d.role='derived')) DESC,
                              e.start_ms, e.event_id LIMIT ?",
                    placeholders(sources.len())
                ),
                &p,
                |r| r.text(0),
            )?;
            if candidates.is_empty() {
                break;
            }
            let mut files = Vec::new();
            let target = req.target_bytes;
            let (bytes, shots) = self.db.transaction(|| {
                let (mut bytes, mut shots) = (0i64, 0i64);
                for id in &candidates {
                    if usage - bytes <= target {
                        break;
                    }
                    let shas = self.db.query("SELECT sha256 FROM event_blobs WHERE event_id=? AND role='original'", &[text(id)], |r| r.text(0))?;
                    self.db.execute("DELETE FROM event_blobs WHERE event_id=? AND role='original'", &[text(id)])?;
                    bytes += self.drop_unreferenced_blobs(shas, &mut files)?.1;
                    self.record_change("event", id, "update")?;
                    shots += 1;
                }
                let (total_bytes, total_shots) = self.screenshot_space_freed()?;
                for (k, v) in [(META_FREED_BYTES, total_bytes + bytes), (META_FREED_SCREENSHOTS, total_shots + shots)] {
                    self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(k), text(v.to_string())])?;
                }
                Ok((bytes, shots))
            })?;
            for f in files {
                let _ = fs::remove_file(f);
            }
            usage -= bytes;
            report.freed_bytes += bytes;
            report.screenshots += shots;
            if shots == 0 {
                break;
            }
        }
        report.storage_bytes = self.app_stats()?.storage_bytes;
        report.reached_target = report.storage_bytes <= req.target_bytes;
        (report.total_freed_bytes, report.total_screenshots) = self.screenshot_space_freed()?;
        Ok(report)
    }
}
