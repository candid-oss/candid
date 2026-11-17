use super::*;

impl TimelineStore {
    pub(crate) fn automation_get(&self, key: &str) -> Result<Value> {
        self.db
            .first(
                "SELECT value FROM store_meta WHERE key=?",
                &[text(key)],
                |r| r.text(0),
            )?
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .unwrap_or(Ok(Value::Null))
    }

    pub(crate) fn automation_put(&self, key: &str, value: &Value) -> Result<()> {
        self.db.execute(
            "INSERT INTO store_meta VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value IS NOT excluded.value",
            &[text(key), text(value.to_string())],
        )
    }

    pub fn automation_validate(&self, r: &Value) -> Result<()> {
        let c = self.automation_config()?;
        let id = r["skill"].as_str().ok_or_else(|| bad("缺少技能"))?;
        let p = c.skills.get(id).ok_or_else(|| bad("未知技能"))?;
        if !p.enabled || self.policy_fingerprint(id, &c)? != r["config_revision"] {
            return Err(denied("configuration_changed"));
        }
        if !r["model_config_revision"].is_null()
            && r["model_config_revision"] != sha256_hex(&serde_json::to_vec(&self.model_config()?)?)
        {
            return Err(denied("model_configuration_changed"));
        }
        let current = self.sources()?;
        for snapshot in r["source_policies"].as_array().into_iter().flatten() {
            let Some(s) = current.iter().find(|s| {
                s.source_id == snapshot[0].as_str().unwrap_or("")
                    && s.status != "revoked"
                    && s.policy.agent_visible
                    && s.policy.store_text
            }) else {
                return Err(denied("source_unavailable"));
            };
            if serde_json::to_value(&s.policy)? != snapshot[1] {
                return Err(denied("source_policy_changed"));
            }
        }
        for reference in r["references"].as_array().into_iter().flatten() {
            if let Some(id) = reference["event_id"].as_str() {
                let value = self.db.first(
                    "SELECT event_id FROM events WHERE event_id=? AND sensitivity<>'protected'",
                    &[text(id)],
                    |r| r.text(0),
                )?;
                if value
                    .map(|id| self.fingerprint_stored(&id))
                    .transpose()?
                    .as_deref()
                    != reference["revision"].as_str()
                {
                    return Err(denied("source_unavailable"));
                }
            }
        }
        Ok(())
    }

    pub(super) fn automation_record(&self, r: &Value) -> Result<()> {
        let mut r = r.clone();
        let id = r["id"]
            .as_str()
            .ok_or_else(|| bad("缺少运行编号"))?
            .to_string();
        let rev = self.automation_get(&format!("automation.run/1:{id}"))?["revision"]
            .as_u64()
            .unwrap_or(0)
            + 1;
        r["revision"] = json!(rev);
        r["updated_ms"] = json!(self.clock.now_ms());
        self.automation_put(&format!("automation.revision/1:{id}:{rev}"), &r)?;
        self.automation_put(&format!("automation.run/1:{id}"), &r)
    }

    pub fn automation_rows(&self, limit: usize) -> Result<Vec<Value>> {
        let raws=self.db.query("SELECT value FROM store_meta WHERE key LIKE 'automation.run/1:%' ORDER BY CAST(json_extract(value,'$.created_ms') AS INTEGER) DESC,key LIMIT ?",&[int(limit.min(1000) as i64)],|r|r.text(0))?;
        raws.into_iter()
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .collect()
    }

    pub fn automation_run(&self, id: &str) -> Result<Value> {
        let r = self.automation_get(&format!("automation.run/1:{id}"))?;
        if r.is_null() {
            return Err(TimelineError::NotFound("运行记录".into()));
        }
        if r["format"] != "candid-agent-run/1" {
            return Err(bad("不支持的运行记录版本"));
        }
        Ok(r)
    }

