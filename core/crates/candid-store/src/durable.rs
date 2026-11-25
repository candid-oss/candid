//! Durable queue and reference-only record revisions. All state shares timeline.sqlite.
use crate::{
    db::{int, text, RowExt},
    records::{rebuild, Revision, Rules},
    uuid_v4, Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};
pub(crate) const DDL: &str = r#"
CREATE TABLE record_revisions(id TEXT PRIMARY KEY,source_id TEXT NOT NULL REFERENCES sources(source_id),rules TEXT NOT NULL,keys_json TEXT NOT NULL,created_ms INTEGER NOT NULL) STRICT;
CREATE TABLE record_selected(source_id TEXT PRIMARY KEY REFERENCES sources(source_id),revision_id TEXT NOT NULL REFERENCES record_revisions(id)) STRICT;
CREATE TABLE record_audit(seq INTEGER PRIMARY KEY,source_id TEXT NOT NULL,previous_id TEXT,selected_id TEXT NOT NULL,at_ms INTEGER NOT NULL) STRICT;
CREATE TABLE job_checkpoints(job_id TEXT PRIMARY KEY REFERENCES jobs(job_id) ON DELETE CASCADE,owner TEXT,checkpoint TEXT NOT NULL DEFAULT '{}') STRICT;
CREATE TABLE media_objects(id TEXT PRIMARY KEY,metadata TEXT NOT NULL,blob_path TEXT NOT NULL,digest TEXT NOT NULL,byte_count INTEGER NOT NULL) STRICT;
CREATE TABLE media_parts(id TEXT PRIMARY KEY REFERENCES media_objects(id) ON DELETE CASCADE,recording_id TEXT NOT NULL,metadata TEXT NOT NULL) STRICT;
CREATE TABLE media_edits(id TEXT PRIMARY KEY,title TEXT NOT NULL,notes TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'stopped') STRICT;
CREATE TABLE media_deleted(id TEXT PRIMARY KEY) STRICT;
CREATE TABLE media_recognitions(id TEXT NOT NULL REFERENCES media_objects(id) ON DELETE CASCADE,mode TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(id,mode)) STRICT;
CREATE TABLE media_migrations(origin TEXT PRIMARY KEY,receipt TEXT NOT NULL) STRICT;
"#;
fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}
impl TimelineStore {
    pub fn stage_record_revision(&self, source: &str, rules: Rules) -> Result<String> {
        let r = self.rebuild_records(source, rules)?;
        if r.evidence.is_empty() {
            return Err(bad("此来源没有可重建观测"));
        }
        let id = r.id()?;
        // Only source identities are persisted: never copy private evidence into a snapshot.
        let keys: Vec<_> = r.evidence.iter().map(|o| o.key.clone()).collect();
        self.db.execute(
            "INSERT OR IGNORE INTO record_revisions VALUES(?,?,?,?,?)",
            &[
                text(&id),
                text(source),
                text(serde_json::to_string(&r.rules)?),
                text(serde_json::to_string(&keys)?),
                int(self.clock.now_ms()),
            ],
        )?;
        Ok(id)
    }
    pub fn record_revision(&self, id: &str, client: &str) -> Result<Revision> {
        let result = self.record_revision_inner(id, client);
        if self.load_client(client)?.is_some() {
            self.db.execute("INSERT INTO access_log(log_id,at_ms,client_id,tool,params_json,result_count,result_bytes,denied) VALUES(?,?,?,'records',?,?,?,?)",&[text(uuid_v4()),int(self.clock.now_ms()),text(client),text(json!({"id":id}).to_string()),int(if result.is_ok(){1}else{0}),int(result.as_ref().ok().and_then(|r|serde_json::to_vec(r).ok()).map(|b|b.len() as i64).unwrap_or(0)),crate::db::opt_text(result.as_ref().err().map(|e|e.code()))])?;
        }
        result
    }
    fn record_revision_inner(&self, id: &str, client: &str) -> Result<Revision> {
        let (ctx, revoked) = self
            .load_client(client)?
            .ok_or_else(|| TimelineError::UnknownClient(client.into()))?;
        if revoked || !ctx.scope.derived || !ctx.scope.text {
            return Err(TimelineError::Denied("record_text_denied".into()));
        }
        let (source, rules, keys) = self
            .db
            .first(
                "SELECT source_id,rules,keys_json FROM record_revisions WHERE id=?",
                &[text(id)],
                |r| (r.text(0), r.text(1), r.text(2)),
            )?
            .ok_or_else(|| TimelineError::NotFound("修订".into()))?;
        let keys: Vec<String> = serde_json::from_str(&keys)?;
        let f = self.scope_filter(&ctx, "e");
        for key in &keys {
            if self.db.scalar(&format!("SELECT COUNT(*) FROM events e WHERE e.source_id=? AND e.source_key=? AND e.sensitivity<>'protected' AND ({})",f.sql()),&[vec![text(&source),text(key)],f.params.clone()].concat())?!=1{return Err(TimelineError::Denied("revision_source_unavailable".into()));}
        }
        let current = self.rebuild_records(&source, serde_json::from_str(&rules)?)?;
        let observations: Vec<_> = current
            .evidence
            .into_iter()
            .filter(|o| keys.contains(&o.key))
            .collect();
        if let Some(policy) = self
            .sources()?
            .into_iter()
            .find(|s| s.source_id == source)
            .map(|s| s.policy)
        {
            if observations
                .iter()
                .any(|o| policy.exclude.apps.contains(&o.app))
                || !policy.exclude.threads.is_empty()
                || !policy.exclude.patterns.is_empty()
            {
                return Err(TimelineError::Denied(
                    "revision_source_policy_changed".into(),
                ));
            }
        }
        let result = rebuild(&observations, current.rules)?;
        if serde_json::to_vec(&result)?.len() as i64 > ctx.scope.max_result_bytes {
            return Err(bad("修订超过访问方返回大小限制"));
        }
        if result.id()? != id {
            return Err(bad("修订来源已改变，请重建"));
        }
        Ok(result)
    }
    pub fn select_record_revision(&self, id: &str, client: &str) -> Result<Value> {
        let r = self.record_revision(id, client)?;
        let source = r
            .evidence
            .first()
            .map(|o| o.source.clone())
            .or(self.db.first(
                "SELECT source_id FROM record_revisions WHERE id=?",
                &[text(id)],
                |r| r.text(0),
            )?)
            .ok_or_else(|| bad("无来源"))?;
        self.db.transaction(||{
            let previous=self.db.first("SELECT revision_id FROM record_selected WHERE source_id=?",&[text(&source)],|r|r.text(0))?;
            if previous.as_deref()!=Some(id){self.db.execute("INSERT OR REPLACE INTO record_selected VALUES(?,?)",&[text(&source),text(id)])?;self.db.execute("INSERT INTO record_audit(source_id,previous_id,selected_id,at_ms) VALUES(?,?,?,?)",&[text(&source),crate::db::opt_text(previous),text(id),int(self.clock.now_ms())])?;}
            Ok(json!({"selected":id,"source":source}))
        })
    }
    pub fn record_management(&self, v: &Value, client: &str) -> Result<Value> {
        if v["action"].as_str().is_some_and(|a|a.starts_with("annotation_")) {return self.annotation_command(v,client);}
        let id = v["id"].as_str().unwrap_or("");
        match v["action"].as_str().unwrap_or("query") {
            "stage" => Ok(
                json!({"id":self.stage_record_revision(v["source"].as_str().unwrap_or(""),v.get("rules").map(|v|serde_json::from_value(v.clone())).transpose()?.unwrap_or_default())?}),
            ),
            "select" => self.select_record_revision(id, client),
            "query" => Ok(serde_json::to_value(self.record_revision(id, client)?)?),
            "list" => Ok(
                json!({"revisions":self.db.query("SELECT id,source_id,created_ms FROM record_revisions ORDER BY created_ms",&[],|r|json!({"id":r.text(0),"source":r.text(1),"created_ms":r.int(2)}))?,"audit":self.db.query("SELECT source_id,previous_id,selected_id,at_ms FROM record_audit ORDER BY seq",&[],|r|json!({"source":r.text(0),"previous":r.opt_text(1),"selected":r.text(2),"at_ms":r.int(3)}))?}),
            ),
            _ => Err(bad("未知修订操作")),
        }
    }
    /// Call inside the store transaction when a job and its input must be committed together.
    pub(crate) fn enqueue_job(&self, kind: &str, input: &str, config: &str, feature: Option<String>, next_run_ms: Option<i64>) -> Result<String> {
        let now = self.clock.now_ms();
        self.db.execute(
            "INSERT OR IGNORE INTO jobs(job_id,kind,input_ref,config_version,state,created_ms,updated_ms,feature,next_run_ms) VALUES(?,?,?,?,'queued',?,?,?,?)",
            &[text(uuid_v4()), text(kind), text(input), text(config), int(now), int(now), crate::db::opt_text(feature), next_run_ms.map(int).unwrap_or(crate::db::P::Null)],
        )?;
        Ok(self.db.first(
            "SELECT job_id FROM jobs WHERE kind=? AND input_ref=? AND config_version=?",
            &[text(kind), text(input), text(config)], |r| r.text(0),
        )?.unwrap())
    }

