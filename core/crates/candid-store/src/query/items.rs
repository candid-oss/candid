//! `items`: the dates, reminders and commitments found in conversations (`crate::items`), open ones by default.

use super::core::{QErr, QResult};
use crate::db::{int, placeholders, text, P};
use crate::items::{item_from, ITEM_COLUMNS};
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemsRequest {
    /// `open` (default), `done`, `cancelled`, `expired` or `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// `event`, `reminder` or `commitment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `self` (the user promised) or `other` (waiting for them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// A name or alias: only items with this person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    /// Only items starting at or after / before this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

impl TimelineStore {
    pub fn items(&self, req: &ItemsRequest, client_id: &str) -> Result<Value> {
        let _ = self.curated_sync();
        let _ = self.people_sync();
        let _ = self.items_maintain();
        self.audited("items", client_id, req, |v: &Value| v["items"].as_array().map(|a| a.len() as i64), |ctx| -> QResult<Value> {
            if !ctx.scope.text || !ctx.scope.derived {
                return Ok(json!({"items": [], "note": "此访问方不能读取正文或整理结果"}));
            }
            let state = req.state.as_deref().unwrap_or("open");
            if !["open", "done", "cancelled", "expired", "all"].contains(&state) {
                return Err(TimelineError::InvalidInput("state 只能是 open、done、cancelled、expired、all".into()).into());
            }
            let (visible, mut params): (String, Vec<P>) = self.curated_visible_pub(ctx, "m", "message_sources", "message_id");
            let mut sql = format!("SELECT {ITEM_COLUMNS} FROM items i WHERE review<>'rejected' AND EXISTS (SELECT 1 FROM item_changes c JOIN item_sources s ON s.change_id=c.change_id JOIN messages m ON m.message_id=s.message_id WHERE c.item_id=i.item_id AND {visible})");
            if state != "all" {
                sql += " AND state=?";
                params.push(text(state));
            }
            for (column, value, allowed) in [("kind", &req.kind, &["event", "reminder", "commitment"][..]), ("owner", &req.owner, &["self", "other"][..])] {
                if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
                    if !allowed.contains(&v) {
                        return Err(TimelineError::InvalidInput(format!("{column} 取值无效")).into());
                    }
                    sql += &format!(" AND {column}=?");
                    params.push(text(v));
                }
            }
            if let Some(who) = req.who.as_deref().filter(|w| !w.trim().is_empty()) {
                let ids = self.people_matching(who)?;
                let keys = self.entity_keys(&ids)?;
                if keys.is_empty() {
                    return Err(QErr::ScopeMiss("人物".into()));
                }
                sql += &format!(" AND counterpart IN ({})", placeholders(keys.len()));
                params.extend(keys.iter().map(text));
            }
            if let Some(s) = req.start.as_deref().filter(|s| !s.is_empty()) {
                sql += " AND start_ms>=?";
                params.push(int(self.clock.parse(s, false)?));
            }
            if let Some(e) = req.end.as_deref().filter(|s| !s.is_empty()) {
                sql += " AND start_ms<?";
                params.push(int(self.clock.parse(e, true)?));
            }
            sql += " ORDER BY start_ms IS NULL, start_ms, created_ms DESC LIMIT ?";
            params.push(int(req.limit.unwrap_or(30).clamp(1, 100)));
            let rows = self.db.query(&sql, &params, item_from)?;
            let items = rows.iter().map(|r| self.item_json(r)).collect::<Result<Vec<_>>>()?;
            Ok(json!({"items": items, "now": self.clock.iso(self.clock.now_ms())}))
        })
    }
}
