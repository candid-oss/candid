//! Query tools over the AI-readable layer (`crate::curated`): what an AI reads by default.
//!
//! | Tool | Returns |
//! |---|---|
//! | `conversations` | conversations (chat, mail, meeting, agent) with participants, counts and the last message, newest first |
//! | `conversation` | the messages of one conversation in time order |
//! | `find` | full-text hits in messages and excerpts, each with its conversation or page |
//! | `evidence` | the raw records a message or excerpt was built from (needs the client's `text` scope) |
//!
//! A row is visible when it has at least one source and the client may read every one of them; rows a model wrote
//! need the `derived` scope as well.

use super::core::{ClientContext, QResult};
use crate::curated::PROJECTION;
use crate::db::{int, text, RowExt, P};
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationsRequest {
    /// `chat`, `mail`, `meeting` or `agent` (what you wrote to an AI; the replies are not stored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Matches the conversation title or a sender name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationRequest {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Only messages before this time; page backwards with the `before` a previous result gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindRequest {
    pub q: String,
    /// Only messages of conversations of this kind (`chat`, `mail`, `meeting`, `agent`); excerpts are left out then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_kind: Option<String>,
    /// `message` or `excerpt`; both when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRequest {
    /// A `message_id` or `excerpt_id` from another result.
    pub id: String,
}

fn invalid(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

/// Escapes a user query for FTS5: every term quoted, so punctuation is literal.
fn fts_query(q: &str) -> String {
    q.split_whitespace().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" ")
}

