//! Derived JSON originals (AX evidence of `screen.shot`, full recognition results of `image.text`) are stored
//! compressed (data model v1 §6, §9.5): raw DEFLATE (RFC 1951), which the App reads with Foundation's
//! `NSData.decompressed(using: .zlib)` and the core with `flate2`. A compressed blob is marked by its mime type
//! `application/json+deflate` and its file suffix `.json.deflate`; its `sha256` is the hash of the stored (compressed)
//! bytes like every other blob, so integrity checks stay unchanged. `payload.ax_evidence_ref` / `payload.result_ref`
//! keep naming the JSON itself and are not blob lookups.
//!
//! New derived JSON is compressed at ingest; blobs stored plain before are converted once, in small resumable passes
//! (`derived_json_candidates` → `compress_derived_json` outside the store lock → `swap_derived_json`).
use crate::{
    db::{int, placeholders, text, RowExt},
    sha256_hex, Result, TimelineError, TimelineStore,
};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

/// Mime type of a plain JSON blob, as clients declare it.
pub const JSON_MIME: &str = "application/json";
/// Mime type of a derived JSON blob stored as raw DEFLATE.
pub const DEFLATE_JSON_MIME: &str = "application/json+deflate";
/// File suffix of a compressed JSON blob.
pub const DEFLATE_JSON_SUFFIX: &str = "json.deflate";
/// Upper bound on a decompressed JSON original; derived JSON is a few hundred KB at most.
const MAX_JSON_BYTES: u64 = 64 << 20;

/// Raw DEFLATE at the default level (measured within 3% of the best level on AX evidence, at about half the time).
pub fn compress_json(data: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::with_capacity(data.len() / 6 + 64), Compression::default());
    encoder.write_all(data).expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

/// Inverse of `compress_json`; fails on corrupt data or output beyond `MAX_JSON_BYTES`.
pub fn decompress_json(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 6);
    DeflateDecoder::new(data).take(MAX_JSON_BYTES + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MAX_JSON_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "decompressed JSON too large"));
    }
    Ok(out)
}

/// The bytes a blob stands for: compressed JSON is inflated, everything else returned as stored.
/// Returns the served mime type alongside.
pub fn logical_bytes(stored: Vec<u8>, mime: &str) -> std::io::Result<(Vec<u8>, String)> {
    if mime == DEFLATE_JSON_MIME {
        Ok((decompress_json(&stored)?, JSON_MIME.into()))
    } else {
        Ok((stored, mime.into()))
    }
}

/// Derived JSON that should be stored compressed.
pub(crate) fn compresses(role: &str, mime: &str) -> bool {
    role == "derived" && mime == JSON_MIME
}

pub(crate) fn blob_rel_path(sha: &str) -> String {
    format!("blobs/sha256/{}/{}.{DEFLATE_JSON_SUFFIX}", &sha[..2], sha)
}

