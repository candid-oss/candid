//! Per-feature controls reuse store_meta; source policy checks in workflows remain authoritative.
use crate::{
    db::{int, text, RowExt},
    Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};

impl TimelineStore {
    pub fn feature_settings(&self) -> Result<Value> {
        let saved = self
            .db
            .first(
                "SELECT value FROM store_meta WHERE key='features.settings/1'",
                &[],
                |r| r.text(0),
            )?
            .map(|s| serde_json::from_str::<Value>(&s))
            .transpose()?
            .unwrap_or(json!({"version":1,"items":{}}));
        if saved["version"] != 1 || !saved["items"].is_object() {
            return Err(TimelineError::InvalidInput("不支持的 AI 功能设置版本".into()));
        }
        let mut items = saved["items"].as_object().cloned().unwrap_or_default();
        let catalog: Value = serde_json::from_str(crate::workflows::CATALOG)?;
        for f in catalog["features"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["implemented"] == true && f["mode"] != "agent")
        {
            items.entry(f["id"].as_str().unwrap().to_string()).or_insert(json!({
                "enabled":f["default_enabled"],"sources":f["sources"],"allow_remote":f["allow_remote"],
                "daily_calls":f["default_daily_calls"]
            }));
        }
        Ok(json!({"version":1,"items":items}))
    }

