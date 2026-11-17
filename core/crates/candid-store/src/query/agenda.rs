//! The AI page's 「今天」 column (agent-platform-v1 §4.2 ai-settings, §4.3 commitments): what waits for the user in
//! the store, and the App's decisions on it. Only the App calls these (the service refuses tokens); calendar and
//! reminder items themselves are read by the App through EventKit and never stored.
//!
//! | Action | What it does |
//! |---|---|
//! | `today` | open proposals (pending, approved, failed), proposals decided since `since_ms`, commitment candidates, latest day review |
//! | `proposal` | `approve` / `reject` a proposal, then `executed` (with the system's receipt) or `failed` (with the error) |
//! | `commitment` | `confirm` (a new confirmed revision plus a `create_reminder` proposal) or `ignore` a commitment candidate |
//!
//! Proposal states move `pending → approved → executed | failed`, `failed → approved` (retry) and
//! `pending | approved | failed → rejected`. Repeating a decision that already holds returns the proposal unchanged.

use crate::db::{int, text, RowExt};
use crate::ids::uuid_v7;
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use serde_json::{json, Value};

fn bad(message: &str) -> TimelineError {
    TimelineError::InvalidInput(message.into())
}

/// Kinds the App can carry out after approval; anything else stays pending for another executor.
pub const AGENDA_EXECUTABLE_KINDS: [&str; 7] = [
    "create_reminder", "update_reminder", "complete_reminder", "delete_reminder",
    "create_calendar_event", "update_calendar_event", "delete_calendar_event",
];

impl TimelineStore {
    /// Serves `POST /local/agenda`. `app_client` is the App's own (internal) client, which files the proposals made
    /// from confirmed commitments.
    pub fn agenda_command(&self, v: &Value, app_client: &str) -> Result<Value> {
        match v["action"].as_str().unwrap_or("today") {
            "today" => self.agenda_today(v["since_ms"].as_i64()),
            "proposal" => self.agenda_proposal(v),
            "commitment" => self.agenda_commitment(v, app_client),
            _ => Err(bad("未知操作")),
        }
    }

    fn proposal_json(&self, id: &str) -> Result<Option<Value>> {
        self.db.first(
            "SELECT p.proposal_id,p.client_id,COALESCE(c.name,''),p.kind,p.payload_json,p.evidence_json,p.status,p.created_ms,p.decided_ms,p.executed_ms,p.result_json
             FROM proposals p LEFT JOIN clients c USING(client_id) WHERE p.proposal_id=?",
            &[text(id)],
            |r| (r.text(0), r.text(1), r.text(2), r.text(3), r.text(4), r.text(5), r.text(6), r.int(7), r.opt_int(8), r.opt_int(9), r.opt_text(10)),
        )?
        .map(|(id, client, name, kind, payload, evidence, status, created, decided, executed, result)| {
            Ok(json!({
                "proposal_id": id, "client_id": client, "client_name": name, "kind": kind,
                "payload": serde_json::from_str::<Value>(&payload)?, "evidence": serde_json::from_str::<Value>(&evidence)?,
                "status": status, "created": self.clock.iso(created),
                "decided": decided.map(|t| self.clock.iso(t)), "executed": executed.map(|t| self.clock.iso(t)),
                "result": result.map(|r| serde_json::from_str::<Value>(&r)).transpose()?,
            }))
        })
        .transpose()
    }

