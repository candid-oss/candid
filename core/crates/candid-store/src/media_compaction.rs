//! Recording originals step down the fidelity ladder once processed (data model v1 §6, §9.4): new Mac recordings
//! arrive as PCM WAV, transcription and speaker analysis read that WAV, and afterwards the original is replaced by
//! AAC (`audio/mp4`, mono 48 kbps). Only parts imported with `compact_to` are touched; older WAVs stay as they are
//! until the data model conversion. The encoding itself (system `afconvert`) runs in the service, outside the store
//! lock; the store only chooses candidates and swaps the original atomically.
use crate::{
    db::{int, text, RowExt},
    sha256_hex_file, Result, TimelineError, TimelineStore,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// What a new recording part is compacted to once processed.
pub const COMPACT_AUDIO_MIME: &str = "audio/mp4";
/// Encoder settings, recorded with each compaction: mono AAC, 48 kbps, the part's own sample rate (≥ 16 kHz).
pub const COMPACT_AUDIO_BITRATE: u32 = 48_000;

#[derive(Debug, Clone)]
pub struct CompactionCandidate {
    pub id: String,
    pub path: PathBuf,
    pub digest: String,
    /// Frames the compacted file must decode to, for recording parts (their timeline coordinates).
    pub sample_count: Option<u64>,
    pub sample_rate: Option<u32>,
}

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

impl TimelineStore {
    /// Parts whose processing is over: the recording is no longer running, no job that reads its audio (transcription,
    /// speakers) is queued or running — a title or summary job reads text only and may wait indefinitely — and nothing about them changed for `grace_ms` (so a follow-up step, such as speakers after
    /// the last transcript, has been scheduled if it is going to be).
    pub fn media_compaction_candidates(&self, grace_ms: i64, limit: usize) -> Result<Vec<CompactionCandidate>> {
        Ok(self.compaction_scan(grace_ms, limit)?.0)
    }

    /// When the next part becomes a candidate (its grace period ends), if any is only waiting for time to pass.
    /// Parts of running recordings or with audio jobs in flight are not counted: their end is an event of its own.
    /// Also whether any WAV is still waiting to be compacted at all: when none is, only a new audio original can
    /// give compaction work, so the background loop stops watching jobs for it.
    pub fn media_compaction_next_ms(&self, grace_ms: i64) -> Result<(Option<i64>, bool)> {
        let (_, next, waiting) = self.compaction_scan(grace_ms, usize::MAX)?;
        Ok((next, waiting))
    }

    fn compaction_scan(&self, grace_ms: i64, limit: usize) -> Result<(Vec<CompactionCandidate>, Option<i64>, bool)> {
        let now = self.clock.now_ms();
        let mut next: Option<i64> = None;
        let rows = self.db.query(
            "SELECT o.id,o.blob_path,o.digest,o.metadata,p.recording_id,p.metadata FROM media_objects o LEFT JOIN media_parts p USING(id)
             WHERE json_extract(o.metadata,'$.compact_to')=? AND json_extract(o.metadata,'$.mime_type')='audio/wav'
               AND o.id NOT IN (SELECT id FROM media_deleted) ORDER BY o.rowid",
            &[text(COMPACT_AUDIO_MIME)],
            |r| (r.text(0), r.text(1), r.text(2), r.text(3), r.opt_text(4), r.opt_text(5)),
        )?;
        let waiting = !rows.is_empty();
        let mut out = Vec::new();
        for (id, rel, digest, metadata, recording, part) in rows {
            if out.len() >= limit {
                break;
            }
            let m: Value = serde_json::from_str(&metadata)?;
            let imported = m["imported_at"]
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.timestamp_millis())
                .unwrap_or(now);
            let rid = recording.clone().unwrap_or_else(|| id.clone());
            if recording.is_some() {
                let state = self.db.first("SELECT state FROM media_edits WHERE id=?", &[text(&rid)], |r| r.text(0))?;
                if state.as_deref() == Some("recording") || self.media_deleted(&rid)? {
                    continue;
                }
            }
            let (pending, last_job) = self
                .db
                .first(
                    "SELECT COUNT(CASE WHEN state IN ('queued','running') AND kind IN ('speech.recording','speech.recognize','workflow.speakers') THEN 1 END),COALESCE(MAX(updated_ms),0) FROM jobs WHERE input_ref IN (?,?)",
                    &[text(&id), text(&rid)],
                    |r| (r.int(0), r.int(1)),
                )?
                .unwrap_or((0, 0));
            if pending > 0 {
                continue;
            }
            if now - imported.max(last_job) < grace_ms {
                let due = imported.max(last_job) + grace_ms;
                next = Some(next.map_or(due, |n| n.min(due)));
                continue;
            }
            let part: Option<Value> = part.map(|p| serde_json::from_str(&p)).transpose()?;
            out.push(CompactionCandidate {
                path: self.root.join(rel),
                digest,
                sample_count: part.as_ref().and_then(|p| p["sample_count"].as_u64()),
                sample_rate: part.as_ref().and_then(|p| p["sample_rate"].as_u64()).map(|r| r as u32),
                id,
            });
        }
        Ok((out, next, waiting))
    }

    /// Replaces a candidate's WAV with `encoded` (already verified by the caller to decode to the same frames):
    /// the media original, its timeline reference and both files. Returns false, leaving everything as it was, when
    /// the part changed meanwhile (deleted, re-queued for processing, or already compacted).
    pub fn media_compact_original(&self, candidate: &CompactionCandidate, encoded: &Path) -> Result<bool> {
        let head = {
            let mut h = [0u8; 8];
            use std::io::Read;
            fs::File::open(encoded)?.read_exact(&mut h).map_err(|_| bad("压缩后的录音无效"))?;
            h
        };
        if &head[4..8] != b"ftyp" {
            return Err(bad("压缩后的录音不是 MP4/AAC"));
        }
        let size = fs::metadata(encoded)?.len();
        let digest = sha256_hex_file(encoded)?;
        let rel = format!("media/{digest}");
        let destination = self.root.join(&rel);
        if !destination.exists() {
            let staged = self.root.join("media").join(format!(".{digest}.compact"));
            fs::copy(encoded, &staged)?;
            fs::File::open(&staged)?.sync_all()?;
            fs::rename(&staged, &destination)?;
            fs::File::open(self.root.join("media"))?.sync_all()?;
        }
        let old_rel = candidate.path.strip_prefix(&self.root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let now = self.clock.now_ms();
        let swapped = self.db.transaction(|| {
            // Re-checked inside the write: a job queued after the candidate was chosen keeps the WAV.
            let still = self.db.scalar(
                "SELECT COUNT(*) FROM media_objects WHERE id=? AND digest=? AND json_extract(metadata,'$.mime_type')='audio/wav'
                   AND id NOT IN (SELECT id FROM media_deleted)",
                &[text(&candidate.id), text(&candidate.digest)],
            )? == 1;
            let rid = self
                .db
                .first("SELECT recording_id FROM media_parts WHERE id=?", &[text(&candidate.id)], |r| r.text(0))?
                .unwrap_or_else(|| candidate.id.clone());
            let busy = self.db.scalar(
                "SELECT COUNT(*) FROM jobs WHERE input_ref IN (?,?) AND state IN ('queued','running') AND kind IN ('speech.recording','speech.recognize','workflow.speakers')",
                &[text(&candidate.id), text(&rid)],
            )? > 0;
            if !still || busy {
                return Ok(false);
            }
            self.db.execute(
                "UPDATE media_objects SET blob_path=?,digest=?,byte_count=?,
                   metadata=json_remove(json_set(metadata,'$.mime_type',?,'$.original_digest',?,'$.compacted',json_object('from','audio/wav','bytes',?,'at_ms',?,'bitrate',?)),'$.compact_to')
                 WHERE id=?",
                &[
                    text(&rel),
                    text(&digest),
                    int(size as i64),
                    text(COMPACT_AUDIO_MIME),
                    text(&candidate.digest),
                    int(fs::metadata(&candidate.path).map(|m| m.len() as i64).unwrap_or(0)),
                    int(now),
                    int(COMPACT_AUDIO_BITRATE as i64),
                    text(&candidate.id),
                ],
            )?;
            // The timeline's reference event for this original points at the same bytes.
            let events = self.db.query(
                "SELECT e.event_id FROM events e JOIN event_blobs eb ON eb.event_id=e.event_id WHERE e.source_key=? AND eb.sha256=?",
                &[text(format!("media:{}", candidate.id)), text(&candidate.digest)],
                |r| r.text(0),
            )?;
            if !events.is_empty() {
                if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE sha256=?", &[text(&digest)])? == 0 {
                    let blob_rel = format!("blobs/sha256/{}/{}.m4a", &digest[..2], digest);
                    let blob_path = self.root.join(&blob_rel);
                    if let Some(dir) = blob_path.parent() {
                        crate::store::mkdir_private(dir)?;
                    }
                    if !blob_path.exists() && fs::hard_link(&destination, &blob_path).is_err() {
                        fs::copy(&destination, &blob_path)?;
                    }
                    self.db.execute(
                        "INSERT INTO blobs(sha256,byte_count,mime_type,rel_path,pixel_w,pixel_h,duration_ms,created_ms) VALUES(?,?,?,?,NULL,NULL,NULL,?)",
                        &[text(&digest), int(size as i64), text(COMPACT_AUDIO_MIME), text(&blob_rel), int(now)],
                    )?;
                    self.record_change("blob", &digest, "insert")?;
                }
                for event in &events {
                    self.db.execute(
                        "UPDATE OR IGNORE event_blobs SET sha256=? WHERE event_id=? AND sha256=?",
                        &[text(&digest), text(event), text(&candidate.digest)],
                    )?;
                    self.db.execute("DELETE FROM event_blobs WHERE event_id=? AND sha256=?", &[text(event), text(&candidate.digest)])?;
                    self.record_change("event", event, "update")?;
                }
            }
            Ok(true)
        })?;
        if !swapped {
            if self.db.scalar("SELECT COUNT(*) FROM media_objects WHERE blob_path=?", &[text(&rel)])? == 0 {
                let _ = fs::remove_file(&destination);
            }
            return Ok(false);
        }
        // The WAV goes once nothing refers to it any more: neither another media original nor a timeline blob.
        if self.db.scalar("SELECT COUNT(*) FROM media_objects WHERE blob_path=?", &[text(&old_rel)])? == 0 {
            let _ = fs::remove_file(&candidate.path);
        }
        if self.db.scalar("SELECT COUNT(*) FROM event_blobs WHERE sha256=?", &[text(&candidate.digest)])? == 0 {
            if let Some(blob) = self.db.first("SELECT rel_path FROM blobs WHERE sha256=?", &[text(&candidate.digest)], |r| r.text(0))? {
                self.db.execute("DELETE FROM blobs WHERE sha256=?", &[text(&candidate.digest)])?;
                self.record_change("blob", &candidate.digest, "delete")?;
                let _ = fs::remove_file(self.root.join(blob));
            }
        }
        Ok(true)
    }
}
