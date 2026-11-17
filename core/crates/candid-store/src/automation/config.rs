use super::*;

impl TimelineStore {
    pub fn skill_catalog(&self) -> Result<Vec<Value>> {
        let c: Value = serde_json::from_str(crate::workflows::CATALOG)?;
        Ok(c["features"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["mode"] == "agent")
            .cloned()
            .collect())
    }

    pub fn automation_config(&self) -> Result<AutomationConfig> {
        let v = self.automation_get("automation.config/1")?;
        let mut c: AutomationConfig = if v.is_null() {
            AutomationConfig {
                version: 1,
                skills: BTreeMap::new(),
                remote_sources: vec![],
            }
        } else {
            serde_json::from_value(v)?
        };
        if c.version != 1 {
            return Err(bad("不支持的技能设置版本"));
        }
        for f in self.skill_catalog()? {
            let id = f["id"].as_str().unwrap();
            c.skills.entry(id.into()).or_insert_with(|| {
                let mut p = SkillPolicy::default();
                if id == "daily-review" {
                    p.trigger = "daily".into();
                }
                p
            });
        }
        let passed = self.automation_quality()?["passed"] == true;
        for (id, p) in &mut c.skills {
            if !p.automatic_configured {
                p.automatic = id == "daily-review" && passed;
            }
        }
        Ok(c)
    }

    pub(super) fn automation_save_config(&self, v: &Value) -> Result<Value> {
        let mut c: AutomationConfig =
            serde_json::from_value(v.clone()).map_err(|_| bad("技能设置字段无效"))?;
        let previous = self.automation_config()?;
        for (id, p) in &mut c.skills {
            if let Some(old) = previous.skills.get(id) {
                p.automatic_configured |= old.automatic_configured || p.automatic != old.automatic;
            }
        }
        let ids: Vec<String> = self
            .skill_catalog()?
            .iter()
            .filter_map(|f| f["id"].as_str().map(str::to_string))
            .collect();
        if c.version != 1 || c.skills.keys().any(|id| !ids.contains(id)) {
            return Err(bad("不支持的技能设置版本或技能"));
        }
        let sources = self.sources()?;
        let known = |s: &String| s == "*" || sources.iter().any(|v| v.source_id == *s);
        if c.remote_sources.iter().any(|s| s == "*" || !known(s)) {
            return Err(bad("远程来源范围无效"));
        }
        for p in c.skills.values() {
            if !["manual", "daily", "recording_sealed", "episode_ended"]
                .contains(&p.trigger.as_str())
                || chrono::NaiveTime::parse_from_str(&p.time, "%H:%M").is_err()
                || crate::Zone::named(&p.timezone).is_none()
                || p.sources.iter().any(|s| !known(s))
                || !["agent_chat", "summary", "episode", "title"].contains(&p.role.as_str())
                || p.daily_runs > 100
                || !(1..=200).contains(&p.max_model_calls)
                || !(1..=500).contains(&p.max_tool_calls)
                || !(256..=400000).contains(&p.max_output_tokens)
            {
                return Err(bad("技能设置取值无效"));
            }
        }
        self.db.transaction(|| {
            self.automation_put("automation.config/1", &serde_json::to_value(&c)?)?;
            // Config changes invalidate frozen runs; cancellation never expands a live run's scope.
            let runs = self.automation_rows(1000)?;
            for r in runs {
                if ["queued", "running"].contains(&r["state"].as_str().unwrap_or(""))
                    && self.automation_validate(&r).is_err()
                {
                    self.automation_end_inner(
                        &r,
                        "cancelled",
                        Some("configuration_changed"),
                        None,
                    )?;
                }
            }
            Ok(())
        })?;
        Ok(serde_json::to_value(self.automation_config()?)?)
    }

    pub(super) fn policy_fingerprint(&self, id: &str, c: &AutomationConfig) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(&json!([
            c.skills.get(id),
            c.remote_sources
        ]))?))
    }

    pub(crate) fn automation_scope(&self, p: &SkillPolicy) -> Result<(Vec<String>, Value)> {
        let all = self.sources()?;
        let chosen: Vec<_> = all
            .into_iter()
            .filter(|s| {
                p.sources.iter().any(|id| id == "*" || *id == s.source_id)
                    && s.status != "revoked"
                    && s.policy.agent_visible
                    && s.policy.store_text
            })
            .collect();
        let ids = chosen.iter().map(|s| s.source_id.clone()).collect();
        let stamp = json!(chosen
            .iter()
            .map(|s| json!([s.source_id, s.policy]))
            .collect::<Vec<_>>());
        Ok((ids, stamp))
    }

    pub fn automation_quality(&self) -> Result<Value> {
        let q = self.automation_get("automation.quality/1")?;
        let valid = q["store_id"] == self.identity.store_id
            && q["pipeline"] == PIPELINE
            && q["queries"].as_u64().unwrap_or(0) >= 30
            && q["retrieved_relevant"].as_f64().unwrap_or(0.)
                / q["relevant"].as_f64().unwrap_or(1.).max(1.)
                >= 0.8
            && q["noise_correct"].as_f64().unwrap_or(0.)
                / q["noise_checked"].as_f64().unwrap_or(1.).max(1.)
                >= 0.9
            && q["summary_checked"].as_u64().unwrap_or(0) > 0
            && q["summary_errors"] == 0
            && q["grounded"] == true
            && q["policy_revision"] == self.automation_policy_revision()?;
        Ok(json!({"passed":valid,"report":q}))
    }

    pub(super) fn automation_policy_revision(&self) -> Result<String> {
        let source: Vec<_> = self
            .sources()?
            .into_iter()
            .map(|s| json!([s.source_id, s.kind, s.status, s.policy]))
            .collect();
        Ok(sha256_hex(&serde_json::to_vec(&json!([
            source,
            self.model_config()?,
            PIPELINE
        ]))?))
    }
}