    pub(super) fn automation_enqueue(
        &self,
        skill: &str,
        key: &str,
        trigger: &str,
        references: &[Value],
        context: &str,
        executor: &str,
    ) -> Result<Value> {
        let c = self.automation_config()?;
        let p = c.skills.get(skill).ok_or_else(|| bad("未知技能"))?;
        if !p.enabled {
            return Err(denied("feature_disabled"));
        }
        if trigger != "manual"
            && (!p.automatic
                || !self.automation_quality()?["passed"]
                    .as_bool()
                    .unwrap_or(false))
        {
            return Err(denied("waiting_quality"));
        }
        let version = self.policy_fingerprint(skill, &c)?;
        if let Some(id) = self.db.first(
            "SELECT job_id FROM jobs WHERE kind='agent.skill' AND input_ref=? AND config_version=?",
            &[text(key), text(&version)],
            |r| r.text(0),
        )? {
            return self.automation_run(&id);
        }
        let zone = crate::Zone::named(&p.timezone).unwrap();
        let day = zone.local(self.clock.now_ms()).date();
        let from = zone.instant(day.and_hms_opt(0, 0, 0).unwrap());
        if self.db.scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind='agent.skill' AND feature=? AND created_ms>=?",
            &[text(skill), int(from)],
        )? >= p.daily_runs as i64
        {
            return Err(denied("daily_run_budget"));
        }
        let (sources, stamp) = self.automation_scope(p)?;
        let mut ids = Vec::new();
        for reference in references {
            let id = reference["event_id"]
                .as_str()
                .ok_or_else(|| bad("来源引用无效"))?;
            let row=self.db.first("SELECT source_id,event_id FROM events WHERE event_id=? AND sensitivity<>'protected'",&[text(id)],|r|(r.text(0),r.text(1)))?.ok_or_else(||denied("source_not_allowed"))?;
            let revision = self.fingerprint_stored(id)?;
            if !sources.contains(&row.0)
                || reference["revision"]
                    .as_str()
                    .is_some_and(|v| v != revision)
            {
                return Err(denied("source_not_allowed"));
            }
            ids.push(json!({"event_id":id,"source_id":row.0,"revision":revision}));
        }
        let id = uuid_v4();
        let now = self.clock.now_ms();
        let r = json!({"format":"candid-agent-run/1","id":id,"skill":skill,"trigger":trigger,"trigger_key":key,"references":ids,"context":context,"executor":executor,"sources":sources,"source_policies":stamp,"policy":p,"config_revision":version,"state":"queued","created_ms":now,"updated_ms":now,"attempt":0,"steps":[],"result":null,"error":null,"notified":false,"notify_ready":false,"producer":{"component":"Pi","version":"1.5.2","pipeline":"automation/1"},"model":null});
        self.db.transaction(||{
            self.db.execute("INSERT INTO jobs(job_id,kind,input_ref,config_version,state,created_ms,updated_ms,feature) VALUES(?,'agent.skill',?,?,'queued',?,?,?)",&[text(&id),text(key),text(&version),int(now),int(now),text(skill)])?;
            self.automation_record(&r)
        })?;
        self.automation_run(&id)
    }

    pub(super) fn automation_end_inner(
        &self,
        r: &Value,
        state: &str,
        error: Option<&str>,
        result: Option<&Value>,
    ) -> Result<()> {
        let id = r["id"].as_str().unwrap();
        let mut next = r.clone();
        next["state"] = json!(state);
        next["error"] = json!(error);
        if let Some(result) = result {
            next["result"] = result.clone();
            let output: Value = result
                .as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            next["notify_ready"] = json!(
                state == "done"
                    && output["has_updates"] == true
                    && output["summary"]
                        .as_str()
                        .is_some_and(|s| !s.trim().is_empty())
                    && r["references"].as_array().is_some_and(|r| !r.is_empty())
            );
        }
        self.db.execute(
            "UPDATE jobs SET state=?,lease_until_ms=NULL,error_code=?,updated_ms=? WHERE job_id=?",
            &[
                text(state),
                crate::db::opt_text(error),
                int(self.clock.now_ms()),
                text(id),
            ],
        )?;
        self.automation_record(&next)
    }

    pub fn automation_finish(
        &self,
        id: &str,
        attempt: i64,
        state: &str,
        result: &Value,
        error: Option<&str>,
    ) -> Result<Value> {
        if !["done", "failed", "cancelled"].contains(&state) {
            return Err(bad("运行状态无效"));
        }
        let r = self.automation_run(id)?;
        if r["attempt"] != attempt {
            return Err(denied("stale_attempt"));
        }
        self.automation_validate(&r)?;
        if r["state"] == state && r["result"] == *result {
            return Ok(r);
        }
        if r["state"] != "running" {
            return Err(denied("run_not_running"));
        }
        if self.automation_validate(&r).is_err() {
            self.db.transaction(|| {
                self.automation_end_inner(
                    &r,
                    "cancelled",
                    Some("source_or_configuration_changed"),
                    None,
                )
            })?;
            return Err(denied("source_or_configuration_changed"));
        }
        if (!result.is_null() && !result.is_string()) || result.to_string().len() > 64000 {
            return Err(bad("运行结果过长"));
        }
        self.db
            .transaction(|| self.automation_end_inner(&r, state, error, Some(result)))?;
        self.automation_run(id)
    }

    pub fn automation_step(&self, id: &str, tool: &str, count: i64, refs: &[Value]) -> Result<()> {
        let mut r = self.automation_run(id)?;
        self.automation_validate(&r)?;
        if r["state"] != "running" {
            return Err(denied("run_not_running"));
        }
        let mut steps = r["steps"].as_array().cloned().unwrap_or_default();
        if steps.len() < 500 {
            steps.push(json!({"tool":tool,"count":count,"at_ms":self.clock.now_ms()}));
        }
        r["steps"] = json!(steps);
        let mut references = r["references"].as_array().cloned().unwrap_or_default();
        for reference in refs {
            if !references
                .iter()
                .any(|v| v["event_id"] == reference["event_id"])
            {
                references.push(reference.clone());
            }
        }
        r["references"] = json!(references);
        self.db.transaction(|| self.automation_record(&r))
    }

    pub fn automation_heartbeat(&self, id: &str, attempt: i64) -> Result<()> {
        let r = self.automation_run(id)?;
        self.automation_validate(&r)?;
        if r["state"] != "running" || r["attempt"] != attempt {
            return Err(denied("run_not_running"));
        }
        self.db.execute(
            "UPDATE jobs SET lease_until_ms=? WHERE job_id=?",
            &[int(self.clock.now_ms() + 60000), text(id)],
        )
    }

    pub fn automation_claim(&self) -> Result<Value> {
        self.automation_claim_for(None)
    }

    pub(super) fn automation_claim_for(&self, wanted: Option<&str>) -> Result<Value> {
        self.automation_scan()?;
        let candidates=self.db.query("SELECT job_id FROM jobs WHERE kind='agent.skill' AND state='queued' ORDER BY created_ms,job_id LIMIT 1000",&[],|r|r.text(0))?;
        for id in candidates {
            let r = self.automation_run(&id)?;
            if !wanted
                .map(|id| r["id"] == id)
                .unwrap_or(r["executor"] != "foreground")
            {
                continue;
            }

            if self.automation_validate(&r).is_err() {
                continue;
            }
            let id = r["id"].as_str().unwrap();
            let mut next = r.clone();
            let attempt = r["attempt"].as_i64().unwrap_or(0) + 1;
            next["attempt"] = json!(attempt);
            next["state"] = json!("running");
            self.db.transaction(||{
                self.db.execute("UPDATE jobs SET state='running',attempts=?,lease_until_ms=? WHERE job_id=? AND state='queued'",&[int(attempt),int(self.clock.now_ms()+60000),text(id)])?;
                self.automation_record(&next)
            })?;
            return self.automation_run(id);
        }
        Ok(Value::Null)
    }
}
