//! Recording-level task status; physical files remain an internal scheduling detail.
use crate::{
    db::{int, text, RowExt},
    *,
};
use serde_json::{json, Value};
impl TimelineStore {
    pub fn media_job_progress(&self, job: &Value, progress: &Value) -> Result<()> {
        let current = self.job_command(&json!({"action":"get","id":job["id"]}))?;
        let mut checkpoint = current["checkpoint"].clone();
        if !checkpoint.is_object() {
            checkpoint = json!({});
        }
        if checkpoint["progress"] == *progress {
            return Ok(());
        }
        checkpoint["progress"] = progress.clone();
        self.job_command(&json!({"action":"checkpoint","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"checkpoint":checkpoint}))?;
        Ok(())
    }
    pub fn media_job_stopped(&self, job: &Value, confirmed: bool) -> Result<()> {
        let current = self.job_command(&json!({"action":"get","id":job["id"]}))?;
        if current["state"] != "cancelled"
            || current["attempt"] != job["attempt"]
            || current["owner"] != job["owner"]
        {
            return Ok(());
        }
        let mut checkpoint = current["checkpoint"].clone();
        if !checkpoint.is_object() {
            checkpoint = json!({});
        }
        checkpoint["progress"]["stop_confirmed"] = json!(confirmed);
        self.db.execute(
            "UPDATE job_checkpoints SET checkpoint=? WHERE job_id=?",
            &[
                text(checkpoint.to_string()),
                text(job["id"].as_str().unwrap_or("")),
            ],
        )
    }
    pub(crate) fn media_cancel_processing(&self, id: &str) -> Result<Value> {
        self.media_item(id)?;
        let mut inputs = self.media_part_ids(id)?;
        inputs.push(id.into());
        self.db.transaction(|| {
            for input in &inputs {
                self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='user_cancelled',updated_ms=? WHERE input_ref=? AND state IN ('queued','running') AND kind IN ('speech.recording','speech.recognize','workflow.speakers','workflow.title','workflow.summary')",&[int(self.clock.now_ms()),text(input)])?;
            }
            Ok(())
        })?;
        self.media_item(id)
    }
    pub(crate) fn media_processing(
        &self,
        id: &str,
        inputs: &[String],
        item: &mut Value,
    ) -> Result<()> {
        let mut jobs = Vec::new();
        let mut all = inputs.to_vec();
        if !all.iter().any(|x| x == id) {
            all.push(id.into());
        }
        for input in &all {
            for kind in [
                crate::RECORDING_JOB,
                "speech.recognize",
                "workflow.speakers",
                "workflow.title",
                "workflow.summary",
            ] {
                if let Some(job)=self.db.first("SELECT j.job_id,j.state,j.error_code,j.created_ms,j.updated_ms,c.checkpoint,j.attempts FROM jobs j LEFT JOIN job_checkpoints c USING(job_id) WHERE j.input_ref=? AND j.kind=? ORDER BY j.updated_ms DESC,j.rowid DESC LIMIT 1",&[text(input),text(kind)],|r|json!({"id":r.text(0),"state":r.text(1),"reason":r.opt_text(2),"created_ms":r.int(3),"updated_ms":r.int(4),"checkpoint":r.opt_text(5).and_then(|s|serde_json::from_str::<Value>(&s).ok()),"kind":kind,"input_ref":input,"attempts":r.int(6)}))? {jobs.push(job);}
            }
        }
        let mut speech: Vec<_> = jobs
            .iter()
            .filter(|j| {
                matches!(
                    j["kind"].as_str(),
                    Some("speech.recording" | "speech.recognize" | "workflow.speakers")
                )
            })
            .cloned()
            .collect();
        speech.sort_by_key(|j| j["updated_ms"].as_i64().unwrap_or(0));
        let chosen = speech
            .iter()
            .rev()
            .find(|j| j["state"] == "running")
            .or_else(|| speech.iter().rev().find(|j| j["state"] == "queued"))
            .or_else(|| speech.last());
        let completed_parts: f64 = jobs
            .iter()
            .filter(|j| j["kind"] == "speech.recognize" && j["state"] == "done")
            .map(|j| {
                item["recording_parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["id"] == j["input_ref"])
                    .map(|p| {
                        p["sample_count"].as_f64().unwrap_or(0.)
                            / p["sample_rate"].as_f64().unwrap_or(1.)
                    })
                    .unwrap_or(0.)
            })
            .sum();
        let recording_duration = item["duration_seconds"].as_f64();
        let project = |j: &Value| {
            let p = &j["checkpoint"]["progress"];
            let state = if j["state"] == "cancelled"
                && p["cancellation_supported"] == true
                && p["stop_confirmed"].is_null()
            {
                "cancelling"
            } else {
                j["state"].as_str().unwrap_or("idle")
            };
            let total = recording_duration.or_else(|| p["total_seconds"].as_f64());
            // The recording job reports seconds of the whole recording; per-file jobs of earlier versions add up.
            let before = if j["kind"] == "speech.recognize" { completed_parts } else { 0. };
            let completed = p["completed_seconds"]
                .as_f64()
                .map(|c| (before + c).min(total.unwrap_or(f64::MAX)));
            json!({"state":state,"kind":j["kind"],"reason":j["reason"],"stage":if j["kind"]=="workflow.speakers"{json!("speaker_grouping")}else{p["stage"].clone()},"created_ms":j["created_ms"],"updated_ms":p["updated_ms"].as_i64().unwrap_or(j["updated_ms"].as_i64().unwrap_or(0)),"elapsed_seconds":(self.clock.now_ms()-p["started_ms"].as_i64().unwrap_or(j["updated_ms"].as_i64().unwrap_or(self.clock.now_ms()))).max(0)/1000,"completed_seconds":completed,"total_seconds":total,"stage_seconds":(self.clock.now_ms()-p["updated_ms"].as_i64().unwrap_or(j["updated_ms"].as_i64().unwrap_or(self.clock.now_ms()))).max(0)/1000,"cancellation_supported":p["cancellation_supported"],"stop_confirmed":p["stop_confirmed"],"model_may_continue":j["state"]=="cancelled"&&j["attempts"].as_i64().unwrap_or(0)>0&&p["cancellation_supported"]!=true})
        };
        item["processing"] = chosen.map(&project).unwrap_or(Value::Null);
        item["ai_processing"] = Value::Object(
            jobs.iter()
                .filter(|j| j["kind"] == "workflow.title" || j["kind"] == "workflow.summary")
                .map(|j| {
                    (
                        j["kind"]
                            .as_str()
                            .unwrap()
                            .trim_start_matches("workflow.")
                            .to_string(),
                        project(j),
                    )
                })
                .collect(),
        );
        Ok(())
    }
}
