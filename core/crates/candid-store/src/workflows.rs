//! Fixed workflows reuse jobs and ai_calls. Versioned configuration lives in store_meta; outputs go to the
//! derived layer (see `derived.rs`). No secrets are persisted.
use crate::{
    db::{int, text, RowExt},
    sha256_hex, uuid_v4, Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};
pub const CATALOG: &str = include_str!("../../../workflows/catalog.json");
pub const OUTPUT_SCHEMA: &str = include_str!("../../../workflows/outputs.schema.json");
pub const PROMPTS: &str = include_str!("../../../workflows/prompts.json");
fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}
impl TimelineStore {
    pub(crate) fn wf_get(&self, key: &str) -> Result<Value> {
        Ok(self
            .db
            .first(
                "SELECT value FROM store_meta WHERE key=?",
                &[text(key)],
                |r| r.text(0),
            )?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null))
    }
    pub(crate) fn wf_put(&self, key: &str, v: &Value) -> Result<()> {
        self.db.execute(
            "INSERT INTO store_meta VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value IS NOT excluded.value",
            &[text(key), text(v.to_string())],
        )
    }
    pub fn workflow_invalidate(&self, id: &str, reason: &str) -> Result<()> {
        self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code=? WHERE job_id=? AND state IN ('queued','running')",&[text(reason),text(id)])
    }
    pub fn workflow_wait(&self, id: &str, reason: &str) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET error_code=? WHERE job_id=? AND state='queued' AND error_code IS NOT ?",
            &[text(reason), text(id), text(reason)],
        )
    }
    pub(crate) fn workflow_budget(&self, c: &Value, a: &Value, input_bytes: usize) -> Result<()> {
        if let Some(max) = a["limits_json"]["max_input_tokens"].as_u64() {
            if input_bytes as u64 > max {
                return Err(bad("input_limit"));
            }
        }
        let Some(budget) = a["limits_json"]["daily_budget"].as_f64() else {
            return Ok(());
        };
        if !budget.is_finite() || budget < 0.0 {
            return Err(bad("invalid_budget"));
        }
        let model = c["models"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| {
                m["provider_id"] == a["model"]["provider_id"]
                    && m["model_id"] == a["model"]["model_id"]
            })
            .ok_or_else(|| bad("waiting_budget_cost_unknown"))?;
        let prices = &model["cost_json"]["per_mtok"];
        let (Some(input), Some(output)) = (prices["input"].as_f64(), prices["output"].as_f64())
        else {
            return Err(bad("waiting_budget_cost_unknown"));
        };
        if input < 0.0 || output < 0.0 || !input.is_finite() || !output.is_finite() {
            return Err(bad("invalid_prices"));
        }
        let from = self.clock.now_ms() - self.clock.now_ms().rem_euclid(86400000);
        let rows = self.db.query(
            "SELECT cost,currency FROM ai_calls WHERE started_ms>=? AND provider_id=? AND model_id=?",
            &[int(from),text(a["model"]["provider_id"].as_str().unwrap_or("")),text(a["model"]["model_id"].as_str().unwrap_or(""))],
            |r| {
                (
                    if r.is_null(0) {
                        None
                    } else {
                        Some(r.double(0))
                    },
                    r.opt_text(1),
                )
            },
        )?;
        let currency = model["cost_json"]["currency"].as_str();
        if rows
            .iter()
            .any(|(cost, c)| cost.is_none() || c.as_deref() != currency)
        {
            return Err(bad("waiting_budget_cost_unknown"));
        }
        let spent: f64 = rows.iter().filter_map(|r| r.0).sum();
        let cap = a["limits_json"]["max_output_tokens"]
            .as_u64()
            .unwrap_or(1024)
            .min(4096);
        // UTF-8 bytes conservatively bound text tokens. Images/audio require explicit unknown-cost handling.
        let reserve = (input_bytes as f64 * input + cap as f64 * output) / 1_000_000.0;
        if spent + reserve > budget {
            return Err(bad("waiting_budget"));
        }
        Ok(())
    }
    pub fn model_prepare(&self, v: &Value) -> Result<Value> {
        let c = self.model_config()?;
        let model = &v["model"];
        let provider = c["providers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["provider_id"] == model["provider_id"] && p["enabled"] == true)
            .ok_or_else(|| bad("waiting_model"))?;
        let assignments: Vec<_> = c["roles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["model"] == *model)
            .collect();
        let role = assignments
            .iter()
            .find(|a| a["role"] == v["role"])
            .or_else(|| assignments.first())
            .map(|a| (*a).clone())
            .unwrap_or(json!({"model":model}));
        for assignment in &assignments {
            self.workflow_budget(&c, assignment, v["messages"].to_string().len())?;
        }
        self.workflow_budget(&c, &role, v["messages"].to_string().len())?;
        Ok(
            json!({"provider":provider,"assignment":role,"step":v["feature"],"messages":v["messages"],"max_tokens":v["max_tokens"],"temperature":v["temperature"],"json_mode":v["json_mode"],"response_schema":v["response_schema"],"client":"app","speech":false,"job":{"id":v["run_id"]}}),
        )
    }
    pub fn import_usage(&self, v: &Value) -> Result<Value> {
        for call in v["calls"].as_array().into_iter().flatten() {
            let id = call["call_id"]
                .as_str()
                .ok_or_else(|| bad("缺少调用编号"))?;
            self.db.execute("INSERT OR IGNORE INTO ai_calls(call_id,started_ms,ended_ms,feature,client,run_id,provider_id,model_id,destination,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost,currency,cost_source,latency_ms,status,error_code,request_bytes,response_bytes) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",&[
                text(id),int(call["started_ms"].as_i64().unwrap_or(0)),crate::db::opt_int(call["ended_ms"].as_i64()),text(call["feature"].as_str().unwrap_or("legacy")),text(call["client"].as_str().unwrap_or("app")),crate::db::opt_text(call["run_id"].as_str()),text(call["provider_id"].as_str().unwrap_or("")),text(call["model_id"].as_str().unwrap_or("")),text(call["destination"].as_str().unwrap_or("")),crate::db::opt_int(call["input_tokens"].as_i64()),crate::db::opt_int(call["output_tokens"].as_i64()),crate::db::opt_int(call["cache_read_tokens"].as_i64()),crate::db::opt_int(call["cache_write_tokens"].as_i64()),call["cost"].as_f64().map(crate::db::P::Real).unwrap_or(crate::db::P::Null),crate::db::opt_text(call["currency"].as_str()),text(call["cost_source"].as_str().unwrap_or("unknown")),crate::db::opt_int(call["latency_ms"].as_i64()),text(call["status"].as_str().unwrap_or("error")),crate::db::opt_text(call["error_code"].as_str()),crate::db::opt_int(call["request_bytes"].as_i64()),crate::db::opt_int(call["response_bytes"].as_i64())])?;
        }
        Ok(json!({"imported":true}))
    }
    pub fn model_config(&self) -> Result<Value> {
        let v = self.wf_get("models.config/1")?;
        Ok(if v.is_null() {
            json!({"version":1,"providers":[],"models":[],"roles":[]})
        } else {
            v
        })
    }
    pub fn model_command(&self, v: &Value) -> Result<Value> {
        match v["action"].as_str().unwrap_or("get") {
            "get" => self.model_config(),
            "import_usage" => self.import_usage(v),
            "save" => {
                let c = &v["config"];
                if c["version"] != 1 || !c["providers"].is_array() || !c["roles"].is_array() {
                    return Err(bad("不支持的模型配置版本"));
                }
                for p in c["providers"].as_array().unwrap() {
                    if p.as_object().is_none_or(|o| {
                        o.keys().any(|k| {
                            ![
                                "provider_id",
                                "name",
                                "api",
                                "base_url",
                                "secret_ref",
                                "destination",
                                "enabled",
                                "headers_json",
                                "created_ms",
                                "updated_ms",
                            ]
                            .contains(&k.as_str())
                        })
                    }) {
                        return Err(bad("模型配置含未知字段"));
                    }
                    if p["secret_ref"]
                        .as_str()
                        .is_some_and(|r| r.starts_with("sk-") || r.len() > 80)
                    {
                        return Err(bad("只允许保存钥匙串引用"));
                    }
                    let url = p["base_url"].as_str().unwrap_or("");
                    if !(url.starts_with("http://") || url.starts_with("https://"))
                        || url.contains('@')
                        || url.contains('?')
                        || url.contains('#')
                    {
                        return Err(bad("模型地址无效或含凭据"));
                    }
                    for (k, _) in p["headers_json"].as_object().into_iter().flatten() {
                        if [
                            "authorization",
                            "proxy-authorization",
                            "x-api-key",
                            "api-key",
                            "cookie",
                            "x-goog-api-key",
                        ]
                        .contains(&k.to_lowercase().as_str())
                        {
                            return Err(bad("凭据必须存入钥匙串"));
                        }
                    }
                }
                if self.model_config()? != *c {
                    self.db.transaction(||{
                self.db.execute("DELETE FROM ai_roles",&[])?;self.db.execute("DELETE FROM ai_models",&[])?;self.db.execute("DELETE FROM ai_providers",&[])?;
                for p in c["providers"].as_array().unwrap(){self.db.execute("INSERT INTO ai_providers(provider_id,name,api,base_url,secret_ref,destination,enabled,headers_json,created_ms,updated_ms) VALUES(?,?,?,?,?,?,?,?,?,?)",&[text(p["provider_id"].as_str().unwrap_or("")),text(p["name"].as_str().unwrap_or("")),text(p["api"].as_str().unwrap_or("other")),text(p["base_url"].as_str().unwrap_or("")),crate::db::opt_text(p["secret_ref"].as_str()),text(p["destination"].as_str().unwrap_or("on_device")),int(if p["enabled"]==true{1}else{0}),text(p.get("headers_json").unwrap_or(&json!({})).to_string()),int(self.clock.now_ms()),int(self.clock.now_ms())])?;}
                for m in c["models"].as_array().into_iter().flatten(){self.db.execute("INSERT INTO ai_models(provider_id,model_id,modalities_json,cost_json) VALUES(?,?,?,?)",&[text(m["provider_id"].as_str().unwrap_or("")),text(m["model_id"].as_str().unwrap_or("")),text(m.get("modalities_json").unwrap_or(&json!(["text"])).to_string()),crate::db::opt_text(m.get("cost_json").filter(|v|!v.is_null()).map(Value::to_string))])?;}
                for a in c["roles"].as_array().unwrap(){let model=&a["model"];if model.is_object()&&self.db.scalar("SELECT COUNT(*) FROM ai_models WHERE provider_id=? AND model_id=?",&[text(model["provider_id"].as_str().unwrap_or("")),text(model["model_id"].as_str().unwrap_or(""))])?==0{self.db.execute("INSERT INTO ai_models(provider_id,model_id) VALUES(?,?)",&[text(model["provider_id"].as_str().unwrap_or("")),text(model["model_id"].as_str().unwrap_or(""))])?;}
                self.db.execute("INSERT INTO ai_roles(role,provider_id,model_id,fallback_json,limits_json) VALUES(?,?,?,?,?)",&[text(a["role"].as_str().unwrap_or("")),crate::db::opt_text(model["provider_id"].as_str()),crate::db::opt_text(model["model_id"].as_str()),text(a.get("fallback_json").unwrap_or(&json!([])).to_string()),text(a.get("limits_json").unwrap_or(&json!({})).to_string())])?;}
                self.wf_put("models.config/1",c)
                })?;
                    self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='configuration_changed' WHERE state IN ('queued','running') AND (kind LIKE 'workflow.%' OR kind='speech.recognize')",&[])?;
                }
                Ok(c.clone())
            }
            "usage" => Ok(
                json!({"calls":self.db.query("SELECT call_id,started_ms,ended_ms,feature,run_id,provider_id,model_id,destination,input_tokens,output_tokens,cost,currency,cost_source,status,error_code,client,cache_read_tokens,cache_write_tokens,latency_ms,request_bytes,response_bytes FROM ai_calls ORDER BY started_ms",&[],|r|json!({"call_id":r.text(0),"started_ms":r.int(1),"ended_ms":r.opt_int(2),"feature":r.text(3),"run_id":r.opt_text(4),"provider_id":r.text(5),"model_id":r.text(6),"destination":r.text(7),"input_tokens":r.opt_int(8),"output_tokens":r.opt_int(9),"cost":r.get::<_,Option<f64>>(10).ok().flatten(),"currency":r.opt_text(11),"cost_source":r.opt_text(12),"status":r.text(13),"error_code":r.opt_text(14),"client":r.text(15),"cache_read_tokens":r.opt_int(16),"cache_write_tokens":r.opt_int(17),"latency_ms":r.opt_int(18),"request_bytes":r.opt_int(19),"response_bytes":r.opt_int(20)}))?}),
            ),
            _ => Err(bad("未知模型操作")),
        }
    }
    fn workflow_approvals(&self) -> Result<Value> {
        let c = self.model_config()?;
        let mut result = json!({});
        for p in c["providers"].as_array().into_iter().flatten() {
            for source in ["import", "mac_microphone", "mac_system_audio", "mac_region"] {
                let key = format!("{}:{}", source, p["provider_id"].as_str().unwrap_or(""));
                result[&key] = json!(
                    self.wf_get(&format!("workflows.approval/1:{key}"))?
                        == sha256_hex(&serde_json::to_vec(p)?)
                );
            }
        }
        Ok(result)
    }
    pub fn workflow_command(&self, v: &Value) -> Result<Value> {
        match v["action"].as_str().unwrap_or("status") {
            "status" => Ok(
                json!({"catalog":serde_json::from_str::<Value>(CATALOG)?,"output_schema":serde_json::from_str::<Value>(OUTPUT_SCHEMA)?,"features":self.feature_settings()?,"settings":self.wf_get("workflows.settings/1")?,"approvals":self.workflow_approvals()?,"jobs":self.job_command(&json!({"action":"list"}))?["jobs"],"usage":self.model_command(&json!({"action":"usage"}))?}),
            ),
            "features" => self.save_feature_settings(&v["config"]),
            "authorize" => {
                let source = v["source"].as_str().unwrap_or("");
                if !["import", "mac_microphone", "mac_system_audio", "mac_region"].contains(&source) {
                    return Err(bad("来源无效"));
                }
                let c = self.model_config()?;
                let provider = c["providers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["provider_id"] == v["provider_id"])
                    .ok_or_else(|| bad("缺少模型来源"))?;
                let key = format!(
                    "workflows.approval/1:{}:{}",
                    source,
                    v["provider_id"].as_str().unwrap_or("")
                );
                self.wf_put(
                    &key,
                    &if v["allowed"] == true {
                        json!(sha256_hex(&serde_json::to_vec(provider)?))
                    } else {
                        Value::Null
                    },
                )?;
                Ok(json!({"saved":true}))
            }
            "speaker_policy" => self.save_recording_speaker_policy(v["id"].as_str().unwrap_or(""), &v["policy"]),
            "speakers" => self.enqueue_recording_speakers(v["id"].as_str().unwrap_or("")),
            "settings" => {
                let c = &v["settings"];
                if c["version"] != 1 {
                    return Err(bad("不支持的流程设置版本"));
                }
                self.wf_put("workflows.settings/1", c)?;
                if c["automatic"] == false {
                    self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='automatic_disabled' WHERE kind='speech.recognize' AND state IN ('queued','running')",&[])?;
                }
                if c["summary"] == false {
                    self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='automatic_disabled' WHERE kind='workflow.summary' AND state IN ('queued','running')",&[])?;
                }
                if c["titles"] == false {
                    self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='automatic_disabled' WHERE kind='workflow.title' AND state IN ('queued','running')",&[])?;
                }
                Ok(c.clone())
            }
            "enqueue" => {
                let id = v["id"].as_str().ok_or_else(|| bad("缺少资料"))?;
                let step = v["step"].as_str().unwrap_or("title");
                if !["title", "summary", "decisions", "tags", "screenshot"].contains(&step) {
                    return Err(bad("此流程尚不可执行"));
                }
                let item = self.workflow_source(id)?;
                if step != "screenshot" && item["segments"].as_array().is_none_or(|s| s.is_empty())
                {
                    return Err(bad("请先完成转写"));
                }
                let job=self.workflow_enqueue(id, step, &item)?;
                if matches!(job["state"].as_str(),Some("done"|"failed"|"cancelled")){self.job_command(&json!({"action":"restart","id":job["id"]}))}else{Ok(job)}
            }
            "daily" => {
                let day = v["day"].as_str().ok_or_else(|| bad("缺少日期"))?;
                chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                    .map_err(|_| bad("日期格式无效"))?;
                let start = v["start_ms"].as_str().and_then(|s| s.parse::<i64>().ok());
                let end = v["end_ms"].as_str().and_then(|s| s.parse::<i64>().ok());
                let items = self.media_command(&json!({"action":"list","limit":1000}))?;
                let sources: Vec<_> = items["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| {
                        i["kind"] == "audio"
                            && i["occurred_at"]
                                .as_str()
                                .is_some_and(|s| match (start, end) {
                                    (Some(a), Some(b)) => chrono::DateTime::parse_from_rfc3339(s)
                                        .is_ok_and(|t| {
                                            t.timestamp_millis() >= a && t.timestamp_millis() < b
                                        }),
                                    _ => s.starts_with(day),
                                })
                    })
                    .map(|i| i["id"].clone())
                    .collect();
                if sources.is_empty() {
                    return Err(bad("当天没有录音"));
                }
                let id = format!("workflow-day:{day}");
                self.wf_put(&format!("workflow.sources/1:{id}"), &json!(sources))?;
                let item = self.workflow_source(&id)?;
                self.workflow_enqueue(&id, "daily", &item)
            }
            "drafts" => {
                let id = v["id"].as_str().unwrap_or("");
                let item = self.workflow_source(id)?;
                let revision = crate::speakers::segments_revision(&item["segments"]);
                let mut drafts = self.workflow_drafts(id)?;
                if let Some(o) = drafts.as_object_mut() {
                    o.retain(|_, d| d["source_revision"] == revision);
                }
                Ok(drafts)
            }
            _ => Err(bad("未知流程操作")),
        }
    }
    fn workflow_source(&self, id: &str) -> Result<Value> {
        if id.starts_with("workflow-day:") {
            let sources = self.wf_get(&format!("workflow.sources/1:{id}"))?;
            let mut segments = Vec::new();
            for (index, source) in sources
                .as_array()
                .ok_or_else(|| bad("缺少回顾来源"))?
                .iter()
                .enumerate()
            {
                let source = source.as_str().ok_or_else(|| bad("无效来源"))?;
                let item = self.workflow_source(source)?;
                for segment in item["segments"].as_array().into_iter().flatten() {
                    let mut segment = segment.clone();
                    segment["text"] = json!(format!(
                        "[录音{}：{}] {}",
                        index + 1,
                        item["title"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("未命名"),
                        segment["text"].as_str().unwrap_or("")
                    ));
                    segment["source_id"] = json!(source);
                    segments.push(segment);
                }
            }
            return Ok(
                json!({"id":id,"kind":"day","segments":segments,"content_revision":sources}),
            );
        }
        let item = self.media_item(id)?;
        if !self.media_text_allowed(id, item["source"].as_str().unwrap_or(""))? {
            return Err(TimelineError::Denied("media_text_storage_disabled".into()));
        }
        Ok(item)
    }
    fn workflow_enqueue(&self, id: &str, step: &str, item: &Value) -> Result<Value> {
        self.feature_check(step, item, None)?;
        let config = self.model_config()?;
        let version = sha256_hex(&serde_json::to_vec(&json!([
            config,
            crate::speakers::transcript(&item["segments"]),
            item["content_revision"]
        ]))?);
        self.job_command(&json!({"action":"enqueue","kind":format!("workflow.{step}"),"input_ref":id,"config_version":format!("fixed/1:{version}")}))
    }
    pub fn workflow_scan(&self) -> Result<()> {
        let stamp = (self.background_revision("workflows")?, self.clock.now_ms().div_euclid(60000));
        if self.workflow_scan_stamp.get() == Some(stamp) { return Ok(()); }
        let settings = self.wf_get("workflows.settings/1")?;
        // New recordings are transcribed with speakers unless turned off (user decision 2026-11-13: matching and
        // naming happen on their own; every automatic step stays switchable).
        let automatic = settings["automatic"] != false;
        // Titles for transcribed recordings are on unless turned off (user decision 2026-11-12).
        let titles = settings["titles"] != false;
        if !automatic && !titles && settings["summary"] != true {
            self.workflow_scan_stamp.set(Some(stamp));
            return Ok(());
        }
        let items = self.media_command(&json!({"action":"list","limit":1000}))?;
        for item in items["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["kind"] == "audio")
        {
            let id = item["id"].as_str().unwrap_or("");
            if self.workflow_source(id).is_err() {
                continue;
            }
            let mut parts = self.media_part_ids(id)?;
            if parts.is_empty() {
                parts.push(id.into());
            }
            for part in parts {
                let m = self.media_raw(&part)?;
                if automatic
                    && self.feature_source_allowed("transcribe", item)
                    && m["processing_status"] != "ready"
                {
                    let mode = settings["mode"].as_str().unwrap_or("conversation");
                    self.job_command(&json!({"action":"enqueue","kind":"speech.recognize","input_ref":part,"config_version":format!("external-api/1:{mode}")}))?;
                }
            }
            if item["recording_state"] != "recording"
                && item["processing_status"] == "ready"
                && !item["segments"].as_array().is_none_or(|s| s.is_empty())
            {
                if automatic && self.voiceprint_provider().is_ok() && self.feature_source_allowed("transcribe",item) && item["segments"].as_array().is_some_and(|a|a.iter().any(|s|s["speaker"].is_string())) {
                    let _=self.enqueue_recording_speakers(id);
                }
                if titles
                    && self.feature_source_allowed("title", item)
                    && self.workflow_drafts(id)?["title"]["source_revision"]
                        != json!(crate::speakers::segments_revision(&item["segments"]))
                {
                    self.workflow_enqueue(id, "title", item)?;
                }
                if settings["summary"] == true && self.feature_source_allowed("summary", item) {
                    self.workflow_enqueue(id, "summary", item)?;
                }
            }
        }
        self.workflow_scan_stamp.set(Some((self.background_revision("workflows")?, stamp.1)));
        Ok(())
    }
    /// Returns a short-lived immutable request; the caller releases the database mutex before HTTP.
    pub fn workflow_prepare(&self, job: &Value) -> Result<Value> {
        if job["kind"] == "workflow.speakers" {return self.recording_speakers_prepare(job);}
        if job["kind"] == "screen.summarize" { return self.screen_prepare(job); }
        if job["kind"] == crate::screen_excerpt::JOB_KIND { return self.excerpt_prepare(job); }
        if job["kind"] == "people.judge" { return self.people_judge_prepare(job); }
        let id = job["input_ref"].as_str().unwrap_or("");
        let item = self.workflow_source(id)?;
        let kind = job["kind"].as_str().unwrap_or("");
        let speech = kind == "speech.recognize";
        if !speech
            && ![
                "workflow.title",
                "workflow.summary",
                "workflow.decisions",
                "workflow.tags",
                "workflow.screenshot",
                "workflow.daily",
            ]
            .contains(&kind)
        {
            return Err(bad("未知固定流程"));
        }
        let step = kind.strip_prefix("workflow.").unwrap_or("transcribe");
        let mode = job["config_version"]
            .as_str()
            .and_then(|s| s.strip_prefix("external-api/1:"))
            .unwrap_or("quick");
        let role = if speech {
            if mode == "conversation" {
                "diarization"
            } else {
                "asr"
            }
        } else if step == "screenshot" {
            "vision"
        } else if ["summary", "decisions", "daily"].contains(&step) {
            "summary"
        } else {
            "title"
        };
        let c = self.model_config()?;
        self.feature_check(step, &item, None)?;
        // A text step without its own model borrows another text-capable role's model, with the step's prompt.
        let order: &[&str] = match role {
            "title" => &["title", "summary", "vision"],
            "summary" => &["summary", "title", "vision"],
            _ => std::slice::from_ref(&role),
        };
        let provider = |a: &Value| {
            c["providers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["provider_id"] == a["model"]["provider_id"] && p["enabled"] == true)
        };
        let (a, p, borrowed) = order
            .iter()
            .enumerate()
            .find_map(|(i, r)| {
                let a = c["roles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|a| a["role"] == *r && a["model"].is_object())?;
                Some((a, provider(a)?, i > 0))
            })
            .ok_or_else(|| bad("waiting_model"))?;
        self.feature_check(step, &item, Some(p))?;
        // External destinations require an explicit per-source approval. Loopback is the deployed local service.
        // A model server on this Mac (loopback) needs no per-source approval, whatever its label: it is the
        // user's own choice (user decision 2026-11-13). Other hosts keep the one-time approval per source.
        let url = p["base_url"].as_str().unwrap_or("");
        let loopback = [
            "http://127.0.0.1:",
            "http://localhost:",
            "https://localhost:",
            "https://127.0.0.1:",
            "http://[::1]:",
        ]
        .iter()
        .any(|prefix| url.starts_with(prefix));
        if p["destination"] != "on_device" && !loopback {
            let mut sources = vec![item["source"].as_str().unwrap_or("").to_string()];
            if id.starts_with("workflow-day:") {
                sources.clear();
                for source in self
                    .wf_get(&format!("workflow.sources/1:{id}"))?
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    sources.push(
                        self.media_item(source.as_str().unwrap_or(""))?["source"]
                            .as_str()
                            .unwrap_or("")
                            .into(),
                    );
                }
            }
            for source in sources {
                let approval = self.wf_get(&format!(
                    "workflows.approval/1:{}:{}",
                    source,
                    p["provider_id"].as_str().unwrap_or("")
                ))?;
                if approval != sha256_hex(&serde_json::to_vec(p)?) {
                    return Err(TimelineError::Denied("waiting_authorization".into()));
                }
            }
        }
        if p["destination"] == "on_device"
            && !(url.starts_with("http://127.0.0.1:")
                || url.starts_with("http://localhost:")
                || url.starts_with("https://localhost:"))
        {
            return Err(bad("waiting_authorization"));
        }
        if speech && a["limits_json"]["daily_budget"].is_number() {
            return Err(bad("waiting_budget_cost_unknown"));
        }
        let revision = crate::speakers::segments_revision(&item["segments"]);
        if !speech {
            let current = self.workflow_enqueue(id, step, &item)?;
            if current["id"] != job["id"] {
                return Err(bad("source_or_configuration_changed"));
            }
        }
        let prompt = a["prompt"]
            .as_str()
            .filter(|_| !borrowed)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or(
                serde_json::from_str::<Value>(PROMPTS)?[step]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            );
        let content = item["segments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if content.len() > 32000 {
            return Err(bad("input_limit"));
        }
        if step == "screenshot" && a["limits_json"]["daily_budget"].is_number() {
            return Err(bad("waiting_budget_cost_unknown"));
        }
        let messages = if step == "screenshot" {
            if item["kind"] != "image" {
                return Err(bad("需要图片来源"));
            }
            use base64::Engine;
            let bytes = std::fs::read(self.media_path(id)?)?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(bad("input_limit"));
            }
            Some(
                json!([{"role":"system","content":prompt},{"role":"user","content":[{"type":"image_url","image_url":{"url":format!("data:{};base64,{}",item["mime_type"].as_str().unwrap_or("image/png"),base64::engine::general_purpose::STANDARD.encode(bytes))}}]}]),
            )
        } else {
            None
        };
        if !speech {
            self.workflow_budget(&c, a, prompt.len() + content.len())?;
        }
        Ok(
            json!({"job":job,"provider":p,"assignment":a,"config_revision":sha256_hex(&serde_json::to_vec(&c)?),"source_revision":revision,"feature_revision":sha256_hex(&serde_json::to_vec(&self.feature_settings()?["items"][step])?),"step":step,"mode":mode,"speech":speech,"path":if speech{Some(self.media_path(id)?)}else{None},"digest":if speech{Some(crate::sha256_hex_file(&self.media_path(id)?)?)}else{None},"prompt":prompt,"content":content,"messages":messages}),
        )
    }
    fn workflow_feature_reply_allowed(&self, r: &Value, item: &Value) -> Result<()> {
        let step = r["step"].as_str().unwrap_or("");
        if sha256_hex(&serde_json::to_vec(
            &self.feature_settings()?["items"][step],
        )?) != r["feature_revision"]
        {
            return Err(bad("source_or_configuration_changed"));
        }
        self.feature_check(step, item, None)?;
        Ok(())
    }
    pub fn workflow_reply_allowed(&self, r: &Value) -> Result<()> {
        if r["step"] == "screen-episodes" { return self.screen_reply_allowed(r); }
        if r["step"] == crate::screen_excerpt::FEATURE { return self.excerpt_reply_allowed(r); }
        let job = &r["job"];
        let item = self.workflow_source(job["input_ref"].as_str().unwrap_or(""))?;
        if sha256_hex(&serde_json::to_vec(&self.model_config()?)?) != r["config_revision"] {
            return Err(bad("configuration_changed"));
        }
        // Speaker grouping depends on the per-file transcripts, not on the transcript it projects itself.
        let revision = if r["reconcile_speakers"] == true {
            self.recording_speaker_input(job["input_ref"].as_str().unwrap_or(""))?["revision"].clone()
        } else {
            json!(crate::speakers::segments_revision(&item["segments"]))
        };
        if r["speech"] != true && revision != r["source_revision"] {
            return Err(bad("source_changed"));
        }
        self.workflow_feature_reply_allowed(r, &item)?;
        let current = self.job_command(&json!({"action":"get","id":job["id"]}))?;
        if current["state"] != "running"
            || current["attempt"] != job["attempt"]
            || current["owner"] != job["owner"]
            || current["lease_until_ms"].as_i64().unwrap_or(0) <= self.clock.now_ms()
        {
            return Err(TimelineError::Denied("stale_job_lease".into()));
        }
        Ok(())
    }
    pub fn workflow_call_start(&self, request: &Value) -> Result<String> {
        let id = uuid_v4();
        let p = &request["provider"];
        let model = &request["assignment"]["model"]["model_id"];
        self.db.execute("INSERT INTO ai_calls(call_id,started_ms,feature,client,run_id,provider_id,model_id,destination,status,error_code,cost_source) VALUES(?,?,?,?,?,?,?,?,'error','interrupted_unknown_charge','unknown')",&[text(&id),int(self.clock.now_ms()),text(request["step"].as_str().unwrap_or("model")),text(if request["client"]=="app"{"app"}else{"worker"}),text(request["job"]["id"].as_str().unwrap_or("")),text(p["provider_id"].as_str().unwrap_or("")),text(model.as_str().unwrap_or("")),text(p["destination"].as_str().unwrap_or(""))])?;
        let config = self.model_config()?;
        let snapshot = config["models"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["provider_id"] == p["provider_id"] && m["model_id"] == *model)
            .cloned()
            .unwrap_or(Value::Null);
        self.wf_put(&format!("models.call-prices/1:{id}"), &snapshot)?;
        Ok(id)
    }
    pub fn workflow_call_finish(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
        usage: &Value,
    ) -> Result<()> {
        let model = self.wf_get(&format!("models.call-prices/1:{id}"))?;
        let prices = &model["cost_json"]["per_mtok"];
        let estimate = match (
            usage["prompt_tokens"].as_f64(),
            usage["completion_tokens"].as_f64(),
            prices["input"].as_f64(),
            prices["output"].as_f64(),
        ) {
            (Some(i), Some(o), Some(pi), Some(po)) => Some((i * pi + o * po) / 1_000_000.0),
            _ => None,
        };
        let cost = usage["cost"]
            .as_f64()
            .or(estimate)
            .filter(|n| n.is_finite() && *n >= 0.0);
        let source = if usage["cost"].is_number() {
            "provider"
        } else if estimate.is_some() {
            "estimated"
        } else {
            "unknown"
        };
        self.db.execute("UPDATE ai_calls SET ended_ms=?,latency_ms=?-started_ms,status=?,error_code=?,input_tokens=?,output_tokens=?,cost=?,currency=?,cost_source=? WHERE call_id=?",&[int(self.clock.now_ms()),int(self.clock.now_ms()),text(status),crate::db::opt_text(error),crate::db::opt_int(usage["prompt_tokens"].as_i64()),crate::db::opt_int(usage["completion_tokens"].as_i64()),cost.map(crate::db::P::Real).unwrap_or(crate::db::P::Null),crate::db::opt_text(if cost.is_some(){model["cost_json"]["currency"].as_str()}else{None}),text(source),text(id)])
    }
    pub fn workflow_complete(&self, r: &Value, output: &Value) -> Result<Value> {
        if r["step"] == "screen-episodes" { return self.screen_complete(r, output); }
        if r["step"] == crate::screen_excerpt::FEATURE { return self.excerpt_complete(r, output); }
        if r["step"] == "people-judge" { return self.people_judge_complete(r, output); }
        let job = &r["job"];
        let id = job["input_ref"].as_str().unwrap_or("");
        let item = self.workflow_source(id)?;
        self.workflow_feature_reply_allowed(r, &item)?;
        if sha256_hex(&serde_json::to_vec(&self.model_config()?)?) != r["config_revision"] {
            return Err(bad("configuration_changed"));
        }
        if r["speech"] == true {
            return self.media_command(&json!({"action":"result","job_id":job["id"],"attempt":job["attempt"],"owner":job["owner"],"mode":r["mode"],"result":output}));
        }
        if crate::speakers::segments_revision(&item["segments"]) != r["source_revision"] {
            return Err(bad("source_changed"));
        }
        let text_value = output
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 16000)
            .ok_or_else(|| bad("invalid_output"))?;
        let draft = json!({"format":"draft/1","text":text_value,"source_revision":r["source_revision"],"producer":r["assignment"]["model"],"prompt_version":1,"job_id":job["id"]});
        let drafts=self.db.transaction(||{
            let mut drafts=self.workflow_drafts(id)?;
            let step=r["step"].as_str().unwrap_or("title");
            let completed=self.db.scalar("SELECT COUNT(*) FROM jobs j JOIN job_checkpoints c USING(job_id) WHERE j.job_id=? AND j.state='done' AND j.attempts=? AND c.owner=?",&[text(job["id"].as_str().unwrap_or("")),int(job["attempt"].as_i64().unwrap_or(-1)),text(job["owner"].as_str().unwrap_or(""))])?==1;
            if completed&&drafts[step]==draft{return Ok(drafts);}
            if self.db.scalar("SELECT COUNT(*) FROM jobs j JOIN job_checkpoints c USING(job_id) WHERE j.job_id=? AND j.state='running' AND j.attempts=? AND c.owner=? AND j.lease_until_ms>?",&[text(job["id"].as_str().unwrap_or("")),int(job["attempt"].as_i64().unwrap_or(-1)),text(job["owner"].as_str().unwrap_or("")),int(self.clock.now_ms())])?!=1{return Err(TimelineError::Denied("stale_job_lease".into()));}
            self.workflow_draft_put(id,step,&draft)?;drafts[step]=draft;
            self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=?,updated_ms=? WHERE job_id=?",&[text(format!("draft/1:{id}")),int(self.clock.now_ms()),text(job["id"].as_str().unwrap_or(""))])?;Ok(drafts)
        })?;
        self.project_native_media()?;
        Ok(drafts)
    }
}
