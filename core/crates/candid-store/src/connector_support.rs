//! Store-side helpers for ingest connectors that live outside this crate (`candid-connectors`).
//! Connectors still write only through [`TimelineStore::ingest`]; these calls read state or change source settings.
use crate::db::{int, opt_text, text, RowExt};
use crate::{json, IngestEvent, IngestText, Result, SourcePolicy, TimelineError, TimelineStore};
use std::collections::HashMap;
use serde::Serialize;
use serde_json::Value;

/// One connector source row, including its connector configuration (never a secret, only `secret_ref`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConnectorSource {
    pub source_id: String,
    pub kind: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    pub config: Value,
    pub policy: SourcePolicy,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub events: i64,
}

impl TimelineStore {
    pub fn connector_sources(&self, kinds: &[&str]) -> Result<Vec<ConnectorSource>> {
        let mut all = self.db.query(
            "SELECT source_id,kind,display_name,account_ref,secret_ref,config_json,policy_json,status,last_sync_ms,last_error,
               (SELECT COUNT(*) FROM events e WHERE e.source_id=s.source_id) FROM sources s ORDER BY created_ms,source_id",
            &[],
            |r| ConnectorSource {
                source_id: r.text(0),
                kind: r.text(1),
                display_name: r.text(2),
                account_ref: r.opt_text(3),
                secret_ref: r.opt_text(4),
                config: serde_json::from_str(&r.text(5)).unwrap_or(Value::Null),
                policy: serde_json::from_str(&r.text(6)).unwrap_or_default(),
                status: r.text(7),
                last_sync_ms: r.opt_int(8),
                last_error: r.opt_text(9),
                events: r.int(10),
            },
        )?;
        all.retain(|s| kinds.contains(&s.kind.as_str()));
        Ok(all)
    }

    pub fn connector_source(&self, source_id: &str) -> Result<Option<ConnectorSource>> {
        let kinds: Vec<String> = self.db.query("SELECT kind FROM sources WHERE source_id=?", &[text(source_id)], |r| r.text(0))?;
        let Some(kind) = kinds.first() else { return Ok(None) };
        Ok(self.connector_sources(&[kind.as_str()])?.into_iter().find(|s| s.source_id == source_id))
    }

    /// Replaces the connector configuration (validated by the connector). Revoked sources keep their state.
    pub fn set_source_config(&self, source_id: &str, config: &Value, display_name: Option<&str>) -> Result<()> {
        self.db.transaction(|| {
            self.db.execute(
                "UPDATE sources SET config_json=?, display_name=COALESCE(?,display_name), updated_ms=? WHERE source_id=?",
                &[text(json::canonical(config)), opt_text(display_name), int(self.clock.now_ms()), text(source_id)],
            )?;
            if self.db.changes() == 0 {
                return Err(TimelineError::UnknownSource(source_id.into()));
            }
            self.record_change("source", source_id, "update")
        })
    }

    /// Records a bounded error code without touching a revoked or paused source; returns whether the code changed,
    /// so callers report a persistent condition (e.g. missing Full Disk Access) once instead of every pass.
    pub fn note_source_error(&self, source_id: &str, code: Option<&str>) -> Result<bool> {
        let row = self.db.first("SELECT status,last_error FROM sources WHERE source_id=?", &[text(source_id)], |r| (r.text(0), r.opt_text(1)))?;
        let Some((status, last)) = row else { return Err(TimelineError::UnknownSource(source_id.into())) };
        if status == "revoked" || status == "paused" || last.as_deref() == code {
            return Ok(false);
        }
        let next = if code.is_some() { "error" } else { "active" };
        self.db.execute(
            "UPDATE sources SET status=?, last_error=?, updated_ms=? WHERE source_id=?",
            &[text(next), opt_text(code), int(self.clock.now_ms()), text(source_id)],
        )?;
        self.record_change("source", source_id, "update")?;
        Ok(true)
    }

    /// `(source_key, payload, thread source_key)` of every stored event of one kind for a source — the connector's
    /// view of what it already delivered, used to detect deletions and moves at the source.
    pub fn stored_source_events(&self, source_id: &str, kind: &str) -> Result<Vec<(String, Value, Option<String>)>> {
        self.db.query(
            "SELECT e.source_key,e.payload_json,t.source_key FROM events e LEFT JOIN threads t ON t.thread_id=e.thread_id
             WHERE e.source_id=? AND e.kind=? ORDER BY e.ingest_seq",
            &[text(source_id), text(kind)],
            |r| (r.text(0), serde_json::from_str(&r.text(1)).unwrap_or(Value::Null), r.opt_text(2)),
        )
    }

    /// Whether `(source, key)` was deleted before (a resend would be refused as tombstoned).
    pub fn source_key_tombstoned(&self, source_id: &str, source_key: &str) -> Result<bool> {
        self.is_tombstoned("event_key", &crate::ingest::tombstone_key(source_id, source_key))
    }

    /// A per-store random value kept in `store_meta`, created on first use (e.g. the salt for address hashes).
    pub fn store_secret(&self, key: &str) -> Result<String> {
        if !key.starts_with("connectors.") {
            return Err(TimelineError::InvalidInput("store secret key".into()));
        }
        if let Some(v) = self.meta_value(key)? {
            return Ok(v);
        }
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|e| TimelineError::Io(e.to_string()))?;
        let value: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.db.execute("INSERT OR IGNORE INTO store_meta(key,value) VALUES(?,?)", &[text(key), text(&value)])?;
        Ok(self.meta_value(key)?.unwrap_or(value))
    }
}

