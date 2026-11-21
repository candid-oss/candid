//! One-time conversion of originals stored before data model v1 §9 (WebP screenshots, AAC recordings):
//! existing JPEG screenshots are re-encoded as WebP, and existing WAV recording parts are queued for the same AAC
//! compaction new parts get. Both run in the background, a few originals at a time, and are idempotent: a converted
//! original no longer matches the candidate query, so an interrupted conversion simply continues.
use crate::{
    db::{int, text, RowExt},
    media_compaction::COMPACT_AUDIO_MIME,
    sha256_hex, webp, Result, TimelineError, TimelineStore,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// A JPEG screenshot original still waiting for conversion.
#[derive(Debug, Clone)]
pub struct JpegScreenshot {
    pub sha256: String,
    pub path: PathBuf,
    pub byte_count: i64,
}

/// Totals kept in `store_meta` for the status page and diagnostics.
const SCREENSHOT_TOTALS: &str = "conversion.screenshots_webp";

/// Decodes a JPEG and encodes it as WebP at the screenshot quality. Returns the WebP bytes, width and height.
pub fn jpeg_to_webp(jpeg: &[u8]) -> std::result::Result<(Vec<u8>, u32, u32), String> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(jpeg, options);
    let pixels = decoder.decode().map_err(|e| format!("JPEG 无法解码：{e:?}"))?;
    let (w, h) = decoder.dimensions().ok_or("JPEG 没有尺寸")?;
    let (w, h) = (w as u32, h as u32);
    if pixels.len() < w as usize * h as usize * 4 {
        return Err("JPEG 像素不完整".into());
    }
    let encoded = webp::encode_bgra(&pixels, w, h, w as usize * 4, webp::DEFAULT_QUALITY)?;
    Ok((encoded, w, h))
}

impl TimelineStore {
    /// Screenshot originals still stored as JPEG, oldest first. `CROSS JOIN` keeps SQLite on the `event_blobs(sha256)`
    /// index; left to itself it scans every screenshot event per blob.
    pub fn jpeg_screenshot_candidates(&self, limit: usize) -> Result<Vec<JpegScreenshot>> {
        // Before the cutover to direct capture the old ledger is compared hash by hash; its originals stay as they are.
        if !self.desktop_direct()? {
            return Ok(Vec::new());
        }
        let rows = self.db.query(
            "SELECT b.sha256,b.rel_path,b.byte_count FROM blobs b WHERE b.mime_type='image/jpeg'
               AND EXISTS (SELECT 1 FROM event_blobs eb CROSS JOIN events e ON e.event_id=eb.event_id
                           WHERE eb.sha256=b.sha256 AND eb.role='original' AND e.kind='screen.shot')
             ORDER BY b.created_ms LIMIT ?",
            &[int(limit as i64)],
            |r| (r.text(0), r.text(1), r.int(2)),
        )?;
        Ok(rows.into_iter().map(|(sha256, rel, byte_count)| JpegScreenshot { sha256, path: self.root.join(rel), byte_count }).collect())
    }

    /// How many JPEG screenshots remain and their bytes.
    pub fn jpeg_screenshot_backlog(&self) -> Result<(i64, i64)> {
        Ok(self
            .db
            .first(
                "SELECT COUNT(*),COALESCE(SUM(b.byte_count),0) FROM blobs b WHERE b.mime_type='image/jpeg'
                   AND EXISTS (SELECT 1 FROM event_blobs eb CROSS JOIN events e ON e.event_id=eb.event_id
                               WHERE eb.sha256=b.sha256 AND eb.role='original' AND e.kind='screen.shot')",
                &[],
                |r| (r.int(0), r.int(1)),
            )?
            .unwrap_or((0, 0)))
    }

