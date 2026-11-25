//! `day`: what the user did in a day or a time span, as activity blocks (`crate::activity`) in time order, plus the
//! day-level summaries that already exist. The default entry for 「这天做了什么」; raw events stay behind `timeline`.

use super::core::{ClientContext, QResult};
use crate::db::{int, text, RowExt};
use crate::store::TimelineStore;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DayRequest {
    /// One day (YYYY-MM-DD, today, yesterday); shorthand for start = end = date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

impl TimelineStore {
    /// The ids of `ids` (messages or excerpts) the client may read.
    fn day_visible(&self, ctx: &ClientContext, table: &str, key: &str, rows: &str, alias: &str, ids: &[String]) -> Result<Vec<String>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let (visible, mut params) = self.curated_visible_pub(ctx, alias, table, key);
        let mut p = ids.iter().map(text).collect::<Vec<_>>();
        p.append(&mut params);
        self.db.query(&format!("SELECT {alias}.{key} FROM {rows} {alias} WHERE {alias}.{key} IN ({}) AND {visible}", crate::db::placeholders(ids.len())), &p, |r| r.text(0))
    }

    pub fn day(&self, req: &DayRequest, client_id: &str) -> Result<Value> {
        let _ = self.curated_sync();
        self.audited("day", client_id, req, |v: &Value| v["blocks"].as_array().map(|a| a.len() as i64), |ctx| -> QResult<Value> {
            if !ctx.scope.text || !ctx.scope.derived {
                return Ok(json!({"blocks": [], "note": "此访问方不能读取正文或整理结果"}));
            }
            let limit = req.limit.unwrap_or(50).clamp(1, 200) as usize;
            let (start, end) = match req.date.as_deref().filter(|d| !d.trim().is_empty()) {
                Some(d) => (Some(d), Some(d)),
                None => (req.start.as_deref(), req.end.as_deref()),
            };
            let (ws, we) = self.window(start, end, None, true)?;
            let rows = self.episode_rows(ctx, ws, we, None, 500, None, None, None)?;
            let mut blocks = vec![];
            for ep in rows.iter().filter(|e| e.episode_id.starts_with("screen:")) {
                // Policy changes (a revoked source, a hidden app) hide the block, as for the screen status list.
                let data = self.automation_get(&format!("screen.input/1:{}", ep.episode_id))?;
                if self.screen_validate(&data).is_err() {
                    continue;
                }
                let light = self.screen_episode_light(&data)?;
                let input = self.activity_input(&light)?;
                let messages: Vec<_> = input.refs.iter().filter(|(r, _)| r.starts_with('m')).map(|(_, id)| id.clone()).collect();
                let messages = self.day_visible(ctx, "message_sources", "message_id", "messages", "m", &messages)?;
                let excerpts = self.day_visible(ctx, "excerpt_sources", "excerpt_id", "excerpts", "x", &input.excerpts)?;
                let conversations: Vec<String> = if messages.is_empty() { vec![] } else { input.conversations.clone() };
                blocks.push(json!({
                    "episode_id": ep.episode_id, "start": ep.start, "end": ep.end,
                    "app": light.app, "window": light.window,
                    "title": ep.title, "summary": ep.summary,
                    "apps": light.frames.iter().map(|frame| frame.app.as_str()).collect::<std::collections::BTreeSet<_>>(),
                    "by": if ep.producer == "screen-summary/1" { "model" } else if ep.producer == "user" { "user" } else { "rule" },
                    "conversations": conversations, "excerpts": excerpts,
                }));
                if blocks.len() >= limit {
                    break;
                }
            }
            let mut params = vec![];
            let mut sql = "SELECT d.digest_id,d.start_ms,d.status,d.body_json FROM digests d WHERE d.level='day' AND d.status NOT IN ('superseded','rejected') AND d.rev=(SELECT MAX(x.rev) FROM digests x WHERE x.digest_id=d.digest_id)".to_string();
            if let Some(s) = ws {
                sql += " AND d.end_ms>?";
                params.push(int(s));
            }
            if let Some(e) = we {
                sql += " AND d.start_ms<?";
                params.push(int(e));
            }
            sql += " ORDER BY d.start_ms";
            let daily: Vec<Value> = self
                .db
                .query(&sql, &params, |r| (r.text(0), r.int(1), r.text(2), r.text(3)))?
                .into_iter()
                .filter_map(|(id, at, status, body)| {
                    let body: Value = serde_json::from_str(&body).ok()?;
                    Some(json!({"day": id.trim_start_matches("day:"), "at": self.clock.iso(at), "status": status, "text": body["text"]}))
                })
                .collect();
            Ok(json!({
                "window": self.time_window(ws, we),
                "blocks": blocks,
                "daily": daily,
                "note": "blocks 是按时间排列的活动块，by=rule 的块只有应用时长，没有内容概括；相关对话用 conversation 展开，摘录和消息用 evidence 核对出处；daily 是已有的天级汇总（录音日回顾）。",
            }))
        })
    }
}
