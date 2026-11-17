//! §9 one-shot migrations from the desktop ledger and the Hub (read-only on the old stores, resumable, idempotent).

use crate::db::{int, text, Db, RowExt};
use crate::desktop::{ms_from_reference, DesktopRecord, RecognitionResult, RecordKind};
use crate::ids::{sha256_hex, sha256_hex_file, uuid_v4};
use crate::json;
use crate::store::TimelineStore;
use crate::types::{IngestBatch, IngestBlob, IngestCursor, IngestEvent, IngestText, IngestThread, SourcePolicy};
use crate::{Result, TimelineError};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const DESKTOP_SOURCE_NAME: &str = "Mac 桌面";
pub(crate) const MEDIA_SOURCE_NAME: &str = "资料库";
pub(crate) const OBSERVATION_SOURCE_NAME: &str = "Hub 设备观察（迁入）";
/// `store_meta` key: `direct` once desktop capture writes into this store.
pub const META_CAPTURE_MODE: &str = "desktop.capture_mode";
/// `store_meta` key: JSON record of the switch (time, counts, the check that allowed it).
pub const META_CUTOVER: &str = "desktop.cutover";

/// Result of one migration pass. Re-running after an interruption continues and then reports `completed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub source_id: String,
    pub completed: bool,
    pub inserted: usize,
    pub duplicates: usize,
    pub conflicts: usize,
    pub skipped: usize,
    pub failures: Vec<String>,
    pub batches: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindCount {
    pub ledger: i64,
    pub store: i64,
}

/// `核对项` of §9 for the desktop ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationVerification {
    pub counts_by_kind: BTreeMap<String, KindCount>,
    pub blobs_checked: usize,
    pub blob_mismatches: Vec<String>,
    pub missing_events: Vec<String>,
}

/// A ledger check whose screenshot hashes are still to be read (see [`TimelineStore::check_desktop_ledger`]).
#[derive(Clone, Debug, Default)]
pub struct LedgerCheck {
    pub verification: MigrationVerification,
    /// `(record id, expected sha256, original path)` for every present screenshot.
    pub files: Vec<(String, String, PathBuf)>,
}

impl LedgerCheck {
    /// Reads every listed original and records the ones whose hash differs.
    pub fn hash_files(&mut self) {
        for (id, hash, path) in &self.files {
            if sha256_hex_file(path).ok().as_deref() != Some(hash.as_str()) {
                self.verification.blob_mismatches.push(format!("{id}: 文件 hash 不一致"));
            }
        }
    }
}

impl MigrationVerification {
    pub fn ok(&self) -> bool {
        self.counts_by_kind.values().all(|c| c.ledger == c.store) && self.blob_mismatches.is_empty() && self.missing_events.is_empty()
    }
}

/// Swift's `"\(double)"`: integral values keep `.0`.
pub(crate) fn swift_double_description(d: f64) -> String {
    if d.is_finite() && d.fract() == 0.0 && d.abs() < 1e16 {
        format!("{}.0", d as i64)
    } else {
        crate::json::format_double(d)
    }
}

/// Python/ISO 8601 timestamps from the Hub: fractions cut to milliseconds; a naive time is UTC.
pub(crate) fn parse_iso(text_: Option<&str>) -> Option<i64> {
    let mut s = text_?.trim_matches(|c: char| c == ' ' || c == '\t').to_string();
    if s.is_empty() {
        return None;
    }
    if let Some(dot) = s.find('.') {
        if let Some(end_rel) = s[dot..].find(['+', '-', 'Z']) {
            let end = dot + end_rel;
            let digits: String = s[dot + 1..end].chars().take(3).collect();
            s = format!("{}.{:0<3}{}", &s[..dot], digits, &s[end..]);
        }
    }
    if let Some(ms) = crate::clock::parse_internet_date_time(&s) {
        return Some(ms);
    }
    let b = s.as_bytes();
    if b.len() == 19 && b[10] == b'T' {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S") {
            return Some(n.and_utc().timestamp_millis());
        }
    }
    None
}

pub(crate) struct HubMediaRow {
    pub id: String,
    pub filename: String,
    pub kind: String,
    pub blob: String,
    pub mime: String,
    pub occurred: Option<String>,
    pub result: Option<String>,
}

/// Puts a source's original file into staging. A hard link costs no space and leaves the source file as it was
/// (originals are named by their hash and never rewritten); across volumes it falls back to a copy.
pub(crate) fn stage_original(file: &Path, staging: &Path) -> Result<()> {
    if fs::hard_link(file, staging).is_err() {
        fs::copy(file, staging)?;
    }
    Ok(())
}

