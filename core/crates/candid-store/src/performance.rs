//! Internal query accelerators and transactional invalidation for background work.
//! Revisions contain no user content; permission checks still run when work executes.
use crate::{db::{int, text, Db, RowExt}, Result, TimelineStore};
use serde_json::{json, Value};

pub(crate) fn install(db: &Db) -> Result<()> {
    db.script("CREATE INDEX events_source_key ON events(source_key);\n\
        CREATE INDEX jobs_pending ON jobs(state,next_run_ms,created_ms) WHERE state IN ('queued','running');\n\
        CREATE INDEX media_parts_recording ON media_parts(recording_id);\n\
        CREATE TABLE background_revisions(domain TEXT PRIMARY KEY,revision INTEGER NOT NULL) STRICT;\n\
        INSERT INTO background_revisions VALUES('jobs',0),('workflows',0);")?;
    for (domain, tables) in [
        ("jobs", & ["jobs", "job_checkpoints"][..]),
        ("workflows", & ["media_objects", "media_parts", "media_edits", "media_deleted", "media_recognitions", "corrections", "digests"][..]),
    ] {
        for table in tables {
            for action in ["INSERT", "UPDATE", "DELETE"] {
                db.script(&format!("CREATE TRIGGER background_{table}_{action} AFTER {action} ON {table} BEGIN UPDATE background_revisions SET revision=revision+1 WHERE domain='{domain}'; END;"))?;
            }
        }
    }
    // Sync timestamps do not change authorization. A policy/status change must invalidate immediately.
    for (action, condition) in [("INSERT", ""), ("DELETE", ""), ("UPDATE", "WHEN OLD.policy_json IS NOT NEW.policy_json OR OLD.status IS NOT NEW.status OR OLD.kind IS NOT NEW.kind OR OLD.display_name IS NOT NEW.display_name")] {
        db.script(&format!("CREATE TRIGGER background_sources_{action} AFTER {action} ON sources {condition} BEGIN UPDATE background_revisions SET revision=revision+1 WHERE domain='workflows'; END;"))?;
    }
    for (action, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
        let changed = if action == "UPDATE" { "AND OLD.value IS NOT NEW.value" } else { "" };
        db.script(&format!("CREATE TRIGGER background_meta_{action} AFTER {action} ON store_meta WHEN ({row}.key GLOB 'models.*' OR {row}.key GLOB 'workflows.*' OR {row}.key GLOB 'voice*' OR {row}.key IN ('features.settings/1','automation.config/1','automation.quality/1')) {changed} BEGIN UPDATE background_revisions SET revision=revision+1 WHERE domain='workflows'; END;"))?;
    }
    Ok(())
}

/// Media projection depends on media evidence and its grants, not unrelated connector health or models.
pub(crate) fn install_media_projection(db: &Db) -> Result<()> {
    db.script("INSERT INTO background_revisions VALUES('media_projection',0);
        CREATE INDEX events_projection_owner ON events(json_extract(payload_json,'$.native_projection_owner')) WHERE kind='speech.segment';")?;
    for table in ["media_objects", "media_parts", "media_edits", "media_deleted", "media_recognitions", "corrections", "digests"] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            db.script(&format!("CREATE TRIGGER projection_{table}_{action} AFTER {action} ON {table} BEGIN UPDATE background_revisions SET revision=revision+1 WHERE domain='media_projection'; END;"))?;
        }
    }
    let media = |row: &str| format!("{row}.kind IN ('media.import','mac.microphone','mac.system_audio','mac.region')");
    for (action, condition) in [
        ("INSERT", media("NEW")),
        ("DELETE", media("OLD")),
        ("UPDATE", format!("({} OR {}) AND (OLD.policy_json IS NOT NEW.policy_json OR OLD.status IS NOT NEW.status OR OLD.kind IS NOT NEW.kind OR OLD.display_name IS NOT NEW.display_name)", media("OLD"), media("NEW"))),
    ] {
        db.script(&format!("CREATE TRIGGER projection_sources_{action} AFTER {action} ON sources WHEN {condition} BEGIN UPDATE background_revisions SET revision=revision+1 WHERE domain='media_projection'; END;"))?;
    }
    for (action, row) in [("INSERT", "NEW"), ("UPDATE", "NEW"), ("DELETE", "OLD")] {
        let changed = if action == "UPDATE" { "AND OLD.value IS NOT NEW.value" } else { "" };
        db.script(&format!("CREATE TRIGGER projection_meta_{action} AFTER {action} ON store_meta WHEN {row}.key GLOB 'voice*' {changed} BEGIN UPDATE background_revisions SET revision=revision+1 WHERE domain='media_projection'; END;"))?;
    }
    Ok(())
}

impl TimelineStore {
    pub fn background_data_revision(&self) -> Result<i64> { self.current_seq() }

    pub fn background_revision(&self, domain: &str) -> Result<i64> {
        self.db.scalar("SELECT revision FROM background_revisions WHERE domain=?", &[text(domain)])
    }

    /// Internal worker input: only due queued work and expired leases, never historical job details.
    pub fn pending_workflow_jobs(&self) -> Result<Value> {
        let now = self.clock.now_ms();
        let ids = self.db.query("SELECT job_id FROM jobs INDEXED BY jobs_pending WHERE state IN ('queued','running') AND ((state='queued' AND (next_run_ms IS NULL OR next_run_ms<=?)) OR (state='running' AND lease_until_ms<=?)) AND (kind='speech.recognize' OR kind='screen.summarize' OR kind GLOB 'workflow.*') ORDER BY created_ms,job_id", &[int(now), int(now)], |r| r.text(0))?;
        let jobs = ids.iter().map(|id| self.job_command(&json!({"action":"get","id":id}))).collect::<Result<Vec<_>>>()?;
        Ok(json!({"jobs":jobs}))
    }

    pub fn next_workflow_deadline(&self) -> Result<Option<i64>> {
        self.db.first("SELECT MIN(CASE state WHEN 'queued' THEN next_run_ms ELSE lease_until_ms END) FROM jobs INDEXED BY jobs_pending WHERE state IN ('queued','running') AND (kind='speech.recognize' OR kind='screen.summarize' OR kind GLOB 'workflow.*') AND CASE state WHEN 'queued' THEN next_run_ms ELSE lease_until_ms END>?", &[int(self.clock.now_ms())], |r| r.opt_int(0)).map(Option::flatten)
    }
}
