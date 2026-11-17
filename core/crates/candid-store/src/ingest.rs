//! §6.1 ingest: one transaction per batch, idempotent on `(source_id, source_key)`.

use crate::chunk::{TextChunk, TextChunker};
use crate::db::{boolean, int, opt_int, opt_text, text, RowExt, P};
use crate::ids::{sha256_hex, sha256_hex_file, uuid_v7};
use crate::json;
use crate::schema;
use crate::store::{mkdir_private, standardize, TimelineStore};
use crate::types::{IngestBatch, IngestBlob, IngestEvent, IngestTextPart, SourcePolicy};
use crate::{Result, TimelineError};
use rusqlite::types::Value as SqlValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const TEXT_FIELDS: &[&str] = &["prompt", "body", "selection", "title", "transcript", "ocr", "vlm", "message", "subject", "note", "quoted"];
const TIME_BASES: &[&str] = &["observed", "source", "imported", "estimated"];
const ACTORS: &[&str] = &["self", "other", "agent", "system", "unknown"];
const SENSITIVITIES: &[&str] = &["normal", "sensitive", "protected"];
const THREAD_KINDS: &[&str] = &["agent_session", "ai_session", "mail_thread", "meeting", "recording", "calendar_series", "chat"];
const BLOB_ROLES: &[&str] = &["original", "preview", "derived"];

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct IngestResult {
    pub inserted: usize,
    pub duplicates: usize,
    pub conflicts: usize,
    pub tombstoned: usize,
    /// Text samples folded into an earlier event (`merge_text`).
    pub merged: usize,
    /// Events deleted because the source removed them (`removed_source_keys`).
    pub removed: usize,
    /// Stored events whose payload changed (`payload_revisions`).
    pub revised: usize,
    pub threads_inserted: usize,
    pub threads_updated: usize,
    pub blobs_stored: usize,
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    /// source_key → event_id for every event now present in the store (inserted or duplicate).
    pub event_ids: BTreeMap<String, String>,
}

#[derive(Clone)]
pub(crate) struct PreparedText {
    field: String,
    ord: i64,
    chunk: TextChunk,
    visible: Option<bool>,
    focus_distance: Option<i64>,
}

#[derive(Clone)]
pub(crate) struct PreparedBlob {
    blob: IngestBlob,
    mime: String,
    staging: PathBuf,
    byte_count: i64,
}

#[derive(Clone)]
pub(crate) struct PreparedEvent {
    pub(crate) event: IngestEvent,
    pub(crate) texts: Vec<PreparedText>,
    blobs: Vec<PreparedBlob>,
    incoming_json: String,
}

pub(crate) struct SourceRow {
    pub status: String,
    pub policy: SourcePolicy,
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

pub(crate) fn guess_mime(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "heic" => "image/heic",
        "webp" => "image/webp",
        "m4a" | "mp4" => "audio/mp4",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "caf" => "audio/x-caf",
        _ => "application/octet-stream",
    }
}

fn safe_extension(ext: &str) -> String {
    let e = ext.to_lowercase();
    if e.chars().count() <= 8 && e.chars().all(|c| c.is_alphanumeric()) {
        e
    } else {
        String::new()
    }
}

fn opt_str(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("-")
}

fn opt_num(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
}

#[allow(clippy::too_many_arguments)]
fn fingerprint_text(scalars: [String; 16], texts: &[String], blobs: &[String]) -> String {
    let mut parts: Vec<&str> = scalars.iter().map(String::as_str).collect();
    parts.push("#t");
    parts.extend(texts.iter().map(String::as_str));
    parts.push("#b");
    parts.extend(blobs.iter().map(String::as_str));
    sha256_hex(parts.join("\u{1f}"))
}

