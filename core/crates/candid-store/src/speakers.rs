//! Speakers in a recording (agent-platform-v1 §2.6 speaker-labeling): each speaker is recognised automatically,
//! possibly someone (waiting for one click), named by you, or anonymous. Your choice is a `speaker` correction;
//! people are `person` entities, tied to the voice library by a `voice` alias, so a rename or merge there shows
//! up in every recording.
use crate::{
    db::{int, text, RowExt},
    uuid_v7, Result, TimelineError, TimelineStore,
};
use serde_json::{json, Map, Value};

/// The voice service's own matching policy (services/voice/asr/speaker_store.py); the app does not set its own.
pub const MATCH_THRESHOLD: f64 = 0.65;
pub const MATCH_MARGIN: f64 = 0.05;
/// How far below the threshold a closest person is still offered as 「可能是」.
pub const MAYBE_BELOW: f64 = 0.10;

/// The transcript as recognised, without the names and states this module adds for display. Revisions of
/// workflow drafts are taken over this, so labelling or re-matching a speaker never makes a title stale.
pub(crate) fn transcript(segments: &Value) -> Value {
    let mut t = segments.clone();
    for s in t.as_array_mut().into_iter().flatten() {
        if let Some(o) = s.as_object_mut() {
            if let Some(Value::Object(raw))=o.remove("speaker_raw_fields") {
                for key in ["speaker","voice_ref","match","identity"] {o.remove(key);}
                o.extend(raw);
            }
            o.remove("speaker_display_label");
            o.remove("speaker_correction_key");
            o.remove("speaker_name");
            o.remove("speaker_status");
        }
    }
    t
}

pub(crate) fn segments_revision(segments: &Value) -> String {
    crate::sha256_hex(&serde_json::to_vec(&transcript(segments)).unwrap_or_default())
}

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

impl TimelineStore {
    fn entity_name(&self, entity: &str) -> Result<Option<(String, String)>> {
        // Follow merges to the surviving person.
        let mut id = entity.to_string();
        for _ in 0..8 {
            let Some((name, status, into)) = self.db.first(
                "SELECT name,status,merged_into FROM entities WHERE entity_id=?",
                &[text(&id)],
                |r| (r.text(0), r.text(1), r.opt_text(2)),
            )?
            else {
                return Ok(None);
            };
            match (status.as_str(), into) {
                ("merged", Some(next)) => id = next,
                ("rejected", _) => return Ok(None),
                _ => return Ok(Some((id, name))),
            }
        }
        Ok(None)
    }

    fn voice_entity(&self, voice: &str) -> Result<Option<String>> {
        self.db.first(
            "SELECT entity_id FROM entity_aliases WHERE alias_kind='voice' AND alias=? ORDER BY entity_id LIMIT 1",
            &[text(voice)],
            |r| r.text(0),
        )
    }

    /// The person entity for a voice-library person, created on first use. Runs inside a transaction.
    pub(crate) fn person_for_voice(&self, voice: Option<&str>, name: &str) -> Result<String> {
        if let Some(voice) = voice {
            if let Some(id) = self.voice_entity(voice)? {
                return Ok(self.entity_name(&id)?.map(|(id, _)| id).unwrap_or(id));
            }
        }
        let now = self.clock.now_ms();
        let id = uuid_v7(Some(now));
        self.db.execute("INSERT INTO entities VALUES(?,'person',?,'confirmed',NULL,?,?)", &[text(&id), text(name), int(now), int(now)])?;
        self.db.execute("INSERT INTO entity_aliases VALUES(?,?,'name','user')", &[text(&id), text(name)])?;
        if let Some(voice) = voice {
            self.db.execute("INSERT INTO entity_aliases VALUES(?,?,'voice','user')", &[text(&id), text(voice)])?;
        }
        Ok(id)
    }