/// A stored mail event as the mail connector needs it (identity, thread, payload; never the text).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MailEventRef {
    pub event_id: String,
    pub source_id: String,
    pub source_key: String,
    pub thread_key: Option<String>,
    pub title: Option<String>,
    pub start_ms: Option<i64>,
    pub payload: Value,
}

impl TimelineStore {
    fn mail_events_where(&self, filter: &str, params: &[crate::db::P], limit: i64) -> Result<Vec<MailEventRef>> {
        let sql = format!(
            "SELECT e.event_id,e.source_id,e.source_key,t.source_key,e.title,e.start_ms,e.payload_json FROM events e
             LEFT JOIN threads t ON t.thread_id=e.thread_id WHERE e.kind='mail.message' AND {filter}
             ORDER BY COALESCE(e.start_ms,e.ingested_ms) DESC, e.ingest_seq DESC LIMIT {limit}"
        );
        self.db.query(&sql, params, |r| MailEventRef {
            event_id: r.text(0),
            source_id: r.text(1),
            source_key: r.text(2),
            thread_key: r.opt_text(3),
            title: r.opt_text(4),
            start_ms: r.opt_int(5),
            payload: serde_json::from_str(&r.text(6)).unwrap_or(Value::Null),
        })
    }

    pub fn mail_event(&self, event_id: &str) -> Result<Option<MailEventRef>> {
        Ok(self.mail_events_where("e.event_id=?", &[text(event_id)], 1)?.into_iter().next())
    }

    /// Every stored mail of one thread (thread `source_key`), newest first.
    pub fn mail_thread_events(&self, source_id: &str, thread_key: &str) -> Result<Vec<MailEventRef>> {
        self.mail_events_where("e.source_id=? AND t.source_key=?", &[text(source_id), text(thread_key)], 1000)
    }

    /// The newest mail events of a source (or of all mail sources), for lists in the App.
    pub fn mail_recent(&self, source_id: Option<&str>, limit: usize) -> Result<Vec<MailEventRef>> {
        let limit = limit.clamp(1, 500) as i64;
        match source_id {
            Some(id) => self.mail_events_where("e.source_id=?", &[text(id)], limit),
            None => self.mail_events_where("1=1", &[], limit),
        }
    }

    /// Changes the level of stored mail: the payload becomes a new revision (the old one is kept), and `Some(text)`
    /// replaces the event's text (a body added when raised, dropped when lowered) including its search chunks.
    /// All changes of one call commit together. Returns how many events changed.
    pub fn apply_mail_levels(&self, source_id: &str, changes: &[(String, Value, Option<Vec<IngestText>>)]) -> Result<usize> {
        let policy = self.source_row(source_id)?.ok_or_else(|| TimelineError::UnknownSource(source_id.into()))?.policy;
        self.db.transaction(|| {
            let now = self.clock.now_ms();
            let mut changed = 0;
            for (key, payload, texts) in changes {
                if !payload.is_object() {
                    return Err(TimelineError::InvalidInput("payload".into()));
                }
                let Some((id, old)) = self.db.first("SELECT event_id,payload_json FROM events WHERE source_id=? AND source_key=? AND kind='mail.message'", &[text(source_id), text(key)], |r| (r.text(0), r.text(1)))? else {
                    continue;
                };
                let mut previous: Value = serde_json::from_str(&old).unwrap_or_default();
                let previous_rev = previous.get("revision").and_then(|v| v.as_i64()).unwrap_or(0);
                previous.as_object_mut().map(|o| o.remove("revision"));
                if json::canonical(&previous) != json::canonical(payload) {
                    let last = self.db.scalar("SELECT COALESCE(MAX(revision),0) FROM agent_event_revisions WHERE event_id=?", &[text(&id)])?;
                    let rev = last.max(previous_rev) + 1;
                    self.db.execute("INSERT INTO agent_event_revisions VALUES(?,?,?,?)", &[text(&id), int(rev), text(old), int(now)])?;
                    let mut next = payload.clone();
                    next["revision"] = serde_json::json!(rev);
                    self.db.execute("UPDATE events SET payload_json=? WHERE event_id=?", &[text(json::canonical(&next)), text(&id)])?;
                }
                if let Some(fields) = texts {
                    let old_chunks = self.db.query("SELECT DISTINCT chunk_id FROM event_text WHERE event_id=?", &[text(&id)], |r| r.int(0))?;
                    self.db.execute("DELETE FROM event_text WHERE event_id=?", &[text(&id)])?;
                    let mut event = IngestEvent::new(key, "mail.message", None);
                    event.text = fields.clone();
                    let prepared = self.prepare(&event, &policy)?;
                    self.insert_text(&prepared, &id, source_id, now, &mut HashMap::new())?;
                    for chunk in old_chunks {
                        if self.db.scalar("SELECT COUNT(*) FROM event_text WHERE chunk_id=?", &[int(chunk)])? == 0 {
                            let t = self.db.first("SELECT text FROM text_chunks WHERE chunk_id=?", &[int(chunk)], |r| r.text(0))?.unwrap_or_default();
                            self.db.execute("INSERT INTO text_fts(text_fts,rowid,text) VALUES('delete',?,?)", &[int(chunk), text(t)])?;
                            self.db.execute("DELETE FROM text_chunks WHERE chunk_id=?", &[int(chunk)])?;
                        }
                    }
                }
                self.record_change("event", &id, "update")?;
                changed += 1;
            }
            Ok(changed)
        })
    }
}
