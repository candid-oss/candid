//! One job per recording (`speech.recording`): after the recording stops, the whole recording is transcribed once
//! in long windows, with speakers and voiceprints in the same model call (`mode=conversation`) or as text only on
//! the small model (`mode=quick`). The result is one transcript for the recording; the capture's 30-second files
//! stay an internal detail. Older recordings keep their per-file transcripts and earlier speaker results.
use crate::{db::{text, RowExt}, *, speakers::segments_revision};
use serde_json::{json, Value};
fn bad(s: &str) -> TimelineError { TimelineError::InvalidInput(s.into()) }

/// The single job kind for a recording's audio. Cost: by model (local voice service = heavy).
pub const RECORDING_JOB: &str = "speech.recording";
/// Kinds of earlier versions: per-file transcription and a separate whole-recording speaker pass. Nothing queues
/// them any more; queued ones are cancelled once (`retire_recording_jobs`) and the recording gets one new job.
pub const LEGACY_RECORDING_JOBS: [&str; 2] = ["speech.recognize", "workflow.speakers"];
const RESULT_KEY: &str = "workflows.speakers.result/1";

pub(crate) fn mode_of(job: &Value) -> &str {
    match job["config_version"].as_str().and_then(|s| s.strip_prefix("recording/1:")).and_then(|s| s.split(':').next()) {
        Some("quick") => "quick",
        _ => "conversation",
    }
}

impl TimelineStore {
    pub fn recording_speaker_policy(&self, id: &str) -> Result<Value> {
        let v = self.wf_get(&format!("workflows.speakers.policy/1:{id}"))?;
        Ok(if v.is_null() { json!({"version":1,"preset":"balanced","max_speakers":0}) } else { v })
    }
    pub fn save_recording_speaker_policy(&self, id: &str, policy: &Value) -> Result<Value> {
        self.recording_input(id)?;
        if policy.as_object().is_none_or(|p|p.len()!=3) || policy["version"]!=1
            || !matches!(policy["preset"].as_str(),Some("conservative"|"balanced"|"merge_more"))
            || policy["max_speakers"].as_u64().is_none_or(|n|n>20) { return Err(bad("说话人设置无效")); }
        self.wf_put(&format!("workflows.speakers.policy/1:{id}"),policy)?;
        self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='configuration_changed' WHERE kind=? AND input_ref=? AND state IN ('queued','running')", &[text(RECORDING_JOB), text(id)])?;
        self.rerun_recording(id, None)
    }

    /// The recording's audio identity: its files in order and each file's original digest (compaction keeps it).
    pub(crate) fn recording_audio_revision(&self, id: &str) -> Result<String> {
        let mut files = self.media_part_ids(id)?;
        if files.is_empty() { files.push(id.into()); }
        let rows = self.db.query(
            "SELECT o.id,COALESCE(json_extract(o.metadata,'$.original_digest'),o.digest) FROM json_each(?) j JOIN media_objects o ON o.id=j.value ORDER BY j.key",
            &[text(json!(files).to_string())], |r| json!([r.text(0), r.text(1)]))?;
        Ok(sha256_hex(serde_json::to_vec(&json!(["recording-audio/1", rows]))?))
    }

    /// What the recording job reads: the audio files with their offsets, and the current transcript's speaker
    /// spans so names given before carry over to the new speakers. Refused while the recording is still running.
    pub fn recording_input(&self, id: &str) -> Result<Value> {
        let item = self.media_item(id)?;
        if item["kind"] != "audio" || !self.media_text_allowed(id,item["source"].as_str().unwrap_or(""))? { return Err(TimelineError::Denied("录音处理权限已撤回".into())); }
        if item["recording_state"] == "recording" { return Err(bad("waiting_recording")); }
        let mut entries = Vec::new();
        for s in item["segments"].as_array().into_iter().flatten().take(20000) {
            let Some(key) = s["speaker_correction_key"].as_str().or(s["speaker"].as_str()) else { continue };
            let named = self.resolve_speaker(id,key,&Value::Null)?;
            entries.push(json!({"interval":[s["start_seconds"],s["end_seconds"]],"correction_key":key,"protected":if named["status"]=="confirmed" {named["person_id"].clone()} else {Value::Null}}));
        }
        let parts = item["recording_parts"].as_array().cloned().unwrap_or_default();
        let files = if parts.is_empty() { vec![json!({"id":id,"path":self.media_path(id)?,"offset":0.0})] } else {
            parts.iter().map(|p| Ok(json!({"id":p["id"],"path":self.media_path(p["id"].as_str().unwrap_or(""))?,"offset":p["sample_start"].as_f64().unwrap_or(0.)/p["sample_rate"].as_f64().unwrap_or(1.)}))).collect::<Result<Vec<_>>>()?
        };
        Ok(json!({"scope":id,"revision":self.recording_audio_revision(id)?,"entries":entries,"files":files,"policy":self.recording_speaker_policy(id)?}))
    }