    fn agenda_today(&self, since: Option<i64>) -> Result<Value> {
        let since = since.unwrap_or_else(|| self.clock.now_ms() - 86_400_000);
        let ids = self.db.query(
            "SELECT proposal_id FROM proposals WHERE status IN ('pending','approved','failed')
                OR (status IN ('executed','rejected') AND COALESCE(executed_ms,decided_ms,created_ms)>=?)
             ORDER BY created_ms DESC LIMIT 100",
            &[int(since)],
            |r| r.text(0),
        )?;
        let mut proposals = vec![];
        for id in ids {
            if let Some(p) = self.proposal_json(&id)? {
                proposals.push(p);
            }
        }
        let rows = self.db.query(
            "SELECT m.memory_id,m.rev,m.text,m.valid_to_ms,m.producer,m.model_ref,m.confidence,m.created_ms FROM memories m
             WHERE m.kind='commitment' AND m.status='candidate' AND m.rev=(SELECT MAX(rev) FROM memories x WHERE x.memory_id=m.memory_id)
             ORDER BY m.created_ms DESC LIMIT 30",
            &[],
            |r| (r.text(0), r.int(1), r.text(2), r.opt_int(3), r.text(4), r.opt_text(5), r.get::<_, Option<f64>>(6).ok().flatten(), r.int(7)),
        )?;
        let mut commitments = vec![];
        for (id, rev, body, due, producer, model, confidence, created) in rows {
            // Where it came from: the first event it rests on (an episode's own evidence is not unfolded here).
            let source = self.db.first(
                "SELECT e.event_id,e.kind,e.start_ms,e.title FROM memory_sources s JOIN events e ON e.event_id=s.ref_id
                 WHERE s.memory_id=? AND s.rev=? AND s.ref_type='event' ORDER BY e.start_ms LIMIT 1",
                &[text(&id), int(rev)],
                |r| json!({"event_id": r.text(0), "kind": r.text(1), "start": r.opt_int(2).map(|t| self.clock.iso(t)), "title": r.opt_text(3)}),
            )?;
            commitments.push(json!({
                "memory_id": id, "rev": rev, "text": body, "due": due.map(|t| self.clock.iso(t)), "producer": producer,
                "model_ref": model, "confidence": confidence, "created": self.clock.iso(created), "source": source,
            }));
        }
        let review = self.db.first(
            "SELECT digest_id,rev,status,producer,model_ref,start_ms,created_ms,body_json FROM digests
             WHERE level='day' AND status IN ('draft','confirmed','edited') ORDER BY start_ms DESC, created_ms DESC LIMIT 1",
            &[],
            |r| (r.text(0), r.int(1), r.text(2), r.text(3), r.opt_text(4), r.int(5), r.int(6), r.text(7)),
        )?;
        let review = match review {
            None => Value::Null,
            Some((id, rev, status, producer, model, start, created, body)) => {
                let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let text = body["text"].as_str().or(body["summary"].as_str()).unwrap_or("").to_string();
                json!({"digest_id": id, "rev": rev, "status": status, "producer": producer, "model_ref": model,
                       "start": self.clock.iso(start), "created": self.clock.iso(created), "text": text,
                       "citations": body["citations"].clone()})
            }
        };
        Ok(json!({"proposals": proposals, "commitments": commitments, "review": review}))
    }

    fn agenda_proposal(&self, v: &Value) -> Result<Value> {
        let id = v["proposal_id"].as_str().ok_or_else(|| bad("缺少 proposal_id"))?;
        let decision = v["decision"].as_str().unwrap_or("");
        let (kind, status) = self
            .db
            .first("SELECT kind,status FROM proposals WHERE proposal_id=?", &[text(id)], |r| (r.text(0), r.text(1)))?
            .ok_or_else(|| TimelineError::NotFound("提案".into()))?;
        let now = self.clock.now_ms();
        let target = match decision {
            "approve" => "approved",
            "reject" => "rejected",
            "executed" => "executed",
            "failed" => "failed",
            _ => return Err(bad("decision 只能是 approve、reject、executed、failed")),
        };
        if status == target {
            return self.proposal_json(id).map(|p| p.unwrap_or(Value::Null));
        }
        let allowed = match target {
            "approved" => ["pending", "failed"].contains(&status.as_str()) && AGENDA_EXECUTABLE_KINDS.contains(&kind.as_str()),
            "rejected" => ["pending", "approved", "failed"].contains(&status.as_str()),
            _ => status == "approved",
        };
        if !allowed {
            return Err(TimelineError::InvalidInput(format!("提案当前是 {status}，不能改为 {target}")));
        }
        self.db.transaction(|| {
            match target {
                "approved" | "rejected" => self.db.execute(
                    "UPDATE proposals SET status=?,decided_ms=?,result_json=NULL WHERE proposal_id=?",
                    &[text(target), int(now), text(id)],
                )?,
                "executed" => {
                    let receipt = &v["receipt"];
                    if !receipt.is_object() || receipt["item_id"].as_str().is_none_or(str::is_empty) {
                        return Err(bad("执行回执需要 receipt.item_id"));
                    }
                    self.db.execute(
                        "UPDATE proposals SET status='executed',executed_ms=?,result_json=? WHERE proposal_id=?",
                        &[int(now), text(json!({"receipt": receipt}).to_string()), text(id)],
                    )?
                }
                _ => {
                    let error = v["error"].as_str().unwrap_or("执行失败").chars().take(500).collect::<String>();
                    self.db.execute(
                        "UPDATE proposals SET status='failed',result_json=? WHERE proposal_id=?",
                        &[text(json!({"error": error}).to_string()), text(id)],
                    )?
                }
            }
            self.record_change("proposal", id, "update")
        })?;
        self.proposal_json(id).map(|p| p.unwrap_or(Value::Null))
    }