impl TimelineStore {
    /// One transaction: threads, events, chunks, blob references, change rows, then the cursor (§6.1).
    /// Any failure rolls the whole batch back; re-sending is safe because `(source_id, source_key)` is idempotent.
    pub fn ingest(&self, batch: &IngestBatch) -> Result<IngestResult> {
        let source = self.source_row(&batch.source_id)?.ok_or_else(|| TimelineError::UnknownSource(batch.source_id.clone()))?;
        if source.status != "active" && source.status != "error" {
            return Err(TimelineError::SourceNotWritable(format!("{} {}", batch.source_id, source.status)));
        }
        for thread in &batch.threads {
            if !THREAD_KINDS.contains(&thread.kind.as_str()) {
                return Err(TimelineError::InvalidInput(format!("thread kind {}", thread.kind)));
            }
            if thread.source_key.is_empty() {
                return Err(TimelineError::InvalidInput("thread source_key 为空".into()));
            }
        }
        let detach = match batch.on_missing_parent.as_deref() {
            None | Some("error") => false,
            Some("detach") => true,
            Some(other) => return Err(TimelineError::InvalidInput(format!("on_missing_parent {other}"))),
        };
        let keys: HashSet<&str> = batch.events.iter().map(|e| e.source_key.as_str()).collect();
        let mut events = Vec::with_capacity(batch.events.len());
        for event in &batch.events {
            let mut event = event.clone();
            if detach {
                if let Some(parent) = &event.parent_key {
                    if !keys.contains(parent.as_str()) && self.resolve_key(&batch.source_id, parent, event.start_ms)?.is_none() {
                        event.parent_key = None;
                    }
                }
            }
            events.push(event);
        }
        let mut prepared = Vec::new();
        for event in &events {
            validate(event)?;
            if is_excluded(event, &source.policy) {
                continue;
            }
            prepared.push(self.prepare(event, &source.policy)?);
        }
        for r in &batch.payload_revisions {
            if r.source_key.is_empty() || !r.payload.is_object() {
                return Err(TimelineError::InvalidInput("payload_revisions 无效".into()));
            }
        }
        let mut created_files: Vec<PathBuf> = Vec::new();
        let mut removed_files: Vec<PathBuf> = Vec::new();
        let outcome = self.db.transaction(|| {
            self.db.execute("PRAGMA defer_foreign_keys=ON", &[])?;
            let mut result = self.ingest_in_transaction(batch, &prepared, &mut created_files, true)?;
            self.apply_source_changes(batch, &mut result, &mut removed_files)?;
            Ok(result)
        });
        match outcome {
            Ok(result) => {
                for f in &removed_files {
                    let _ = fs::remove_file(f);
                }
                for p in &prepared {
                    for b in &p.blobs {
                        let _ = fs::remove_file(&b.staging); // leftover for duplicates
                    }
                }
                Ok(result)
            }
            Err(TimelineError::SimulatedCrash) => Err(TimelineError::SimulatedCrash),
            Err(e) => {
                for f in &created_files {
                    let _ = fs::remove_file(f);
                }
                Err(e)
            }
        }
    }

    pub(crate) fn source_row(&self, id: &str) -> Result<Option<SourceRow>> {
        self.db.first("SELECT kind,status,policy_json FROM sources WHERE source_id=?", &[text(id)], |r| SourceRow {
            status: r.text(1),
            policy: serde_json::from_str(&r.text(2)).unwrap_or_default(),
        })
    }

    pub(crate) fn prepare(&self, e: &IngestEvent, policy: &SourcePolicy) -> Result<PreparedEvent> {
        let protected = e.sensitivity == "protected";
        let mut texts = Vec::new();
        if policy.store_text && !protected {
            let mut ord_by_field: HashMap<&str, i64> = HashMap::new();
            for t in &e.text {
                let mut base = 0usize; // scalar offset of the part within the field's concatenated text
                let single = [IngestTextPart { content: t.content.clone(), visible: t.visible, focus_distance: t.focus_distance }];
                let parts: &[IngestTextPart] = t.parts.as_deref().unwrap_or(&single);
                for part in parts {
                    for mut chunk in TextChunker::chunk(&part.content) {
                        let ord = ord_by_field.entry(t.field.as_str()).or_insert(0);
                        chunk.char_start += base;
                        texts.push(PreparedText {
                            field: t.field.clone(),
                            ord: *ord,
                            chunk,
                            visible: part.visible,
                            focus_distance: part.focus_distance,
                        });
                        *ord += 1;
                    }
                    base += TextChunker::normalise(&part.content).chars().count() + 2;
                }
            }
        }
        let staging_root = format!("{}/", standardize(&self.staging_directory()).display());
        let mut blobs = Vec::new();
        if policy.store_media && !protected {
            for b in &e.blobs {
                let path = standardize(Path::new(&b.staging_path));
                if !path.to_string_lossy().starts_with(&staging_root) {
                    return Err(TimelineError::InvalidInput("staging_path 必须位于数据根目录的 staging/ 内".into()));
                }
                if !path.exists() {
                    return Err(TimelineError::BlobMismatch(format!(
                        "暂存文件不存在: {}",
                        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    )));
                }
                let actual = sha256_hex_file(&path)?;
                if actual != b.sha256 {
                    return Err(TimelineError::BlobMismatch(format!("声明 {} 实际 {}", &b.sha256[..12], &actual[..12])));
                }
                let size = fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
                let mime = b.mime_type.clone().unwrap_or_else(|| guess_mime(&ext_of(&path)).to_string());
                blobs.push(PreparedBlob { blob: b.clone(), mime, staging: path, byte_count: size });
            }
        } else {
            for b in &e.blobs {
                let path = standardize(Path::new(&b.staging_path));
                if path.to_string_lossy().starts_with(&staging_root) {
                    let _ = fs::remove_file(&path); // dropped by policy; never kept
                }
            }
        }
        let mut stored = e.clone();
        if protected || !policy.store_text {
            stored.text.clear();
            if let Some(p) = stored.payload.as_object_mut() { p.remove("record_evidence"); }
        }
        Ok(PreparedEvent { incoming_json: json::canonical_of(e), event: stored, texts, blobs })
    }

