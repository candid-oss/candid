//! Derived default-query projection. Physical originals and superseded model results remain referenceable.
use crate::{
    db::{int, text, RowExt},
    Result, *,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
};
fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}
fn literal_glob(id: &str) -> String {
    id.chars().map(|c| match c { '[' => "[[]".into(), '*' => "[*]".into(), '?' => "[?]".into(), _ => c.to_string() }).collect()
}
impl TimelineStore {
    pub(crate) fn media_bounds(&self, id: &str) -> Result<(f64, Option<(u32, u64)>)> {
        let path = self.media_path(id)?;
        let mut f = fs::File::open(&path)?;
        let size = f.metadata()?.len();
        let mut h = [0u8; 12];
        f.read_exact(&mut h)?;
        if &h[..4] == b"RIFF" && &h[8..] == b"WAVE" {
            let mut offset = 12u64;
            let (mut rate, mut align, mut frames) = (0u32, 0u16, 0u64);
            while offset + 8 <= size {
                f.seek(SeekFrom::Start(offset))?;
                let mut chunk = [0u8; 8];
                f.read_exact(&mut chunk)?;
                let n = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
                if offset + 8 + n > size {
                    return Err(bad("原音文件损坏"));
                }
                if &chunk[..4] == b"fmt " && n >= 16 {
                    let mut fmt = [0u8; 16];
                    f.read_exact(&mut fmt)?;
                    rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                    align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
                }
                if &chunk[..4] == b"data" {
                    frames += n;
                }
                offset += 8 + n + (n % 2);
            }
            if rate == 0 || align == 0 || frames == 0 || frames % align as u64 != 0 {
                return Err(bad("原音样本范围无效"));
            }
            return Ok((
                (frames / align as u64) as f64 / rate as f64,
                Some((rate, frames / align as u64)),
            ));
        }
        // Core Audio is part of macOS, independent of Python or a model environment.
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("/usr/bin/afinfo")
                .arg(&path)
                .env("LC_ALL", "C")
                .output()?;
            if output.status.success() {
                let info = String::from_utf8_lossy(&output.stdout);
                // AAC (recordings compacted after processing) reports exact frames, priming and padding excluded, so
                // sample coordinates stay as precise as with the original WAV.
                let frames = info.lines().find_map(|l| {
                    l.trim().strip_prefix("audio ")?.split_once(" valid frames")?.0.trim().parse::<u64>().ok()
                });
                let rate = info.lines().find_map(|l| {
                    let format = l.trim().strip_prefix("Data format:")?;
                    format.split(',').find(|p| p.trim().ends_with("Hz"))?.trim().trim_end_matches("Hz").trim().parse::<u32>().ok()
                });
                if let (Some(frames), Some(rate)) = (frames, rate) {
                    if frames > 0 && rate > 0 {
                        return Ok((frames as f64 / rate as f64, Some((rate, frames))));
                    }
                }
                for line in info.lines() {
                    if let Some(value) = line.trim().strip_prefix("estimated duration:") {
                        if let Some(duration) = value
                            .split_whitespace()
                            .next()
                            .and_then(|s| s.parse::<f64>().ok())
                            .filter(|n| n.is_finite() && *n > 0.)
                        {
                            return Ok((duration, None));
                        }
                    }
                }
            }
        }
        Err(bad("无法核对原音时长，处理结果未提交"))
    }
    /// Only generated media projections may be revised in place; L0 desktop observations remain immutable.
    fn revise_media_projection(&self, source: &str, id: &str, e: &IngestEvent) -> Result<()> {
        let policy = self
            .sources()?
            .into_iter()
            .find(|s| s.source_id == source)
            .ok_or_else(|| bad("来源缺失"))?
            .policy;
        let mut projected = e.clone();
        if let Some(raw)=self.db.first("SELECT json_extract(payload_json,'$.native_legacy_source_id') FROM events WHERE event_id=?",&[text(id)],|r|r.opt_text(0))?.flatten(){projected.payload["native_legacy_source_id"]=json!(raw);}
        let e = &projected;
        let prepared = self.prepare(e, &policy)?;
        let old=self.db.first("SELECT rowid,title,app_name,url,payload_json,start_ms,end_ms FROM events WHERE event_id=?",&[text(id)],|r|(r.int(0),r.opt_text(1),r.opt_text(2),r.opt_text(3),r.text(4),r.opt_int(5),r.opt_int(6)))?.ok_or_else(||bad("媒体记录缺失"))?;
        let incoming = serde_json::to_string(&e.payload)?;
        let texts=self.db.query("SELECT field,c.text FROM event_text t JOIN text_chunks c USING(chunk_id) WHERE event_id=? ORDER BY field,ord",&[text(id)],|r|(r.text(0),r.text(1)))?;
        let desired: Vec<_> = e
            .text
            .iter()
            .flat_map(|t| {
                TextChunker::chunk(&t.content)
                    .into_iter()
                    .map(|c| (t.field.clone(), c.text))
            })
            .collect();
        if old.1 == e.title
            && serde_json::from_str::<Value>(&old.4)? == e.payload
            && old.5 == e.start_ms
            && old.6 == e.end_ms
            && texts == desired
        {
            return Ok(());
        }
        self.db.transaction(|| {
            self.db.execute(
                "INSERT INTO meta_fts(meta_fts,rowid,title,app_name,url) VALUES('delete',?,?,?,?)",
                &[
                    int(old.0),
                    crate::db::opt_text(old.1),
                    crate::db::opt_text(old.2),
                    crate::db::opt_text(old.3),
                ],
            )?;
            self.db.execute(
                "UPDATE events SET title=?,start_ms=?,end_ms=?,payload_json=? WHERE event_id=?",
                &[
                    crate::db::opt_text(e.title.clone()),
                    crate::db::opt_int(e.start_ms),
                    crate::db::opt_int(e.end_ms),
                    text(incoming),
                    text(id),
                ],
            )?;
            self.db.execute(
                "INSERT INTO meta_fts(rowid,title,app_name,url) VALUES(?,?,NULL,NULL)",
                &[int(old.0), crate::db::opt_text(e.title.clone())],
            )?;
            let old_chunks = self.db.query(
                "SELECT chunk_id FROM event_text WHERE event_id=?",
                &[text(id)],
                |r| r.int(0),
            )?;
            self.db
                .execute("DELETE FROM event_text WHERE event_id=?", &[text(id)])?;
            self.insert_text(
                &prepared,
                id,
                source,
                self.clock.now_ms(),
                &mut Default::default(),
            )?;
            for chunk in old_chunks {
                if self.db.scalar(
                    "SELECT COUNT(*) FROM event_text WHERE chunk_id=?",
                    &[int(chunk)],
                )? == 0
                {
                    if let Some(body) = self.db.first(
                        "SELECT text FROM text_chunks WHERE chunk_id=?",
                        &[int(chunk)],
                        |r| r.text(0),
                    )? {
                        self.db.execute(
                            "INSERT INTO text_fts(text_fts,rowid,text) VALUES('delete',?,?)",
                            &[int(chunk), text(body)],
                        )?;
                        self.db
                            .execute("DELETE FROM text_chunks WHERE chunk_id=?", &[int(chunk)])?;
                    }
                }
            }
            self.record_change("event", id, "update")
        })
    }
    pub(crate) fn media_text_allowed(&self, id: &str, label: &str) -> Result<bool> {
        let kind = media_kind(label)?;
        if self
            .sources()?
            .iter()
            .any(|s| s.kind == kind && s.display_name == "资料库" && !s.policy.store_text)
        {
            return Ok(false);
        }
        let mut ids = self.media_part_ids(id)?;
        ids.push(id.into());
        for original in ids {
            if self.db.scalar("SELECT COUNT(*) FROM events e JOIN sources old ON old.source_id=json_extract(e.payload_json,'$.native_legacy_source_id') OR old.source_id IN (SELECT value FROM json_each(e.payload_json,'$.native_legacy_sources')) WHERE e.source_key=? AND json_extract(old.policy_json,'$.store_text')<>1",&[text(format!("media:{original}"))])?>0{return Ok(false);}
        }
        Ok(true)
    }
    pub(crate) fn check_media_legacy(&self, id: &str) -> Result<()> {
        let denied=self.db.scalar("SELECT COUNT(*) FROM events e JOIN sources s ON s.source_id=json_extract(e.payload_json,'$.native_legacy_source_id') WHERE e.source_key=? AND (s.status='revoked' OR json_extract(s.policy_json,'$.store_media')<>1)",&[text(format!("media:{id}"))])?;
        if denied > 0 {
            return Err(TimelineError::Denied("legacy_media_source_revoked".into()));
        }
        Ok(())
    }
    pub(crate) fn check_media_source(&self, label: &str) -> Result<()> {
        let kind = media_kind(label)?;
        for source in self
            .sources()?
            .iter()
            .filter(|s| s.kind == kind && s.display_name == "资料库")
        {
            if source.status == "revoked" || !source.policy.store_media {
                return Err(TimelineError::Denied("media_source_revoked".into()));
            }
        }
        Ok(())
    }
    fn native_media_source(&self, label: &str) -> Result<String> {
        let kind = media_kind(label)?;
        if let Some(source) = self
            .sources()?
            .into_iter()
            .find(|s| s.kind == kind && s.display_name == "资料库")
        {
            return Ok(source.source_id);
        }
        let legacy = self.sources()?.into_iter().find(|s| {
            s.kind == "media.import"
                && s.display_name == "资料库"
                && s.device_id.as_deref() == Some("hub-media")
        });
        self.register_source_with(
            kind,
            "资料库",
            &legacy
                .map(|s| s.policy)
                .unwrap_or_else(|| SourcePolicy::new(true, true)),
            Some(&format!("native-media:{label}")),
            None,
            None,
            &json!({"native_media":1}),
            None,
        )
    }
    /// Reclassify a legacy mirror in place: stable event IDs, original links and old grants are retained.
    fn rehome_media_reference(&self, id: &str, target: &str) -> Result<()> {
        let glob = literal_glob(id);
        let related = "event_id IN (SELECT event_id FROM events WHERE source_key=? OR source_key GLOB ? OR source_key GLOB ? OR source_key GLOB ? UNION SELECT event_id FROM events WHERE kind='speech.segment' AND json_extract(payload_json,'$.native_projection_owner')=?)";
        let sources=self.db.query(&format!("SELECT DISTINCT source_id FROM events WHERE source_id<>? AND {related}"),&[text(target),text(format!("media:{id}")),text(format!("native-speech:{glob}:*")),text(format!("media-seg:{glob}:*")),text(format!("media-text:{glob}*")),text(id)],|r|r.text(0))?;
        let event_sources = sources.clone();
        let mut sources = sources;
        for old in self.db.query("SELECT source_id FROM sources WHERE display_name='资料库' AND source_id<>? AND kind IN ('media.import','mac.microphone','mac.system_audio','mac.region')",&[text(target)],|r|r.text(0))? {if !sources.contains(&old){sources.push(old);}}
        self.db.transaction(||{for old in sources {
            // Both source permissions apply until the user explicitly reconciles a legacy mixed vault.
            if event_sources.contains(&old) {
                self.db.execute(&format!("UPDATE events SET source_id=?,payload_json=json_set(payload_json,'$.native_legacy_source_id',COALESCE(json_extract(payload_json,'$.native_legacy_source_id'),?)) WHERE source_id=? AND {related}"),&[text(target),text(&old),text(&old),text(format!("media:{id}")),text(format!("native-speech:{glob}:*")),text(format!("media-seg:{glob}:*")),text(format!("media-text:{glob}*")),text(id)])?;
            }
            self.db.execute("UPDATE threads SET source_id=? WHERE source_id=? AND source_key=?",&[text(target),text(&old),text(format!("media:{id}"))])?;
            let old_key=crate::ingest::tombstone_key(&old,&format!("media:{id}"));let new_key=crate::ingest::tombstone_key(target,&format!("media:{id}"));
            self.db.execute("INSERT OR IGNORE INTO tombstones SELECT entity,?,deleted_ms,reason FROM tombstones WHERE entity='event_key' AND entity_id=?",&[text(new_key),text(old_key)])?;
        }Ok(())})
    }
    pub(crate) fn project_media_records(&self) -> Result<()> {
        if self.db.scalar(
            "SELECT (SELECT COUNT(*) FROM media_objects)+(SELECT COUNT(*) FROM media_deleted)",
            &[],
        )? == 0
        {
            return Ok(());
        }
        let rows=self.db.query("SELECT id,metadata,digest FROM media_objects WHERE id NOT IN (SELECT id FROM media_deleted)",&[],|r|(r.text(0),r.text(1),r.text(2)))?;
        // Ensure physical reference events exist first; bytes share their inode with the vault when possible.
        for (id, metadata, digest) in &rows {
            let m: Value = serde_json::from_str(metadata)?;
            let source = self.native_media_source(m["source"].as_str().unwrap_or(""))?;
            self.rehome_media_reference(id, &source)?;
            let key = format!("media:{id}");
            if self.event_id(&source, &key)?.is_none() {
                let mut e = IngestEvent::new(
                    &key,
                    if m["kind"] == "audio" {
                        "audio.recording"
                    } else {
                        "image.capture"
                    },
                    media_at(&m),
                );
                e.title = m["filename"].as_str().map(str::to_string);
                e.payload = json!({"hub_media_id":id,"origin_filename":m["filename"]});
                let staged = self.staging_directory().join(format!("{}.bin", uuid_v4()));
                let original = self.media_path(id)?;
                if fs::hard_link(&original, &staged).is_err() {
                    fs::copy(original, &staged)?;
                }
                let mut blob = IngestBlob::new(&staged.to_string_lossy(), digest);
                blob.mime_type = m["mime_type"].as_str().map(str::to_string);
                e.blobs.push(blob);
                let result = self.ingest(&IngestBatch::events(&source, vec![e]));
                let _ = fs::remove_file(staged);
                result?;
            }
        }
        let logical=self.db.query("SELECT DISTINCT recording_id FROM media_parts WHERE recording_id NOT IN (SELECT id FROM media_deleted)",&[],|r|r.text(0))?;
        let standalone: Vec<_> = rows
            .iter()
            .filter(|(id, _, _)| self.media_part(id).is_err())
            .map(|r| r.0.clone())
            .collect();
        for id in logical.into_iter().chain(standalone) {
            let m = match self.media_item(&id) {
                Ok(m) => m,
                Err(TimelineError::Denied(_)) => continue,
                Err(e) => return Err(e),
            };
            let source = self.native_media_source(m["source"].as_str().unwrap_or(""))?;
            self.rehome_media_reference(&id, &source)?;
            let key = format!("media:{id}");
            let at = media_at(&m);
            let mut e = IngestEvent::new(
                &key,
                if m["kind"] == "audio" {
                    "audio.recording"
                } else {
                    "image.capture"
                },
                at,
            );
            e.title = Some(
                m["title"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .or(m["filename"].as_str())
                    .unwrap_or("")
                    .into(),
            );
            e.end_ms = m["duration_seconds"]
                .as_f64()
                .and_then(|d| at.map(|t| t + (d * 1000.).round() as i64));
            e.payload = json!({"hub_media_id":id,"origin_filename":m["filename"],"recording_parts":m["recording_parts"],"gaps":m["gaps"],"recording_state":m["recording_state"]});
            if let Some(parts) = e.payload["recording_parts"].as_array_mut() {
                for part in parts {
                    let media_id = part["id"].as_str().ok_or_else(|| bad("原分段身份缺失"))?;
                    let physical_key = format!("media:{media_id}");
                    part["event_id"] = json!(self
                        .event_id(&source, &physical_key)?
                        .ok_or_else(|| bad("原分段引用缺失"))?);
                    part["source_key"] = json!(physical_key);
                }
            }

            let mut ceilings = vec![];
            let mut evidence_ids = self.media_part_ids(&id)?;
            evidence_ids.push(id.clone());
            for original in evidence_ids {
                for raw in self.db.query(
                    "SELECT payload_json FROM events WHERE source_key=?",
                    &[text(format!("media:{original}"))],
                    |r| r.text(0),
                )? {
                    let payload: Value = serde_json::from_str(&raw)?;
                    if let Some(legacy) = payload["native_legacy_source_id"].as_str() {
                        if !ceilings.contains(&legacy.to_string()) {
                            ceilings.push(legacy.to_string());
                        }
                    }
                    if let Some(many) = payload["native_legacy_sources"].as_array() {
                        for legacy in many.iter().filter_map(Value::as_str) {
                            if !ceilings.contains(&legacy.to_string()) {
                                ceilings.push(legacy.to_string());
                            }
                        }
                    }
                }
            }
            if !ceilings.is_empty() {
                e.payload["native_legacy_sources"] = json!(ceilings);
            }
            if let Some(notes) = m["notes"].as_str().filter(|s| !s.is_empty()) {
                e.text.push(IngestText::new("note", notes));
            }
            if self.event_id(&source, &key)?.is_none() {
                self.ingest(&IngestBatch::events(&source, vec![e.clone()]))?;
            }
            let parent = self
                .event_id(&source, &key)?
                .ok_or_else(|| bad("录音来源缺失"))?;
            self.revise_media_projection(&source, &parent, &e)?;
            let parts = self.media_part_ids(&id)?;
            for part in &parts {
                if let Some(part_event) = self.event_id(&source, &format!("media:{part}"))? {
                    self.db.execute("UPDATE events SET parent_event_id=?,payload_json=json_set(payload_json,'$.native_hidden',1,'$.logical_recording_id',?) WHERE event_id=? AND (parent_event_id IS NOT ? OR json_extract(payload_json,'$.native_hidden') IS NOT 1 OR json_extract(payload_json,'$.logical_recording_id') IS NOT ?)",&[text(&parent),text(&id),text(&part_event),text(&parent),text(&id)])?;
                }
            }
            let mut current = vec![];
            let revision = sha256_hex(serde_json::to_vec(&json!([m["segments"], m["producer"]]))?);
            for (i, seg) in m["segments"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .enumerate()
            {
                let start = seg["start_seconds"]
                    .as_f64()
                    .ok_or_else(|| bad("转写范围无效"))?;
                let end = seg["end_seconds"]
                    .as_f64()
                    .ok_or_else(|| bad("转写范围无效"))?;
                let speech_key = format!("native-speech:{id}:{revision}:{i}");
                current.push(speech_key.clone());
                if self.event_id(&source, &speech_key)?.is_none() {
                    let mut d = IngestEvent::new(
                        &speech_key,
                        "speech.segment",
                        at.map(|t| t + (start * 1000.).round() as i64),
                    );
                    d.end_ms = at.map(|t| t + (end * 1000.).round() as i64);
                    d.parent_key = Some(key.clone());
                    d.payload = json!({"recording_event_id":parent,"start_seconds":start,"end_seconds":end,"source_segment_id":seg["source_segment_id"],"producer":seg.get("producer").unwrap_or(&m["producer"]),"native_projection_owner":id});
                    if let Some(rate) = m["recording_parts"]
                        .as_array()
                        .and_then(|p| p.first())
                        .and_then(|p| p["sample_rate"].as_i64())
                    {
                        d.payload["sample_start"] = json!((start * rate as f64).round() as i64);
                        d.payload["sample_end"] = json!((end * rate as f64).round() as i64);
                        d.payload["sample_rate"] = json!(rate);
                    }
                    if let Some(ceilings) = e.payload.get("native_legacy_sources") {
                        d.payload["native_legacy_sources"] = ceilings.clone();
                    }
                    d.text.push(IngestText::new(
                        "transcript",
                        seg["text"].as_str().unwrap_or(""),
                    ));
                    self.ingest(&IngestBatch::events(&source, vec![d]))?;
                }
            }
            // Query-time visibility leaves immutable model revisions and physical references intact.
            let mut owners = parts;
            owners.push(id.clone());
            for owner in owners {
                let pattern = literal_glob(&owner);
                for (event,key) in self.db.query("SELECT event_id,source_key FROM events WHERE source_id=? AND kind='speech.segment' AND (json_extract(payload_json,'$.native_projection_owner')=? OR source_key GLOB ? OR source_key GLOB ?)",&[text(&source),text(&owner),text(format!("native-speech:{pattern}:*")),text(format!("media-seg:{pattern}:*"))],|r|(r.text(0),r.text(1)))? {let hidden=!current.contains(&key);self.db.execute("UPDATE events SET payload_json=json_set(payload_json,'$.native_hidden',?) WHERE event_id=? AND json_extract(payload_json,'$.native_hidden') IS NOT ?",&[int(hidden as i64),text(event),int(hidden as i64)])?;}
            }
        }
        for id in self
            .db
            .query("SELECT id FROM media_deleted", &[], |r| r.text(0))?
        {
            let events=self.db.query("SELECT event_id FROM events WHERE source_key=? AND source_id IN (SELECT source_id FROM sources WHERE display_name='资料库' AND kind IN ('media.import','mac.microphone','mac.system_audio','mac.region'))",&[text(format!("media:{id}"))],|r|r.text(0))?;
            if !events.is_empty() {
                self.delete_events(&events, "user")?;
            }
        }
        Ok(())
    }
}
fn media_at(m: &Value) -> Option<i64> {
    m["occurred_at"]
        .as_str()
        .or(m["imported_at"].as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
}

fn media_kind(label: &str) -> Result<&'static str> {
    match label {
        "import" => Ok("media.import"),
        "mac_microphone" => Ok("mac.microphone"),
        "mac_system_audio" => Ok("mac.system_audio"),
        "mac_region" => Ok("mac.region"),
        _ => Err(bad("未知媒体来源")),
    }
}