/// Writes `data` to `dest` via a hidden temporary file in the same directory: private, fsynced, then renamed.
pub(crate) fn write_blob_file(dest: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = dest.parent() {
        crate::store::mkdir_private(dir)?;
    }
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dest.with_file_name(format!(".{name}.{}.tmp", crate::uuid_v4()));
    let written = (|| -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
        f.write_all(data)?;
        // Plain fsync, as in ingest (std's sync_all is a full device flush on Apple platforms).
        use std::os::unix::io::AsRawFd;
        // SAFETY: fsync on a descriptor owned by `f`.
        unsafe { libc::fsync(f.as_raw_fd()) };
        fs::rename(&tmp, dest)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// A plain derived JSON blob still to convert.
#[derive(Debug, Clone)]
pub struct JsonCandidate {
    pub sha256: String,
    pub path: PathBuf,
}

/// A candidate compressed outside the store lock; its file is already in place under `blobs/`.
#[derive(Debug, Clone)]
pub struct CompressedJson {
    pub old_sha256: String,
    pub sha256: String,
    pub rel_path: String,
    pub byte_count: i64,
    pub original_bytes: i64,
}

/// Outcome of one conversion step.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JsonConversionStep {
    pub converted: usize,
    pub skipped: usize,
    pub bytes_before: i64,
    pub bytes_after: i64,
}

impl TimelineStore {
    /// Plain JSON blobs referenced only as `derived`, in hash order after `after` (resume point within a run).
    pub fn derived_json_candidates(&self, after: &str, limit: usize) -> Result<Vec<JsonCandidate>> {
        let rows = self.db.query(
            "SELECT b.sha256,b.rel_path FROM blobs b WHERE b.mime_type=? AND b.sha256>?
               AND EXISTS (SELECT 1 FROM event_blobs eb WHERE eb.sha256=b.sha256 AND eb.role='derived')
               AND NOT EXISTS (SELECT 1 FROM event_blobs eb WHERE eb.sha256=b.sha256 AND eb.role<>'derived')
             ORDER BY b.sha256 LIMIT ?",
            &[text(JSON_MIME), text(after), int(limit as i64)],
            |r| (r.text(0), r.text(1)),
        )?;
        Ok(rows.into_iter().map(|(sha256, rel)| JsonCandidate { sha256, path: self.root.join(rel) }).collect())
    }

    /// Number of plain derived JSON blobs left.
    pub fn derived_json_remaining(&self) -> Result<i64> {
        self.db.scalar(
            "SELECT COUNT(*) FROM blobs b WHERE b.mime_type=?
               AND EXISTS (SELECT 1 FROM event_blobs eb WHERE eb.sha256=b.sha256 AND eb.role='derived')
               AND NOT EXISTS (SELECT 1 FROM event_blobs eb WHERE eb.sha256=b.sha256 AND eb.role<>'derived')",
            &[text(JSON_MIME)],
        )
    }

    /// Replaces converted blobs in one transaction: new blob rows, event references, then the old rows; the old files
    /// go after the commit. A candidate whose old row is gone or changed meanwhile is skipped (its new file is
    /// removed unless something else uses it).
    pub fn swap_derived_json(&self, done: &[CompressedJson]) -> Result<JsonConversionStep> {
        if done.is_empty() {
            return Ok(JsonConversionStep::default());
        }
        let now = self.clock.now_ms();
        let mut step = JsonConversionStep::default();
        let mut old_files = Vec::new();
        let mut unused_new = Vec::new();
        self.db.transaction(|| {
            for c in done {
                let old = self.db.first(
                    "SELECT rel_path,byte_count FROM blobs WHERE sha256=? AND mime_type=?",
                    &[text(&c.old_sha256), text(JSON_MIME)],
                    |r| (r.text(0), r.int(1)),
                )?;
                let Some((old_rel, old_bytes)) = old else {
                    unused_new.push(c.clone());
                    step.skipped += 1;
                    continue;
                };
                let foreign = self.db.scalar("SELECT COUNT(*) FROM event_blobs WHERE sha256=? AND role<>'derived'", &[text(&c.old_sha256)])?;
                if foreign > 0 {
                    unused_new.push(c.clone());
                    step.skipped += 1;
                    continue;
                }
                if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE sha256=?", &[text(&c.sha256)])? == 0 {
                    self.db.execute(
                        "INSERT INTO blobs(sha256,byte_count,mime_type,rel_path,pixel_w,pixel_h,duration_ms,created_ms) VALUES(?,?,?,?,NULL,NULL,NULL,?)",
                        &[text(&c.sha256), int(c.byte_count), text(DEFLATE_JSON_MIME), text(&c.rel_path), int(now)],
                    )?;
                    self.record_change("blob", &c.sha256, "insert")?;
                }
                let events = self.db.query("SELECT event_id FROM event_blobs WHERE sha256=?", &[text(&c.old_sha256)], |r| r.text(0))?;
                for chunk in events.chunks(400) {
                    let mut p = vec![text(&c.sha256), text(&c.old_sha256)];
                    p.extend(chunk.iter().map(text));
                    self.db.execute(
                        &format!("UPDATE OR IGNORE event_blobs SET sha256=? WHERE sha256=? AND event_id IN ({})", placeholders(chunk.len())),
                        &p,
                    )?;
                }
                self.db.execute("DELETE FROM event_blobs WHERE sha256=?", &[text(&c.old_sha256)])?;
                for event in &events {
                    self.record_change("event", event, "update")?;
                }
                self.db.execute("DELETE FROM blobs WHERE sha256=?", &[text(&c.old_sha256)])?;
                self.record_change("blob", &c.old_sha256, "delete")?;
                old_files.push(self.root.join(old_rel));
                step.converted += 1;
                step.bytes_before += old_bytes;
                step.bytes_after += c.byte_count;
            }
            Ok(())
        })
        .inspect_err(|_| {
            // Nothing changed; files written for this step are orphans unless a row already used them.
            for c in done {
                if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE rel_path=?", &[text(&c.rel_path)]).unwrap_or(1) == 0 {
                    let _ = fs::remove_file(self.root.join(&c.rel_path));
                }
            }
        })?;
        for f in old_files {
            let _ = fs::remove_file(f);
        }
        for c in unused_new {
            if self.db.scalar("SELECT COUNT(*) FROM blobs WHERE rel_path=?", &[text(&c.rel_path)])? == 0 {
                let _ = fs::remove_file(self.root.join(&c.rel_path));
            }
        }
        Ok(step)
    }

    /// One conversion step with the store held throughout (tests and one-shot tools; the service compresses
    /// outside the lock with `compress_derived_json`).
    pub fn convert_derived_json_step(&self, after: &str, limit: usize) -> Result<(JsonConversionStep, Option<String>)> {
        let candidates = self.derived_json_candidates(after, limit)?;
        let last = candidates.last().map(|c| c.sha256.clone());
        let mut done = Vec::new();
        let mut failed = 0;
        for c in &candidates {
            match compress_derived_json(&self.root, c) {
                Ok(d) => done.push(d),
                Err(_) => failed += 1,
            }
        }
        let mut step = self.swap_derived_json(&done)?;
        step.skipped += failed;
        Ok((step, last))
    }
}

