//! §9.1 incremental mirroring from the desktop ledger and the Hub media library (both opened read-only).

use crate::db::{text, Db, RowExt};
use crate::desktop::{ms_from_reference, DesktopRecord, RecognitionResult};
use crate::ids::sha256_hex;
use crate::ingest::tombstone_key;
use crate::migration::{swift_double_description, DESKTOP_SOURCE_NAME, MEDIA_SOURCE_NAME};
use crate::store::TimelineStore;
use crate::types::{IngestBatch, IngestCursor, IngestEvent, IngestText};
use crate::Result;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Result of one incremental pass from an existing store into the timeline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub source_id: String,
    pub inserted: usize,
    /// Rows already in the timeline; skipped before any file work.
    pub unchanged: usize,
    /// Events removed because the source deleted them.
    pub deleted: usize,
    /// Recognition or transcription results replaced by a newer run.
    pub revised: usize,
    pub conflicts: usize,
    /// Rows that could not be mapped this time. Recent ones are retried next pass.
    pub failures: Vec<String>,
}

impl SyncReport {
    pub fn changed(&self) -> bool {
        self.inserted + self.deleted + self.revised > 0
    }
}

impl TimelineStore {
    /// How far back each desktop pass re-reads (seconds), so records written a little out of order and transient
    /// failures are picked up on the next pass.
    pub const LEDGER_SYNC_OVERLAP: f64 = 600.0;

    /// Mirrors the desktop ledger into the timeline: new records since the last pass, records the ledger deleted, and
    /// recognition results that were added or re-run. The ledger is opened read-only and never changed.
    pub fn sync_desktop_ledger(&self, ledger_root: &Path, source_id: Option<&str>) -> Result<SyncReport> {
        let sid = self.find_or_create_source("mac.desktop", DESKTOP_SOURCE_NAME, source_id, Some("desktop-ledger"))?;
        let outcome = self.sync_desktop_ledger_inner(ledger_root, &sid);
        self.finish_sync_pass(&sid, outcome)
    }