    pub fn job_command(&self, v: &Value) -> Result<Value> {
        let now = self.clock.now_ms();
        let id = v["id"].as_str().unwrap_or("");
        let action = v["action"].as_str().unwrap_or("list");
        if action == "claim"
            && self.db.scalar(
                "SELECT COUNT(*) FROM store_meta WHERE key='media.migration.pending'",
                &[],
            )? > 0
        {
            return Err(bad("媒体迁移未完成，任务保持排队"));
        }
        self.db.transaction(||{
            match action {
                "enqueue"=>{let kind=v["kind"].as_str().filter(|x|!x.is_empty()).ok_or_else(||bad("缺少任务类型"))?;let input=v["input_ref"].as_str().filter(|x|!x.is_empty()).ok_or_else(||bad("缺少输入"))?;let config=v["config_version"].as_str().filter(|x|!x.is_empty()).ok_or_else(||bad("缺少配置版本"))?;
                    let feature=v["feature"].as_str().map(String::from).or_else(||feature_of(kind));
                    return self.job_get(&self.enqueue_job(kind, input, config, feature, None)?);
                },
                "claim"=>{let owner=v["owner"].as_str().filter(|s|!s.is_empty()).ok_or_else(||bad("缺少执行者"))?;let lease=v["lease_ms"].as_i64().unwrap_or(30000);if !(100..=3600000).contains(&lease){return Err(bad("租约范围无效"));}
                    let kind=v["kind"].as_str().unwrap_or("");
                    let next=self.db.first("SELECT job_id FROM jobs WHERE (state='queued' OR (state='running' AND lease_until_ms<=?)) AND (next_run_ms IS NULL OR next_run_ms<=?) AND (?='' OR kind=?) AND (?='' OR job_id=?) ORDER BY created_ms,job_id LIMIT 1",&[int(now),int(now),text(kind),text(kind),text(id),text(id)],|r|r.text(0))?;
                    let Some(id)=next else{return Ok(Value::Null)};
                    self.db.execute("UPDATE jobs SET state='running',attempts=attempts+1,error_code=NULL,lease_until_ms=?,updated_ms=? WHERE job_id=?",&[int(now+lease),int(now),text(&id)])?;
                    self.db.execute("INSERT INTO job_checkpoints(job_id,owner) VALUES(?,?) ON CONFLICT(job_id) DO UPDATE SET owner=excluded.owner",&[text(&id),text(owner)])?;return self.job_get(&id);
                },
                "restart"=>{
                    let was_done=self.db.scalar("SELECT COUNT(*) FROM jobs WHERE job_id=? AND state='done'",&[text(id)])?==1;
                    self.db.execute("UPDATE jobs SET state='queued',next_run_ms=NULL,lease_until_ms=NULL,output_ref=NULL,error_code=NULL,updated_ms=? WHERE job_id=? AND state IN ('done','failed','cancelled')",&[int(now),text(id)])?;
                    if was_done {self.db.execute("UPDATE job_checkpoints SET checkpoint='{}',owner=NULL WHERE job_id=?",&[text(id)])?;}
                    else if let Some(raw)=self.db.first("SELECT checkpoint FROM job_checkpoints WHERE job_id=?",&[text(id)],|r|r.text(0))? {
                        let mut checkpoint:Value=serde_json::from_str(&raw)?;
                        if let Some(object)=checkpoint.as_object_mut(){object.remove("progress");}
                        self.db.execute("UPDATE job_checkpoints SET checkpoint=? WHERE job_id=?",&[text(checkpoint.to_string()),text(id)])?;
                    }
                },
                "cancel"=>{self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,updated_ms=? WHERE job_id=? AND state IN ('queued','running','failed')",&[int(now),text(id)])?;},
                "checkpoint"|"finish"|"retry"|"heartbeat"=>{
                    let valid=self.db.scalar("SELECT COUNT(*) FROM jobs j JOIN job_checkpoints c USING(job_id) WHERE j.job_id=? AND j.state='running' AND j.attempts=? AND c.owner=? AND j.lease_until_ms>?",&[text(id),int(v["attempt"].as_i64().unwrap_or(-1)),text(v["owner"].as_str().unwrap_or("")),int(now)])?;
                    if valid!=1{
                        let old=self.job_get(id)?;
                        if action=="finish" && ["done","failed"].contains(&old["state"].as_str().unwrap_or("")) && old["attempt"]==v["attempt"] && old["owner"]==v["owner"] && old["output_ref"]==v["output_ref"] && old["error_code"]==v["error_code"] {return Ok(old);}
                        return Err(TimelineError::Denied("stale_job_lease".into()));
                    }
                    if action=="checkpoint" {self.db.execute("UPDATE job_checkpoints SET checkpoint=? WHERE job_id=?",&[text(serde_json::to_string(&v["checkpoint"])?),text(id)])?;}
                    else if action=="heartbeat" {self.db.execute("UPDATE jobs SET lease_until_ms=?,updated_ms=? WHERE job_id=?",&[int(now+v["lease_ms"].as_i64().unwrap_or(30000).clamp(100,3600000)),int(now),text(id)])?;}
                    else {let state=if action=="retry"{"queued"}else if v["error_code"].is_string(){"failed"}else{"done"};self.db.execute("UPDATE jobs SET state=?,output_ref=?,error_code=?,lease_until_ms=NULL,next_run_ms=?,updated_ms=? WHERE job_id=?",&[text(state),crate::db::opt_text(v["output_ref"].as_str()),crate::db::opt_text(v["error_code"].as_str()),int(now+v["delay_ms"].as_i64().unwrap_or(0).max(0)),int(now),text(id)])?;}
                },
                "get"=>return self.job_get(id),
                "run_now"=>return self.job_run_now(v),
                "list"=>return Ok(json!({"jobs":self.db.query("SELECT job_id FROM jobs ORDER BY created_ms",&[],|r|r.text(0))?.iter().map(|id|self.job_get(id)).collect::<Result<Vec<_>>>()?})),
                _=>return Err(bad("未知任务操作"))
            }self.job_get(id)
        })
    }
    fn job_get(&self, id: &str) -> Result<Value> {
        let mut result=self.db.first("SELECT j.job_id,j.kind,j.input_ref,j.config_version,j.state,j.attempts,j.lease_until_ms,c.owner,c.checkpoint,j.error_code,j.output_ref,j.feature,j.created_ms,j.updated_ms,j.manual FROM jobs j LEFT JOIN job_checkpoints c USING(job_id) WHERE j.job_id=?",&[text(id)],|r|json!({"id":r.text(0),"kind":r.text(1),"input_ref":r.text(2),"config_version":r.text(3),"state":r.text(4),"attempt":r.int(5),"lease_until_ms":r.opt_int(6),"owner":r.opt_text(7),"checkpoint":serde_json::from_str::<Value>(&r.text(8)).unwrap_or(json!({})),"error_code":r.opt_text(9),"output_ref":r.opt_text(10),"feature":r.opt_text(11),"created_ms":r.int(12),"updated_ms":r.int(13),"manual":r.int(14)==1}))?.ok_or_else(||TimelineError::NotFound("任务".into()))?;
        // Checkpoints of recording jobs hold transcript text (per-file results, finished windows).
        if matches!(result["kind"].as_str(), Some("speech.recording" | "speech.recognize" | "workflow.speakers")) {
            let id = result["input_ref"].as_str().unwrap_or("");
            let allowed = self.media_item(id).ok().is_some_and(|m| {
                self.media_text_allowed(id, m["source"].as_str().unwrap_or(""))
                    .unwrap_or(false)
            });
            if !allowed {
                result["checkpoint"] = json!({});
            }
        }
        Ok(result)
    }
}

/// The feature (an entry of the catalog) a job belongs to, so runs and usage can be listed per feature.
fn feature_of(kind: &str) -> Option<String> {
    let step = kind.strip_prefix("workflow.");
    Some(match (kind, step) {
        ("speech.recognize", _) => "recording".into(),
        ("speech.recording", _) => "transcribe".into(),
        (_, Some("title" | "summary")) => "recording".into(),
        (_, Some(step)) => step.into(),
        _ => return None,
    })
}
