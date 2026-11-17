//! Rebuild from persisted observations, retaining app/window/gap boundaries.
use super::*;
use crate::db::{text, RowExt};
use crate::{IngestText, TimelineStore};
impl TimelineStore {
    /// Internal management API. The service must apply its client authorization
    /// and audit before exposing a revision; revoked sources are refused here too.
    pub fn rebuild_records(&self, source: &str, rules: Rules) -> Result<Revision> {
        let allowed = self.db.scalar("SELECT COUNT(*) FROM sources WHERE source_id=? AND status<>'revoked' AND json_extract(policy_json,'$.store_text')=1", &[text(source)])?;
        if allowed != 1 {
            return Err(TimelineError::Denied("source_not_readable".into()));
        }
        let rows = self.db.query("SELECT event_id,source_key,kind,start_ms,COALESCE(app_bundle_id,''),title,payload_json,sensitivity,ingest_seq FROM events WHERE source_id=? AND start_ms IS NOT NULL AND kind IN ('screen.text','app.focus','window.focus','coverage.gap','system.state') ORDER BY start_ms,ingest_seq", &[text(source)], |r| (r.text(0),r.text(1),r.text(2),r.int(3),r.text(4),r.opt_text(5),r.text(6),r.text(7),r.int(8)))?;
        let mut observations = vec![];
        for (id, key, kind, at, app, title, payload, sensitivity, sequence) in rows {
            let mut event = IngestEvent::new(&key, &kind, Some(at));
            event.app_bundle_id = Some(app.clone());
            event.title = title;
            event.payload = serde_json::from_str(&payload)?;
            event.sensitivity = sensitivity;
            for (field,body) in self.db.query("SELECT t.field,c.text FROM event_text t JOIN text_chunks c USING(chunk_id) WHERE event_id=? ORDER BY t.field,t.ord", &[text(&id)], |r| (r.text(0),r.text(1)))? { event.text.push(IngestText::new(&field,&body)); }
            if let Some(mut o) = from_event(source, &event)? {
                o.sequence = sequence;
                observations.push(o);
            } else {
                observations.push(Observation {
                    source: source.into(),
                    key,
                    at_ms: at,
                    sequence,
                    app,
                    evidence: Evidence {
                        format: "candid-record-evidence/1".into(),
                        object: ObjectIdentity {
                            key: id,
                            basis: "window".into(),
                        },
                        field: None,
                        input_value: None,
                        input_status: "unavailable".into(),
                        active: false,
                        reading: vec![],
                        confirmation: None,
                    },
                });
            }
        }
        rebuild(&observations, rules)
    }
}