    fn sync_desktop_ledger_inner(&self, ledger_root: &Path, sid: &str) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let sid = sid.to_string();
        report.source_id = sid.clone();
        let (ledger, version) = Self::open_ledger(ledger_root)?;
        self.sync_ledger_records(&ledger, ledger_root, &sid, &mut report)?;
        self.sync_ledger_deletions(&ledger, &sid, &mut report)?;
        if version >= 5 {
            self.sync_ledger_recognition(&ledger, &sid, &mut report)?;
        }
        if report.changed() {
            self.set_source_status(&sid, "active", None)?;
        }
        Ok(report)
    }

    /// Bookkeeping at the end of one mirroring pass. `last_sync_ms` is the last successful check, so a pass that
    /// finished cleanly advances it even when nothing new arrived; a pass that failed leaves it alone and marks an
    /// active source as errored (a finite code, never content). Rows that could not be mapped count as unfinished.
    fn finish_sync_pass(&self, sid: &str, outcome: Result<SyncReport>) -> Result<SyncReport> {
        match &outcome {
            Ok(r) if r.failures.is_empty() => self.touch_source_sync(sid)?,
            Ok(_) => {}
            Err(_) => {
                if self.source_row(sid)?.map(|s| s.status == "active").unwrap_or(false) {
                    self.set_source_status(sid, "error", Some("sync_failed"))?;
                }
            }
        }
        outcome
    }

    fn is_tombstoned_key(&self, sid: &str, key: &str) -> Result<bool> {
        self.is_tombstoned("event_key", &tombstone_key(sid, key))
    }

    fn set_cursor(&self, sid: &str, stream: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO source_cursors(source_id,stream,cursor,updated_ms) VALUES(?,?,?,?) ON CONFLICT(source_id,stream) DO UPDATE SET cursor=excluded.cursor, updated_ms=excluded.updated_ms",
            &[text(sid), text(stream), text(value), crate::db::int(self.clock.now_ms())],
        )
    }

    fn sync_ledger_records(&self, ledger: &Db, root: &Path, sid: &str, report: &mut SyncReport) -> Result<()> {
        let stream = "ledger.live";
        let since = self.cursor(sid, stream)?.and_then(|c| c.parse::<f64>().ok()).map(|v| v - Self::LEDGER_SYNC_OVERLAP).unwrap_or(-1e18);
        let rows = ledger.query(
            "SELECT id, occurred FROM records WHERE occurred >= ? ORDER BY occurred, id",
            &[rusqlite::types::Value::Real(if since.is_finite() { since } else { -1e18 })],
            |r| (r.text(0), r.double(1)),
        )?;
        if rows.is_empty() {
            return Ok(());
        }
        let blobs_dir = root.join("blobs");
        for slice in rows.chunks(200) {
            let mut events = Vec::new();
            let mut staged: Vec<PathBuf> = Vec::new();
            let mut keys_in_batch: HashSet<String> = HashSet::new();
            for (id, _) in slice {
                if self.event_id(sid, id)?.is_some() || self.is_tombstoned_key(sid, id)? {
                    report.unchanged += 1;
                    continue;
                }
                let payload = ledger.first("SELECT payload FROM records WHERE id=?", &[text(id)], |r| r.blob(0))?;
                let Some(record) = payload.and_then(|p| serde_json::from_slice::<DesktopRecord>(&p).ok()) else {
                    report.failures.push(format!("无法解码记录 {id}"));
                    continue;
                };
                let parent_known = match &record.parent_id {
                    Some(p) => keys_in_batch.contains(p) || self.event_id(sid, p)?.is_some(),
                    None => false,
                };
                match self.desktop_event(&record, parent_known, &blobs_dir, &mut staged) {
                    Ok(e) => {
                        events.push(e);
                        keys_in_batch.insert(id.clone());
                    }
                    Err(e) => report.failures.push(format!("{id}: {}", e.message())),
                }
            }
            // The cursor always advances: the overlap window retries recent failures, and an old record that can never
            // be mapped (its image was deleted) must not make every later pass re-read all history.
            let last = &slice[slice.len() - 1];
            let batch = IngestBatch {
                on_missing_parent: None,
                merge_text: None, removed_source_keys: vec![], payload_revisions: vec![],
                source_id: sid.into(),
                cursor_after: Some(IngestCursor::new(stream, &swift_double_description(last.1))),
                threads: vec![],
                events,
            };
            let r = self.ingest(&batch);
            for f in &staged {
                let _ = fs::remove_file(f);
            }
            let r = r?;
            report.inserted += r.inserted;
            report.conflicts += r.conflicts;
        }
        Ok(())
    }

    fn sync_ledger_deletions(&self, ledger: &Db, sid: &str, report: &mut SyncReport) -> Result<()> {
        let stream = "ledger.tombstones";
        let since = self.cursor(sid, stream)?.and_then(|c| c.parse::<f64>().ok()).unwrap_or(-1.0);
        let rows = ledger.query(
            "SELECT id, deleted_at FROM tombstones WHERE deleted_at > ? ORDER BY deleted_at",
            &[rusqlite::types::Value::Real(since)],
            |r| (r.text(0), r.double(1)),
        )?;
        let Some(newest) = rows.last().map(|r| r.1) else { return Ok(()) };
        let mut ids = Vec::new();
        for (key, _) in &rows {
            if let Some(id) = self.event_id(sid, key)? {
                ids.push(id);
            }
            ids.extend(self.recognition_events(sid, key)?);
        }
        if !ids.is_empty() {
            report.deleted += self.delete_events(&ids, "user")?.events;
        }
        self.set_cursor(sid, stream, &swift_double_description(newest))
    }

    fn sync_ledger_recognition(&self, ledger: &Db, sid: &str, report: &mut SyncReport) -> Result<()> {
        // The completion time identifies a run without decoding the result; only new runs are decoded.
        let rows = ledger.query(
            "SELECT screenshot_id, kind, CAST(json_extract(CAST(result AS TEXT),'$.completedAt') AS TEXT)
            FROM image_recognition WHERE result IS NOT NULL ORDER BY screenshot_id, kind",
            &[],
            |r| (r.text(0), r.text(1), r.opt_text(2).unwrap_or_else(|| "0".into())),
        )?;
        for (shot_id, kind, run) in rows {
            let key = format!("ledger-recognition:{shot_id}:{kind}@{run}");
            if self.event_id(sid, &key)?.is_some() || self.is_tombstoned_key(sid, &key)? {
                continue;
            }
            let Some(shot_event) = self.event_id(sid, &shot_id)? else { continue }; // the screenshot syncs first
            let bytes = ledger.first("SELECT result FROM image_recognition WHERE screenshot_id=? AND kind=?", &[text(&shot_id), text(&kind)], |r| r.blob(0))?;
            let result = bytes.and_then(|b| serde_json::from_slice::<RecognitionResult>(&b).ok());
            let shot = self.db.first("SELECT start_ms,app_bundle_id,app_name,title FROM events WHERE event_id=?", &[text(&shot_event)], |r| {
                (r.opt_int(0), r.opt_text(1), r.opt_text(2), r.opt_text(3))
            })?;
            let (Some(result), Some(shot)) = (result, shot) else {
                report.failures.push(format!("识别结果 {shot_id}/{kind} 无法读取"));
                continue;
            };
            let base = format!("ledger-recognition:{shot_id}:{kind}");
            let previous = self.db.query(
                "SELECT event_id FROM events WHERE source_id=? AND (source_key=? OR source_key LIKE ?)",
                &[text(sid), text(&base), text(format!("{base}@%"))],
                |r| r.text(0),
            )?;
            let method = if kind == "ocr" { "ocr" } else { "vlm" };
            let mut e = IngestEvent::new(&key, "image.text", shot.0);
            e.app_bundle_id = shot.1;
            e.app_name = shot.2;
            e.title = shot.3;
            e.parent_key = Some(shot_id.clone());
            e.payload = json!({"image_event_id": shot_event, "method": method, "model_id": result.source, "completed_ms": ms_from_reference(result.completed_at)});
            if let Some(boxes) = ocr_boxes(&result) {
                e.payload["boxes"] = boxes;
            }
            e.text = vec![IngestText::new(method, &result.text)];
            let r = self.ingest(&IngestBatch::events(sid, vec![e]))?;
            report.inserted += r.inserted;
            report.conflicts += r.conflicts;
            if !previous.is_empty() {
                self.delete_events(&previous, "superseded")?;
                report.revised += 1;
            }
        }
        Ok(())
    }

    fn recognition_events(&self, sid: &str, screenshot: &str) -> Result<Vec<String>> {
        self.db.query(
            "SELECT event_id FROM events WHERE source_id=? AND source_key LIKE ?",
            &[text(sid), text(format!("ledger-recognition:{screenshot}:%"))],
            |r| r.text(0),
        )
    }

    // MARK: Hub media library

    /// Mirrors the Hub media library (recordings, images and their recognition) into the timeline. New items are copied
    /// in, items the library deleted are removed, and a changed transcript or image text replaces the earlier one.
    pub fn sync_hub_media(&self, hub_database: &Path, vault_root: &Path, source_id: Option<&str>) -> Result<SyncReport> {
        let sid = self.find_or_create_source("media.import", MEDIA_SOURCE_NAME, source_id, Some("hub-media"))?;
        let outcome = self.sync_hub_media_inner(hub_database, vault_root, &sid);
        self.finish_sync_pass(&sid, outcome)
    }

    fn sync_hub_media_inner(&self, hub_database: &Path, vault_root: &Path, sid: &str) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let sid = sid.to_string();
        report.source_id = sid.clone();
        let hub = Db::open(hub_database, true, false)?;
        for row in Self::hub_media_rows(&hub)? {
            let key = format!("media:{}", row.id);
            let revision = row.result.as_ref().map(|r| sha256_hex(r.as_bytes())[..16].to_string());
            let parent_id = self.event_id(&sid, &key)?;
            if parent_id.is_none() && self.is_tombstoned_key(&sid, &key)? {
                continue;
            }
            let rev_tag = revision.clone().unwrap_or_else(|| "-".into());
            let first_dependent = if row.kind == "audio" { format!("media-seg:{}:{rev_tag}:0", row.id) } else { format!("media-text:{}:{rev_tag}", row.id) };
            if parent_id.is_some() {
                let current = revision.is_none() || self.event_id(&sid, &first_dependent)?.is_some() || self.is_tombstoned_key(&sid, &first_dependent)?;
                if current {
                    report.unchanged += 1;
                    continue;
                }
            }
            let mut staged: Vec<PathBuf> = Vec::new();
            let outcome = (|| -> Result<()> {
                let mapped = match self.hub_media_events(&row, vault_root, Some(&rev_tag), &mut staged, parent_id.is_none()) {
                    Ok(m) => m,
                    Err(e) => {
                        report.failures.push(format!("{}: {}", row.id, e.message()));
                        return Ok(());
                    }
                };
                if parent_id.is_none() {
                    let r = self.ingest(&IngestBatch { on_missing_parent: None, merge_text: None, removed_source_keys: vec![], payload_revisions: vec![], source_id: sid.clone(), cursor_after: None, threads: mapped.0.into_iter().collect(), events: vec![mapped.1] })?;
                    report.inserted += r.inserted;
                    report.conflicts += r.conflicts;
                }
                let Some(recording_id) = self.event_id(&sid, &key)? else { return Ok(()) };
                let previous = self.db.query(
                    "SELECT event_id FROM events WHERE source_id=? AND (source_key LIKE ? OR source_key LIKE ?)",
                    &[text(&sid), text(format!("media-seg:{}:%", row.id)), text(format!("media-text:{}%", row.id))],
                    |r| r.text(0),
                )?;
                let dependents: Vec<IngestEvent> = mapped
                    .2
                    .into_iter()
                    .map(|mut d| {
                        let mut p = match d.payload.clone() {
                            Value::Object(m) => m,
                            _ => Map::new(),
                        };
                        p.insert(if d.kind == "speech.segment" { "recording_event_id" } else { "image_event_id" }.into(), json!(recording_id));
                        d.payload = Value::Object(p);
                        d
                    })
                    .collect();
                if !dependents.is_empty() {
                    let r = self.ingest(&IngestBatch::events(&sid, dependents))?;
                    report.inserted += r.inserted;
                    report.conflicts += r.conflicts;
                }
                if !previous.is_empty() {
                    self.delete_events(&previous, "superseded")?;
                    report.revised += 1;
                }
                Ok(())
            })();
            for f in &staged {
                let _ = fs::remove_file(f);
            }
            outcome?;
        }
        // Items the library deleted.
        let deleted = hub.query("SELECT id FROM media_timeline_deleted", &[], |r| r.text(0))?;
        let mut ids = Vec::new();
        for d in deleted {
            if let Some(id) = self.event_id(&sid, &format!("media:{d}"))? {
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            report.deleted += self.delete_events(&ids, "user")?.events;
        }
        if report.changed() {
            self.set_source_status(&sid, "active", None)?;
        }
        Ok(report)
    }
}

