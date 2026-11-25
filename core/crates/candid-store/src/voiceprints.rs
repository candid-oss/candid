//! Private versioned provenance in store_meta; no public schema or database migration.
use crate::{
    db::{text, RowExt},
    sha256_hex_file, Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};
fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}
impl TimelineStore {
    pub(crate) fn voiceprint_authorization_advance(&self, id: &str) -> Result<()> {
        self.db.execute("INSERT INTO store_meta(key,value) VALUES(?,'1') ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT)",&[text(format!("voiceprints.authorization/1:{id}"))])
    }
    pub fn voiceprint_provider(&self) -> Result<Value> {
        self.local_voice_provider("diarization")
    }
    /// The voice service assigned to `role`, accepted only on numeric loopback: it holds voiceprints and audio.
    pub(crate) fn local_voice_provider(&self, role: &str) -> Result<Value> {
        let c = self.model_config()?;
        let a = c["roles"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["role"] == role)
            .ok_or_else(|| bad("请先在模型设置中选择多人语音服务"))?;
        let p = c["providers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["provider_id"] == a["model"]["provider_id"] && p["enabled"] == true)
            .ok_or_else(|| bad("请先启用多人语音服务"))?;
        let url = p["base_url"].as_str().unwrap_or("");
        // Numeric loopback only: no DNS, redirects or external proxy for biometric management.
        let host = url
            .strip_prefix("http://127.0.0.1:")
            .or_else(|| url.strip_prefix("http://[::1]:"))
            .ok_or_else(|| bad("声纹管理只允许本机环回模型服务"))?;
        if p["destination"] != "on_device"
            || host.split('/').next().unwrap_or("").parse::<u16>().is_err()
        {
            return Err(bad("声纹管理只允许本机环回模型服务"));
        }
        Ok(p.clone())
    }
    pub fn voiceprint_source(&self, id: &str) -> Result<Value> {
        let item = self.media_item(id)?;
        if item["kind"] != "audio" || !self.media_part_ids(id)?.is_empty() {
            return Err(bad("请选择一个物理录音段或导入录音"));
        }
        if !self.media_text_allowed(id, item["source"].as_str().unwrap_or(""))? {
            return Err(TimelineError::Denied("录音处理权限已撤回".into()));
        }
        let path = self.media_path(id)?;
        let kind = match item["source"].as_str().unwrap_or("") {
            "import" => "media.import",
            "mac_microphone" => "mac.microphone",
            "mac_system_audio" => "mac.system_audio",
            "mac_region" => "mac.region",
            _ => return Err(bad("未知录音来源")),
        };
        // Only dependencies of this physical original, including migrated source references.
        // Private generations advance on actual policy changes and revocation transitions only.
        let key = format!("media:{id}");
        let permissions=self.db.query("SELECT s.source_id,s.policy_json,s.status,COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='voiceprints.authorization/1:'||s.source_id),0) FROM sources s WHERE (s.kind=? AND s.display_name='资料库') OR s.source_id IN (SELECT e.source_id FROM events e WHERE e.source_key=? UNION SELECT json_extract(e.payload_json,'$.native_legacy_source_id') FROM events e WHERE e.source_key=? UNION SELECT j.value FROM events e,json_each(e.payload_json,'$.native_legacy_sources') j WHERE e.source_key=?) ORDER BY s.source_id",&[text(kind),text(&key),text(&key),text(&key)],|r|json!({"id":r.text(0),"policy":r.text(1),"revoked":r.text(2)=="revoked","epoch":r.int(3)}))?;
        Ok(json!({"id":id,"path":path,"digest":sha256_hex_file(&path)?,"permissions":permissions}))
    }

    pub fn voiceprint_check(&self, source: &Value) -> Result<()> {
        if self.voiceprint_source(source["id"].as_str().unwrap_or(""))? != *source {
            return Err(bad("录音或来源权限已改变，请重新分析"));
        }
        Ok(())
    }
    pub fn voiceprint_save(&self, id: &str, v: &Value) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO store_meta VALUES(?,?)",
            &[
                text(format!("voiceprints.analysis/1:{id}")),
                text(v.to_string()),
            ],
        )
    }
    pub fn voiceprint_load(&self, id: &str) -> Result<Value> {
        self.db
            .first(
                "SELECT value FROM store_meta WHERE key=?",
                &[text(format!("voiceprints.analysis/1:{id}"))],
                |r| r.text(0),
            )?
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .unwrap_or_else(|| Err(bad("分析已清理，请重新分析")))
    }
    pub fn voiceprint_enrollment(&self, v: &Value) -> Result<()> {
        // Durable source/interval/model/consent audit survives transient analysis cache removal.
        let result = &v["result"];
        let entry = json!({"format":"voiceprint-enrollment/1","cleanup_required":v["cleanup_required"],"provider":v["provider"],"source":v["source"],"selection":result["source"],"models":{"voice":result["analysis"]["voice_model"],"transcription":result["analysis"]["transcription_model"]},"matching_policy":result["analysis"]["matching_policy"],"enrollments":v["enrollments"]});
        self.db.execute(
            "INSERT OR REPLACE INTO store_meta VALUES(?,?)",
            &[
                text(format!(
                    "voiceprints.enrollment/1:{}",
                    result["analysis"]["analysis_id"].as_str().unwrap_or("")
                )),
                text(entry.to_string()),
            ],
        )
    }
    pub fn voiceprint_merge_provenance(&self, from: &str, to: &str) -> Result<()> {
        let rows=self.db.query("SELECT key,value FROM store_meta WHERE key LIKE 'voiceprints.enrollment/1:%' OR key LIKE 'voiceprints.analysis/1:%'",&[],|r|(r.text(0),r.text(1)))?;
        for (key, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw)?;
            for e in v["enrollments"].as_array_mut().into_iter().flatten() {
                if e["person"]["id"] == from {
                    e["merged_from"] = json!(from);
                    e["person"]["id"] = json!(to);
                    if e["receipt"].is_object() {
                        e["receipt"]["person_id"] = json!(to);
                    }
                }
            }
            self.db.execute(
                "UPDATE store_meta SET value=? WHERE key=?",
                &[text(v.to_string()), text(key)],
            )?;
        }
        Ok(())
    }
    pub fn voiceprint_analyses(&self) -> Result<Vec<Value>> {
        self.db
            .query(
                "SELECT value FROM store_meta WHERE key LIKE 'voiceprints.analysis/1:%'",
                &[],
                |r| r.text(0),
            )?
            .into_iter()
            .map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .collect()
    }
    pub fn voiceprint_cleanup_candidates(&self) -> Result<Vec<Value>> {
        let rows=self.db.query("SELECT key,value FROM store_meta WHERE key LIKE 'voiceprints.analysis/1:%' OR key LIKE 'voiceprints.enrollment/1:%'",&[],|r|(r.text(0),r.text(1)))?;
        let mut pending = Vec::new();
        for (key, raw) in rows {
            let mut v: Value = serde_json::from_str(&raw)?;
            if v["invalidated"] == true {
                continue;
            }
            let invalid =
                v["cleanup_required"] == true || self.voiceprint_check(&v["source"]).is_err();
            if invalid {
                v["cleanup_key"] = json!(key);
                pending.push(v);
            }
        }
        Ok(pending)
    }
    pub fn voiceprint_cleanup_done(&self, v: &Value) -> Result<()> {
        let key = v["cleanup_key"].as_str().unwrap_or("");
        if key.starts_with("voiceprints.analysis/1:") {
            return self
                .db
                .execute("DELETE FROM store_meta WHERE key=?", &[text(key)]);
        }
        let mut audit = v.clone();
        audit["invalidated"] = json!(true);
        audit.as_object_mut().unwrap().remove("cleanup_key");
        self.db.execute(
            "UPDATE store_meta SET value=? WHERE key=?",
            &[text(audit.to_string()), text(key)],
        )
    }
    pub fn voiceprint_forget(&self, id: &str) -> Result<()> {
        self.db.execute(
            "DELETE FROM store_meta WHERE key=?",
            &[text(format!("voiceprints.analysis/1:{id}"))],
        )
    }
}