    pub fn save_feature_settings(&self, config: &Value) -> Result<Value> {
        let invalid = || TimelineError::InvalidInput("AI 功能设置无效或版本不支持".into());
        if config["version"] != 1
            || config.as_object().is_none_or(|m| {
                m.keys()
                    .any(|k| !["version", "items"].contains(&k.as_str()))
            })
        {
            return Err(invalid());
        }
        let items = config["items"].as_object().ok_or_else(invalid)?;
        let catalog: Value = serde_json::from_str(crate::workflows::CATALOG)?;
        for (id, p) in items {
            if !catalog["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["id"] == *id && f["implemented"] == true && f["mode"] != "agent")
            {
                return Err(invalid());
            }
            if p.as_object().is_none_or(|m| {
                m.len() != 4
                    || m.keys().any(|k| {
                        !["enabled", "sources", "allow_remote", "daily_calls"].contains(&k.as_str())
                    })
            }) || !p["enabled"].is_boolean()
                || !p["allow_remote"].is_boolean()
                || p["daily_calls"].as_u64().is_none_or(|n| n > 10000)
                || p["sources"].as_array().is_none_or(|a| {
                    a.iter().any(|v| {
                        ![
                            json!("import"),
                            json!("mac_microphone"),
                            json!("mac_system_audio"),
                            json!("mac_region"),
                        ]
                        .contains(v) && !((id == "screen-episodes" || id == crate::screen_excerpt::FEATURE || id == crate::item_extract::FEATURE) && (v == "*" || self.sources().map(|s|s.iter().any(|s|json!(s.source_id)==*v)).unwrap_or(false)))
                    })
                })
            {
                return Err(invalid());
            }
        }
        let previous_screen_enabled = self.feature_settings()?["items"]["screen-episodes"]["enabled"] == true;
        // Save and cancel pending work together. Late replies are rejected by the existing lease/revision check.
        self.db.transaction(|| {
            self.db.execute("INSERT OR REPLACE INTO store_meta VALUES('features.settings/1',?)", &[text(config.to_string())])?;
            if !previous_screen_enabled && config["items"]["screen-episodes"]["enabled"] == true {
                self.automation_put("screen.automatic-since/1", &json!(self.clock.now_ms()))?;
            }
            for f in catalog["features"].as_array().unwrap().iter().filter(|f| f["implemented"] == true && f["mode"] != "agent") {
                let id = f["id"].as_str().unwrap();
                if items.get(id).is_some_and(|p| p["enabled"] == false) {
                    self.db.execute("UPDATE jobs SET state='cancelled',lease_until_ms=NULL,error_code='feature_disabled' WHERE kind=? AND state IN ('queued','running')", &[text(f["job_kind"].as_str().unwrap())])?;
                } else if id != "screen-episodes" {
                    self.db.execute("UPDATE jobs SET state='queued',error_code=NULL,updated_ms=? WHERE kind=? AND state='cancelled' AND error_code='feature_disabled'", &[int(self.clock.now_ms()),text(f["job_kind"].as_str().unwrap())])?;
                }
            }
            Ok(())
        })?;
        self.feature_settings()
    }

    pub(crate) fn feature_source_allowed(&self, step: &str, item: &Value) -> bool {
        self.feature_check(step, item, None).is_ok()
    }

    /// `(calls today, daily limit)` for a catalog feature. Every model call counts, local or remote, failed or not;
    /// the day is the UTC day, like the money budget. This is the one place daily limits are decided.
    pub fn feature_budget(&self, feature: &str) -> Result<(i64, i64)> {
        let now = self.clock.now_ms();
        let used = self.db.scalar("SELECT COUNT(*) FROM ai_calls WHERE feature=? AND started_ms>=?", &[text(feature), int(now - now.rem_euclid(86_400_000))])?;
        let limit = self.feature_settings()?["items"][feature]["daily_calls"].as_i64().unwrap_or(100);
        Ok((used, limit))
    }

    pub(crate) fn feature_check(
        &self,
        step: &str,
        item: &Value,
        provider: Option<&Value>,
    ) -> Result<()> {
        let settings = self.feature_settings()?;
        let p = &settings["items"][step];
        if p.is_null() {
            return Ok(());
        }
        let denied = |reason: &str| TimelineError::Denied(reason.into());
        if p["enabled"] != true {
            return Err(denied("feature_disabled"));
        }
        let sources = p["sources"].as_array().unwrap();
        // Day aggregates contain every contributing source; an empty allowed set never grants access.
        let id = item["id"].as_str().unwrap_or("");
        if id.starts_with("workflow-day:") {
            let raw = self.db.first(
                "SELECT value FROM store_meta WHERE key=?",
                &[text(format!("workflow.sources/1:{id}"))],
                |r| r.text(0),
            )?;
            let refs: Value = raw
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or(json!([]));
            if refs.as_array().is_none_or(|a| a.is_empty()) {
                return Err(denied("waiting_feature_source"));
            }
            for reference in refs.as_array().unwrap() {
                let media = self.media_item(reference.as_str().unwrap_or(""))?;
                if !sources.contains(&media["source"]) {
                    return Err(denied("waiting_feature_source"));
                }
            }
        } else if !sources.contains(&json!("*")) && !sources.contains(&item["source"]) {
            return Err(denied("waiting_feature_source"));
        }
        if let Some(provider) = provider {
            let url = provider["base_url"].as_str().unwrap_or("");
            let authority = url
                .strip_prefix("http://")
                .or_else(|| url.strip_prefix("https://"))
                .unwrap_or("")
                .split('/')
                .next()
                .unwrap_or("");
            let local = ["localhost", "127.0.0.1", "[::1]"].iter().any(|h| {
                authority == *h
                    || authority.strip_prefix(h).is_some_and(|port| {
                        port.starts_with(':') && port[1..].parse::<u16>().is_ok()
                    })
            });
            if p["allow_remote"] != true && !local {
                return Err(denied("waiting_feature_remote"));
            }
            let (used, limit) = self.feature_budget(step)?;
            if used >= limit {
                return Err(denied("waiting_feature_budget"));
            }
        }
        Ok(())
    }
}

/// URL authority equality, never a host-prefix match.
pub(crate) fn is_loopback(url:&str)->bool {
    let authority=url.strip_prefix("http://").or_else(||url.strip_prefix("https://")).unwrap_or("").split('/').next().unwrap_or("");
    ["localhost","127.0.0.1","[::1]"].iter().any(|h|authority==*h||authority.strip_prefix(h).is_some_and(|p|p.starts_with(':')&&p[1..].parse::<u16>().is_ok()))
}
