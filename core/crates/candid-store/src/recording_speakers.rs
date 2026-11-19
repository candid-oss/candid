//! Recording-wide speaker projection. Original transcripts and physical segment identities stay intact.
use crate::{db::text, *, speakers::{segments_revision, transcript}};
use serde_json::{json, Value};
fn bad(s: &str) -> TimelineError { TimelineError::InvalidInput(s.into()) }
impl TimelineStore {
    pub fn recording_speaker_policy(&self, id: &str) -> Result<Value> {
        let v = self.wf_get(&format!("workflows.speakers.policy/1:{id}"))?;
        Ok(if v.is_null() { json!({"version":1,"preset":"balanced","max_speakers":0}) } else { v })
    }
    pub fn save_recording_speaker_policy(&self, id: &str, policy: &Value) -> Result<Value> {
        self.recording_speaker_input(id)?;
        if policy.as_object().is_none_or(|p|p.len()!=3) || policy["version"]!=1
            || !matches!(policy["preset"].as_str(),Some("conservative"|"balanced"|"merge_more"))
            || policy["max_speakers"].as_u64().is_none_or(|n|n>20) { return Err(bad("说话人设置无效")); }
        self.wf_put(&format!("workflows.speakers.policy/1:{id}"),policy)?;
        self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='configuration_changed' WHERE kind='workflow.speakers' AND input_ref=? AND state IN ('queued','running')", &[text(id)])?;
        let job=self.enqueue_recording_speakers(id)?;
        if matches!(job["state"].as_str(),Some("done"|"failed"|"cancelled")) {self.job_command(&json!({"action":"restart","id":job["id"]}))}else{Ok(job)}
    }
    pub fn recording_speaker_input(&self, id: &str) -> Result<Value> {
        let item = self.media_item(id)?;
        if item["kind"] != "audio" || !self.media_text_allowed(id,item["source"].as_str().unwrap_or(""))? { return Err(TimelineError::Denied("录音处理权限已撤回".into())); }
        if item["recording_state"] == "recording" { return Err(bad("waiting_recording_transcript")); }
        let mut originals=self.media_part_ids(id)?;if originals.is_empty(){originals.push(id.into());}
        for original in originals {
            if self.media_raw(&original)?["processing_status"]!="ready" || self.db.scalar("SELECT COUNT(*) FROM jobs WHERE input_ref=? AND kind='speech.recognize' AND state IN ('queued','running')",&[text(original)])?>0 {return Err(bad("waiting_recording_transcript"));}
        }
        let raw = transcript(&item["segments"]);
        let segments = raw.as_array().filter(|s|!s.is_empty()&&s.len()<=10000).ok_or_else(||bad("请先完成多人转写"))?;
        let mut entries = Vec::new();
        for (index,s) in segments.iter().enumerate() {
            let key = s["speaker"].as_str().unwrap_or("unresolved");
            let current_key=item["segments"][index]["speaker"].as_str().unwrap_or(key);
            let named = self.resolve_speaker(id,current_key,&Value::Null)?;
            entries.push(json!({"source_group":key,"interval":[s["start_seconds"],s["end_seconds"]],"protected":if named["status"]=="confirmed" {named["person_id"].clone()} else {Value::Null},"source_segment_id":s["source_segment_id"],"correction_key":current_key}));
        }
        let parts = item["recording_parts"].as_array().cloned().unwrap_or_default();
        let files = if parts.is_empty() { vec![json!({"id":id,"path":self.media_path(id)?,"offset":0.0})] } else {
            parts.iter().map(|p| Ok(json!({"id":p["id"],"path":self.media_path(p["id"].as_str().unwrap_or(""))?,"offset":p["sample_start"].as_f64().unwrap_or(0.)/p["sample_rate"].as_f64().unwrap_or(1.)}))).collect::<Result<Vec<_>>>()?
        };
        // A whole-recording result replaces the displayed transcript; the job still depends on the
        // underlying per-file transcripts, whose revision the projection keeps.
        let revision=item["transcript_source_revision"].as_str().map(str::to_string).unwrap_or_else(||segments_revision(&raw));
        Ok(json!({"scope":id,"revision":revision,"entries":entries,"files":files,"policy":self.recording_speaker_policy(id)?}))
    }
    pub fn enqueue_recording_speakers(&self,id:&str)->Result<Value> {
        let input=self.recording_speaker_input(id)?;
        // Version 2: whole-recording windows. Bumping it gives existing recordings one new run.
        let version=sha256_hex(serde_json::to_vec(&json!(["recording-speakers/2",input["revision"],input["policy"],self.model_config()?]))?);
        self.job_command(&json!({"action":"enqueue","kind":"workflow.speakers","feature":"transcribe","input_ref":id,"config_version":version}))
    }
    pub(crate) fn recording_speakers_prepare(&self,job:&Value)->Result<Value> {
        let id=job["input_ref"].as_str().unwrap_or("");
        let input=self.recording_speaker_input(id)?;
        let p=self.voiceprint_provider().map_err(|_|bad("waiting_model"))?;
        let item=self.media_item(id)?;
        self.feature_check("transcribe",&item,Some(&p))?;
        let config=self.model_config()?;
        let assignment=config["roles"].as_array().into_iter().flatten().find(|a|a["role"]=="diarization").cloned().unwrap_or(Value::Null);
        Ok(json!({"job":job,"provider":p,"assignment":assignment,"step":"transcribe","reconcile_speakers":true,"speech":false,"input":input,
            "source_revision":input["revision"],"config_revision":sha256_hex(serde_json::to_vec(&config)?),
            "feature_revision":sha256_hex(serde_json::to_vec(&self.feature_settings()?["items"]["transcribe"])?)}))
    }
    pub fn recording_speakers_complete(&self,r:&Value,output:&Value)->Result<Value> {
        let id=r["job"]["input_ref"].as_str().unwrap_or("");
        let existing=self.wf_get(&format!("workflows.speakers.result/1:{id}"))?;
        let current_job=self.job_command(&json!({"action":"get","id":r["job"]["id"]}))?;
        if current_job["state"]=="done" && existing["job_id"]==r["job"]["id"] && existing["assignments"]==output["assignments"] && existing["segments"]==output["segments"] && existing["revision"]==output["revision"] {
            self.recording_speaker_input(id)?;return self.media_item(id);
        }
        self.workflow_reply_allowed(r)?;
        let input=self.recording_speaker_input(id)?;
        if input["policy"]!=r["input"]["policy"] || input["entries"]!=r["input"]["entries"] || input["revision"]!=output["revision"] || !matches!(output["format"].as_str(),Some("recording-speakers/1"|"recording-speakers/2")) {return Err(bad("source_or_configuration_changed"));}
        if output["format"]=="recording-speakers/2" {Self::check_whole_recording(output)?;} else {
            let entries=input["entries"].as_array().unwrap();
            let assignments=output["assignments"].as_array().ok_or_else(||bad("说话人整理结果无效"))?;
            if assignments.len()!=entries.len() {return Err(bad("说话人整理区间不完整"));}
            let mut seen=std::collections::BTreeSet::new();
            for a in assignments {
                let i=a["index"].as_u64().ok_or_else(||bad("说话人区间无效"))? as usize;
                if i>=entries.len() || !seen.insert(i) || a["speaker"].as_str().is_none_or(|s|s.is_empty()||s.len()>200)
                    || a["label"].as_str().is_none_or(|s|s.is_empty()||s.len()>80)
                    || a["voice_ref"].as_str().is_some_and(|s|s.len()>128) {return Err(bad("说话人整理结果无效"));}
            }
            let mut protected:std::collections::BTreeMap<String,std::collections::BTreeSet<String>>=Default::default();
            let mut refs=std::collections::BTreeSet::new();
            let mut speakers=std::collections::BTreeSet::new();let mut unresolved=0;
            for a in assignments {
                let i=a["index"].as_u64().unwrap() as usize;
                let speaker=a["speaker"].as_str().unwrap();
                if speaker=="unresolved" {unresolved+=1;}else{speakers.insert(speaker);}
                if let Some(person)=entries[i]["protected"].as_str() {protected.entry(speaker.into()).or_default().insert(person.into());}
                if let Some(reference)=a["voice_ref"].as_str() {refs.insert(reference);}
            }
            let declared:std::collections::BTreeSet<_>=output["voice_refs"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if protected.values().any(|s|s.len()>1) || refs!=declared || output["speaker_count"].as_u64()!=Some(speakers.len() as u64)
                || output["unresolved_segments"].as_u64()!=Some(unresolved) || output["producer"]["model_id"].as_str().is_none_or(str::is_empty)
                || output["producer"]["model_revision"].as_str().is_none_or(str::is_empty) {return Err(bad("说话人整理结果无效"));}
        }
        self.db.transaction(|| {
            let mut result=output.clone();result["job_id"]=r["job"]["id"].clone();
            let mut queue=self.voice_refs_to_forget(None)?;
            for reference in existing["voice_refs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !output["voice_refs"].as_array().is_some_and(|a|a.iter().any(|v|v==reference)) && !queue.iter().any(|r|r==reference) {queue.push(reference.into());}
            }
            self.wf_put("voice.forget/1",&json!(queue))?;
            self.wf_put(&format!("workflows.speakers.result/1:{id}"),&result)?;
            let job=&r["job"];
            self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=?,updated_ms=? WHERE job_id=?",&[text(format!("speakers:{id}")),crate::db::int(self.clock.now_ms()),text(job["id"].as_str().unwrap_or(""))])?;
            Ok(())
        })?;
        self.media_projection_stamp.set(None);
        self.project_native_media()?;
        self.media_item(id)
    }
    /// A whole-recording result carries its own transcript: one set of speakers, short acknowledgements left out.
    fn check_whole_recording(output:&Value)->Result<()> {
        let invalid=||bad("说话人整理结果无效");
        let segments=output["segments"].as_array().filter(|s|s.len()<=20000).ok_or_else(invalid)?;
        let (mut speakers,mut refs,mut unresolved)=(std::collections::BTreeSet::new(),std::collections::BTreeSet::new(),0u64);
        let mut previous=0.0;
        for s in segments {
            let (Some(a),Some(b))=(s["start_seconds"].as_f64(),s["end_seconds"].as_f64()) else {return Err(invalid());};
            if !a.is_finite()||!b.is_finite()||a<0.||b<=a||a+1e-6<previous
                || s["text"].as_str().is_none_or(|t|t.is_empty()||t.len()>20000)
                || s["speaker"].as_str().is_none_or(|t|t.is_empty()||t.len()>200)
                || s["label"].as_str().is_none_or(|t|t.is_empty()||t.len()>80)
                || !(s["voice_ref"].is_null()||s["voice_ref"].as_str().is_some_and(|r|!r.is_empty()&&r.len()<=128))
                || !(s["correction_key"].is_null()||s["correction_key"].as_str().is_some_and(|r|r.len()<=200))
                || !(s["match"].is_null()||s["match"].is_object()) {return Err(invalid());}
            previous=a;
            if s["speaker"]=="unresolved" {unresolved+=1;} else {speakers.insert(s["speaker"].as_str().unwrap());}
            if let Some(r)=s["voice_ref"].as_str() {refs.insert(r);}
        }
        let declared:std::collections::BTreeSet<_>=output["voice_refs"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if refs!=declared || output["speaker_count"].as_u64()!=Some(speakers.len() as u64) || output["unresolved_segments"].as_u64()!=Some(unresolved)
            || output["producer"]["model_id"].as_str().is_none_or(str::is_empty) || output["producer"]["model_revision"].as_str().is_none_or(str::is_empty) {return Err(invalid());}
        Ok(())
    }
    pub fn discard_recording_speaker_refs(&self,refs:&Value)->Result<()> {
        let live=self.voice_refs()?;let mut queue=self.voice_refs_to_forget(None)?;
        for reference in refs.as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s|!s.is_empty()&&s.len()<=128) {
            if !live.iter().any(|r|r==reference) && !queue.iter().any(|r|r==reference) {queue.push(reference.into());}
        }
        self.wf_put("voice.forget/1",&json!(queue))
    }
    pub(crate) fn apply_recording_speakers(&self,id:&str,item:&mut Value)->Result<()> {
        let result=self.wf_get(&format!("workflows.speakers.result/1:{id}"))?;
        item["speaker_policy"]=self.recording_speaker_policy(id)?;
        if result["revision"]!=segments_revision(&item["segments"]) {return Ok(());}
        if result["format"]=="recording-speakers/2" {return self.apply_whole_recording(id,item,&result);}
        let Some(segments)=item["segments"].as_array_mut() else{return Ok(())};
        for a in result["assignments"].as_array().into_iter().flatten() {
            let Some(s)=a["index"].as_u64().and_then(|i|segments.get_mut(i as usize)) else{continue};
            let mut raw=serde_json::Map::new();
            for key in ["speaker","voice_ref","match","identity"] {if let Some(value)=s.get(key) {raw.insert(key.into(),value.clone());}}
            s["speaker_raw_fields"]=Value::Object(raw);
            s["speaker"]=if a["speaker"]=="unresolved" {Value::Null}else{a["speaker"].clone()};s["voice_ref"]=a["voice_ref"].clone();s["match"]=a["match"].clone();s["speaker_display_label"]=a["label"].clone();s["speaker_correction_key"]=a["correction_key"].clone();
        }
        item["content_revision"]=json!(format!("{}:{}",item["content_revision"].as_str().unwrap_or(""),sha256_hex(serde_json::to_vec(&result)?)));
        item["speaker_grouping"]=json!({"version":1,"speaker_count":result["speaker_count"],"unresolved_segments":result["unresolved_segments"],"producer":result["producer"]});
        Ok(())
    }
    /// The recording reads as one transcript with one set of speakers; the capture's files stay invisible.
    fn apply_whole_recording(&self,id:&str,item:&mut Value,result:&Value)->Result<()> {
        item["transcript_source_revision"]=result["revision"].clone();
        let parts:Vec<(String,f64,f64)>=item["recording_parts"].as_array().into_iter().flatten().map(|p|{
            let rate=p["sample_rate"].as_f64().filter(|r|*r>0.).unwrap_or(1.);
            let start=p["sample_start"].as_f64().unwrap_or(0.)/rate;
            (p["id"].as_str().unwrap_or("").to_string(),start,start+p["sample_count"].as_f64().unwrap_or(0.)/rate)
        }).collect();
        let segments:Vec<Value>=result["segments"].as_array().into_iter().flatten().map(|s|{
            let (a,b)=(s["start_seconds"].as_f64().unwrap_or(0.),s["end_seconds"].as_f64().unwrap_or(0.));
            // The physical file holding most of the line, for playback and taking voice samples.
            let source=parts.iter().max_by(|x,y|(b.min(x.2)-a.max(x.1)).total_cmp(&(b.min(y.2)-a.max(y.1)))).map(|p|p.0.clone()).unwrap_or_else(||id.to_string());
            let mut line=json!({"start_seconds":a,"end_seconds":b,"text":s["text"],"speaker":if s["speaker"]=="unresolved"{Value::Null}else{s["speaker"].clone()},
                "voice_ref":s["voice_ref"],"match":s["match"],"speaker_display_label":s["label"],"source_segment_id":source,"producer":result["producer"]});
            if let Some(key)=s["correction_key"].as_str() {line["speaker_correction_key"]=json!(key);}
            line
        }).collect();
        item["segments"]=json!(segments);
        item["content_revision"]=json!(format!("{}:{}",item["content_revision"].as_str().unwrap_or(""),sha256_hex(serde_json::to_vec(result)?)));
        item["speaker_grouping"]=json!({"version":2,"speaker_count":result["speaker_count"],"unresolved_segments":result["unresolved_segments"],"dropped_segments":result["dropped_segments"],"producer":result["producer"]});
        Ok(())
    }
}