    fn agenda_commitment(&self, v: &Value, app_client: &str) -> Result<Value> {
        let id = v["memory_id"].as_str().ok_or_else(|| bad("缺少 memory_id"))?;
        let decision = v["decision"].as_str().unwrap_or("");
        let status = match decision {
            "confirm" => "confirmed",
            "ignore" => "rejected",
            _ => return Err(bad("decision 只能是 confirm 或 ignore")),
        };
        let rev = self.memory_latest(id)?;
        let mut row = self.memory_row(id, rev)?.ok_or_else(|| TimelineError::NotFound("承诺".into()))?;
        if row.kind != "commitment" {
            return Err(bad("不是承诺"));
        }
        let existing = || -> Result<Option<String>> {
            self.db.first(
                "SELECT proposal_id FROM proposals WHERE kind='create_reminder' AND json_extract(payload_json,'$.memory_id')=? ORDER BY created_ms LIMIT 1",
                &[text(id)],
                |r| r.text(0),
            )
        };
        if row.status == status {
            // A repeated decision (a retry after a lost answer) returns what the first one made.
            let proposal = match existing()? {
                Some(p) => self.proposal_json(&p)?,
                None => None,
            };
            return Ok(json!({"memory_id": id, "status": status, "proposal": proposal}));
        }
        if row.status != "candidate" {
            return Err(TimelineError::InvalidInput(format!("承诺当前是 {}，不能再处理", row.status)));
        }
        row.status = status.into();
        let now = self.clock.now_ms();
        let value = json!({"decision": decision, "from_rev": rev, "by": {"client": app_client}});
        let proposal_id = self.db.transaction(|| {
            self.memory_insert(id, rev + 1, &row, Some(rev), &[])?;
            self.correction("memory", id, if status == "confirmed" { "confirm" } else { "reject" }, &value)?;
            if status != "confirmed" {
                return Ok(None);
            }
            if let Some(p) = existing()? {
                return Ok(Some(p));
            }
            let mut payload = json!({"title": row.text, "memory_id": id, "notes": "来自承诺候选"});
            if let Some(due) = v["due"].as_str().map(String::from).or_else(|| row.valid_to.map(|t| self.clock.iso(t))) {
                payload["due"] = json!(due);
            }
            let evidence = self.db.query(
                "SELECT DISTINCT ref_id FROM memory_sources WHERE memory_id=? AND rev=? AND ref_type='event' ORDER BY ref_id",
                &[text(id), int(rev)],
                |r| r.text(0),
            )?;
            let pid = uuid_v7(Some(now));
            self.db.execute(
                "INSERT INTO proposals(proposal_id,client_id,kind,payload_json,evidence_json,status,created_ms) VALUES(?,?,'create_reminder',?,?,'pending',?)",
                &[text(&pid), text(app_client), text(crate::json::canonical(&payload)), text(crate::json::canonical_of(&evidence)), int(now)],
            )?;
            self.record_change("proposal", &pid, "insert")?;
            Ok(Some(pid))
        })?;
        let proposal = match proposal_id {
            Some(p) => self.proposal_json(&p)?,
            None => None,
        };
        Ok(json!({"memory_id": id, "status": status, "proposal": proposal}))
    }
}
