use super::*;

impl TimelineStore {
    pub fn automation_command(&self, v: &Value) -> Result<Value> {
        match v["action"].as_str().unwrap_or("status") {
            "status" => {
                self.automation_scan()?;
                let mut runs = if v["include_runs"] == false {
                    vec![]
                } else if v["notifications_only"] == true {
                    self.db.query("SELECT m.value FROM jobs j JOIN store_meta m ON m.key='automation.run/1:'||j.job_id WHERE j.kind='agent.skill' AND j.state='done' AND json_extract(m.value,'$.policy.notify')=1 AND json_extract(m.value,'$.notify_ready')=1 AND COALESCE(json_extract(m.value,'$.notified'),0)=0 ORDER BY j.created_ms LIMIT 100", &[], |r| r.text(0))?.into_iter().map(|s| serde_json::from_str(&s).map_err(Into::into)).collect::<Result<Vec<Value>>>()?
                } else {
                    self.automation_rows(100)?
                };
                for r in &mut runs {
                    if self.automation_validate(r).is_err() {
                        r["result"] = Value::Null;
                        r["context"] = json!("");
                        r["references"] = json!([]);
                        r["steps"] = json!([]);
                        r["unavailable"] = json!(true);
                    }
                }
                for r in &mut runs {
                    if let Some(result) = r["result"].as_str() {
                        let value: Value = serde_json::from_str(result).unwrap_or(Value::Null);
                        let preview = value["summary"]
                            .as_str()
                            .unwrap_or(result)
                            .chars()
                            .take(512)
                            .collect::<String>();
                        r["result"] = json!(preview);
                    }
                    r["context"] = json!("");
                    if let Some(steps) = r["steps"].as_array_mut() {
                        if steps.len() > 3 {
                            steps.drain(..steps.len() - 3);
                        }
                    }
                    if let Some(refs) = r["references"].as_array_mut() {
                        refs.truncate(20);
                    }
                    r["truncated"] = json!(true);
                }
                Ok(
                    json!({"config":self.automation_config()?,"quality":self.automation_quality()?,"runs":runs,"sources":self.sources()?,"catalog":self.skill_catalog()?}),
                )
            }
            "get" => {
                let r = self.automation_run(v["id"].as_str().unwrap_or(""))?;
                self.automation_validate(&r)?;
                Ok(r)
            }
            "config" => self.automation_save_config(&v["config"]),
            "run" => {
                let skill = v["skill"].as_str().ok_or_else(|| bad("缺少技能"))?;
                let key = v["key"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("manual:{skill}:{}", uuid_v4()));
                let refs = v["references"].as_array().cloned().unwrap_or_default();
                let context = v["context"].as_str().unwrap_or("");
                if context.len() > 16000 {
                    return Err(bad("上下文过长"));
                }
                self.automation_enqueue(
                    skill,
                    &key,
                    "manual",
                    &refs,
                    context,
                    if v["executor"] == "foreground" {
                        "foreground"
                    } else {
                        "background"
                    },
                )
            }
            "claim" => self.automation_claim_for(v["id"].as_str()),
            "heartbeat" => {
                self.automation_heartbeat(
                    v["id"].as_str().unwrap_or(""),
                    v["attempt"].as_i64().unwrap_or(0),
                )?;
                Ok(json!({"ok":true}))
            }
            "finish" => self.automation_finish(
                v["id"].as_str().unwrap_or(""),
                v["attempt"].as_i64().unwrap_or(0),
                v["state"].as_str().unwrap_or("failed"),
                &v["result"],
                v["error"].as_str(),
            ),
            "cancel" => {
                let r = self.automation_run(v["id"].as_str().unwrap_or(""))?;
                if ["queued", "running"].contains(&r["state"].as_str().unwrap_or("")) {
                    self.db.transaction(|| {
                        self.automation_end_inner(&r, "cancelled", Some("user_cancelled"), None)
                    })?;
                }
                self.automation_run(r["id"].as_str().unwrap())
            }
            "retry" => {
                let mut r = self.automation_run(v["id"].as_str().unwrap_or(""))?;
                self.automation_validate(&r)?;
                if !["failed", "cancelled"].contains(&r["state"].as_str().unwrap_or("")) {
                    return Err(bad("此运行不能重试"));
                }
                r["state"] = json!("queued");
                r["error"] = Value::Null;
                self.db.transaction(|| {
                    self.db.execute(
                        "UPDATE jobs SET state='queued',error_code=NULL WHERE job_id=?",
                        &[text(r["id"].as_str().unwrap())],
                    )?;
                    self.automation_record(&r)
                })?;
                self.automation_run(r["id"].as_str().unwrap())
            }
            "quality" => {
                let q = &v["report"];
                if q["format"] != "candid-context-evaluation/1"
                    || q["store_id"] != self.identity.store_id
                    || q["pipeline"] != PIPELINE
                    || q["dataset"].as_str().is_none_or(|s| s.is_empty())
                    || q["policy_revision"] != self.automation_policy_revision()?
                {
                    return Err(bad("评估报告版本或范围不匹配"));
                }
                self.automation_put("automation.quality/1", q)?;
                self.automation_quality()
            }
            "evaluation_scope" => Ok(
                json!({"store_id":self.identity.store_id,"pipeline":PIPELINE,"policy_revision":self.automation_policy_revision()?}),
            ),
            "notified" => {
                let mut r = self.automation_run(v["id"].as_str().unwrap_or(""))?;
                r["notified"] = json!(true);
                self.db.transaction(|| self.automation_record(&r))?;
                Ok(json!({"ok":true}))
            }
            _ => Err(bad("未知技能操作")),
        }
    }
}
