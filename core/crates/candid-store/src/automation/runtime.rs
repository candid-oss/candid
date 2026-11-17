use super::*;

impl TimelineStore {
    /// Freezes the core-owned scope; a caller cannot widen it.
    pub fn automation_pi_config(&self, id: &str, attempt: i64) -> Result<Value> {
        let r = self.automation_run(id)?;
        self.automation_validate(&r)?;
        if r["state"] != "running" || r["attempt"] != attempt {
            return Err(denied("run_not_running"));
        }
        let p = &r["policy"];
        Ok(
            json!({"run_id":id,"sources":r["sources"],"agents":[],"role":p["role"],"allow_remote":p["allow_remote"],"writes":["draft","reference","commitment","annotate"],"propose_kinds":[],"budget":{"max_model_calls":p["max_model_calls"],"max_tool_calls":p["max_tool_calls"],"max_output_tokens":p["max_output_tokens"]}}),
        )
    }

    pub fn automation_provider_allowed(&self, id: &str, provider: &Value) -> Result<()> {
        let r = self.automation_run(id)?;
        self.automation_validate(&r)?;
        let url = provider["base_url"].as_str().unwrap_or("");
        if crate::feature_settings::is_loopback(url) {
            return Ok(());
        }
        let c = self.automation_config()?;
        if r["policy"]["allow_remote"] != true
            || r["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|s| !c.remote_sources.iter().any(|v| json!(v) == *s))
        {
            return Err(denied("remote_not_allowed"));
        }
        Ok(())
    }

    pub fn automation_usage(&self, id: &str) -> Result<Value> {
        let (calls, tokens) = self
            .db
            .first(
                "SELECT COUNT(*),COALESCE(SUM(output_tokens),0) FROM ai_calls WHERE run_id=?",
                &[text(id)],
                |r| (r.int(0), r.int(1)),
            )?
            .unwrap_or((0, 0));
        let r = self.automation_run(id)?;
        Ok(
            json!({"model_calls":calls,"output_tokens":tokens,"tool_calls":r["steps"].as_array().map(|s|s.len()).unwrap_or(0)}),
        )
    }

    pub fn automation_audit_tool(&self, id: &str, tool: &str, body: &[u8]) -> Result<()> {
        fn gather(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Object(m) => {
                    if let Some(id) = m.get("event_id").and_then(Value::as_str) {
                        if out.len() < 500 && !out.iter().any(|s| s == id) {
                            out.push(id.into());
                        }
                    }
                    for v in m.values() {
                        gather(v, out)
                    }
                }
                Value::Array(a) => {
                    for v in a {
                        gather(v, out)
                    }
                }
                _ => {}
            }
        }
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let mut ids = Vec::new();
        gather(&v, &mut ids);
        let r = self.automation_run(id)?;
        let sources = r["sources"].as_array().cloned().unwrap_or_default();
        let mut refs = Vec::new();
        for eid in ids {
            if let Some((sid, rev)) = self.db.first(
                "SELECT source_id,event_id FROM events WHERE event_id=?",
                &[text(&eid)],
                |r| (r.text(0), r.text(1)),
            )? {
                if sources.contains(&json!(sid)) {
                    refs.push(json!({"event_id":eid,"source_id":sid,"revision":self.fingerprint_stored(&rev)?}));
                }
            }
        }
        self.automation_step(id, tool, refs.len() as i64, &refs)
    }

    pub fn automation_bind_model(&self, id: &str, model: &Value) -> Result<()> {
        let mut r = self.automation_run(id)?;
        self.automation_validate(&r)?;
        r["model"] = model.clone();
        r["model_config_revision"] = json!(sha256_hex(&serde_json::to_vec(&self.model_config()?)?));
        self.db.transaction(|| self.automation_record(&r))
    }
}