    /// Adds `speakers` to a recording and `speaker_name` / `speaker_status` to each segment.
    pub(crate) fn apply_speaker_labels(&self, id: &str, item: &mut Value) -> Result<()> {
        self.apply_recording_speakers(id,item)?;
        let Some(segments) = item["segments"].as_array_mut() else { return Ok(()) };
        let mut order: Vec<String> = vec![];
        let mut speakers: Map<String, Value> = Map::new();
        for s in segments.iter() {
            let Some(key) = s["speaker"].as_str() else { continue };
            if !speakers.contains_key(key) {
                order.push(key.into());
                // A later re-match against the current library wins over the one made at transcription.
                let rematched = match s["voice_ref"].as_str() {
                    Some(r) => self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(format!("voice.match/1:{r}"))], |r| r.text(0))?
                        .and_then(|v| serde_json::from_str::<Value>(&v).ok()),
                    None => None,
                };
                let is_rematched=rematched.is_some();
                let mut matched = rematched.unwrap_or_else(|| s["match"].clone());
                if is_rematched && s.get("speaker_raw_fields").is_some() && matched.is_object() {
                    matched["threshold"]=json!(0.70);
                    matched["allow_auto"]=json!(matched["score"].as_f64().is_some_and(|n|n>=0.70) && matched["margin"].as_f64().is_none_or(|n|n>=MATCH_MARGIN));
                }
                let mut resolved=self.resolve_speaker(id,key,&matched)?;
                if resolved["status"]!="confirmed" && resolved["dismissed"]!=true {
                    if let Some(old)=s["speaker_correction_key"].as_str() {
                        let correction=self.resolve_speaker(id,old,&matched)?;
                        if correction["status"]=="confirmed" {resolved=correction;resolved["key"]=json!(key);}
                    }
                }
                if resolved["label"].is_null() {resolved["label"]=json!(format!("说话人 {}",order.len()));}
                resolved["match_reason"]=json!(if s["voice_ref"].is_null(){"no_sample"}else if matched["score"].is_null(){"no_candidate"}else if resolved["status"]=="auto"{"matched"}else if resolved["status"]=="maybe"{"needs_confirmation"}else{"below_threshold"});
                speakers.insert(key.into(), resolved);
            }
            let entry = speakers.get_mut(key).unwrap();
            if let Some(label)=s.get("speaker_display_label") {entry["label"]=label.clone();}
            entry["segments"] = json!(entry["segments"].as_i64().unwrap_or(0) + 1);
            entry["seconds"] = json!(entry["seconds"].as_f64().unwrap_or(0.)
                + (s["end_seconds"].as_f64().unwrap_or(0.) - s["start_seconds"].as_f64().unwrap_or(0.)).max(0.));
        }
        for s in segments.iter_mut() {
            let Some(entry) = s["speaker"].as_str().and_then(|k| speakers.get(k)) else { continue };
            let shown = if ["auto", "confirmed"].contains(&entry["status"].as_str().unwrap_or("")) { entry["name"].clone() } else { Value::Null };
            if s.get("speaker_raw_fields").is_some() {s["identity"]=shown.clone();}
            s["speaker_display_label"] = entry["label"].clone();
            s["speaker_name"] = shown;
            s["speaker_status"] = entry["status"].clone();
        }
        if !order.is_empty() {
            item["speakers"] = json!(order.iter().map(|k| speakers[k].clone()).collect::<Vec<_>>());
            item["content_revision"]=json!(format!("{}:voices:{}",item["content_revision"].as_str().unwrap_or(""),crate::sha256_hex(&serde_json::to_vec(&item["speakers"])?)));
        }
        Ok(())
    }

    pub(crate) fn resolve_speaker(&self, id: &str, key: &str, matched: &Value) -> Result<Value> {
        let threshold=matched["threshold"].as_f64().filter(|n|n.is_finite()&&(0.5..=0.95).contains(n)).unwrap_or(MATCH_THRESHOLD);
        let score = matched["score"].as_f64();
        let margin = matched["margin"].as_f64();
        let voice = matched["person_id"].as_str();
        let mut out = json!({"key":key,"status":"anonymous","name":null,"person_id":null,"score":score,"margin":margin,
            "threshold":threshold,"segments":0,"seconds":0.0});
        if let Some(value) = self.db.first(
            "SELECT value_json FROM corrections WHERE target_type='speaker' AND target_id=? ORDER BY created_ms DESC, rowid DESC LIMIT 1",
            &[text(format!("{id}|{key}"))],
            |r| r.text(0),
        )? {
            let value: Value = serde_json::from_str(&value)?;
            if let Some((entity, name)) = value["entity_id"].as_str().map(|e| self.entity_name(e)).transpose()?.flatten() {
                out["status"] = json!("confirmed");
                out["name"] = json!(name);
                out["person_id"] = json!(entity);
                out["samples_saved"] = value["samples_saved"].clone();
                return Ok(out);
            }
            if value["anonymous"] == true {
                out["status"] = json!("anonymous");
                out["dismissed"] = json!(true);
                return Ok(out);
            }
        }
        let (Some(score), Some(voice)) = (score, voice) else { return Ok(out) };
        if self.db.scalar("SELECT COUNT(*) FROM store_meta WHERE key=?", &[text(format!("voice.deleted/1:{voice}"))])? > 0 {
            return Ok(out);
        }
        let (entity, name) = match self.voice_entity(voice)?.map(|e| self.entity_name(&e)).transpose()?.flatten() {
            Some((e, n)) => (Some(e), n),
            None => (None, matched["name"].as_str().unwrap_or("").to_string()),
        };
        if name.is_empty() {
            return Ok(out);
        }
        let clear = matched["allow_auto"] != false && (matched["status"] == "matched" || (score >= threshold && margin.is_none_or(|m| m >= MATCH_MARGIN)));
        if clear || score >= threshold - MAYBE_BELOW {
            out["status"] = json!(if clear { "auto" } else { "maybe" });
            out["name"] = json!(name);
            out["person_id"] = json!(entity);
            out["voice_id"] = json!(voice);
        }
        Ok(out)
    }

    /// Records who a speaker is: an existing person (`person_id` entity, or `voice_id` from the library), a new
    /// name, or anonymous. `samples_saved` says whether voice samples went into the library.
    pub fn label_speaker(&self, v: &Value) -> Result<Value> {
        let id = v["id"].as_str().ok_or_else(|| bad("缺少录音"))?;
        let key = v["speaker"].as_str().filter(|k| !k.is_empty() && k.len() <= 200).ok_or_else(|| bad("缺少说话人"))?;
        let item = self.media_item(id)?;
        if !item["segments"].as_array().into_iter().flatten().any(|s| s["speaker"] == key) {
            return Err(bad("录音里没有这个说话人"));
        }
        let now = self.clock.now_ms();
        self.db.transaction(|| {
            let value = if v["anonymous"] == true {
                json!({"anonymous":true})
            } else {
                let entity = match v["person_id"].as_str() {
                    Some(e) => self.entity_name(e)?.ok_or_else(|| bad("人物不存在"))?.0,
                    None => {
                        let name = v["name"].as_str().map(str::trim).filter(|n| (1..=80).contains(&n.chars().count())).ok_or_else(|| bad("名字需要 1–80 个字"))?;
                        let voice = v["voice_id"].as_str();
                        match (voice, self.db.first("SELECT entity_id FROM entity_aliases a JOIN entities e USING(entity_id) WHERE a.alias_kind='name' AND a.alias=? AND e.kind='person' AND e.status='confirmed' LIMIT 1", &[text(name)], |r| r.text(0))?) {
                            (None, Some(existing)) => existing,
                            _ => self.person_for_voice(voice, name)?,
                        }
                    }
                };
                if let Some(voice) = v["voice_id"].as_str() {
                    self.db.execute("INSERT OR IGNORE INTO entity_aliases VALUES(?,?,'voice','user')", &[text(&entity), text(voice)])?;
                }
                json!({"entity_id":entity,"samples_saved":v["samples_saved"] == true})
            };
            self.db.execute(
                "INSERT INTO corrections VALUES(?,'speaker',?,'relabel',?,?)",
                &[text(uuid_v7(Some(now))), text(format!("{id}|{key}")), text(value.to_string()), int(now)],
            )
        })?;
        self.media_item(id)
    }

    /// People for the 「这是谁」 picker: named people, most recently labelled first.
    pub fn speaker_people(&self) -> Result<Value> {
        let rows = self.db.query(
            "SELECT e.entity_id,e.name,(SELECT alias FROM entity_aliases a WHERE a.entity_id=e.entity_id AND a.alias_kind='voice' LIMIT 1),
                    (SELECT MAX(c.created_ms) FROM corrections c WHERE c.target_type='speaker' AND json_extract(c.value_json,'$.entity_id')=e.entity_id)
             FROM entities e WHERE e.kind='person' AND e.status='confirmed' ORDER BY 4 DESC NULLS LAST, e.name",
            &[],
            |r| json!({"person_id":r.text(0),"name":r.text(1),"voice_id":r.opt_text(2),"last_labelled_ms":r.opt_int(3)}),
        )?;
        Ok(json!({"people":rows}))
    }

    /// Keeps person entities in step with the voice library after a rename, merge or delete there.
    pub fn voice_library_changed(&self, action: &str, voice: &str, v: &Value) -> Result<()> {
        let now = self.clock.now_ms();
        self.db.transaction(|| {
            let entity = self.voice_entity(voice)?;
            match action {
                "rename" => {
                    let Some(name) = v["name"].as_str() else { return Ok(()) };
                    match entity {
                        Some(e) => {
                            self.db.execute("UPDATE entities SET name=?,updated_ms=? WHERE entity_id=?", &[text(name), int(now), text(&e)])?;
                            self.db.execute("INSERT OR IGNORE INTO entity_aliases VALUES(?,?,'name','user')", &[text(&e), text(name)])?;
                        }
                        // Recordings matched automatically carry the old name; the entity now holds the new one.
                        None => {
                            self.person_for_voice(Some(voice), name)?;
                        }
                    }
                }
                "merge" => {
                    let target = v["target_id"].as_str().ok_or_else(|| bad("缺少合并目标"))?;
                    let name = v["target_name"].as_str().unwrap_or("");
                    let into = match self.voice_entity(target)? {
                        Some(t) => t,
                        None if !name.is_empty() => self.person_for_voice(Some(target), name)?,
                        None => return Ok(()),
                    };
                    if let Some(e) = entity.filter(|e| *e != into) {
                        self.db.execute("UPDATE entities SET status='merged',merged_into=?,updated_ms=? WHERE entity_id=?", &[text(&into), int(now), text(&e)])?;
                    }
                    self.db.execute("INSERT OR IGNORE INTO entity_aliases VALUES(?,?,'voice','user')", &[text(&into), text(voice)])?;
                }
                "delete" => {
                    // Earlier recordings go back to anonymous; the entity and your labels go with the person.
                    self.db.execute("INSERT OR REPLACE INTO store_meta VALUES(?,?)", &[text(format!("voice.deleted/1:{voice}")), text(now.to_string())])?;
                    if let Some(e) = entity {
                        self.db.execute("DELETE FROM corrections WHERE target_type='speaker' AND json_extract(value_json,'$.entity_id')=?", &[text(&e)])?;
                        self.db.execute("UPDATE entities SET status='rejected',updated_ms=? WHERE entity_id=?", &[int(now), text(&e)])?;
                    }
                }
                _ => {}
            }
            Ok(())
        })
    }

    /// Every speaker reference in stored transcripts, for re-matching after the voice library changes.
    pub fn voice_refs(&self) -> Result<Vec<String>> {
        let mut refs = std::collections::BTreeSet::new();
        for metadata in self.db.query("SELECT metadata FROM media_objects WHERE metadata LIKE '%voice_ref%'", &[], |r| r.text(0))? {
            let m: Value = serde_json::from_str(&metadata).unwrap_or(Value::Null);
            for s in m["segments"].as_array().into_iter().flatten() {
                if let Some(r) = s["voice_ref"].as_str() {
                    refs.insert(r.to_string());
                }
            }
        }
        for value in self.db.query("SELECT value FROM store_meta WHERE key GLOB 'workflows.speakers.result/1:*'",&[],|r|r.text(0))? {
            let result:Value=serde_json::from_str(&value).unwrap_or(Value::Null);
            for r in result["voice_refs"].as_array().into_iter().flatten().filter_map(Value::as_str) {refs.insert(r.into());}
        }
        Ok(refs.into_iter().collect())
    }

    /// Stores re-match results (reference → match or null); null clears to "no clear match".
    pub fn voice_matches_save(&self, matches: &Map<String, Value>) -> Result<()> {
        self.db.transaction(|| {
            for (r, m) in matches {
                self.db.execute("INSERT OR REPLACE INTO store_meta VALUES(?,?)", &[text(format!("voice.match/1:{r}")), text(if m.is_null() { json!({}) } else { m.clone() }.to_string())])?;
            }
            Ok(())
        })
    }

    /// References of deleted recordings, waiting to be removed from the voice service; `done` clears them.
    pub fn voice_refs_to_forget(&self, done: Option<&[String]>) -> Result<Vec<String>> {
        let current: Vec<String> = self
            .db
            .first("SELECT value FROM store_meta WHERE key='voice.forget/1'", &[], |r| r.text(0))?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let Some(done) = done else { return Ok(current) };
        let left: Vec<String> = current.into_iter().filter(|r| !done.contains(r)).collect();
        self.db.execute("INSERT OR REPLACE INTO store_meta VALUES('voice.forget/1',?)", &[text(json!(left).to_string())])?;
        for r in done {
            self.db.execute("DELETE FROM store_meta WHERE key=?", &[text(format!("voice.match/1:{r}"))])?;
        }
        Ok(left)
    }

    /// Called inside the media delete transaction: queue the recording's speaker references for removal.
    pub(crate) fn voice_refs_forget_later(&self, id: &str) -> Result<()> {
        let metadata = self.db.first("SELECT metadata FROM media_objects WHERE id=?", &[text(id)], |r| r.text(0))?;
        let m: Value = metadata.and_then(|v|serde_json::from_str(&v).ok()).unwrap_or(Value::Null);
        let mut queue = self.voice_refs_to_forget(None)?;
        for s in m["segments"].as_array().into_iter().flatten() {
            if let Some(r) = s["voice_ref"].as_str() {
                if !queue.iter().any(|q| q == r) {
                    queue.push(r.to_string());
                }
            }
        }
        let result=self.wf_get(&format!("workflows.speakers.result/1:{id}"))?;
        for r in result["voice_refs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if !queue.iter().any(|x|x==r) {queue.push(r.into());}
        }
        self.db.execute("DELETE FROM store_meta WHERE key=?",&[text(format!("workflows.speakers.result/1:{id}"))])?;
        self.db.execute("INSERT OR REPLACE INTO store_meta VALUES('voice.forget/1',?)", &[text(json!(queue).to_string())])
    }
}