    fn ingest_in_transaction(&self, batch: &IngestBatch, prepared: &[PreparedEvent], created_files: &mut Vec<PathBuf>, mark_synced: bool) -> Result<IngestResult> {
        let mut result = IngestResult::default();
        let now = self.clock.now_ms();
        let sid = &batch.source_id;
        // threads
        let mut thread_ids: HashMap<String, String> = HashMap::new();
        for t in &batch.threads {
            let participants = json::canonical_of(&t.participants.clone().unwrap_or_default());
            let payload = json::canonical(t.payload.as_ref().unwrap_or(&serde_json::json!({})));
            let existing = self.db.first(
                "SELECT thread_id,title,started_ms,ended_ms,participants_json,payload_json FROM threads WHERE source_id=? AND source_key=?",
                &[text(sid), text(&t.source_key)],
                |r| (r.text(0), r.opt_text(1), r.opt_int(2), r.opt_int(3), r.text(4), r.text(5)),
            )?;
            if let Some(existing) = existing {
                thread_ids.insert(t.source_key.clone(), existing.0.clone());
                let title = t.title.clone().or(existing.1.clone());
                let started = t.started_ms.or(existing.2);
                let ended = t.ended_ms.or(existing.3);
                let part = if t.participants.is_none() { existing.4.clone() } else { participants };
                let pay = if t.payload.is_none() { existing.5.clone() } else { payload };
                if title != existing.1 || started != existing.2 || ended != existing.3 || part != existing.4 || pay != existing.5 {
                    self.db.execute(
                        "UPDATE threads SET title=?,started_ms=?,ended_ms=?,participants_json=?,payload_json=? WHERE thread_id=?",
                        &[opt_text(title), opt_int(started), opt_int(ended), text(part), text(pay), text(&existing.0)],
                    )?;
                    self.record_change("thread", &existing.0, "update")?;
                    result.threads_updated += 1;
                }
            } else {
                let id = uuid_v7(Some(t.started_ms.unwrap_or(now)));
                self.db.execute(
                    "INSERT INTO threads(thread_id,source_id,source_key,kind,title,started_ms,ended_ms,participants_json,payload_json) VALUES(?,?,?,?,?,?,?,?,?)",
                    &[
                        text(&id),
                        text(sid),
                        text(&t.source_key),
                        text(&t.kind),
                        opt_text(t.title.clone()),
                        opt_int(t.started_ms),
                        opt_int(t.ended_ms),
                        text(participants),
                        text(payload),
                    ],
                )?;
                thread_ids.insert(t.source_key.clone(), id.clone());
                self.record_change("thread", &id, "insert")?;
                result.threads_inserted += 1;
            }
        }
        // decide per event
        let merging = batch.merge_text.unwrap_or(false);
        let merge_config = if merging { self.text_merge_config() } else { Default::default() };
        let mut merge_cache: HashMap<String, i64> = HashMap::new();
        let mut plans: Vec<(PreparedEvent, String)> = Vec::new();
        let mut batch_ids: HashMap<String, String> = HashMap::new(); // source_key → event_id inserted in this batch
        for p in prepared {
            let e = &p.event;
            let key_token = tombstone_key(sid, &e.source_key);
            if self.db.scalar("SELECT COUNT(*) FROM tombstones WHERE entity='event_key' AND entity_id=?", &[text(&key_token)])? > 0 {
                result.tombstoned += 1;
                continue;
            }
            let in_batch = batch_ids.get(&e.source_key).cloned();
            let existing = match &in_batch {
                Some(id) => Some(id.clone()),
                None => self.event_id(sid, &e.source_key)?,
            };
            // A sample an earlier event absorbed (a resend): nothing new.
            if existing.is_none() && merging {
                if let Some(id) = self.merged_event(sid, &e.source_key, e.start_ms)? {
                    result.event_ids.insert(e.source_key.clone(), id);
                    result.duplicates += 1;
                    continue;
                }
            }
            if let Some(existing_id) = existing {
                result.event_ids.insert(e.source_key.clone(), existing_id.clone());
                let mut identical = if in_batch.is_some() {
                    true
                } else {
                    // An event that absorbed later samples has changed since it was first sent.
                    self.fingerprint_stored(&existing_id)? == fingerprint_incoming(p) || (merging && self.has_merge_record(&existing_id)?)
                };
                if in_batch.is_some() {
                    if let Some((planned, _)) = plans.iter().find(|(_, id)| *id == existing_id) {
                        identical = fingerprint_incoming(planned) == fingerprint_incoming(p);
                    }
                }
                if identical {
                    result.duplicates += 1;
                } else {
                    self.record_conflict(sid, p, &existing_id, now)?;
                    result.conflicts += 1;
                }
                continue;
            }
            if merging {
                if let Some(id) = self.merge_text(sid, p, &mut plans, &merge_config, now, &mut merge_cache)? {
                    batch_ids.insert(e.source_key.clone(), id.clone());
                    result.event_ids.insert(e.source_key.clone(), id);
                    result.merged += 1;
                    continue;
                }
            }
            let id = uuid_v7(Some(e.start_ms.unwrap_or(now)));
            batch_ids.insert(e.source_key.clone(), id.clone());
            result.event_ids.insert(e.source_key.clone(), id.clone());
            plans.push((p.clone(), id));
        }
        if !plans.is_empty() {
            let mut seq = self.next_ingest_seq(plans.len())?;
            result.first_seq = Some(seq);
            let mut chunk_cache: HashMap<String, i64> = HashMap::new();
            #[allow(clippy::explicit_counter_loop)]
            for (p, event_id) in &plans {
                let e = &p.event;
                let mut parent_id = None;
                if let Some(parent_key) = &e.parent_key {
                    parent_id = match batch_ids.get(parent_key) {
                        Some(id) => Some(id.clone()),
                        None => self.resolve_key(sid, parent_key, e.start_ms)?,
                    };
                    if parent_id.is_none() {
                        return Err(TimelineError::MissingParent(format!("{} → {}", e.source_key, parent_key)));
                    }
                }
                let mut thread_id = None;
                if let Some(key) = &e.thread_key {
                    thread_id = match thread_ids.get(key) {
                        Some(id) => Some(id.clone()),
                        None => self.db.first("SELECT thread_id FROM threads WHERE source_id=? AND source_key=?", &[text(sid), text(key)], |r| r.text(0))?,
                    };
                    if thread_id.is_none() {
                        return Err(TimelineError::InvalidInput(format!("thread_key {key} 不存在")));
                    }
                }
                self.db.execute(
                    "INSERT INTO events(event_id,source_id,source_key,kind,start_ms,end_ms,tz_offset_min,time_basis,time_uncertainty_ms,app_bundle_id,app_name,
                      title,url,thread_id,parent_event_id,actor,sensitivity,payload_version,payload_json,ingest_seq,ingested_ms)
                    VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    &[
                        text(event_id),
                        text(sid),
                        text(&e.source_key),
                        text(&e.kind),
                        opt_int(e.start_ms),
                        opt_int(e.end_ms),
                        opt_int(e.tz_offset_min),
                        text(&e.time_basis),
                        opt_int(e.time_uncertainty_ms),
                        opt_text(e.app_bundle_id.clone()),
                        opt_text(e.app_name.clone()),
                        opt_text(e.title.clone()),
                        opt_text(e.url.clone()),
                        opt_text(thread_id),
                        opt_text(parent_id),
                        text(&e.actor),
                        text(&e.sensitivity),
                        int(e.payload_version),
                        text(json::canonical(&e.payload)),
                        int(seq),
                        int(now),
                    ],
                )?;
                let rowid = self.db.last_insert_rowid();
                self.db.execute(
                    "INSERT INTO meta_fts(rowid,title,app_name,url) VALUES(?,?,?,?)",
                    &[int(rowid), opt_text(e.title.clone()), opt_text(e.app_name.clone()), opt_text(e.url.clone())],
                )?;
                self.insert_text(p, event_id, sid, now, &mut chunk_cache)?;
                for b in &p.blobs {
                    if self.store_blob(b, event_id, now, created_files)? {
                        result.blobs_stored += 1;
                    }
                }
                self.record_change("event", event_id, "insert")?;
                result.inserted += 1;
                result.last_seq = Some(seq);
                seq += 1;
            }
        }
        if let Some(hook) = &self.hooks.ingest {
            hook("beforeCommit")?;
        }
        if let Some(cursor) = &batch.cursor_after {
            self.db.execute(
                "INSERT INTO source_cursors(source_id,stream,cursor,updated_ms) VALUES(?,?,?,?)
                ON CONFLICT(source_id,stream) DO UPDATE SET cursor=excluded.cursor, updated_ms=excluded.updated_ms",
                &[text(sid), text(&cursor.stream), text(&cursor.cursor), int(now)],
            )?;
        }
        if mark_synced { self.db.execute(
            "UPDATE sources SET last_sync_ms=?, status=CASE WHEN status='error' THEN 'active' ELSE status END, last_error=NULL WHERE source_id=?",
            &[int(now), text(sid)],
        )?; }
        Ok(result)
    }

    // MARK: Text

    pub(crate) fn insert_text(&self, p: &PreparedEvent, event_id: &str, source_id: &str, now: i64, cache: &mut HashMap<String, i64>) -> Result<()> {
        if p.texts.is_empty() {
            return Ok(());
        }
        let e = &p.event;
        let mut previous_chunks: HashSet<i64> = HashSet::new();
        let screen_text = e.kind == "screen.text";
        if screen_text {
            let t = e.start_ms.unwrap_or(now);
            let prev = self.db.first(
                "SELECT event_id FROM events WHERE source_id=? AND kind='screen.text' AND app_bundle_id IS ? AND title IS ?
                  AND COALESCE(start_ms,ingested_ms)<=? AND event_id<>? ORDER BY COALESCE(start_ms,ingested_ms) DESC, ingest_seq DESC LIMIT 1",
                &[text(source_id), opt_text(e.app_bundle_id.clone()), opt_text(e.title.clone()), int(t), text(event_id)],
                |r| r.text(0),
            )?;
            if let Some(prev) = prev {
                previous_chunks = self.db.query("SELECT chunk_id FROM event_text WHERE event_id=?", &[text(&prev)], |r| r.int(0))?.into_iter().collect();
            }
        }
        for t in &p.texts {
            let chunk_id = if let Some(cached) = cache.get(&t.chunk.hash) {
                *cached
            } else if let Some(existing) = self.db.first("SELECT chunk_id FROM text_chunks WHERE chunk_hash=?", &[text(&t.chunk.hash)], |r| r.int(0))? {
                cache.insert(t.chunk.hash.clone(), existing);
                existing
            } else {
                self.db.execute(
                    "INSERT INTO text_chunks(chunk_hash,text,char_count,lang,first_seen_ms) VALUES(?,?,?,?,?)",
                    &[text(&t.chunk.hash), text(&t.chunk.text), int(t.chunk.scalar_count as i64), opt_text(t.chunk.lang), int(now)],
                )?;
                let id = self.db.last_insert_rowid();
                cache.insert(t.chunk.hash.clone(), id);
                self.db.execute("INSERT INTO text_fts(rowid,text) VALUES(?,?)", &[int(id), text(&t.chunk.text)])?;
                id
            };
            let is_new = !screen_text || !previous_chunks.contains(&chunk_id);
            self.db.execute(
                "INSERT INTO event_text(event_id,field,ord,chunk_id,visible,focus_distance,is_new,char_start) VALUES(?,?,?,?,?,?,?,?)",
                &[
                    text(event_id),
                    text(&t.field),
                    int(t.ord),
                    int(chunk_id),
                    t.visible.map(boolean).unwrap_or(SqlValue::Null),
                    opt_int(t.focus_distance),
                    boolean(is_new),
                    int(t.chunk.char_start as i64),
                ],
            )?;
        }
        Ok(())
    }

    // MARK: Blobs (staging → hash → atomic move)

    /// Returns true when a new blob row (and file) was created.
    fn store_blob(&self, b: &PreparedBlob, event_id: &str, now: i64, created_files: &mut Vec<PathBuf>) -> Result<bool> {
        let sha = &b.blob.sha256;
        let mut created = false;
        if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE sha256=?", &[text(sha)])? == 0 {
            let ext = safe_extension(&ext_of(&b.staging));
            let rel = format!("blobs/sha256/{}/{}{}", &sha[..2], sha, if ext.is_empty() { String::new() } else { format!(".{ext}") });
            let dest = self.root.join(&rel);
            if let Some(dir) = dest.parent() {
                mkdir_private(dir)?;
            }
            if dest.exists() {
                fs::remove_file(&dest)?; // leftover from a crashed attempt, never referenced
            }
            fs::rename(&b.staging, &dest)?;
            created_files.push(dest.clone());
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o600));
            if let Ok(f) = fs::OpenOptions::new().write(true).open(&dest) {
                // Plain fsync, as Swift's FileHandle.synchronize(): std's sync_all is F_FULLFSYNC on Apple platforms,
                // a full device flush per blob that made the first import twice as slow.
                use std::os::unix::io::AsRawFd;
                // SAFETY: fsync on a descriptor owned by `f`.
                unsafe { libc::fsync(f.as_raw_fd()) };
            }
            if let Some(hook) = &self.hooks.ingest {
                hook("afterBlobMove")?;
            }
            self.db.execute(
                "INSERT INTO blobs(sha256,byte_count,mime_type,rel_path,pixel_w,pixel_h,duration_ms,created_ms) VALUES(?,?,?,?,?,?,?,?)",
                &[
                    text(sha),
                    int(b.byte_count),
                    text(&b.mime),
                    text(&rel),
                    opt_int(b.blob.pixel_w),
                    opt_int(b.blob.pixel_h),
                    opt_int(b.blob.duration_ms),
                    int(now),
                ],
            )?;
            self.record_change("blob", sha, "insert")?;
            created = true;
        }
        self.db.execute("INSERT OR IGNORE INTO event_blobs(event_id,sha256,role) VALUES(?,?,?)", &[text(event_id), text(sha), text(&b.blob.role)])?;
        Ok(created)
    }

    // MARK: Idempotency fingerprint

    pub(crate) fn fingerprint_stored(&self, id: &str) -> Result<String> {
        let row = self
            .db
            .first(
                "SELECT e.kind,e.start_ms,e.end_ms,e.tz_offset_min,e.time_basis,e.time_uncertainty_ms,e.app_bundle_id,e.app_name,e.title,e.url,
                   (SELECT source_key FROM threads WHERE thread_id=e.thread_id),(SELECT source_key FROM events WHERE event_id=e.parent_event_id),
                   e.actor,e.sensitivity,e.payload_version,e.payload_json FROM events e WHERE e.event_id=?",
                &[text(id)],
                |r| {
                    let payload = r.text(15);
                    [
                        r.text(0),
                        opt_num(r.opt_int(1)),
                        opt_num(r.opt_int(2)),
                        opt_num(r.opt_int(3)),
                        r.text(4),
                        opt_num(r.opt_int(5)),
                        opt_str(&r.opt_text(6)).to_string(),
                        opt_str(&r.opt_text(7)).to_string(),
                        opt_str(&r.opt_text(8)).to_string(),
                        opt_str(&r.opt_text(9)).to_string(),
                        opt_str(&r.opt_text(10)).to_string(),
                        opt_str(&r.opt_text(11)).to_string(),
                        r.text(12),
                        r.text(13),
                        r.int(14).to_string(),
                        json::parse(&payload).map(|v| json::canonical(&v)).unwrap_or(payload),
                    ]
                },
            )?
            .ok_or_else(|| TimelineError::NotFound(id.into()))?;
        let texts = self.db.query(
            "SELECT t.field,t.ord,c.chunk_hash,t.visible,t.focus_distance FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id
            WHERE t.event_id=? ORDER BY t.rowid",
            &[text(id)],
            |r| format!("{}|{}|{}|{}|{}", r.text(0), r.int(1), r.text(2), opt_num(r.opt_int(3)), opt_num(r.opt_int(4))),
        )?;
        let mut blobs = self.db.query("SELECT sha256,role FROM event_blobs WHERE event_id=?", &[text(id)], |r| format!("{}|{}", r.text(0), r.text(1)))?;
        blobs.sort();
        Ok(fingerprint_text(row, &texts, &blobs))
    }

    fn record_conflict(&self, source_id: &str, p: &PreparedEvent, existing_id: &str, now: i64) -> Result<()> {
        let key = &p.event.source_key;
        if self.db.scalar(
            "SELECT COUNT(*) FROM ingest_conflicts WHERE source_id=? AND source_key=? AND resolved_ms IS NULL AND incoming_json=?",
            &[text(source_id), text(key), text(&p.incoming_json)],
        )? > 0
        {
            return Ok(());
        }
        let mut at = now;
        while self.db.scalar(
            "SELECT COUNT(*) FROM ingest_conflicts WHERE source_id=? AND source_key=? AND detected_ms=?",
            &[text(source_id), text(key), int(at)],
        )? > 0
        {
            at += 1;
        }
        self.db.execute(
            "INSERT INTO ingest_conflicts(source_id,source_key,existing_event_id,incoming_json,detected_ms) VALUES(?,?,?,?,?)",
            &[text(source_id), text(key), text(existing_id), text(&p.incoming_json), int(at)],
        )
    }

    pub fn conflict_count(&self, source_id: Option<&str>) -> Result<i64> {
        let p: P = opt_text(source_id);
        self.db.scalar("SELECT COUNT(*) FROM ingest_conflicts WHERE resolved_ms IS NULL AND (? IS NULL OR source_id=?)", &[p.clone(), p])
    }
}

