//! Native media storage. Local commands accept files, never require a model runtime.
use crate::{
    db::{int, text, Db, RowExt},
    sha256_hex_file, uuid_v4, Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;
fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}
fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(&format!("缺少 {k}")))
}
fn key(s: &str) -> String {
    s.nfkc().collect::<String>().to_lowercase()
}
fn uuid(s: &str) -> Result<()> {
    if s.len() != 36
        || !s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(bad("资料编号无效"));
    }
    Ok(())
}
fn mime(path: &Path, kind: &str) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = vec![0; 4096];
    let n = f.read(&mut h)?;
    h.truncate(n);
    let m = if kind == "image" {
        if h.starts_with(b"\x89PNG\r\n\x1a\n") && h.get(12..16) == Some(b"IHDR") {
            Some("image/png")
        } else if h.starts_with(b"\xff\xd8\xff") {
            Some("image/jpeg")
        } else if crate::webp::is_webp(&h) {
            Some("image/webp")
        } else {
            None
        }
    } else if kind == "audio" {
        if h.starts_with(b"RIFF") && h.get(8..12) == Some(b"WAVE") {
            Some("audio/wav")
        } else if h.starts_with(b"fLaC") {
            Some("audio/flac")
        } else if h.starts_with(b"OggS") {
            Some("audio/ogg")
        } else if h.get(4..8) == Some(b"ftyp") {
            Some("audio/mp4")
        } else if h.starts_with(b"ID3") || h.len() > 1 && h[0] == 255 && h[1] & 224 == 224 {
            Some("audio/mpeg")
        } else {
            None
        }
    } else {
        None
    };
    m.map(str::to_string)
        .ok_or_else(|| bad("文件格式与类型不符"))
}
impl TimelineStore {
    pub fn media_command(&self, v: &Value) -> Result<Value> {
        let action = v["action"].as_str().unwrap_or("list");
        let id = v["id"].as_str().unwrap_or("");
        if action != "migrate"
            && self.db.scalar(
                "SELECT COUNT(*) FROM store_meta WHERE key='media.migration.pending'",
                &[],
            )? > 0
        {
            return Err(bad("旧库迁移未完成，请完成核对后重试"));
        }
        let result = match action {
            "import" => self.media_import(v),
            "list" => {
                let q = key(v["q"].as_str().unwrap_or(""));
                let ids=self.db.query("SELECT id FROM media_objects WHERE id NOT IN (SELECT id FROM media_parts) UNION SELECT DISTINCT recording_id FROM media_parts",&[],|r|r.text(0))?;
                let mut items = vec![];
                for id in ids {
                    if let Ok(item) = self.media_item(&id) {
                        if q.is_empty() || key(&item.to_string()).contains(&q) {
                            items.push(item);
                        }
                    }
                }
                items.sort_by(|a, b| {
                    b["occurred_at"]
                        .as_str()
                        .or(b["imported_at"].as_str())
                        .cmp(&a["occurred_at"].as_str().or(a["imported_at"].as_str()))
                });
                items.truncate(v["limit"].as_u64().unwrap_or(100).min(1000) as usize);
                Ok(json!({"items":items}))
            }
            "item" => self.media_item(id),
            "edit" => {
                self.media_item(id)?;
                let title = v["title"].as_str().unwrap_or("");
                let notes = v["notes"].as_str().unwrap_or("");
                if title.chars().count() > 255 || notes.chars().count() > 20000 {
                    return Err(bad("标题或备注过长"));
                }
                self.db.transaction(||{
                    self.db.execute("INSERT INTO media_edits(id,title,notes) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET title=excluded.title,notes=excluded.notes",&[text(id),text(title),text(notes)])?;
                    self.db.execute("INSERT OR REPLACE INTO store_meta VALUES(?,'true')",&[text(format!("media.manual/1:{id}"))])
                })?;
                self.media_item(id)
            }
            "link" => {
                self.db
                    .transaction(|| self.media_link(id, &v["recording"]))?;
                self.media_item(field(&v["recording"], "recording_id")?)
            }
            "state" => {
                if v["state"].is_null() {
                    return Ok(
                        json!({"id":id,"recording_state":if self.media_deleted(id)?{"deleted"}else{"present"}}),
                    );
                }
                self.media_item(id)?;
                let state = field(v, "state")?;
                if !["recording", "stopped", "interrupted"].contains(&state) {
                    return Err(bad("录音状态无效"));
                }
                self.db.execute("INSERT INTO media_edits(id,title,notes,state) VALUES(?,'','',?) ON CONFLICT(id) DO UPDATE SET state=excluded.state",&[text(id),text(state)])?;
                self.media_item(id)
            }
            "speaker" => self.label_speaker(v),
            "speaker_people" => self.speaker_people(),
            "delete" => {
                uuid(id)?;
                self.db.transaction(||{let mut ids=self.media_part_ids(id)?;ids.push(id.into());for sid in ids{self.voice_refs_forget_later(&sid)?;self.db.execute("INSERT OR IGNORE INTO media_deleted VALUES(?)",&[text(&sid)])?;self.db.execute("DELETE FROM media_objects WHERE id=?",&[text(&sid)])?;self.workflow_drafts_forget(&sid)?;self.db.execute("DELETE FROM store_meta WHERE key=? OR key IN (SELECT replace(key,'workflow.sources/1:','workflows.drafts/1:') FROM store_meta WHERE key LIKE 'workflow.sources/1:%' AND EXISTS(SELECT 1 FROM json_each(value) WHERE value=?))",&[text(format!("workflows.drafts/1:{sid}")),text(&sid)])?;self.db.execute("UPDATE job_checkpoints SET checkpoint='{}' WHERE job_id IN (SELECT job_id FROM jobs WHERE input_ref=?)",&[text(&sid)])?;self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL WHERE input_ref=?",&[text(&sid)])?;}Ok(json!({"deleted":id}))})
            }
            "content" => {
                self.media_item(id)?;
                let ids = self.media_part_ids(id)?;
                if !ids.is_empty() {
                    let mut parts = vec![];
                    for sid in ids {
                        let meta = self.media_part(&sid)?;
                        parts.push(json!({"id":sid,"path":self.media_path(&sid)?,"sample_start":meta["sample_start"],"sample_count":meta["sample_count"],"sample_rate":meta["sample_rate"]}));
                    }
                    return Ok(json!({"format":"candid-playlist/1","parts":parts}));
                }
                let path = self.media_path(id)?;
                Ok(json!({"path":path}))
            }
            "read" => {
                self.media_item(id)?;
                let path = self.media_path(id)?;
                let mut f = fs::File::open(path)?;
                let size = f.metadata()?.len();
                let start = v["offset"].as_u64().unwrap_or(0);
                if start > size {
                    return Err(bad("读取范围超出原件"));
                }
                let length = v["length"]
                    .as_u64()
                    .unwrap_or(262144)
                    .min(262144)
                    .min(size - start);
                f.seek(SeekFrom::Start(start))?;
                let mut bytes = vec![0; length as usize];
                f.read_exact(&mut bytes)?;
                use base64::Engine;
                Ok(
                    json!({"offset":start,"size":size,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}),
                )
            }
            "cancel_processing" => self.media_cancel_processing(id),
            "recognize" => {
                self.media_item(id)?;
                let mode = v["mode"].as_str().unwrap_or("quick");
                if !["quick", "conversation"].contains(&mode) {
                    return Err(bad("识别模式无效"));
                }
                let receipt = v["request_id"]
                    .as_str()
                    .map(|key| format!("media.recognize/1:{key}"));
                let identity = json!({"id":id,"mode":mode}).to_string();
                if let Some(key) = &receipt {
                    if let Some(old) = self.db.first(
                        "SELECT value FROM store_meta WHERE key=?",
                        &[text(key)],
                        |r| r.text(0),
                    )? {
                        if old != identity {
                            return Err(bad("重复请求的输入冲突"));
                        }
                        return self.media_item(id);
                    }
                }
                let mut ids = self.media_part_ids(id)?;
                if ids.is_empty() {
                    ids.push(id.into());
                }
                for sid in ids {
                    let job=self.job_command(&json!({"action":"enqueue","kind":"speech.recognize","input_ref":sid,"config_version":format!("external-api/1:{mode}")}))?;
                    if ["done", "failed", "cancelled"]
                        .contains(&job["state"].as_str().unwrap_or(""))
                    {
                        self.job_command(&json!({"action":"restart","id":job["id"]}))?;
                    }
                }
                if let Some(key) = receipt {
                    self.db.execute(
                        "INSERT OR REPLACE INTO store_meta VALUES(?,?)",
                        &[text(key), text(identity)],
                    )?;
                }
                self.media_item(id)
            }
            "result" => {
                if let Some(format) = v.get("format") {
                    if format != "candid-workflow-result/1" || v["request"]["speech"] == true {
                        return Err(bad("不支持的处理结果版本"));
                    }
                    return self.workflow_complete(&v["request"], &v["output"]);
                }
                let job_id = field(v, "job_id")?;
                let input = self
                    .db
                    .first(
                        "SELECT input_ref FROM jobs WHERE job_id=? AND kind='speech.recognize'",
                        &[text(job_id)],
                        |r| r.text(0),
                    )?
                    .ok_or_else(|| bad("任务类型不符"))?;
                let original = self.media_item(&input)?;
                if !self.media_text_allowed(&input, original["source"].as_str().unwrap_or(""))? {
                    return Err(TimelineError::Denied("media_text_storage_disabled".into()));
                }
                let result = &v["result"];
                let mode = field(v, "mode")?;
                if !["quick", "conversation"].contains(&mode) {
                    return Err(bad("识别模式无效"));
                }
                let config = self
                    .db
                    .first(
                        "SELECT config_version FROM jobs WHERE job_id=?",
                        &[text(job_id)],
                        |r| r.text(0),
                    )?
                    .unwrap_or_default();
                if let Some(expected) = config.strip_prefix("external-api/1:") {
                    if expected != mode {
                        return Err(bad("结果模式与任务模式不符"));
                    }
                }
                let digest = self
                    .db
                    .first(
                        "SELECT digest FROM media_objects WHERE id=?",
                        &[text(&input)],
                        |r| r.text(0),
                    )?
                    .ok_or_else(|| bad("原件不存在"))?;
                for object in [v, result, &result["producer"], &result["source"]] {
                    for name in ["source_sha256", "source_hash", "input_sha256"] {
                        if let Some(hash) = object.get(name) {
                            if hash.as_str() != Some(digest.as_str()) {
                                return Err(bad("结果来源原件校验不符"));
                            }
                        }
                    }
                }
                if let Some(hash) = result["source"].get("sha256") {
                    if hash.as_str() != Some(digest.as_str()) {
                        return Err(bad("结果来源原件校验不符"));
                    }
                }
                if self.media_raw(&input)?["kind"] != "audio" {
                    return Err(bad("转写来源必须是录音"));
                }
                if sha256_hex_file(&self.media_path(&input)?)? != digest {
                    return Err(TimelineError::BlobMismatch("转写原件内容已改变".into()));
                }
                let (duration, samples) = self.media_bounds(&input)?;

                if !result["producer"].is_object()
                    || result["producer"]["model_id"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                    || result["producer"]["revision"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                {
                    return Err(bad("处理结果需要模型和流水线版本"));
                }
                let segments = result["segments"]
                    .as_array()
                    .ok_or_else(|| bad("缺少转写分段"))?;
                for s in segments {
                    let a = s["start_seconds"]
                        .as_f64()
                        .ok_or_else(|| bad("缺少开始范围"))?;
                    let b = s["end_seconds"]
                        .as_f64()
                        .ok_or_else(|| bad("缺少结束范围"))?;
                    if !a.is_finite()
                        || !b.is_finite()
                        || a < 0.
                        || b <= a
                        || b > duration
                        || s["text"].as_str().is_none_or(|t| t.trim().is_empty())
                    {
                        return Err(bad("转写区间或文字无效"));
                    }
                    if let Some((rate, frames)) = samples {
                        let sample_start = (a * rate as f64).round() as u64;
                        let sample_end = (b * rate as f64).round() as u64;
                        if sample_start >= sample_end || sample_end > frames {
                            return Err(bad("转写样本半开区间无效"));
                        }
                        if let Some(declared) = s.get("sample_start") {
                            if declared.as_u64() != Some(sample_start) {
                                return Err(bad("转写开始样本与时间不符"));
                            }
                        }
                        if let Some(declared) = s.get("sample_end") {
                            if declared.as_u64() != Some(sample_end) {
                                return Err(bad("转写结束样本与时间不符"));
                            }
                        }
                        if let Some(declared) = s.get("sample_count") {
                            if declared.as_u64() != Some(sample_end - sample_start) {
                                return Err(bad("转写样本长度与时间不符"));
                            }
                        }
                        if let Some(declared) = s.get("sample_rate") {
                            if declared.as_u64() != Some(rate as u64) {
                                return Err(bad("转写采样率与原件不符"));
                            }
                        }
                    } else if s.get("sample_start").is_some() || s.get("sample_end").is_some() {
                        return Err(bad("此格式尚无可核对的原始样本坐标"));
                    }
                }
                let completed=self.db.scalar("SELECT COUNT(*) FROM jobs j JOIN job_checkpoints c USING(job_id) WHERE j.job_id=? AND j.state='done' AND j.attempts=? AND c.owner=?",&[text(job_id),int(v["attempt"].as_i64().unwrap_or(-1)),text(v["owner"].as_str().unwrap_or(""))])?==1;
                if completed {
                    let old = self.db.first(
                        "SELECT result FROM media_recognitions WHERE id=? AND mode=?",
                        &[text(&input), text(v["mode"].as_str().unwrap_or("quick"))],
                        |r| r.text(0),
                    )?;
                    if old
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .as_ref()
                        == Some(&v["result"])
                    {
                        self.project_native_media()?;
                        return self.media_item(&input);
                    }
                    return Err(bad("已完成任务的重试结果冲突"));
                }
                self.db.transaction(||{
      // Verify the lease here without nesting the queue's transaction.
      if self.db.scalar("SELECT COUNT(*) FROM jobs j JOIN job_checkpoints c USING(job_id) WHERE j.job_id=? AND j.state='running' AND j.attempts=? AND c.owner=? AND j.lease_until_ms>?",&[text(job_id),int(v["attempt"].as_i64().unwrap_or(-1)),text(v["owner"].as_str().unwrap_or("")),int(self.clock.now_ms())])?!=1{return Err(TimelineError::Denied("stale_job_lease".into()));}
      let mut m=self.media_raw(&input)?;m["segments"]=result["segments"].clone();
      if let Some((rate,_))=samples {for segment in m["segments"].as_array_mut().unwrap(){let a=(segment["start_seconds"].as_f64().unwrap()*rate as f64).round() as u64;let b=(segment["end_seconds"].as_f64().unwrap()*rate as f64).round() as u64;segment["sample_start"]=json!(a);segment["sample_end"]=json!(b);segment["sample_count"]=json!(b-a);segment["sample_rate"]=json!(rate);}}m["producer"]=result["producer"].clone();m["processing_status"]=json!("ready");m["processing_mode"]=v["mode"].clone();
      let history=format!("history:{}:{}:{}",mode,job_id,v["attempt"].as_i64().unwrap_or(-1));
      self.db.execute("INSERT OR IGNORE INTO media_recognitions VALUES(?,?,?)",&[text(&input),text(history),text(result.to_string())])?;
      self.db.execute("INSERT OR REPLACE INTO media_recognitions VALUES(?,?,?)",&[text(&input),text(v["mode"].as_str().unwrap_or("quick")),text(result.to_string())])?;
      self.db.execute("UPDATE media_objects SET metadata=? WHERE id=?",&[text(m.to_string()),text(&input)])?;
      self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,updated_ms=? WHERE job_id=?",&[int(self.clock.now_ms()),text(job_id)])?;Ok(m)
    })
            }
            "migrate" => self.migrate_media(Path::new(field(v, "hub")?)),
            _ => Err(bad("未知资料操作")),
        };
        if result.is_ok()
            && [
                "import",
                "link",
                "edit",
                "state",
                "migrate",
                "recognize",
                "result",
                "delete",
            ]
            .contains(&action)
        {
            self.project_native_media()?;
        }
        if result.is_ok() && action == "delete" {
            self.collect_media_orphans()?;
        }
        result
    }
    pub fn prepare_media(&self, hub: Option<&Path>) -> Result<()> {
        let mut origins = self
            .db
            .query("SELECT origin FROM media_migrations", &[], |r| r.text(0))?;
        if let Some(hub) = hub.filter(|p| p.join("memory.sqlite").exists()) {
            origins.push(hub.to_string_lossy().to_string());
        }
        for origin in origins {
            if let Err(e) = self.migrate_media(Path::new(&origin)) {
                self.db.execute(
                    "INSERT OR REPLACE INTO store_meta VALUES('media.migration.pending',?)",
                    &[text(&origin)],
                )?;
                return Err(e);
            }
        }
        self.project_native_media()?;
        let stamp = (self.background_revision("media_projection")?, self.clock.now_ms().div_euclid(300000));
        if self.media_cleanup_stamp.get() != Some(stamp) {
            self.collect_media_orphans()?;
            self.media_cleanup_stamp.set(Some(stamp));
        }
        Ok(())
    }
    fn collect_media_orphans(&self) -> Result<()> {
        let paths: std::collections::HashSet<_> = self
            .db
            .query("SELECT DISTINCT blob_path FROM media_objects", &[], |r| {
                r.text(0)
            })?.into_iter().collect();
        let directory = self.root.join("media");
        if directory.exists() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let rel = format!("media/{}", entry.file_name().to_string_lossy());
                if !paths.contains(&rel) && entry.file_type()?.is_file() {
                    fs::remove_file(entry.path())?;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn media_deleted(&self, id: &str) -> Result<bool> {
        Ok(self
            .db
            .scalar("SELECT COUNT(*) FROM media_deleted WHERE id=?", &[text(id)])?
            > 0)
    }
    pub(crate) fn media_part_ids(&self, id: &str) -> Result<Vec<String>> {
        self.db.query("SELECT id FROM media_parts WHERE recording_id=? ORDER BY json_extract(metadata,'$.sample_start')",&[text(id)],|r|r.text(0))
    }
    pub(crate) fn media_part(&self, id: &str) -> Result<Value> {
        let s = self
            .db
            .first(
                "SELECT metadata FROM media_parts WHERE id=?",
                &[text(id)],
                |r| r.text(0),
            )?
            .ok_or_else(|| bad("缺少分段"))?;
        Ok(serde_json::from_str(&s)?)
    }
    pub(crate) fn media_path(&self, id: &str) -> Result<PathBuf> {
        if self.media_deleted(id)? {
            return Err(TimelineError::NotFound("已删除资料".into()));
        }
        let p = self
            .db
            .first(
                "SELECT blob_path FROM media_objects WHERE id=?",
                &[text(id)],
                |r| r.text(0),
            )?
            .ok_or_else(|| TimelineError::NotFound("原件".into()))?;
        Ok(self.root.join(p))
    }
    pub(crate) fn media_raw(&self, id: &str) -> Result<Value> {
        if self.media_deleted(id)? {
            return Err(TimelineError::NotFound("资料".into()));
        }
        let s = self
            .db
            .first(
                "SELECT metadata FROM media_objects WHERE id=?",
                &[text(id)],
                |r| r.text(0),
            )?
            .ok_or_else(|| TimelineError::NotFound("资料".into()))?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn media_item(&self, id: &str) -> Result<Value> {
        if self.media_deleted(id)? {
            return Err(TimelineError::NotFound("资料".into()));
        }
        let ids = self.media_part_ids(id)?;
        self.check_media_legacy(id)?;
        for part in &ids {
            self.check_media_legacy(part)?;
        }
        let mut item = if ids.is_empty() {
            self.media_raw(id)?
        } else {
            let mut item = self.media_raw(&ids[0])?;
            let mut parts = vec![];
            let mut segments = vec![];
            let mut gaps = vec![];
            let mut cursor = 0;
            let mut rate = 1;
            for sid in &ids {
                let p = self.media_part(sid)?;
                let mut part = p.clone();
                part["id"] = json!(sid);
                parts.push(part);
                let start = p["sample_start"].as_i64().unwrap();
                let count = p["sample_count"].as_i64().unwrap();
                rate = p["sample_rate"].as_i64().unwrap();
                if start > cursor {
                    gaps.push(json!({"sample_start":cursor,"sample_count":start-cursor,"sample_rate":rate}));
                }
                cursor = start + count;
                let row = self.media_raw(sid)?;
                for s in row["segments"].as_array().unwrap_or(&vec![]) {
                    let mut s = s.clone();
                    for k in ["start_seconds", "end_seconds"] {
                        s[k] = json!(s[k].as_f64().unwrap_or(0.) + start as f64 / rate as f64);
                    }
                    if let Some(a) = s["sample_start"].as_u64() {
                        s["source_sample_start"] = json!(a);
                        s["sample_start"] = json!(a + start as u64);
                    }
                    if let Some(b) = s["sample_end"].as_u64() {
                        s["source_sample_end"] = json!(b);
                        s["sample_end"] = json!(b + start as u64);
                    }
                    if let Some(label) = s["speaker"].as_str().map(str::to_owned) {
                        s["source_speaker"] = json!(label);
                        s["speaker"] = json!(format!("录音段{} · {}", parts.len(), label));
                    }
                    s["source_segment_id"] = json!(sid);
                    s["producer"] = row["producer"].clone();
                    segments.push(s);
                }
            }
            item["id"] = json!(id);
            item["filename"] = json!("Mac 录音.wav");
            item["occurred_at"] = self.media_part(&ids[0])?["started_at"].clone();
            item["segments"] = json!(segments);
            item["recording_parts"] = json!(parts);
            item["gaps"] = json!(gaps);
            item["duration_seconds"] = json!(cursor as f64 / rate as f64);
            item["recording_state"] = json!("stopped");
            item["content_revision"] = json!(ids.join(":"));
            item
        };
        self.check_media_source(item["source"].as_str().unwrap_or(""))?;
        if let Some((title, notes, state)) = self.db.first(
            "SELECT title,notes,state FROM media_edits WHERE id=?",
            &[text(id)],
            |r| (r.text(0), r.text(1), r.text(2)),
        )? {
            item["title"] = json!(title);
            item["notes"] = json!(notes);
            item["recording_state"] = json!(state);
        }
        {
            let mut drafts = self.workflow_drafts(id)?;
            if drafts.as_object().is_some_and(|o| !o.is_empty()) {
                let revision = crate::speakers::segments_revision(&item["segments"]);
                if drafts["title"]["source_revision"] == revision
                    && self.db.scalar(
                        "SELECT COUNT(*) FROM store_meta WHERE key=?",
                        &[text(format!("media.manual/1:{id}"))],
                    )? == 0
                    && item["title"].as_str().is_none_or(|s| s.is_empty())
                {
                    item["title"] = drafts["title"]["text"].clone();
                }
                if let Some(values) = drafts.as_object_mut() {
                    values.retain(|_, d| d["source_revision"] == revision);
                }
                item["ai_drafts"] = drafts;
            }
        }
        let mut inputs = ids;
        if inputs.is_empty() {
            inputs.push(id.into());
        }
        self.media_processing(id,&inputs,&mut item)?;
        for input in inputs {
            let state = self.db.first(
                "SELECT state FROM jobs WHERE input_ref=? AND kind='speech.recognize' ORDER BY updated_ms DESC LIMIT 1",
                &[text(input)],
                |r| r.text(0),
            )?;
            if let Some(state) = state {
                if ["queued", "running", "failed"].contains(&state.as_str()) {
                    item["processing_status"] = json!(if state == "running" {
                        "processing"
                    } else {
                        &state
                    });
                    break;
                }
            }
        }
        if let Some(state)=self.db.first("SELECT state FROM jobs WHERE input_ref=? AND kind='workflow.speakers' ORDER BY updated_ms DESC,rowid DESC LIMIT 1",&[text(id)],|r|r.text(0))? {
            item["speaker_processing"]=json!(match state.as_str(){"running"=>"processing","done"=>"ready","cancelled"=>"idle",other=>other});
        }
        self.apply_speaker_labels(id, &mut item)?;
        if !self.media_text_allowed(id, item["source"].as_str().unwrap_or(""))? {
            item["speakers"] = Value::Null;
            item["segments"] = json!([]);
            item["notes"] = Value::Null;
            item["title"] = Value::Null;
            item["ai_drafts"] = Value::Null;
        }
        Ok(item)
    }
    fn media_import(&self, v: &Value) -> Result<Value> {
        let id = field(v, "id")?;
        uuid(id)?;
        if self.media_deleted(id)? {
            return Err(bad("已删除记录不能重新导入"));
        }
        let name = field(v, "filename")?;
        if name.len() > 1024 || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
            return Err(bad("文件名无效"));
        }
        let kind = field(v, "kind")?;
        let source = field(v, "source")?;
        if !["import", "mac_microphone", "mac_system_audio", "mac_region"].contains(&source)
            || ["mac_microphone", "mac_system_audio"].contains(&source) && kind != "audio"
            || source == "mac_region" && kind != "image"
        {
            return Err(bad("来源与类型不符"));
        }
        self.check_media_source(source)?;
        let path = Path::new(field(v, "path")?);
        if fs::symlink_metadata(path)?.file_type().is_symlink() || !path.is_file() {
            return Err(bad("需要普通原件文件"));
        }
        let kind_mime = mime(path, kind)?;
        let size = fs::metadata(path)?.len();
        if size == 0 || size > if kind == "image" { 64 << 20 } else { 512 << 20 } {
            return Err(bad("原件大小无效"));
        }
        if let Some(t) = v["occurred_at"].as_str() {
            chrono::DateTime::parse_from_rfc3339(t).map_err(|_| bad("发生时间必须带时区"))?;
        }
        let dir = self.root.join("media");
        fs::create_dir_all(&dir)?;
        let temp = dir.join(format!(".{}.pending", uuid_v4()));
        let result = (|| {
            let mut input = fs::File::open(path)?;
            let mut output = fs::File::create(&temp)?;
            std::io::copy(&mut input, &mut output)?;
            output.flush()?;
            output.sync_all()?;
            let digest = sha256_hex_file(&temp)?;
            let old = self.db.first(
                "SELECT digest,metadata FROM media_objects WHERE id=?",
                &[text(id)],
                |r| (r.text(0), r.text(1)),
            )?;
            if let Some((hash, m)) = old {
                let m: Value = serde_json::from_str(&m)?;
                // A part compacted to AAC after processing still answers a retried upload of its original WAV.
                if hash != digest && m["original_digest"].as_str() != Some(digest.as_str())
                    || m["filename"] != v["filename"]
                    || m["source"] != v["source"]
                    || m["kind"] != v["kind"]
                    || m["occurred_at"] != v["occurred_at"]
                {
                    return Err(bad("同一记录身份内容冲突"));
                }
                if v["recording"].is_object() {
                    self.db
                        .transaction(|| self.media_link(id, &v["recording"]))?;
                }
                return self.media_item(v["recording"]["recording_id"].as_str().unwrap_or(id));
            }
            let rel = format!("media/{digest}");
            let destination = self.root.join(&rel);
            if destination.exists() {
                if sha256_hex_file(&destination)? != digest {
                    return Err(bad("已有原件校验失败"));
                }
            } else {
                fs::rename(&temp, &destination)?;
                fs::File::open(&dir)?.sync_all()?;
            }
            let mut item = json!({"id":id,"filename":name,"kind":kind,"source":source,"occurred_at":v["occurred_at"],"imported_at":chrono::Utc::now().to_rfc3339(),"processing_status":"idle","processing_mode":null,"error":null,"segments":[] ,"mime_type":kind_mime});
            // New Mac recordings keep their WAV only until processed, then become AAC (data model v1 §9.4).
            if ["mac_microphone", "mac_system_audio"].contains(&source) && kind_mime == "audio/wav" {
                item["compact_to"] = json!(crate::media_compaction::COMPACT_AUDIO_MIME);
            }
            self.db.transaction(|| {
                self.db.execute(
                    "INSERT INTO media_objects VALUES(?,?,?,?,?)",
                    &[
                        text(id),
                        text(serde_json::to_string(&item)?),
                        text(rel),
                        text(digest),
                        int(size as i64),
                    ],
                )?;
                if v["recording"].is_object() {
                    self.media_link(id, &v["recording"])?;
                }
                Ok(())
            })?;
            self.media_item(v["recording"]["recording_id"].as_str().unwrap_or(id))
        })();
        let _ = fs::remove_file(temp);
        result
    }
    fn media_link(&self, id: &str, p: &Value) -> Result<()> {
        let rid = field(p, "recording_id")?;
        uuid(rid)?;
        if id == rid
            || self.media_deleted(rid)?
            || self.db.scalar(
                "SELECT COUNT(*) FROM media_objects WHERE id=?",
                &[text(rid)],
            )? > 0
        {
            return Err(bad("录音身份冲突或已删除"));
        }
        let row = self.media_raw(id)?;
        if !matches!(row["source"].as_str(), Some("mac_microphone" | "mac_system_audio")) || row["kind"] != "audio" {
            return Err(bad("只有 Mac 录音可关联"));
        }
        let index = p["segment_index"]
            .as_i64()
            .or(p["index"].as_i64())
            .ok_or_else(|| bad("缺少序号"))?;
        let start = p["sample_start"].as_i64().unwrap_or(-1);
        let count = p["sample_count"].as_i64().unwrap_or(0);
        let rate = p["sample_rate"].as_i64().unwrap_or(0);
        let at = p["recording_started_at"]
            .as_str()
            .or(p["started_at"].as_str())
            .ok_or_else(|| bad("缺少开始时间"))?;
        chrono::DateTime::parse_from_rfc3339(at).map_err(|_| bad("开始时间无效"))?;
        if index < 0
            || start < 0
            || !(8000..=192000).contains(&rate)
            || count <= 0
            // Up to an hour: parts written as fragmented AAC are ten minutes long; WAV parts were 30–60 s.
            || count > rate * 3600
            || start > 192000 * 86400
        {
            return Err(bad("样本范围无效"));
        }
        let meta = json!({"index":index,"sample_start":start,"sample_count":count,"sample_rate":rate,"started_at":at});
        if let Some((owner, old)) = self.db.first(
            "SELECT recording_id,metadata FROM media_parts WHERE id=?",
            &[text(id)],
            |r| (r.text(0), r.text(1)),
        )? {
            if owner != rid || serde_json::from_str::<Value>(&old)? != meta {
                return Err(bad("分段归属冲突"));
            }
            return Ok(());
        }
        // A new link: part coordinates must agree with the original's frame count and sample rate. AAC parts
        // (fragmented MP4 written while recording) report exact valid frames through Core Audio. (An existing link
        // returns above, so a part whose WAV was later compacted to AAC still re-links.)
        let bytes = fs::read(self.media_path(id)?)?;
        if bytes.get(4..8) == Some(b"ftyp") {
            match self.media_bounds(id)?.1 {
                Some((r, frames)) if r as i64 == rate && frames as i64 == count => {}
                _ => return Err(bad("样本范围与原件不符")),
            }
        } else {
            Self::check_wav_part(&bytes, rate, count)?;
        }
        for sid in self.media_part_ids(rid)? {
            let other = self.media_part(&sid)?;
            let a = other["sample_start"].as_i64().unwrap();
            let b = a + other["sample_count"].as_i64().unwrap();
            let i = other["index"].as_i64().unwrap();
            if self.media_raw(&sid)?["source"] != row["source"]
                || other["sample_rate"] != json!(rate)
                || other["started_at"] != json!(at)
                || index == i
                || start.max(a) < (start + count).min(b)
                || (i < index) != (a < start)
            {
                return Err(bad("分段序号、区间或来源冲突"));
            }
        }
        self.db.execute(
            "INSERT INTO media_parts VALUES(?,?,?)",
            &[text(id), text(rid), text(meta.to_string())],
        )
    }
    pub fn project_native_media(&self) -> Result<()> {
        let revision = self.background_revision("media_projection")?;
        if self.media_projection_stamp.get() == Some(revision) { return Ok(()); }
        self.project_media_records()?;
        self.media_projection_stamp.set(Some(self.background_revision("media_projection")?));
        Ok(())
    }
    pub fn migrate_media(&self, hub: &Path) -> Result<Value> {
        let origin = hub.canonicalize()?.to_string_lossy().to_string();
        if !self.media_locks.borrow().contains_key(&origin) {
            let lock = Db::open(
                &hub.join("vault/media_timeline/service.sqlite"),
                false,
                true,
            )?;
            lock.execute("PRAGMA busy_timeout=0", &[])?;
            lock.execute("BEGIN EXCLUSIVE", &[])
                .map_err(|_| TimelineError::Locked)?;
            self.media_locks.borrow_mut().insert(origin.clone(), lock);
        }
        if let Some(receipt) = self.db.first(
            "SELECT receipt FROM media_migrations WHERE origin=?",
            &[text(&origin)],
            |r| r.text(0),
        )? {
            self.db.execute(
                "DELETE FROM store_meta WHERE key='media.migration.pending' AND value=?",
                &[text(&origin)],
            )?;
            return Ok(serde_json::from_str(&receipt)?);
        }
        let old = Db::open(&hub.join("memory.sqlite"), true, false)?;
        if old.scalar(
            "SELECT version FROM media_timeline_schema WHERE slot=1",
            &[],
        )? != 1
        {
            return Err(bad("未知旧媒体库版本"));
        }
        let root = hub.join("vault/media_timeline");
        let side = root.join("recordings-v1.json");
        let index: Value = if side.exists() {
            serde_json::from_slice(&fs::read(side)?)?
        } else {
            json!({"version":1,"recordings":{},"edits":{},"deleted":[]})
        };
        if index["version"] != 1 {
            return Err(bad("未知录音索引版本"));
        }
        let rows=old.query("SELECT m.id,m.filename,m.kind,m.source,m.occurred_at,m.imported_at,m.blob,m.processing_status,m.processing_mode,r.result FROM media_timeline m LEFT JOIN media_timeline_recognition r ON r.media_id=m.id AND r.mode=m.processing_mode",&[],|r|(r.text(0),r.text(1),r.text(2),r.text(3),r.opt_text(4),r.text(5),r.text(6),r.text(7),r.opt_text(8),r.opt_text(9)))?;
        self.db.execute(
            "INSERT OR REPLACE INTO store_meta VALUES('media.migration.pending',?)",
            &[text(&origin)],
        )?;
        let mut legacy_deleted =
            old.query("SELECT id FROM media_timeline_deleted", &[], |r| r.text(0))?;
        legacy_deleted.extend(
            index["deleted"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string)),
        );
        if let Some(recordings) = index["recordings"].as_object() {
            for (rid, rec) in recordings {
                if legacy_deleted.contains(rid) {
                    legacy_deleted.extend(
                        rec["parts"]
                            .as_object()
                            .map(|p| p.keys().cloned().collect::<Vec<_>>())
                            .unwrap_or_default(),
                    );
                }
            }
        }
        // Each object is durable and idempotent; incomplete passes are replayed before a receipt is written.
        for (id, name, kind, source, at, imported, blob, status, mode, result) in &rows {
            if Path::new(blob).components().count() != 1
                || blob.starts_with('.')
                || blob.contains(['/', '\\'])
            {
                return Err(bad("旧原件路径越界"));
            }
            if self.media_deleted(id)? || legacy_deleted.contains(id) {
                continue;
            }
            let original = root.join("blobs").join(blob);
            if let Some(stem) = blob
                .split('.')
                .next()
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                if sha256_hex_file(&original)? != stem {
                    return Err(bad("旧原件校验失败"));
                }
            }
            let mut v = json!({"action":"import","id":id,"filename":name,"kind":kind,"source":source,"occurred_at":at,"path":root.join("blobs").join(blob)});
            self.media_import(&v)?;
            v = self.media_raw(id)?;
            v["imported_at"] = json!(imported);
            v["processing_mode"] = json!(mode);
            if let Some(result) = result {
                let result: Value = serde_json::from_str(result)?;
                v["segments"] = result["segments"]
                    .as_array()
                    .map(|a| json!(a))
                    .unwrap_or(json!([]));
                v["producer"] = result["producer"].clone();
                v["processing_status"] = json!("ready");
            }
            self.db.execute(
                "UPDATE media_objects SET metadata=? WHERE id=?",
                &[text(v.to_string()), text(id)],
            )?;
            if ["queued", "processing"].contains(&status.as_str()) {
                self.job_command(&json!({"action":"enqueue","kind":"speech.recognize","input_ref":id,"config_version":format!("external-api/1:{}",mode.as_deref().unwrap_or("quick"))}))?;
            }
        }
        self.db.transaction(||{
   for (id,mode,result) in old.query("SELECT media_id,mode,result FROM media_timeline_recognition",&[],|r|(r.text(0),r.text(1),r.text(2)))? {if self.db.scalar("SELECT COUNT(*) FROM media_objects WHERE id=?",&[text(&id)])?>0{self.db.execute("INSERT OR REPLACE INTO media_recognitions VALUES(?,?,?)",&[text(id),text(mode),text(result)])?;}}
   if let Some(recordings)=index["recordings"].as_object(){for (rid,rec) in recordings{if let Some(parts)=rec["parts"].as_object(){for (sid,p) in parts{if self.db.scalar("SELECT COUNT(*) FROM media_objects WHERE id=?",&[text(sid)])?==0{if index["deleted"].as_array().is_some_and(|a|a.contains(&json!(rid))) || self.media_deleted(sid)? {continue;}return Err(bad("旧录音引用原件缺失"));}let mut p=p.clone();p["recording_id"]=json!(rid);self.media_link(sid,&p)?;}}self.db.execute("INSERT OR IGNORE INTO media_edits(id,title,notes,state) VALUES(?,'','',?)",&[text(rid),text(rec["state"].as_str().unwrap_or("interrupted"))])?;}}
   if let Some(edits)=index["edits"].as_object(){for (id,e) in edits{self.db.execute("INSERT INTO media_edits(id,title,notes) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET title=excluded.title,notes=excluded.notes",&[text(id),text(e["title"].as_str().unwrap_or("")),text(e["notes"].as_str().unwrap_or(""))])?;}}
   for id in legacy_deleted{self.db.execute("INSERT OR IGNORE INTO media_deleted VALUES(?)",&[text(id)])?;}
   let receipt=json!({"version":1,"objects":rows.len(),"owner":"rust","old_vault":"retained"});self.db.execute("INSERT OR REPLACE INTO media_migrations VALUES(?,?)",&[text(&origin),text(receipt.to_string())])?;self.db.execute("DELETE FROM store_meta WHERE key='media.migration.pending'",&[])?;Ok(receipt)
  })
    }
}

impl TimelineStore {
    /// A WAV part: its format chunk's rate and its data chunk's frames must match the declared coordinates.
    fn check_wav_part(bytes: &[u8], rate: i64, count: i64) -> Result<()> {
        let mut offset = 12;
        let mut actual_rate = 0;
        let mut align = 0;
        let mut data = 0;
        while offset + 8 <= bytes.len() {
            let n = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if offset + 8 + n > bytes.len() {
                return Err(bad("WAV 分段损坏"));
            }
            if &bytes[offset..offset + 4] == b"fmt " && n >= 16 {
                actual_rate =
                    u32::from_le_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()) as i64;
                align =
                    u16::from_le_bytes(bytes[offset + 20..offset + 22].try_into().unwrap()) as i64;
            }
            if &bytes[offset..offset + 4] == b"data" {
                data = n as i64;
            }
            offset += 8 + n + (n % 2);
        }
        if actual_rate != rate || align == 0 || data / align != count {
            return Err(bad("样本范围与原件不符"));
        }
        Ok(())
    }
}