/// Reads a candidate, checks its hash, and writes the compressed file next to it (not yet referenced).
pub fn compress_derived_json(root: &Path, c: &JsonCandidate) -> Result<CompressedJson> {
    let data = fs::read(&c.path)?;
    if sha256_hex(&data) != c.sha256 {
        return Err(TimelineError::BlobMismatch(format!("{} 与记录不一致", &c.sha256[..12])));
    }
    let packed = compress_json(&data);
    let sha = sha256_hex(&packed);
    let rel = blob_rel_path(&sha);
    let dest = root.join(&rel);
    if !dest.exists() {
        write_blob_file(&dest, &packed)?;
    }
    Ok(CompressedJson { old_sha256: c.sha256.clone(), sha256: sha, rel_path: rel, byte_count: packed.len() as i64, original_bytes: data.len() as i64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_marking() {
        let json = br#"{"nodes":[{"role":"AXStaticText","value":"synthetic text"},{"role":"AXButton","value":"OK"}]}"#.repeat(20);
        let packed = compress_json(&json);
        assert!(packed.len() * 4 < json.len());
        assert_eq!(decompress_json(&packed).unwrap(), json);
        let (bytes, mime) = logical_bytes(packed.clone(), DEFLATE_JSON_MIME).unwrap();
        assert_eq!((bytes, mime.as_str()), (json.clone(), JSON_MIME));
        assert_eq!(logical_bytes(json.clone(), "image/webp").unwrap().1, "image/webp");
        assert!(decompress_json(b"not deflate at all").is_err());
        assert!(compresses("derived", JSON_MIME) && !compresses("original", JSON_MIME) && !compresses("derived", DEFLATE_JSON_MIME));
        assert!(blob_rel_path(&sha256_hex(&packed)).ends_with(".json.deflate"));
    }

    /// Raw DEFLATE written by Foundation (`NSData.compressed(using: .zlib)`) for `{"text":"synthetic"}`.
    #[test]
    fn reads_foundation_output() {
        let foundation = [0xab, 0x56, 0x2a, 0x49, 0xad, 0x28, 0x51, 0xb2, 0x52, 0x2a, 0xae, 0xcc, 0x2b, 0xc9, 0x48, 0x2d, 0xc9, 0x4c, 0x56, 0xaa, 0x05, 0x00];
        assert_eq!(decompress_json(&foundation).unwrap(), br#"{"text":"synthetic"}"#);
    }
}
