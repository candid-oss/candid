//! `person`: a person card (aliases, recent exchanges, open items and memos) and the App's correction entry.
//! Open items and memos are placeholders until the `items` table exists; they are empty arrays for now.

use super::core::{ClientContext, QErr, QResult};
use crate::db::{int, placeholders, text, RowExt, P};
use crate::people::{normalize, PersonCorrectionRequest};
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonRequest {
    /// A name or alias (matched on the person's name and every alias).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// A `person_id` from an earlier result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
    /// Only exchanges before this time; page backwards with the `before` a previous result gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

impl TimelineStore {
    /// Surviving people matching an id, a name or an alias (exact first, then containing).
    pub(crate) fn people_matching(&self, who: &str) -> Result<Vec<String>> {
        let rows = self.db.query(
            "SELECT e.entity_id,e.name,a.alias_kind,a.alias FROM entities e LEFT JOIN entity_aliases a ON a.entity_id=e.entity_id WHERE e.kind='person' AND e.status IN ('candidate','confirmed')",
            &[],
            |r| (r.text(0), r.text(1), r.opt_text(2), r.opt_text(3)),
        )?;
        let who = who.trim();
        if rows.iter().any(|r| r.0 == who) {
            return Ok(vec![who.to_string()]);
        }
        let q = normalize(who);
        if q.is_empty() {
            return Ok(vec![]);
        }
        let bare = |a: &str| a.split_once('/').map(|x| x.1.to_string()).unwrap_or_else(|| a.to_string());
        let mut exact = BTreeSet::new();
        let mut partial = BTreeSet::new();
        for (id, name, _, alias) in &rows {
            let mut forms = vec![normalize(name)];
            if let Some(a) = alias {
                forms.push(normalize(a));
                forms.push(normalize(&bare(a)));
            }
            if forms.contains(&q) {
                exact.insert(id.clone());
            } else if forms.iter().any(|f| f.contains(&q)) {
                partial.insert(id.clone());
            }
        }
        Ok(if exact.is_empty() { partial } else { exact }.into_iter().collect())
    }

    /// The given people plus everyone merged into them (voice-library merges leave old ids behind).
    pub(crate) fn people_family(&self, ids: &[String]) -> Result<Vec<String>> {
        let mut out = BTreeSet::new();
        for id in ids {
            out.extend(self.db.query(
                "WITH RECURSIVE f(id) AS (SELECT ? UNION SELECT e.entity_id FROM entities e JOIN f ON e.merged_into=f.id) SELECT id FROM f",
                &[text(id)],
                |r| r.text(0),
            )?);
        }
        Ok(out.into_iter().collect())
    }

    fn person_card(&self, id: &str) -> Result<Value> {
        let (name, status) = self.db.first("SELECT name,status FROM entities WHERE entity_id=?", &[text(id)], |r| (r.text(0), r.text(1)))?.ok_or_else(|| TimelineError::NotFound("人物".into()))?;
        let aliases = self.db.query("SELECT alias_kind,alias,source FROM entity_aliases WHERE entity_id=? ORDER BY alias_kind,alias", &[text(id)], |r| {
            let (k, a) = (r.text(0), r.text(1));
            json!({"key": if k == "other" { a.clone() } else { format!("{k}:{a}") }, "kind": k, "alias": a, "source": r.text(2)})
        })?;
        // Merges the model made for you (judged the same person, with a supporting rule); undo = "not the same person".
        let auto_merged = self.people_auto_merges(id)?;
        Ok(json!({"person_id": id, "name": name, "status": status, "aliases": aliases, "auto_merged": auto_merged}))
    }

    pub fn person(&self, req: &PersonRequest, client_id: &str) -> Result<Value> {
        let _ = self.curated_sync();
        let _ = self.people_sync();
        self.audited("person", client_id, req, |v: &Value| Some(v["messages"].as_array().map(|a| a.len()).unwrap_or(0) as i64), |ctx| -> QResult<Value> {
            let ids = match (req.person_id.as_deref(), req.who.as_deref()) {
                (Some(id), _) => self.people_matching(id)?,
                (None, Some(w)) if !w.trim().is_empty() => self.people_matching(w)?,
                _ => return Err(TimelineError::InvalidInput("缺少 who 或 person_id".into()).into()),
            };
            if ids.is_empty() {
                return Err(QErr::ScopeMiss("人物".into()));
            }
            if ids.len() > 1 {
                let cards: Vec<Value> = ids.iter().filter_map(|i| self.person_card(i).ok()).collect();
                return Ok(json!({"candidates": cards, "note": "有多个人匹配；用 person_id 指定一个。"}));
            }
            let id = &ids[0];
            let mut card = self.person_card(id)?;
            card["open_items"] = json!([]);
            card["memos"] = json!([]);
            if !ctx.scope.text {
                card["messages"] = json!([]);
                card["note"] = json!("此访问方不能读取正文");
                return Ok(card);
            }
            let family = self.people_family(&ids)?;
            let limit = req.limit.unwrap_or(20).clamp(1, 100);
            let (visible, vparams) = self.curated_visible(ctx, "m", "message_sources", "message_id");
            let hold = placeholders(family.len());
            let mut params: Vec<P> = vparams;
            // Their own messages, plus yours in conversations where they are the only other person.
            let mut sql = format!(
                "SELECT m.message_id,m.conversation_id,m.sent_ms,m.actor,m.sender,m.text FROM messages m WHERE {visible} AND (m.sender_entity_id IN ({hold}) OR (m.actor='self' \
                 AND m.conversation_id IN (SELECT conversation_id FROM messages WHERE sender_entity_id IN ({hold})) \
                 AND NOT EXISTS (SELECT 1 FROM messages o WHERE o.conversation_id=m.conversation_id AND o.actor<>'self' AND (o.sender_entity_id IS NULL OR o.sender_entity_id NOT IN ({hold})))))"
            );
            for _ in 0..3 {
                params.extend(family.iter().map(text));
            }
            if let Some(b) = req.before.as_deref() {
                sql += " AND m.sent_ms<?";
                params.push(int(self.clock.parse(b, false)?));
            }
            sql += " ORDER BY m.sent_ms DESC, m.seq DESC LIMIT ?";
            params.push(int(limit + 1));
            let mut rows = self.db.query(&sql, &params, |r| (r.text(0), r.text(1), r.int(2), r.text(3), r.opt_text(4), r.text(5)))?;
            let more = rows.len() as i64 > limit;
            rows.truncate(limit as usize);
            let convs: BTreeSet<&String> = rows.iter().map(|r| &r.1).collect();
            let mut info: BTreeMap<String, Value> = BTreeMap::new();
            for c in convs {
                if let Some(v) = self.db.first("SELECT kind,app,title FROM conversations WHERE conversation_id=?", &[text(c)], |r| json!({"kind": r.text(0), "app": r.opt_text(1), "title": r.opt_text(2)}))? {
                    info.insert(c.clone(), v);
                }
            }
            let before = if more { rows.last().map(|r| self.clock.iso(r.2)) } else { None };
            let messages: Vec<Value> = rows
                .iter()
                .map(|r| {
                    let mut v = self.message_json(r);
                    v["conversation"] = info.get(&r.1).cloned().unwrap_or(Value::Null);
                    v
                })
                .collect();
            card["messages"] = json!(messages);
            card["before"] = json!(before);
            Ok(card)
        })
    }

    /// The App's correction entry: merge two people, mark two as not the same person, rename, reject or confirm.
    pub fn app_person_correct(&self, req: &PersonCorrectionRequest, client_id: &str) -> Result<Value> {
        self.audited("app_person_correct", client_id, req, |_: &Value| Some(1), |_ctx: &ClientContext| -> QResult<Value> {
            self.require_internal(client_id)?;
            Ok(self.person_correct(req)?)
        })
    }

    /// 「可能是同一人」: pairs the model called the same person that no rule backs up (merge or "not the same person"),
    /// and the automatic merges it made (undo = "not the same person"). Read from cached verdicts only.
    pub fn app_people_candidates(&self, client_id: &str) -> Result<Value> {
        self.audited("app_people_candidates", client_id, &json!({}), |v: &Value| Some(v["candidates"].as_array().map(|a| a.len()).unwrap_or(0) as i64), |_ctx: &ClientContext| -> QResult<Value> {
            self.require_internal(client_id)?;
            Ok(self.people_candidate_list()?)
        })
    }
}