pub(crate) fn tombstone_key(source_id: &str, source_key: &str) -> String {
    format!("{source_id}:{}", sha256_hex(source_key))
}

fn fingerprint_incoming(p: &PreparedEvent) -> String {
    let e = &p.event;
    let texts: Vec<String> = p
        .texts
        .iter()
        .map(|t| format!("{}|{}|{}|{}|{}", t.field, t.ord, t.chunk.hash, opt_num(t.visible.map(|v| v as i64)), opt_num(t.focus_distance)))
        .collect();
    let mut blobs: Vec<String> = p.blobs.iter().map(|b| format!("{}|{}", b.blob.sha256, b.blob.role)).collect();
    blobs.sort();
    fingerprint_text(
        [
            e.kind.clone(),
            opt_num(e.start_ms),
            opt_num(e.end_ms),
            opt_num(e.tz_offset_min),
            e.time_basis.clone(),
            opt_num(e.time_uncertainty_ms),
            opt_str(&e.app_bundle_id).into(),
            opt_str(&e.app_name).into(),
            opt_str(&e.title).into(),
            opt_str(&e.url).into(),
            opt_str(&e.thread_key).into(),
            opt_str(&e.parent_key).into(),
            e.actor.clone(),
            e.sensitivity.clone(),
            e.payload_version.to_string(),
            json::canonical(&e.payload),
        ],
        &texts,
        &blobs,
    )
}