    /// Replaces a JPEG screenshot original with its WebP re-encoding: every event that referenced the JPEG now
    /// references the WebP, and the JPEG file goes. Returns false, changing nothing, if the JPEG is no longer stored
    /// (deleted or converted meanwhile).
    pub fn replace_screenshot_with_webp(&self, old: &JpegScreenshot, webp_bytes: &[u8], width: u32, height: u32) -> Result<bool> {
        if webp_bytes.len() < 12 || &webp_bytes[0..4] != b"RIFF" || &webp_bytes[8..12] != b"WEBP" {
            return Err(TimelineError::InvalidInput("不是 WebP".into()));
        }
        let digest = sha256_hex(webp_bytes);
        let rel = format!("blobs/sha256/{}/{}.webp", &digest[..2], digest);
        let destination = self.root.join(&rel);
        let fresh_file = !destination.exists();
        if fresh_file {
            if let Some(dir) = destination.parent() {
                crate::store::mkdir_private(dir)?;
            }
            let staged = self.root.join("staging").join(format!(".{digest}.webp"));
            {
                let mut f = fs::File::create(&staged)?;
                f.write_all(webp_bytes)?;
                f.sync_all()?;
            }
            fs::rename(&staged, &destination)?;
        }
        let now = self.clock.now_ms();
        let swapped = self.db.transaction(|| {
            if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE sha256=? AND mime_type='image/jpeg'", &[text(&old.sha256)])? == 0 {
                return Ok(false);
            }
            if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE sha256=?", &[text(&digest)])? == 0 {
                self.db.execute(
                    "INSERT INTO blobs(sha256,byte_count,mime_type,rel_path,pixel_w,pixel_h,duration_ms,created_ms) VALUES(?,?,?,?,?,?,NULL,?)",
                    &[text(&digest), int(webp_bytes.len() as i64), text("image/webp"), text(&rel), int(width as i64), int(height as i64), int(now)],
                )?;
                self.record_change("blob", &digest, "insert")?;
            }
            let events = self.db.query("SELECT DISTINCT event_id FROM event_blobs WHERE sha256=?", &[text(&old.sha256)], |r| r.text(0))?;
            self.db.execute("UPDATE OR IGNORE event_blobs SET sha256=? WHERE sha256=?", &[text(&digest), text(&old.sha256)])?;
            self.db.execute("DELETE FROM event_blobs WHERE sha256=?", &[text(&old.sha256)])?;
            for event in &events {
                self.record_change("event", event, "update")?;
            }
            self.db.execute("DELETE FROM blobs WHERE sha256=?", &[text(&old.sha256)])?;
            self.record_change("blob", &old.sha256, "delete")?;
            let totals = self.store_meta_json(SCREENSHOT_TOTALS)?;
            let count = totals["count"].as_i64().unwrap_or(0) + 1;
            let before = totals["bytes_before"].as_i64().unwrap_or(0) + old.byte_count;
            let after = totals["bytes_after"].as_i64().unwrap_or(0) + webp_bytes.len() as i64;
            self.set_store_meta_json(SCREENSHOT_TOTALS, &serde_json::json!({"count":count,"bytes_before":before,"bytes_after":after,"updated_ms":now}))?;
            Ok(true)
        })?;
        if !swapped {
            if fresh_file && self.db.scalar("SELECT COUNT(*) FROM blobs WHERE sha256=?", &[text(&digest)])? == 0 {
                let _ = fs::remove_file(&destination);
            }
            return Ok(false);
        }
        let _ = fs::remove_file(&old.path);
        Ok(true)
    }

    /// Queues WAV recording parts stored before AAC compaction existed for the same compaction new parts get
    /// (`media_compaction`). Returns how many were marked.
    pub fn queue_legacy_wav_compaction(&self) -> Result<usize> {
        self.db.execute(
            "UPDATE media_objects SET metadata=json_set(metadata,'$.compact_to',?)
             WHERE json_extract(metadata,'$.mime_type')='audio/wav' AND json_extract(metadata,'$.compact_to') IS NULL
               AND id IN (SELECT id FROM media_parts WHERE json_extract(metadata,'$.sample_count') IS NOT NULL)
               AND id NOT IN (SELECT id FROM media_deleted)",
            &[text(COMPACT_AUDIO_MIME)],
        )?;
        Ok(self.db.changes() as usize)
    }

    fn store_meta_json(&self, key: &str) -> Result<serde_json::Value> {
        let raw = self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(key)], |r| r.text(0))?;
        Ok(raw.and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(serde_json::Value::Null))
    }

    fn set_store_meta_json(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        self.db.execute("INSERT INTO store_meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", &[text(key), text(value.to_string())])?;
        Ok(())
    }

    /// The one-time conversion totals, for diagnostics.
    pub fn conversion_status(&self) -> Result<serde_json::Value> {
        let (left, left_bytes) = self.jpeg_screenshot_backlog()?;
        Ok(serde_json::json!({"screenshots": self.store_meta_json(SCREENSHOT_TOTALS)?, "jpeg_left": left, "jpeg_left_bytes": left_bytes}))
    }
}

/// Reads a candidate's file, for callers converting outside the store lock.
pub fn read_original(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}