fn like(q: &str) -> String {
    format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

impl TimelineStore {
    /// SQL that is true when row `alias` (with key column `key`) of `table` is visible to the client.
    fn curated_visible(&self, ctx: &ClientContext, alias: &str, table: &str, key: &str) -> (String, Vec<P>) {
        let f = self.scope_filter(ctx, "e");
        let mut params = f.params.clone();
        let derived = if ctx.scope.derived { "1" } else { "0" };
        params.push(text(PROJECTION));
        (
            format!(
                "(EXISTS (SELECT 1 FROM {table} s WHERE s.{key}={alias}.{key}) AND NOT EXISTS (SELECT 1 FROM {table} s JOIN events e ON e.event_id=s.event_id WHERE s.{key}={alias}.{key} AND NOT ({})) AND ({alias}.producer=? OR {derived}))",
                f.sql()
            ),
            params,
        )
    }

    fn message_json(&self, row: &(String, String, i64, String, Option<String>, String)) -> Value {
        let (id, conversation, at, actor, sender, body) = row;
        // `ref` is the record to cite, so the App can link the answer back to where it was seen.
        let cite = self.db.first("SELECT MIN(event_id) FROM message_sources WHERE message_id=?", &[text(id)], |r| r.opt_text(0)).ok().flatten().flatten();
        json!({"message_id": id, "conversation_id": conversation, "time": self.clock.iso(*at), "actor": actor, "sender": sender, "text": body, "ref": cite})
    }

    pub fn conversations(&self, req: &ConversationsRequest, client_id: &str) -> Result<Value> {
        let _ = self.curated_sync();
        self.audited("conversations", client_id, req, |v: &Value| v["conversations"].as_array().map(|a| a.len() as i64), |ctx| -> QResult<Value> {
            if !ctx.scope.text {
                return Ok(json!({"conversations": [], "note": "此访问方不能读取正文"}));
            }
            let limit = req.limit.unwrap_or(20).clamp(1, 50);
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), None, false)?;
            let (visible, vparams) = self.curated_visible(ctx, "m", "message_sources", "message_id");
            let mut sql = format!("SELECT c.conversation_id,c.kind,c.app,c.title FROM conversations c WHERE EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id=c.conversation_id AND {visible}");
            let mut params = vparams.clone();
            if let Some(s) = ws {
                sql += " AND m.sent_ms>=?";
                params.push(int(s));
            }
            if let Some(e) = we {
                sql += " AND m.sent_ms<?";
                params.push(int(e));
            }
            if let Some(who) = req.who.as_deref().filter(|s| !s.trim().is_empty()) {
                sql += " AND (c.title LIKE ? ESCAPE '\\' OR m.sender LIKE ? ESCAPE '\\')";
                params.extend([text(like(who.trim())), text(like(who.trim()))]);
            }
            sql += ")";
            if let Some(kind) = &req.kind {
                if !["chat", "mail", "meeting", "agent"].contains(&kind.as_str()) {
                    return Err(invalid("kind 只能是 chat、mail、meeting 或 agent").into());
                }
                sql += " AND c.kind=?";
                params.push(text(kind));
            }
            if let Some(app) = req.app.as_deref().filter(|s| !s.trim().is_empty()) {
                sql += " AND c.app LIKE ? ESCAPE '\\'";
                params.push(text(like(app.trim())));
            }
            sql += " ORDER BY c.last_ms DESC LIMIT ?";
            params.push(int(limit));
            let rows = self.db.query(&sql, &params, |r| (r.text(0), r.text(1), r.opt_text(2), r.opt_text(3)))?;
            let mut out = vec![];
            for (id, kind, app, title) in rows {
                let mut p = vec![text(&id)];
                p.extend(vparams.clone());
                let stats = self.db.first(
                    &format!("SELECT COUNT(*),MIN(sent_ms),MAX(sent_ms) FROM messages m WHERE m.conversation_id=? AND {visible}"),
                    &p,
                    |r| (r.int(0), r.opt_int(1), r.opt_int(2)),
                )?;
                let Some((count, Some(first), Some(last))) = stats else { continue };
                let senders = self.db.query(
                    &format!("SELECT DISTINCT m.sender FROM messages m WHERE m.conversation_id=? AND m.sender IS NOT NULL AND {visible} LIMIT 12"),
                    &p,
                    |r| r.text(0),
                )?;
                let last_message = self.db.first(
                    &format!("SELECT m.message_id,m.conversation_id,m.sent_ms,m.actor,m.sender,substr(m.text,1,200) FROM messages m WHERE m.conversation_id=? AND {visible} ORDER BY m.sent_ms DESC, m.seq DESC LIMIT 1"),
                    &p,
                    |r| (r.text(0), r.text(1), r.int(2), r.text(3), r.opt_text(4), r.text(5)),
                )?;
                out.push(json!({
                    "conversation_id": id, "kind": kind, "app": app, "title": title,
                    "first": self.clock.iso(first), "last": self.clock.iso(last), "messages": count,
                    "participants": senders, "last_message": last_message.as_ref().map(|m| self.message_json(m)),
                }));
            }
            Ok(json!({"conversations": out}))
        })
    }

    pub fn conversation(&self, req: &ConversationRequest, client_id: &str) -> Result<Value> {
        let _ = self.curated_sync();
        self.audited("conversation", client_id, req, |v: &Value| v["messages"].as_array().map(|a| a.len() as i64), |ctx| -> QResult<Value> {
            if !ctx.scope.text {
                return Ok(json!({"messages": [], "note": "此访问方不能读取正文"}));
            }
            let limit = req.limit.unwrap_or(30).clamp(1, 200);
            let head = self.db.first("SELECT kind,app,title FROM conversations WHERE conversation_id=?", &[text(&req.conversation_id)], |r| (r.text(0), r.opt_text(1), r.opt_text(2)))?;
            let Some((kind, app, title)) = head else {
                return Err(super::core::QErr::ScopeMiss("对话".into()));
            };
            let (visible, vparams) = self.curated_visible(ctx, "m", "message_sources", "message_id");
            let mut sql = format!("SELECT m.message_id,m.conversation_id,m.sent_ms,m.actor,m.sender,m.text FROM messages m WHERE m.conversation_id=? AND {visible}");
            let mut params = vec![text(&req.conversation_id)];
            params.extend(vparams);
            if let Some(s) = req.start.as_deref() {
                sql += " AND m.sent_ms>=?";
                params.push(int(self.clock.parse(s, false)?));
            }
            if let Some(b) = req.before.as_deref() {
                sql += " AND m.sent_ms<?";
                params.push(int(self.clock.parse(b, false)?));
            }
            // The latest `limit` messages, returned oldest first.
            sql += " ORDER BY m.sent_ms DESC, m.seq DESC LIMIT ?";
            params.push(int(limit + 1));
            let mut rows = self.db.query(&sql, &params, |r| (r.text(0), r.text(1), r.int(2), r.text(3), r.opt_text(4), r.text(5)))?;
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            if rows.is_empty() {
                return Err(super::core::QErr::ScopeMiss("对话".into()));
            }
            rows.reverse();
            let before = if more { rows.first().map(|r| self.clock.iso(r.2)) } else { None };
            let messages: Vec<Value> = rows.iter().map(|r| self.message_json(r)).collect();
            Ok(json!({"conversation_id": req.conversation_id, "kind": kind, "app": app, "title": title, "messages": messages, "before": before}))
        })
    }

    pub fn find(&self, req: &FindRequest, client_id: &str) -> Result<Value> {
        let _ = self.curated_sync();
        self.audited("find", client_id, req, |v: &Value| v["results"].as_array().map(|a| a.len() as i64), |ctx| -> QResult<Value> {
            if !ctx.scope.text {
                return Ok(json!({"results": [], "note": "此访问方不能读取正文"}));
            }
            let q = req.q.trim();
            if q.is_empty() {
                return Err(invalid("缺少 q").into());
            }
            let limit = req.limit.unwrap_or(10).clamp(1, 50);
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), None, false)?;
            // Trigram FTS needs three characters per term; shorter queries scan with LIKE.
            let use_fts = q.split_whitespace().all(|t| t.chars().count() >= 3);
            let mut results: Vec<(i64, Value)> = vec![];
            if let Some(k) = req.conversation_kind.as_deref() {
                if !["chat", "mail", "meeting", "agent"].contains(&k) {
                    return Err(invalid("conversation_kind 只能是 chat、mail、meeting 或 agent").into());
                }
            }
            if req.kind.as_deref().is_none_or(|k| k == "message") {
                let (visible, vparams) = self.curated_visible(ctx, "m", "message_sources", "message_id");
                let (matcher, mp) = if use_fts {
                    ("m.seq IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".to_string(), vec![text(fts_query(q))])
                } else {
                    ("m.text LIKE ? ESCAPE '\\'".to_string(), vec![text(like(q))])
                };
                let mut sql = format!("SELECT m.message_id,m.conversation_id,m.sent_ms,m.actor,m.sender,m.text,c.kind,c.app,c.title FROM messages m JOIN conversations c ON c.conversation_id=m.conversation_id WHERE {matcher} AND {visible}");
                let mut params = mp;
                params.extend(vparams);
                if let Some(k) = req.conversation_kind.as_deref() {
                    sql += " AND c.kind=?";
                    params.push(text(k));
                }
                if let Some(s) = ws {
                    sql += " AND m.sent_ms>=?";
                    params.push(int(s));
                }
                if let Some(e) = we {
                    sql += " AND m.sent_ms<?";
                    params.push(int(e));
                }
                sql += " ORDER BY m.sent_ms DESC LIMIT ?";
                params.push(int(limit));
                for (id, conv, at, actor, sender, body, kind, app, title) in self.db.query(&sql, &params, |r| (r.text(0), r.text(1), r.int(2), r.text(3), r.opt_text(4), r.text(5), r.text(6), r.opt_text(7), r.opt_text(8)))? {
                    let mut v = self.message_json(&(id, conv, at, actor, sender, body));
                    v["type"] = json!("message");
                    v["conversation"] = json!({"kind": kind, "app": app, "title": title});
                    results.push((at, v));
                }
            }
            if req.conversation_kind.is_none() && req.kind.as_deref().is_none_or(|k| k == "excerpt") {
                let (visible, vparams) = self.curated_visible(ctx, "x", "excerpt_sources", "excerpt_id");
                let (matcher, mp) = if use_fts {
                    ("x.seq IN (SELECT rowid FROM excerpts_fts WHERE excerpts_fts MATCH ?)".to_string(), vec![text(fts_query(q))])
                } else {
                    ("(x.text LIKE ? ESCAPE '\\' OR x.title LIKE ? ESCAPE '\\')".to_string(), vec![text(like(q)), text(like(q))])
                };
                let mut sql = format!("SELECT x.excerpt_id,x.app,x.title,x.url,x.start_ms,x.end_ms,x.text FROM excerpts x WHERE {matcher} AND {visible}");
                let mut params = mp;
                params.extend(vparams);
                if let Some(s) = ws {
                    sql += " AND x.end_ms>=?";
                    params.push(int(s));
                }
                if let Some(e) = we {
                    sql += " AND x.start_ms<?";
                    params.push(int(e));
                }
                sql += " ORDER BY x.start_ms DESC LIMIT ?";
                params.push(int(limit));
                for (id, app, title, url, start, end, body) in self.db.query(&sql, &params, |r| (r.text(0), r.opt_text(1), r.opt_text(2), r.opt_text(3), r.int(4), r.int(5), r.text(6)))? {
                    results.push((start, json!({"type": "excerpt", "excerpt_id": id, "app": app, "title": title, "url": url, "start": self.clock.iso(start), "end": self.clock.iso(end), "text": body})));
                }
            }
            results.sort_by(|a, b| b.0.cmp(&a.0));
            results.truncate(limit as usize);
            Ok(json!({"results": results.into_iter().map(|r| r.1).collect::<Vec<_>>()}))
        })
    }

    pub fn evidence(&self, req: &EvidenceRequest, client_id: &str) -> Result<Value> {
        self.audited("evidence", client_id, req, |v: &Value| v["records"].as_array().map(|a| a.len() as i64), |ctx| -> QResult<Value> {
            let (table, key, rows) = if req.id.starts_with("ex:") || self.db.scalar("SELECT COUNT(*) FROM excerpts WHERE excerpt_id=?", &[text(&req.id)])? > 0 {
                ("excerpt_sources", "excerpt_id", "excerpts")
            } else {
                ("message_sources", "message_id", "messages")
            };
            let alias = if rows == "messages" { "m" } else { "x" };
            let (visible, vparams) = self.curated_visible(ctx, alias, table, key);
            let mut params = vec![text(&req.id)];
            params.extend(vparams);
            if self.db.scalar(&format!("SELECT COUNT(*) FROM {rows} {alias} WHERE {alias}.{key}=? AND {visible}"), &params)? == 0 {
                return Err(super::core::QErr::ScopeMiss("记录".into()));
            }
            let ids = self.db.query(&format!("SELECT event_id FROM {table} WHERE {key}=? ORDER BY event_id"), &[text(&req.id)], |r| r.text(0))?;
            let summaries = self.summaries(&ids, ctx)?;
            let records: Vec<Value> = ids.iter().filter_map(|id| summaries.get(id)).map(|s| serde_json::to_value(s).unwrap_or(Value::Null)).collect();
            Ok(json!({"id": req.id, "records": records, "note": "原始记录；需要全文时用 get_event 展开。"}))
        })
    }
}