/// OCR line boxes `[x, y, width, height]` on a 0–1000 grid, one per line of the result text, so later steps can
/// split a screenshot into columns and tell sides of a chat apart. Absent when the lines do not match the text.
fn ocr_boxes(result: &crate::desktop::RecognitionResult) -> Option<Value> {
    let lines = result.ocr_lines.as_ref()?.as_array()?;
    if lines.is_empty() || lines.len() != result.text.split('\n').count() {
        return None;
    }
    let boxes: Option<Vec<Value>> = lines
        .iter()
        .map(|l| Some(json!([l["x"].as_i64()?, l["y"].as_i64()?, l["width"].as_i64()?, l["height"].as_i64()?])))
        .collect();
    boxes.map(Value::Array)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(text: &str, lines: Value) -> RecognitionResult {
        serde_json::from_value(json!({"source": "Apple Vision", "text": text, "completedAt": 0.0, "usedOCRReference": false, "ocrLines": lines})).unwrap()
    }

    #[test]
    fn ocr_boxes_follow_the_text_lines() {
        let r = result("左边\n右边", json!([{"text": "左边", "x": 10, "y": 20, "width": 100, "height": 12}, {"text": "右边", "x": 800, "y": 40, "width": 90, "height": 12}]));
        assert_eq!(ocr_boxes(&r), Some(json!([[10, 20, 100, 12], [800, 40, 90, 12]])));
        let mismatched = result("只有一行", json!([{"text": "a", "x": 1, "y": 1, "width": 1, "height": 1}, {"text": "b", "x": 1, "y": 1, "width": 1, "height": 1}]));
        assert_eq!(ocr_boxes(&mismatched), None);
        assert_eq!(ocr_boxes(&result("x", Value::Null)), None);
    }
}