    fn recording_config(&self, input: &Value, mode: &str) -> Result<String> {
        let version = sha256_hex(serde_json::to_vec(&json!([input["revision"],input["policy"],self.model_config()?]))?);
        Ok(format!("recording/1:{mode}:{version}"))
    }

    /// Queues the recording's one job. Idempotent: the same audio, mode, policy and model setup map to the same job.
    /// Any other unfinished job of the recording is superseded, so a recording never has two in flight.
    pub fn enqueue_recording(&self, id: &str, mode: &str) -> Result<Value> {
        if !["quick", "conversation"].contains(&mode) { return Err(bad("识别模式无效")); }
        let input = self.recording_input(id)?;
        let config = self.recording_config(&input, mode)?;
        self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='superseded',updated_ms=? WHERE kind=? AND input_ref=? AND config_version<>? AND state IN ('queued','running')",
            &[crate::db::int(self.clock.now_ms()), text(RECORDING_JOB), text(id), text(&config)])?;
        self.job_command(&json!({"action":"enqueue","kind":RECORDING_JOB,"feature":"transcribe","input_ref":id,"config_version":config}))
    }

    /// An explicit request to transcribe again (button, speaker settings): queues, or restarts a finished job.
    pub(crate) fn rerun_recording(&self, id: &str, mode: Option<&str>) -> Result<Value> {
        let mode = mode.map(str::to_string).unwrap_or_else(|| self.recording_mode(id));
        let job = self.enqueue_recording(id, &mode)?;
        if matches!(job["state"].as_str(), Some("done"|"failed"|"cancelled")) { self.job_command(&json!({"action":"restart","id":job["id"]})) } else { Ok(job) }
    }

    /// The mode the recording was last transcribed in, else the automatic setting (conversation by default).
    pub(crate) fn recording_mode(&self, id: &str) -> String {
        let result = self.wf_get(&format!("{RESULT_KEY}:{id}")).unwrap_or(Value::Null);
        if let Some(mode) = result["mode"].as_str().filter(|m| result["audio_revision"].is_string() && ["quick","conversation"].contains(m)) {
            return mode.into();
        }
        self.wf_get("workflows.settings/1").ok().and_then(|s| s["mode"].as_str().map(str::to_string))
            .filter(|m| ["quick","conversation"].contains(&m.as_str())).unwrap_or_else(|| "conversation".into())
    }

    /// Earlier versions queued one job per 30-second file and a second whole-recording pass. Their queued jobs are
    /// cancelled (`superseded`); the scan then gives each unfinished recording one job. Returns how many.
    pub fn retire_recording_jobs(&self) -> Result<i64> {
        let before = self.db.scalar("SELECT COUNT(*) FROM jobs WHERE kind IN (?,?) AND state IN ('queued','running')", &[text(LEGACY_RECORDING_JOBS[0]), text(LEGACY_RECORDING_JOBS[1])])?;
        if before > 0 {
            self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='superseded',updated_ms=? WHERE kind IN (?,?) AND state IN ('queued','running')",
                &[crate::db::int(self.clock.now_ms()), text(LEGACY_RECORDING_JOBS[0]), text(LEGACY_RECORDING_JOBS[1])])?;
        }
        Ok(before)
    }

    /// The voice service for a recording: the diarization model for conversation, the ASR model for quick. Feature
    /// controls are checked against whatever is assigned (a remote one waits for `allow_remote`); the job itself
    /// runs only on a numeric-loopback service, since windows carry voiceprint references.
    fn recording_provider(&self, mode: &str, item: &Value) -> Result<(Value, Value)> {
        let role = if mode == "quick" { "asr" } else { "diarization" };
        let c = self.model_config()?;
        let assignment = c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == role && a["model"].is_object()).cloned().ok_or_else(|| bad("waiting_model"))?;
        let assigned = c["providers"].as_array().into_iter().flatten().find(|p| p["provider_id"] == assignment["model"]["provider_id"] && p["enabled"] == true).cloned().ok_or_else(|| bad("waiting_model"))?;
        self.feature_check("transcribe", item, Some(&assigned))?;
        let provider = self.local_voice_provider(role).map_err(|_| bad("waiting_model"))?;
        Ok((provider, assignment))
    }

    pub(crate) fn recording_prepare(&self, job: &Value) -> Result<Value> {
        let id = job["input_ref"].as_str().unwrap_or("");
        let mode = mode_of(job);
        let input = self.recording_input(id)?;
        let item = self.media_item(id)?;
        let (p, assignment) = self.recording_provider(mode, &item)?;
        // Audio, policy or model setup changed since it was queued: a newer job covers the recording.
        if job["config_version"].as_str() != Some(self.recording_config(&input, mode)?.as_str()) { return Err(bad("superseded")); }
        if assignment["limits_json"]["daily_budget"].is_number() { return Err(bad("waiting_budget_cost_unknown")); }
        let config = self.model_config()?;
        Ok(json!({"job":job,"provider":p,"assignment":assignment,"step":"transcribe","recording":true,"speech":false,"mode":mode,"input":input,
            "source_revision":input["revision"],"config_revision":sha256_hex(serde_json::to_vec(&config)?),
            "feature_revision":sha256_hex(serde_json::to_vec(&self.feature_settings()?["items"]["transcribe"])?)}))
    }

    pub fn recording_complete(&self,r:&Value,output:&Value)->Result<Value> {
        let id=r["job"]["input_ref"].as_str().unwrap_or("");
        let existing=self.wf_get(&format!("{RESULT_KEY}:{id}"))?;
        let current_job=self.job_command(&json!({"action":"get","id":r["job"]["id"]}))?;
        if current_job["state"]=="done" && existing["job_id"]==r["job"]["id"] && existing["segments"]==output["segments"] && existing["revision"]==output["revision"] {
            return self.media_item(id);
        }
        self.workflow_reply_allowed(r)?;
        let input=self.recording_input(id)?;
        if input["policy"]!=r["input"]["policy"] || input["entries"]!=r["input"]["entries"] || input["revision"]!=output["revision"] || output["format"]!="recording-speakers/2" {return Err(bad("source_or_configuration_changed"));}
        Self::check_whole_recording(output, r["mode"].as_str().unwrap_or("conversation"))?;
        // No line may extend past the recording's audio.
        let duration=self.media_item(id)?["duration_seconds"].as_f64().or(self.media_bounds(id).ok().map(|b|b.0));
        if let Some(total)=duration {
            if output["segments"].as_array().into_iter().flatten().any(|s|s["end_seconds"].as_f64().is_none_or(|b|b>total+0.01)) {return Err(bad("转写区间超出录音"));}
        }
        self.db.transaction(|| {
            let mut result=output.clone();
            result["job_id"]=r["job"]["id"].clone();
            result["mode"]=r["mode"].clone();
            result["audio_revision"]=input["revision"].clone();
            let mut queue=self.voice_refs_to_forget(None)?;
            for reference in existing["voice_refs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !output["voice_refs"].as_array().is_some_and(|a|a.iter().any(|v|v==reference)) && !queue.iter().any(|r|r==reference) {queue.push(reference.into());}
            }
            self.wf_put("voice.forget/1",&json!(queue))?;
            self.wf_put(&format!("{RESULT_KEY}:{id}"),&result)?;
            let job=&r["job"];
            self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=?,updated_ms=? WHERE job_id=?",&[text(format!("recording:{id}")),crate::db::int(self.clock.now_ms()),text(job["id"].as_str().unwrap_or(""))])?;
            Ok(())
        })?;
        self.media_projection_stamp.set(None);
        self.project_native_media()?;
        self.media_item(id)
    }
    /// A whole-recording result carries its own transcript: one set of speakers (none in quick mode), short
    /// acknowledgements left out.
    fn check_whole_recording(output:&Value, mode:&str)->Result<()> {
        let invalid=||bad("说话人整理结果无效");
        let quick = mode == "quick";
        let segments=output["segments"].as_array().filter(|s|s.len()<=20000).ok_or_else(invalid)?;
        let (mut speakers,mut refs,mut unresolved)=(std::collections::BTreeSet::new(),std::collections::BTreeSet::new(),0u64);
        let mut previous=0.0;
        for s in segments {
            let (Some(a),Some(b))=(s["start_seconds"].as_f64(),s["end_seconds"].as_f64()) else {return Err(invalid());};
            let text_ok = |v:&Value, max:usize| v.as_str().is_some_and(|t|!t.is_empty()&&t.len()<=max);
            let speaker_ok = if quick { s["speaker"].is_null() && s["label"].is_null() } else { text_ok(&s["speaker"],200) && text_ok(&s["label"],80) };
            if !a.is_finite()||!b.is_finite()||a<0.||b<=a||a+1e-6<previous
                || !text_ok(&s["text"],20000) || !speaker_ok
                || !(s["voice_ref"].is_null()||s["voice_ref"].as_str().is_some_and(|r|!r.is_empty()&&r.len()<=128))
                || !(s["correction_key"].is_null()||s["correction_key"].as_str().is_some_and(|r|r.len()<=200))
                || !(s["match"].is_null()||s["match"].is_object()) {return Err(invalid());}
            previous=a;
            match s["speaker"].as_str() { Some("unresolved") => unresolved+=1, Some(k) => {speakers.insert(k);}, None => {} }
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
        let result=self.wf_get(&format!("{RESULT_KEY}:{id}"))?;
        item["speaker_policy"]=self.recording_speaker_policy(id)?;
        if result["audio_revision"].is_string() {
            // One job's transcript of the recording, valid while the audio is the same.
            if result["audio_revision"].as_str() != Some(self.recording_audio_revision(id)?.as_str()) {return Ok(());}
            item["processing_status"]=json!("ready");
            item["processing_mode"]=result["mode"].clone();
            return self.apply_whole_recording(id,item,&result);
        }
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
        if result["mode"] != "quick" {
            item["speaker_grouping"]=json!({"version":2,"speaker_count":result["speaker_count"],"unresolved_segments":result["unresolved_segments"],"dropped_segments":result["dropped_segments"],"producer":result["producer"]});
        }
        Ok(())
    }
}