fn validate(e: &IngestEvent) -> Result<()> {
    crate::records::validate_payload(e)?;
    if e.source_key.is_empty() {
        return Err(TimelineError::InvalidInput("source_key 为空".into()));
    }
    let kind = schema::kind(&e.kind).ok_or_else(|| TimelineError::InvalidInput(format!("未知事件种类 {}", e.kind)))?;
    if !TIME_BASES.contains(&e.time_basis.as_str()) {
        return Err(TimelineError::InvalidInput(format!("time_basis {}", e.time_basis)));
    }
    if !ACTORS.contains(&e.actor.as_str()) {
        return Err(TimelineError::InvalidInput(format!("actor {}", e.actor)));
    }
    if !SENSITIVITIES.contains(&e.sensitivity.as_str()) {
        return Err(TimelineError::InvalidInput(format!("sensitivity {}", e.sensitivity)));
    }
    if e.payload_version != 1 {
        return Err(TimelineError::InvalidInput(format!("{} payload_version {} 不受支持", e.kind, e.payload_version)));
    }
    let payload = e.payload.as_object().ok_or_else(|| TimelineError::InvalidInput(format!("{} payload 必须是对象", e.kind)))?;
    for key in kind.required {
        if payload.get(*key).map(|v| v.is_null()).unwrap_or(true) {
            return Err(TimelineError::InvalidInput(format!("{} payload 缺少 {key}", e.kind)));
        }
    }
    if e.start_ms.is_none() && e.time_basis != "imported" {
        return Err(TimelineError::InvalidInput("没有开始时间的事件 time_basis 必须是 imported".into()));
    }
    if let (Some(s), Some(end)) = (e.start_ms, e.end_ms) {
        if end < s {
            return Err(TimelineError::InvalidInput("end_ms 早于 start_ms".into()));
        }
    }
    for t in &e.text {
        if !TEXT_FIELDS.contains(&t.field.as_str()) {
            return Err(TimelineError::InvalidInput(format!("文字字段 {}", t.field)));
        }
    }
    for b in &e.blobs {
        if !BLOB_ROLES.contains(&b.role.as_str()) {
            return Err(TimelineError::InvalidInput(format!("blob role {}", b.role)));
        }
        if b.sha256.len() != 64 || !b.sha256.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
            return Err(TimelineError::InvalidInput("sha256 格式".into()));
        }
    }
    Ok(())
}