fn upper_uuid() -> String {
    uuid_v4().to_uppercase()
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

impl TimelineStore {
    pub(crate) fn find_or_create_source(&self, kind: &str, name: &str, id: Option<&str>, device: Option<&str>) -> Result<String> {
        if let Some(id) = id {
            if self.source_row(id)?.is_none() {
                return Err(TimelineError::UnknownSource(id.into()));
            }
            return Ok(id.into());
        }
        if let Some(existing) = self.db.first("SELECT source_id FROM sources WHERE kind=? AND display_name=?", &[text(kind), text(name)], |r| r.text(0))? {
            return Ok(existing);
        }
        self.register_source_with(
            kind,
            name,
            &SourcePolicy::new(true, true),
            device,
            None,
            None,
            &json!({"version": 1, "migration": "read-only"}),
            None,
        )
    }

    pub(crate) fn open_ledger(ledger_root: &Path) -> Result<(Db, i64)> {
        let ledger = Db::open(&ledger_root.join("ledger.sqlite"), true, false)?;
        let version = ledger.scalar("PRAGMA user_version", &[])?;
        if !(1..=5).contains(&version) {
            return Err(TimelineError::InvalidInput(format!("桌面账本版本 {version} 不受支持")));
        }
        Ok((ledger, version))
    }

    // MARK: Desktop ledger → timeline

    /// Reads `<ledger_root>/ledger.sqlite` strictly read-only and ingests it in resumable, idempotent batches.
    /// Pass `max_batches` to stop early (tests, progress UI); call again to continue.
    pub fn migrate_desktop_ledger(&self, ledger_root: &Path, source_id: Option<&str>, batch_size: usize, max_batches: Option<usize>) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();
        let sid = self.find_or_create_source("mac.desktop", DESKTOP_SOURCE_NAME, source_id, Some("desktop-ledger"))?;
        report.source_id = sid.clone();
        let (ledger, version) = Self::open_ledger(ledger_root)?;
        // order: parents before children, then time
        let parents: HashMap<String, (Option<String>, f64)> = ledger
            .query("SELECT id, json_extract(CAST(payload AS TEXT),'$.parentID'), occurred FROM records", &[], |r| (r.text(0), (r.opt_text(1), r.double(2))))?
            .into_iter()
            .collect();
        let depth = |id: &str| -> i64 {
            let mut d = 0;
            let mut cur = id.to_string();
            let mut seen: HashSet<String> = HashSet::from([id.to_string()]);
            while let Some(p) = parents.get(&cur).and_then(|x| x.0.clone()) {
                if !parents.contains_key(&p) || !seen.insert(p.clone()) {
                    break;
                }
                d += 1;
                cur = p;
            }
            d
        };
        let mut entries: Vec<(i64, f64, String)> = parents.iter().map(|(id, v)| (depth(id), v.1, id.clone())).collect();
        let tuple_cmp = |a: &(i64, f64, String), b: &(i64, f64, String)| {
            a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)).then(a.2.cmp(&b.2))
        };
        entries.sort_by(tuple_cmp);
        let stream = "ledger.records";
        if let Some(saved) = self.cursor(&sid, stream)? {
            let parts: Vec<&str> = saved.splitn(3, '|').collect();
            if parts.len() == 3 {
                if let (Ok(d), Ok(o)) = (parts[0].parse::<i64>(), parts[1].parse::<f64>()) {
                    let mark = (d, o, parts[2].to_string());
                    entries.retain(|e| tuple_cmp(e, &mark) == std::cmp::Ordering::Greater);
                }
            }
        }
        let blobs_dir = ledger_root.join("blobs");
        let mut index = 0;
        let mut blocked = false; // the cursor never advances past a failed record, so a rerun retries it
        while index < entries.len() {
            if max_batches.is_some_and(|m| report.batches >= m) {
                return Ok(report);
            }
            let slice = &entries[index..(index + batch_size).min(entries.len())];
            let mut events = Vec::new();
            let mut staged: Vec<PathBuf> = Vec::new();
            let failures_before = report.failures.len();
            for entry in slice {
                let payload = ledger.first("SELECT payload FROM records WHERE id=?", &[text(&entry.2)], |r| r.blob(0))?;
                let Some(record) = payload.and_then(|p| serde_json::from_slice::<DesktopRecord>(&p).ok()) else {
                    report.failures.push(format!("无法解码记录 {}", entry.2));
                    continue;
                };
                let parent_exists = record.parent_id.as_ref().map(|p| parents.contains_key(p)).unwrap_or(false);
                match self.desktop_event(&record, parent_exists, &blobs_dir, &mut staged) {
                    Ok(e) => events.push(e),
                    Err(e) => {
                        report.failures.push(format!("{}: {}", entry.2, e.message()));
                        report.skipped += 1;
                    }
                }
            }
            let last = &slice[slice.len() - 1];
            if report.failures.len() > failures_before {
                blocked = true;
            }
            let cursor = (!blocked).then(|| IngestCursor::new(stream, &format!("{}|{}|{}", last.0, swift_double_description(last.1), last.2)));
            let batch = IngestBatch { on_missing_parent: None, merge_text: None, removed_source_keys: vec![], payload_revisions: vec![], source_id: sid.clone(), cursor_after: cursor, threads: vec![], events };
            let r = self.ingest(&batch);
            for f in &staged {
                let _ = fs::remove_file(f);
            }
            let r = r?;
            report.inserted += r.inserted;
            report.duplicates += r.duplicates;
            report.conflicts += r.conflicts;
            report.batches += 1;
            index += slice.len();
        }
        // stage 2: recognition results become image.text perception events
        if version >= 5 {
            let rows = ledger.query(
                "SELECT screenshot_id, kind, result FROM image_recognition WHERE result IS NOT NULL ORDER BY screenshot_id, kind",
                &[],
                |r| (r.text(0), r.text(1), r.blob(2)),
            )?;
            let rstream = "ledger.recognition";
            let done = self.cursor(&sid, rstream)?.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
            let mut position = done;
            let mut rblocked = !report.failures.is_empty(); // a skipped screenshot means its recognition rows must be retried too
            while position < rows.len() {
                if max_batches.is_some_and(|m| report.batches >= m) {
                    return Ok(report);
                }
                let slice = &rows[position..(position + batch_size).min(rows.len())];
                let mut events = Vec::new();
                let failures_before = report.failures.len();
                for (shot_id, kind, bytes) in slice {
                    let shot_event = self.event_id(&sid, shot_id)?;
                    let result = serde_json::from_slice::<RecognitionResult>(bytes).ok();
                    let shot = match &shot_event {
                        Some(ev) => self.db.first("SELECT start_ms,app_bundle_id,app_name,title FROM events WHERE event_id=?", &[text(ev)], |r| {
                            (r.opt_int(0), r.opt_text(1), r.opt_text(2), r.opt_text(3))
                        })?,
                        None => None,
                    };
                    let (Some(shot_event), Some(result), Some(shot)) = (shot_event, result, shot) else {
                        report.failures.push(format!("识别结果 {shot_id}/{kind} 找不到截图事件"));
                        report.skipped += 1;
                        continue;
                    };
                    let method = if kind == "ocr" { "ocr" } else { "vlm" };
                    let mut e = IngestEvent::new(&format!("ledger-recognition:{shot_id}:{kind}"), "image.text", shot.0);
                    e.app_bundle_id = shot.1;
                    e.app_name = shot.2;
                    e.title = shot.3;
                    e.parent_key = Some(shot_id.clone());
                    e.payload = json!({"image_event_id": shot_event, "method": method, "model_id": result.source, "completed_ms": ms_from_reference(result.completed_at)});
                    e.text = vec![IngestText::new(method, &result.text)];
                    events.push(e);
                }
                if report.failures.len() > failures_before {
                    rblocked = true;
                }
                let cursor = (!rblocked).then(|| IngestCursor::new(rstream, &(position + slice.len()).to_string()));
                let r = self.ingest(&IngestBatch { on_missing_parent: None, merge_text: None, removed_source_keys: vec![], payload_revisions: vec![], source_id: sid.clone(), cursor_after: cursor, threads: vec![], events })?;
                report.inserted += r.inserted;
                report.duplicates += r.duplicates;
                report.conflicts += r.conflicts;
                report.batches += 1;
                position += slice.len();
            }
        }
        report.completed = report.failures.is_empty();
        Ok(report)
    }

    pub(crate) fn desktop_event(&self, r: &DesktopRecord, parent_exists: bool, blobs_dir: &Path, staged: &mut Vec<PathBuf>) -> Result<IngestEvent> {
        let start = ms_from_reference(r.timestamp);
        let mut legacy = Map::new();
        legacy.insert("run_id".into(), json!(r.run_id));
        if let Some(a) = &r.activity_id {
            legacy.insert("activity_id".into(), json!(a));
        }
        if !r.detail.is_empty() {
            legacy.insert("detail".into(), json!(r.detail));
        }
        let mut payload = Map::new();
        payload.insert("legacy".into(), Value::Object(legacy));
        let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let mut event = IngestEvent::new(&r.id, r.kind.event_kind(), Some(start));
        event.app_bundle_id = non_empty(&r.bundle_id);
        event.app_name = non_empty(&r.app_name);
        event.title = non_empty(&r.window_title);
        event.parent_key = if parent_exists { r.parent_id.clone() } else { None };
        match r.kind {
            RecordKind::Application => {
                payload.insert("reason".into(), json!(r.trigger.clone().unwrap_or_else(|| "switch".into())));
            }
            RecordKind::Window => {
                let token = sha256_hex(format!("{}|{}|{}", r.run_id, r.bundle_id, r.window_title));
                payload.insert("window_token".into(), json!(&token[..16]));
                payload.insert("coverage".into(), json!("window"));
            }
            RecordKind::Context => {
                let c = r.context.as_ref();
                if let Some(evidence) = c.and_then(|c| c.record_evidence.as_ref()).filter(|_| c.and_then(|c| c.is_protected) != Some(true) && c.and_then(|c| c.is_composing) != Some(true)) {
                    evidence.validate()?;
                    payload.insert("record_evidence".into(), serde_json::to_value(evidence)?);
                    if let Some(value) = evidence.input_value.as_ref().filter(|v| !v.is_empty()) { event.text.push(IngestText::new("message", value)); }
                }
                payload.insert("trigger".into(), json!(r.trigger.clone().unwrap_or_else(|| "unknown".into())));
                payload.insert("text_status".into(), json!(c.and_then(|c| c.text_status.clone())));
                payload.insert("focused_role".into(), json!(c.and_then(|c| c.focused_role.clone())));
                payload.insert("focused_label".into(), json!(c.and_then(|c| c.focused_label.clone())));
                payload.insert("is_composing".into(), json!(c.and_then(|c| c.is_composing)));
                let nodes = c.and_then(|c| c.ax_evidence.as_ref()).map(|e| e.nodes.len()).unwrap_or(0);
                payload.insert("reader".into(), json!({"version": 0, "deep_pass": false, "nodes": nodes}));
                if let Some(v) = c.and_then(|c| c.session_active) {
                    payload.insert("session_active".into(), json!(v));
                }
                if let Some(v) = c.and_then(|c| c.on_battery) {
                    payload.insert("on_battery".into(), json!(v));
                }
                if let Some(v) = c.and_then(|c| c.low_power) {
                    payload.insert("low_power".into(), json!(v));
                }
                if let Some(v) = c.and_then(|c| c.thermal_state.clone()) {
                    payload.insert("thermal_state".into(), json!(v));
                }
                if let Some(v) = c.and_then(|c| c.idle_seconds) {
                    payload.insert("idle_seconds".into(), json!(v));
                }
                if c.and_then(|c| c.is_protected) == Some(true) {
                    event.sensitivity = "protected".into();
                } else {
                    if let Some(body) = c.and_then(|c| c.readable_text.clone()).filter(|b| !b.is_empty()) {
                        event.text.push(IngestText::new("body", &body));
                    }
                    if let Some(sel) = c.and_then(|c| c.selected_text.clone()).filter(|s| !s.is_empty()) {
                        event.text.push(IngestText::new("selection", &sel));
                    }
                }
            }
            RecordKind::Screenshot => {
                payload.insert("trigger".into(), json!(r.trigger.clone().unwrap_or_else(|| "unknown".into())));
                if let Some(w) = r.image_width {
                    payload.insert("pixel_w".into(), json!(w));
                }
                if let Some(h) = r.image_height {
                    payload.insert("pixel_h".into(), json!(h));
                }
                let hash = r.image_hash.clone().ok_or_else(|| TimelineError::BlobMismatch("截图记录没有图片 hash".into()))?;
                let prefix: String = hash.chars().take(2).collect();
                let short: String = hash.chars().take(12).collect();
                let file = blobs_dir.join(&prefix).join(format!("{hash}.jpg"));
                if !file.exists() {
                    return Err(TimelineError::BlobMismatch(format!("图片文件缺失 {short}")));
                }
                let copy = self.staging_directory().join(format!("{}.jpg", upper_uuid()));
                stage_original(&file, &copy)?;
                staged.push(copy.clone());
                let actual = sha256_hex_file(&copy)?;
                if actual != hash {
                    return Err(TimelineError::BlobMismatch(format!("图片 hash 不一致 {short}")));
                }
                let mut blob = IngestBlob::new(&copy.to_string_lossy(), &hash);
                blob.mime_type = Some("image/jpeg".into());
                blob.pixel_w = r.image_width;
                blob.pixel_h = r.image_height;
                event.blobs.push(blob);
                if let Some(evidence) = r.context.as_ref().and_then(|c| c.ax_evidence.as_ref()) {
                    let data = evidence.swift_encoded();
                    let ev = self.staging_directory().join(format!("{}.json", upper_uuid()));
                    fs::write(&ev, &data)?;
                    staged.push(ev.clone());
                    let sha = sha256_hex(&data);
                    let mut b = IngestBlob::new(&ev.to_string_lossy(), &sha);
                    b.role = "derived".into();
                    b.mime_type = Some("application/json".into());
                    event.blobs.push(b);
                    payload.insert("ax_evidence_ref".into(), json!(sha));
                }
            }
            RecordKind::Marker => {
                if !r.window_title.is_empty() {
                    payload.insert("label".into(), json!(r.window_title));
                }
                if !r.detail.is_empty() {
                    payload.insert("note".into(), json!(r.detail));
                }
            }
            RecordKind::System => {
                let d = r.detail.to_lowercase();
                let change = if d.contains("wake") || d.contains("唤醒") {
                    "wake"
                } else if d.contains("sleep") || d.contains("睡眠") {
                    "sleep"
                } else if d.contains("unlock") || d.contains("解锁") {
                    "unlock"
                } else if d.contains("lock") || d.contains('锁') {
                    "lock"
                } else if d.contains("power") || d.contains("battery") || d.contains('电') {
                    "power"
                } else {
                    "session"
                };
                payload.insert("change".into(), json!(change));
                payload.insert("value".into(), json!(r.detail));
            }
            RecordKind::Gap => {
                let d = r.detail.to_lowercase();
                let reason = if d.contains("pause") || d.contains("暂停") {
                    "paused"
                } else if d.contains("permission") || d.contains("权限") {
                    "permission_denied"
                } else if d.contains("sleep") || d.contains("睡眠") {
                    "asleep"
                } else if d.contains("crash") || d.contains("崩溃") {
                    "crashed"
                } else {
                    "unknown"
                };
                payload.insert("reason_code".into(), json!(reason));
            }
        }
        if r.kind == RecordKind::Context && r.context.as_ref().is_some_and(|c| c.record_evidence.is_some()) && event.text.is_empty() { event.kind = "coverage.gap".into(); payload.insert("reason_code".into(), json!("text_unavailable")); }
        event.payload = Value::Object(payload);
        Ok(event)
    }

    /// Compares the ledger with the store record by record: every ledger record must be in the store as the same
    /// kind, or have been deleted there since (a tombstone); every present screenshot must reference its original
    /// with the same hash. `hash_blobs: false` checks references and file presence without reading the images.
    /// Events the store has beyond the ledger (direct capture after the cutover) do not count.
    pub fn verify_desktop_ledger_migration(&self, ledger_root: &Path, source_id: Option<&str>, hash_blobs: bool) -> Result<MigrationVerification> {
        let mut check = self.check_desktop_ledger(ledger_root, source_id)?;
        if hash_blobs {
            check.hash_files();
        }
        Ok(check.verification)
    }

    /// The database part of [`Self::verify_desktop_ledger_migration`]; the file hashes are left to
    /// [`LedgerCheck::hash_files`], which needs no store access (the service runs it without holding the store).
    pub fn check_desktop_ledger(&self, ledger_root: &Path, source_id: Option<&str>) -> Result<LedgerCheck> {
        let sid = self.find_or_create_source("mac.desktop", DESKTOP_SOURCE_NAME, source_id, Some("desktop-ledger"))?;
        let (ledger, version) = Self::open_ledger(ledger_root)?;
        let stored: HashMap<String, String> = self
            .db
            .query("SELECT source_key, kind FROM events WHERE source_id=?", &[text(&sid)], |r| (r.text(0), r.text(1)))?
            .into_iter()
            .collect();
        let deleted: HashSet<String> = self
            .db
            .query("SELECT entity_id FROM tombstones WHERE entity='event_key' AND entity_id LIKE ?", &[text(format!("{sid}:%"))], |r| r.text(0))?
            .into_iter()
            .collect();
        let gone = |key: &str| deleted.contains(&crate::ingest::tombstone_key(&sid, key));
        let mut v = MigrationVerification::default();
        let mut files = Vec::new();
        let mut accounted: HashMap<&'static str, i64> = HashMap::new();
        let mut totals: HashMap<&'static str, i64> = HashMap::new();
        let mut present_shots: HashSet<String> = HashSet::new();
        let rows = ledger.query("SELECT id, kind, image_hash FROM records", &[], |r| (r.text(0), r.text(1), r.opt_text(2)))?;
        for (id, kind, hash) in rows {
            let Some(k) = RecordKind::ALL.into_iter().find(|k| k.raw() == kind) else { continue };
            *totals.entry(k.raw()).or_default() += 1;
            match stored.get(&id) {
                Some(event_kind) if event_kind == k.event_kind() => {
                    *accounted.entry(k.raw()).or_default() += 1;
                    if let Some(hash) = hash {
                        present_shots.insert(id.clone());
                        let ev = self.event_id(&sid, &id)?.unwrap_or_default();
                        match self.db.first(
                            "SELECT b.rel_path FROM event_blobs eb JOIN blobs b USING(sha256) WHERE eb.event_id=? AND eb.sha256=? AND eb.role='original'",
                            &[text(&ev), text(&hash)],
                            |r| r.text(0),
                        )? {
                            None => v.blob_mismatches.push(format!("{id}: 没有引用 {}", hash.chars().take(12).collect::<String>())),
                            Some(rel) => {
                                v.blobs_checked += 1;
                                let path = self.root.join(&rel);
                                if !path.exists() {
                                    v.blob_mismatches.push(format!("{id}: 文件缺失"));
                                } else {
                                    files.push((id.clone(), hash, path));
                                }
                            }
                        }
                    }
                }
                Some(_) => v.blob_mismatches.push(format!("{id}: 种类不一致")),
                None if gone(&id) => *accounted.entry(k.raw()).or_default() += 1,
                None => {
                    if hash.is_some() {
                        v.missing_events.push(id);
                    }
                }
            }
        }
        for k in RecordKind::ALL {
            v.counts_by_kind.insert(
                k.raw().into(),
                KindCount { ledger: totals.get(k.raw()).copied().unwrap_or(0), store: accounted.get(k.raw()).copied().unwrap_or(0) },
            );
        }
        if version >= 5 {
            let mut runs: HashSet<String> = HashSet::new();
            for key in self.db.query("SELECT source_key FROM events WHERE source_id=? AND source_key LIKE 'ledger-recognition:%'", &[text(&sid)], |r| r.text(0))? {
                runs.insert(key.split('@').next().unwrap_or(&key).to_string());
            }
            let rows = ledger.query("SELECT screenshot_id, kind FROM image_recognition WHERE result IS NOT NULL", &[], |r| (r.text(0), r.text(1)))?;
            let total = rows.len() as i64;
            let mut ok = 0;
            for (shot, kind) in rows {
                let base = format!("ledger-recognition:{shot}:{kind}");
                // Present, or gone with its screenshot (deleted or expired in the timeline).
                if runs.contains(&base) || (!present_shots.contains(&shot) && gone(&shot)) {
                    ok += 1;
                }
            }
            v.counts_by_kind.insert("recognition".into(), KindCount { ledger: total, store: ok });
        }
        Ok(LedgerCheck { verification: v, files })
    }

    /// Whether desktop capture writes straight into this store (the ledger was imported, checked and retired).
    pub fn desktop_direct(&self) -> Result<bool> {
        Ok(self.meta_value(META_CAPTURE_MODE)?.as_deref() == Some("direct"))
    }

    /// The record of the switch to direct capture, if it happened.
    pub fn desktop_cutover(&self) -> Result<Option<Value>> {
        Ok(self.meta_value(META_CUTOVER)?.and_then(|v| json::parse(&v)))
    }

    /// Switches desktop capture to direct writes and keeps the record of the check that allowed it.
    pub fn mark_desktop_direct(&self, record: &Value) -> Result<()> {
        self.db.transaction(|| {
            for (k, v) in [(META_CAPTURE_MODE, "direct".to_string()), (META_CUTOVER, json::canonical(record))] {
                self.db.execute("INSERT INTO store_meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", &[text(k), text(v)])?;
            }
            Ok(())
        })
    }

    /// The App's capture preferences kept in a ledger (`settings.preferences`), read-only, as stored.
    pub fn legacy_desktop_preferences(ledger_root: &Path) -> Result<Option<Vec<u8>>> {
        let (ledger, _) = Self::open_ledger(ledger_root)?;
        if ledger.scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='settings'", &[])? == 0 {
            return Ok(None);
        }
        ledger.first("SELECT value FROM settings WHERE key='preferences'", &[], |r| r.blob(0))
    }

    // MARK: Hub media library → timeline

    pub(crate) fn hub_media_rows(hub: &Db) -> Result<Vec<HubMediaRow>> {
        hub.query(
            "SELECT m.id, m.filename, m.kind, m.blob, m.mime_type, m.occurred_at,
                   COALESCE((SELECT r.result FROM media_timeline_recognition r WHERE r.media_id=m.id AND r.mode=m.processing_mode),
                            (SELECT r.result FROM media_timeline_recognition r WHERE r.media_id=m.id ORDER BY r.created_at DESC LIMIT 1))
            FROM media_timeline m
            WHERE m.id NOT IN (SELECT id FROM media_timeline_deleted)
            ORDER BY COALESCE(m.occurred_at, m.imported_at), m.id",
            &[],
            |r| HubMediaRow { id: r.text(0), filename: r.text(1), kind: r.text(2), blob: r.text(3), mime: r.text(4), occurred: r.opt_text(5), result: r.opt_text(6) },
        )
    }

    /// Imports `media_timeline` (+ the active recognition result) from a Hub vault. `hub_database` is opened read-only;
    /// original files are copied from `<vault_root>/blobs/` and never moved.
    pub fn migrate_hub_media(&self, hub_database: &Path, vault_root: &Path, source_id: Option<&str>, batch_size: usize, max_batches: Option<usize>) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();
        let sid = self.find_or_create_source("media.import", MEDIA_SOURCE_NAME, source_id, Some("hub-media"))?;
        report.source_id = sid.clone();
        let hub = Db::open(hub_database, true, false)?;
        let rows = Self::hub_media_rows(&hub)?;
        let stream = "hub.media";
        let mut index = self.cursor(&sid, stream)?.and_then(|c| c.parse::<usize>().ok()).map(|n| n.min(rows.len())).unwrap_or(0);
        let mut blocked = false;
        while index < rows.len() {
            if max_batches.is_some_and(|m| report.batches >= m) {
                return Ok(report);
            }
            let end = (index + batch_size).min(rows.len());
            let slice = &rows[index..end];
            let (mut threads, mut events, mut staged) = (Vec::new(), Vec::new(), Vec::new());
            let failures_before = report.failures.len();
            for row in slice {
                match self.hub_media_events(row, vault_root, None, &mut staged, true) {
                    Ok((thread, parent, dependents)) => {
                        threads.extend(thread);
                        events.push(parent);
                        events.extend(dependents);
                    }
                    Err(e) => {
                        report.failures.push(format!("{}: {}", row.id, e.message()));
                        report.skipped += 1;
                    }
                }
            }
            // `recording_event_id`/`image_event_id` are resolved after the parent exists: first parents, then dependents
            let parents: Vec<IngestEvent> = events.iter().filter(|e| e.kind == "audio.recording" || e.kind == "image.capture").cloned().collect();
            let dependents: Vec<IngestEvent> = events.iter().filter(|e| e.kind == "speech.segment" || e.kind == "image.text").cloned().collect();
            if report.failures.len() > failures_before {
                blocked = true;
            }
            let outcome = (|| -> Result<()> {
                let r1 = self.ingest(&IngestBatch { on_missing_parent: None, merge_text: None, removed_source_keys: vec![], payload_revisions: vec![], source_id: sid.clone(), cursor_after: None, threads, events: parents })?;
                let mut fixed = Vec::new();
                for mut d in dependents {
                    let parent_key = d.parent_key.clone().unwrap_or_default();
                    let parent_id = match r1.event_ids.get(&parent_key) {
                        Some(id) => Some(id.clone()),
                        None => self.event_id(&sid, &parent_key)?,
                    };
                    let Some(parent_id) = parent_id else { continue };
                    let mut p = obj(d.payload.clone());
                    p.insert(if d.kind == "speech.segment" { "recording_event_id" } else { "image_event_id" }.into(), json!(parent_id));
                    d.payload = Value::Object(p);
                    fixed.push(d);
                }
                let cursor = (!blocked).then(|| IngestCursor::new(stream, &end.to_string()));
                let r2 = self.ingest(&IngestBatch { on_missing_parent: None, merge_text: None, removed_source_keys: vec![], payload_revisions: vec![], source_id: sid.clone(), cursor_after: cursor, threads: vec![], events: fixed })?;
                report.inserted += r1.inserted + r2.inserted;
                report.duplicates += r1.duplicates + r2.duplicates;
                report.conflicts += r1.conflicts + r2.conflicts;
                Ok(())
            })();
            for f in &staged {
                let _ = fs::remove_file(f);
            }
            outcome?;
            report.batches += 1;
            index = end;
        }
        report.completed = report.failures.is_empty();
        Ok(report)
    }

    /// One Hub media row as a recording or image event plus its transcript segments or image text. `revision` keys the
    /// derived events to one recognition result (`None` keeps the original migration keys).
    pub(crate) fn hub_media_events(
        &self,
        row: &HubMediaRow,
        vault_root: &Path,
        revision: Option<&str>,
        staged: &mut Vec<PathBuf>,
        copy_original: bool,
    ) -> Result<(Option<IngestThread>, IngestEvent, Vec<IngestEvent>)> {
        let start_ms = parse_iso(row.occurred.as_deref());
        let is_audio = row.kind == "audio";
        let key = format!("media:{}", row.id);
        let mut blobs = Vec::new();
        if copy_original {
            let file = vault_root.join("blobs").join(&row.blob);
            if !file.exists() {
                return Err(TimelineError::BlobMismatch("原件缺失".into()));
            }
            let ext = row.blob.split('.').rfind(|s| !s.is_empty()).unwrap_or("bin");
            let copy = self.staging_directory().join(format!("{}.{ext}", upper_uuid()));
            stage_original(&file, &copy)?;
            staged.push(copy.clone());
            let sha = sha256_hex_file(&copy)?;
            let stem = row.blob.split('.').find(|s| !s.is_empty()).unwrap_or("");
            if stem.chars().count() == 64 && stem != sha {
                return Err(TimelineError::BlobMismatch("原件 hash 与文件名不一致".into()));
            }
            let mut b = IngestBlob::new(&copy.to_string_lossy(), &sha);
            b.mime_type = Some(row.mime.clone());
            blobs.push(b);
        }
        let mut payload = Map::new();
        payload.insert("origin_filename".into(), json!(row.filename));
        payload.insert("hub_media_id".into(), json!(row.id));
        payload.insert("mime_type".into(), json!(row.mime));
        if is_audio {
            let codec = row.mime.split('/').rfind(|s| !s.is_empty()).unwrap_or(&row.mime);
            payload.insert("codec".into(), json!(codec));
        }
        let basis = if start_ms.is_none() { "imported" } else { "source" };
        let thread = is_audio.then(|| IngestThread {
            source_key: key.clone(),
            kind: "recording".into(),
            title: Some(row.filename.clone()),
            started_ms: start_ms,
            ..Default::default()
        });
        let mut parent = IngestEvent::new(&key, if is_audio { "audio.recording" } else { "image.capture" }, start_ms);
        parent.time_basis = basis.into();
        parent.title = Some(row.filename.clone());
        parent.thread_key = is_audio.then(|| key.clone());
        parent.payload = Value::Object(payload);
        parent.blobs = blobs;
        let mut dependents = Vec::new();
        let tag = revision.map(|r| format!(":{r}")).unwrap_or_default();
        if let Some(result) = row.result.as_deref().and_then(json::parse) {
            if is_audio {
                if let Some(Value::Array(segments)) = result.get("segments") {
                    let producer = result.get("producer").cloned().unwrap_or(Value::Null);
                    let model = ["model_id", "model", "name"]
                        .iter()
                        .find_map(|k| producer.get(*k).and_then(Value::as_str))
                        .unwrap_or("unknown")
                        .to_string();
                    let asr_revision = producer.get("revision").or_else(|| producer.get("version")).cloned().unwrap_or(Value::Null);
                    for (i, seg) in segments.iter().enumerate() {
                        let (Some(t), Some(s), Some(e)) = (seg.get("text").and_then(Value::as_str), seg.get("start_seconds"), seg.get("end_seconds")) else {
                            continue;
                        };
                        let sec = |v: &Value| -> f64 {
                            if v.is_f64() {
                                v.as_f64().unwrap_or(0.0)
                            } else {
                                v.as_i64().unwrap_or(0) as f64
                            }
                        };
                        let (ss, es) = (sec(s), sec(e));
                        let mut p = Map::new();
                        p.insert("recording_event_id".into(), json!("pending"));
                        p.insert("recording_source_key".into(), json!(key));
                        p.insert("start_seconds".into(), json!(ss));
                        p.insert("end_seconds".into(), json!(es));
                        p.insert("asr".into(), json!({"model_id": model, "revision": asr_revision}));
                        p.insert("producer".into(), producer.clone());
                        if let Some(sp) = seg.get("speaker").and_then(Value::as_str) {
                            p.insert("speaker_label".into(), json!(sp));
                        }
                        if let Some(ident) = seg.get("identity").and_then(Value::as_str) {
                            p.insert("speaker_candidate".into(), json!(ident));
                        }
                        let mut d = IngestEvent::new(&format!("media-seg:{}{tag}:{i}", row.id), "speech.segment", start_ms.map(|m| m + (ss * 1000.0) as i64));
                        d.end_ms = start_ms.map(|m| m + (es * 1000.0) as i64);
                        d.time_basis = basis.into();
                        d.title = Some(row.filename.clone());
                        d.thread_key = Some(key.clone());
                        d.parent_key = Some(key.clone());
                        d.payload = Value::Object(p);
                        d.text = vec![IngestText::new("transcript", t)];
                        dependents.push(d);
                    }
                }
            } else if let Some(t) = result.get("text").and_then(Value::as_str).or_else(|| result.get("markdown").and_then(Value::as_str)) {
                if !t.is_empty() {
                    let mut d = IngestEvent::new(&format!("media-text:{}{tag}", row.id), "image.text", start_ms);
                    d.time_basis = basis.into();
                    d.title = Some(row.filename.clone());
                    d.parent_key = Some(key.clone());
                    d.payload = json!({"image_event_id": "pending", "image_source_key": key, "method": "vlm"});
                    d.text = vec![IngestText::new("vlm", t)];
                    dependents.push(d);
                }
            }
        }
        Ok((thread, parent, dependents))
    }

    // MARK: Hub device observations → timeline

    /// Maps Hub `observations` by `event_type` (unknown types are kept losslessly as `marker`).
    pub fn migrate_hub_observations(&self, hub_database: &Path, source_id: Option<&str>, batch_size: usize, max_batches: Option<usize>) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();
        let sid = self.find_or_create_source("device.pendant", OBSERVATION_SOURCE_NAME, source_id, Some("hub-observations"))?;
        report.source_id = sid.clone();
        let hub = Db::open(hub_database, true, false)?;
        let stream = "hub.observations";
        let mut last_rowid = self.cursor(&sid, stream)?.and_then(|c| c.parse::<i64>().ok()).unwrap_or(0);
        loop {
            if max_batches.is_some_and(|m| report.batches >= m) {
                return Ok(report);
            }
            let rows = hub.query(
                "SELECT rowid, device_id, boot_id, seq, event_type, wall_time_utc, time_uncertainty_ms, payload_json FROM observations
                WHERE rowid>? ORDER BY rowid LIMIT ?",
                &[int(last_rowid), int(batch_size as i64)],
                |r| (r.int(0), r.text(1), r.text(2), r.int(3), r.text(4), r.opt_text(5), r.opt_int(6), r.text(7)),
            )?;
            if rows.is_empty() {
                break;
            }
            let mut events = Vec::new();
            for o in &rows {
                let t = o.4.to_lowercase();
                let start = parse_iso(o.5.as_deref());
                let original = json::parse(&o.7).unwrap_or_else(|| json!({}));
                let mut payload = Map::new();
                payload.insert("original_event_type".into(), json!(o.4));
                payload.insert("original".into(), original);
                payload.insert("device_id".into(), json!(o.1));
                payload.insert("boot_id".into(), json!(o.2));
                payload.insert("seq".into(), json!(o.3));
                let kind = if t.contains("gap") || t.contains("dropped") || t.contains("missing") {
                    payload.insert("reason_code".into(), json!("source_offline"));
                    "coverage.gap"
                } else if ["power", "battery", "boot", "sleep", "state"].iter().any(|k| t.contains(k)) {
                    let change = if t.contains("sleep") {
                        "sleep"
                    } else if t.contains("power") || t.contains("battery") {
                        "power"
                    } else {
                        "session"
                    };
                    payload.insert("change".into(), json!(change));
                    payload.insert("value".into(), json!(o.4));
                    "system.state"
                } else {
                    payload.insert("label".into(), json!(o.4));
                    "marker"
                };
                let mut e = IngestEvent::new(&format!("{}/{}/{}", o.1, o.2, o.3), kind, start);
                e.time_basis = if start.is_none() { "imported" } else { "source" }.into();
                e.time_uncertainty_ms = o.6;
                e.actor = "system".into();
                e.payload = Value::Object(payload);
                events.push(e);
            }
            last_rowid = rows[rows.len() - 1].0;
            let r = self.ingest(&IngestBatch {
                on_missing_parent: None,
                merge_text: None, removed_source_keys: vec![], payload_revisions: vec![],
                source_id: sid.clone(),
                cursor_after: Some(IngestCursor::new(stream, &last_rowid.to_string())),
                threads: vec![],
                events,
            })?;
            report.inserted += r.inserted;
            report.duplicates += r.duplicates;
            report.conflicts += r.conflicts;
            report.batches += 1;
        }
        report.completed = true;
        Ok(report)
    }
}