fn is_excluded(e: &IngestEvent, policy: &SourcePolicy) -> bool {
    if let Some(app) = &e.app_bundle_id {
        if policy.exclude.apps.contains(app) {
            return true;
        }
    }
    if let Some(thread) = &e.thread_key {
        if policy.exclude.threads.contains(thread) {
            return true;
        }
    }
    if !policy.exclude.patterns.is_empty() {
        let mut parts: Vec<&str> = [e.title.as_deref(), e.url.as_deref()].into_iter().flatten().collect();
        parts.extend(e.text.iter().map(|t| t.content.as_str()));
        let haystack = parts.join("\n").to_lowercase();
        for pattern in &policy.exclude.patterns {
            if !pattern.is_empty() && haystack.contains(&pattern.to_lowercase()) {
                return true;
            }
        }
    }
    false
}

impl TimelineStore {
    /// `removed_source_keys` and `payload_revisions` of one batch, inside its transaction (after its events, so a key
    /// both added and removed in one batch ends removed).
    fn apply_source_changes(&self, batch: &IngestBatch, result: &mut IngestResult, files: &mut Vec<PathBuf>) -> Result<()> {
        let sid = &batch.source_id;
        let now = self.clock.now_ms();
        for r in &batch.payload_revisions {
            let Some((id, kind, old)) = self.db.first(
                "SELECT event_id,kind,payload_json FROM events WHERE source_id=? AND source_key=?",
                &[text(sid), text(&r.source_key)],
                |row| (row.text(0), row.text(1), row.text(2)),
            )?
            else {
                continue;
            };
            let mut payload = r.payload.clone();
            let kind_def = schema::kind(&kind).ok_or_else(|| TimelineError::InvalidInput(format!("未知事件种类 {kind}")))?;
            for key in kind_def.required {
                if payload.get(*key).map(|v| v.is_null()).unwrap_or(true) {
                    return Err(TimelineError::InvalidInput(format!("{kind} payload 缺少 {key}")));
                }
            }
            let mut previous: serde_json::Value = serde_json::from_str(&old).unwrap_or_default();
            let previous_rev = previous.get("revision").and_then(|v| v.as_i64()).unwrap_or(0);
            previous.as_object_mut().map(|o| o.remove("revision"));
            if json::canonical(&previous) == json::canonical(&payload) {
                continue;
            }
            let last = self.db.scalar("SELECT COALESCE(MAX(revision),0) FROM agent_event_revisions WHERE event_id=?", &[text(&id)])?;
            let rev = last.max(previous_rev) + 1;
            self.db.execute("INSERT INTO agent_event_revisions VALUES(?,?,?,?)", &[text(&id), int(rev), text(old), int(now)])?;
            payload["revision"] = serde_json::json!(rev);
            self.db.execute("UPDATE events SET payload_json=? WHERE event_id=?", &[text(json::canonical(&payload)), text(&id)])?;
            self.record_change("event", &id, "update")?;
            result.revised += 1;
        }
        if !batch.removed_source_keys.is_empty() {
            let mut ids = Vec::new();
            for key in &batch.removed_source_keys {
                if let Some(id) = self.event_id(sid, key)? {
                    result.event_ids.remove(key);
                    ids.push(id);
                }
            }
            if !ids.is_empty() {
                let report = self.delete_in_transaction(&ids, "source_deleted", files)?;
                result.removed += report.events;
            }
        }
        Ok(())
    }

    /// Private connector path: controlled revisions, preserving event identity and all previous admitted snapshots.
    /// Cursor and every revision commit together. General ingest still reports changed evidence as conflicts.
    pub(crate) fn ingest_agent_batch(
        &self,
        batch: &IngestBatch,
        cursors: &[crate::IngestCursor],
        delta: &crate::agents::CheckpointDelta,
    ) -> Result<(usize, usize)> {
        let source = self
            .source_row(&batch.source_id)?
            .ok_or_else(|| TimelineError::UnknownSource(batch.source_id.clone()))?;
        self.db.transaction(|| {
            let mut inserted=0;let mut revised=0;
            let threads=IngestBatch{source_id:batch.source_id.clone(),threads:batch.threads.clone(),..Default::default()};
            self.ingest_in_transaction(&threads,&[],&mut vec![],false)?;
            for event in &batch.events {
                let mut event=event.clone();
                if event.kind=="ai.work" {
                    let key=event.payload["prompt_event_id"].as_str().unwrap_or("");
                    let Some(id)=self.event_id(&batch.source_id,key)? else {continue;};
                    event.payload["prompt_event_id"]=serde_json::Value::String(id);
                }
                validate(&event)?;
                let snapshot=json::canonical_of(&event);
                let existing=self.event_id(&batch.source_id,&event.source_key)?;
                let rev=if let Some(id)=&existing {
                    let last=self.db.first("SELECT revision,event_json FROM agent_event_revisions WHERE event_id=? ORDER BY revision DESC LIMIT 1",&[text(id)],|r|(r.int(0),r.text(1)))?;
                    if let Some((_,old))=&last {if old==&snapshot {continue;}}
                    last.map(|(r,_)|r+1).unwrap_or(1)
                } else {1};
                event.payload["revision"]=serde_json::json!(rev);
                let p=self.prepare(&event,&source.policy)?;
                let id=if let Some(id)=existing {
                    let old_chunks=self.db.query("SELECT DISTINCT chunk_id FROM event_text WHERE event_id=?",&[text(&id)],|r|r.int(0))?;
                    self.db.execute("DELETE FROM event_text WHERE event_id=?",&[text(&id)])?;
                    self.insert_text(&p,&id,&batch.source_id,self.clock.now_ms(),&mut HashMap::new())?;
                    self.db.execute("UPDATE events SET end_ms=?,payload_json=? WHERE event_id=?",&[opt_int(event.end_ms),text(json::canonical(&event.payload)),text(&id)])?;
                    for chunk in old_chunks {if self.db.scalar("SELECT COUNT(*) FROM event_text WHERE chunk_id=?",&[int(chunk)])?==0 {let t=self.db.first("SELECT text FROM text_chunks WHERE chunk_id=?",&[int(chunk)],|r|r.text(0))?.unwrap_or_default();self.db.execute("INSERT INTO text_fts(text_fts,rowid,text) VALUES('delete',?,?)",&[int(chunk),text(t)])?;self.db.execute("DELETE FROM text_chunks WHERE chunk_id=?",&[int(chunk)])?;}}
                    // Inputs changed: retained derived revisions leave default results until rebuilt.
                    self.db.execute("UPDATE episodes SET status='superseded' WHERE (episode_id,rev) IN (SELECT episode_id,rev FROM episode_evidence WHERE event_id=?)",&[text(&id)])?;
                    self.db.execute("UPDATE memories SET status='superseded' WHERE memory_id IN (SELECT memory_id FROM memory_sources WHERE ref_type='event' AND ref_id=?)",&[text(&id)])?;
                    self.db.execute("UPDATE jobs SET state='cancelled',updated_ms=? WHERE input_ref=? AND state IN ('queued','running')",&[int(self.clock.now_ms()),text(&id)])?;
                    self.record_change("event",&id,"update")?; revised+=1;id
                } else {
                    let part=IngestBatch{source_id:batch.source_id.clone(),threads:vec![],events:vec![event.clone()],..Default::default()};
                    let result=self.ingest_in_transaction(&part,&[p],&mut vec![],false)?;
                    let Some(id)=result.event_ids.get(&event.source_key) else {continue;};inserted+=result.inserted;id.clone()
                };
                self.db.execute("INSERT INTO agent_event_revisions VALUES(?,?,?,?)",&[text(&id),int(rev),text(snapshot),int(self.clock.now_ms())])?;
            }
            // Empty files and unchanged batches still advance the durable checkpoint.
            for (stream,id,kind,body) in &delta.entries {
                self.db.execute("INSERT INTO agent_session_entries VALUES(?,?,?,?,?) ON CONFLICT(source_id,session_stream,entry_id,entry_kind) DO UPDATE SET entry_json=excluded.entry_json WHERE agent_session_entries.entry_json IS NOT excluded.entry_json",
                    &[text(&batch.source_id),text(stream),text(id),text(kind),text(body)])?;
            }
            for stream in &delta.reset_counts {
                self.db.execute("DELETE FROM agent_file_counts WHERE source_id=? AND file_stream=?", &[text(&batch.source_id),text(stream)])?;
            }
            for (stream,hash,n) in &delta.counts {
                self.db.execute("INSERT INTO agent_file_counts VALUES(?,?,?,?) ON CONFLICT(source_id,file_stream,content_hash) DO UPDATE SET occurrences=excluded.occurrences WHERE agent_file_counts.occurrences IS NOT excluded.occurrences",
                    &[text(&batch.source_id),text(stream),text(hash),int(*n as i64)])?;
            }
            for c in batch.cursor_after.iter().chain(cursors.iter()) {self.db.execute("INSERT INTO source_cursors VALUES(?,?,?,?) ON CONFLICT(source_id,stream) DO UPDATE SET cursor=excluded.cursor,updated_ms=excluded.updated_ms WHERE source_cursors.cursor IS NOT excluded.cursor",&[text(&batch.source_id),text(&c.stream),text(&c.cursor),int(self.clock.now_ms())])?;}
            // A good fragment does not certify the entire connector. sync_agents finalizes health once.
            Ok((inserted,revised))
        })
    }
}
