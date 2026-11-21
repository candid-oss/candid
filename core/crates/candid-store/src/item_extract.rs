//! Item extraction (docs/plans/data-layer-v1.md §2.2, model call #5): when a conversation has gone quiet, the
//! `summary` model reads its new messages (with 20 messages of context, the open items with this person and the
//! recently ignored ones) and answers with operations — `create | update | done | cancel | memo` — which
//! [`crate::items`] checks and applies.
//!
//! * **Trigger** (`items_scan`, rules only): a chat with no new message for 10 minutes and no queued screenshot
//!   excerpt of its own; an important mail; a recording that finished (meeting transcript). The first scan after the
//!   feature is switched on only looks at what arrives afterwards.
//! * **Job** (`items.extract`, P1): `input_ref` is the conversation. The new messages are everything after the
//!   conversation's cursor (at most 40 at a time), so a job that waited for the daily limit simply covers more.
//! * **Prompt**: `core/prompts/item-extract.md` (a copy at `<root>/prompts/item-extract.md` overrides it).
//! * **Cache key**: ids of the input messages + snapshot of the open items + prompt version and text + model.
//!
//! The model's last raw reply per conversation is kept as `items.raw/1:<conversation>` for comparing prompts.

use crate::db::{int, text, RowExt};
use crate::items::Apply;
use crate::screen_excerpt::{chat_title, parse_json_output};
use crate::{sha256_hex, Result, TimelineError, TimelineStore};
use chrono::Datelike;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

pub(crate) const FEATURE: &str = "item-extract";
pub(crate) const JOB_KIND: &str = "items.extract";
const PROMPT: &str = include_str!("../../../prompts/item-extract.md");
pub(crate) const PROMPT_VERSION: &str = "item-extract/1";
const FLOOR: &str = "items.floor/1";
const CURSOR: &str = "items.cursor/1:";
const QUEUED: &str = "items.queued/1:";
const CACHE: &str = "items.cache/1:";
const RAW: &str = "items.raw/1:";
const CHAT_IDLE_MS: i64 = 10 * 60_000;
const MEETING_SETTLE_MS: i64 = 60_000;
const CONTEXT: i64 = 20;
const MAX_NEW: i64 = 40;
const MAX_OPEN: i64 = 15;
const MESSAGE_CHARS: usize = 500;
const IGNORED_DAYS: i64 = 30;

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

pub(crate) struct ExtractInput {
    /// What the model reads (the user message).
    pub content: String,
    /// Message number in `content` → (message id, sent_ms).
    pub refs: BTreeMap<i64, (String, i64)>,
    pub new_ids: Vec<String>,
    pub max_seq: i64,
}

const WEEKDAYS: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

/// Messages the model may read: they have a source and none of them is protected.
const READABLE: &str = "EXISTS (SELECT 1 FROM message_sources s WHERE s.message_id=m.message_id) AND NOT EXISTS (SELECT 1 FROM message_sources s JOIN events e ON e.event_id=s.event_id WHERE s.message_id=m.message_id AND e.sensitivity='protected')";

type MessageRow = (i64, String, i64, String, Option<String>, String);

impl TimelineStore {
    pub(crate) fn extract_prompt(&self) -> String {
        std::fs::read_to_string(self.root.join("prompts").join("item-extract.md")).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| PROMPT.to_string())
    }

    /// The model that would answer, as its JSON description (null without one); part of the cache key.
    fn extract_model(&self) -> Result<Value> {
        let c = self.model_config()?;
        Ok(["summary", "title"].iter().find_map(|role| c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == *role && a["model"].is_object()).map(|a| a["model"].clone())).unwrap_or(Value::Null))
    }

    fn extract_cursor(&self, conversation: &str) -> Result<i64> {
        let own = self.automation_get(&format!("{CURSOR}{conversation}"))?.as_i64();
        Ok(own.unwrap_or(self.automation_get(FLOOR)?.as_i64().unwrap_or(0)))
    }

    /// Conversations with a screenshot excerpt still waiting: their messages are not complete yet.
    fn excerpt_busy_conversations(&self) -> Result<HashSet<String>> {
        let rows = self.db.query(
            "SELECT e.app_name,e.title FROM jobs j JOIN events e ON e.event_id=j.input_ref WHERE j.kind='screen.excerpt' AND j.state IN ('queued','running')",
            &[],
            |r| (r.opt_text(0).unwrap_or_default(), r.opt_text(1).unwrap_or_default()),
        )?;
        Ok(rows.into_iter().map(|(app, title)| format!("chat:{}", &sha256_hex(format!("{app}\u{1f}{}", chat_title(&title)).as_bytes())[..24])).collect())
    }

    fn extract_version(&self, conversation: &str, cursor: i64, max_seq: i64) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(&json!([conversation, cursor, max_seq, PROMPT_VERSION, sha256_hex(self.extract_prompt().as_bytes()), self.extract_model()?]))?))
    }

    /// Queues extraction for conversations that are ready (see the module docs). Cheap when nothing is.
    pub fn items_scan(&self) -> Result<Value> {
        if self.feature_settings()?["items"][FEATURE]["enabled"] != true {
            return Ok(json!({"changed": false, "disabled": true}));
        }
        let _ = self.items_maintain();
        let floor = match self.automation_get(FLOOR)?.as_i64() {
            Some(v) => v,
            None => {
                let top = self.db.scalar("SELECT COALESCE(MAX(seq),0) FROM messages", &[])?;
                self.automation_put(FLOOR, &json!(top))?;
                top
            }
        };
        let now = self.clock.now_ms();
        let rows = self.db.query(
            "SELECT m.conversation_id,MAX(m.seq),MAX(m.created_ms),c.kind FROM messages m JOIN conversations c ON c.conversation_id=m.conversation_id WHERE m.seq>? GROUP BY m.conversation_id",
            &[int(floor)],
            |r| (r.text(0), r.int(1), r.int(2), r.text(3)),
        )?;
        let busy = self.excerpt_busy_conversations()?;
        let mut queued = 0;
        for (conversation, max_seq, last, kind) in rows {
            let cursor = self.extract_cursor(&conversation)?;
            if max_seq <= cursor || self.automation_get(&format!("{QUEUED}{conversation}"))?.as_i64() == Some(max_seq) {
                continue;
            }
            let ready = match kind.as_str() {
                "chat" => now - last >= CHAT_IDLE_MS && !busy.contains(&conversation),
                "meeting" => now - last >= MEETING_SETTLE_MS,
                "mail" => self.db.scalar(
                    "SELECT COUNT(*) FROM messages m JOIN message_sources s ON s.message_id=m.message_id JOIN events e ON e.event_id=s.event_id WHERE m.conversation_id=? AND m.seq>? AND json_extract(e.payload_json,'$.level')='important'",
                    &[text(&conversation), int(cursor)],
                )? > 0,
                _ => false,
            };
            if !ready {
                continue;
            }
            let version = self.extract_version(&conversation, cursor, max_seq)?;
            self.job_command(&json!({"action":"enqueue","kind":JOB_KIND,"feature":FEATURE,"input_ref":conversation,"config_version":version}))?;
            self.automation_put(&format!("{QUEUED}{conversation}"), &json!(max_seq))?;
            queued += 1;
        }
        Ok(json!({"changed": queued > 0, "queued": queued}))
    }

    fn day_label(&self, ms: i64) -> String {
        let iso = self.clock.iso(ms);
        let weekday = chrono::NaiveDate::parse_from_str(&iso[..10], "%Y-%m-%d").map(|d| WEEKDAYS[d.weekday().num_days_from_monday() as usize]).unwrap_or("");
        format!("{} {} {}", &iso[..10], weekday, &iso[11..16])
    }

    fn item_line(&self, row: &crate::items::ItemRow) -> String {
        let kind = match row.kind.as_str() {
            "event" => "日程",
            "reminder" => "提醒",
            _ => "承诺",
        };
        let owner = if row.owner == "self" { "我答应的" } else { "等对方做的" };
        let when = row.start_ms.map(|t| self.day_label(t)).or_else(|| row.when_text.clone()).unwrap_or_else(|| "无时间".into());
        let status = match (row.review.as_str(), row.external_ref.is_some()) {
            ("confirmed", true) => "已确认，已写入日历/提醒",
            ("confirmed", false) => "已确认",
            _ => "候选",
        };
        format!("- item_id={} | {kind} | {owner} | {when} | {} | {status}", row.item_id, row.title)
    }

    /// The model input for the next messages of `conversation` after `cursor`; `None` when there are none.
    pub(crate) fn extract_input(&self, conversation: &str, cursor: i64) -> Result<Option<ExtractInput>> {
        let Some((kind, app, title)) = self.db.first("SELECT kind,app,title FROM conversations WHERE conversation_id=?", &[text(conversation)], |r| (r.text(0), r.opt_text(1), r.opt_text(2)))? else {
            return Ok(None);
        };
        let columns = "m.seq,m.message_id,m.sent_ms,m.actor,m.sender,m.text";
        let row = |r: &rusqlite::Row| -> MessageRow { (r.int(0), r.text(1), r.int(2), r.text(3), r.opt_text(4), r.text(5)) };
        let new: Vec<MessageRow> = self.db.query(
            &format!("SELECT {columns} FROM messages m WHERE m.conversation_id=? AND m.seq>? AND {READABLE} ORDER BY m.seq LIMIT ?"),
            &[text(conversation), int(cursor), int(MAX_NEW)],
            row,
        )?;
        let Some(first) = new.first() else { return Ok(None) };
        let first_seq = first.0;
        let mut context: Vec<MessageRow> = self.db.query(
            &format!("SELECT {columns} FROM messages m WHERE m.conversation_id=? AND m.seq<? AND {READABLE} ORDER BY m.seq DESC LIMIT ?"),
            &[text(conversation), int(first_seq), int(CONTEXT)],
            row,
        )?;
        context.reverse();
        // Mail arrives in any order; the model reads each part in the order things were said.
        let mut new = new;
        new.sort_by_key(|m| (m.2, m.0));
        context.sort_by_key(|m| (m.2, m.0));
        // Where the cursor stops this batch, the next one starts; a batch cut short reads only up to its last message.
        let max_seq = new.last().map(|m| m.0).unwrap_or(cursor);
        let counterpart = self.conversation_counterpart(conversation)?;
        let who = |m: &MessageRow| -> String {
            match (m.3.as_str(), m.4.as_deref().filter(|s| !s.is_empty())) {
                ("self", _) => "我".to_string(),
                (_, Some(s)) => format!("{s}（对方）"),
                _ => match &counterpart {
                    Some(c) => format!("{}（对方）", c.name),
                    None => "对方".to_string(),
                },
            }
        };
        let now = self.clock.now_ms();
        let zone = {
            let iso = self.clock.iso(now);
            if iso.ends_with('Z') { "+00:00".to_string() } else { iso[iso.len() - 6..].to_string() }
        };
        let place = match (kind.as_str(), &counterpart) {
            ("mail", _) => "邮件".to_string(),
            ("meeting", _) => "会议录音".to_string(),
            ("agent", _) => "你写给 AI 的提示词".to_string(),
            (_, Some(_)) => format!("{} · 私聊", app.clone().unwrap_or_default()),
            _ => format!("{} · 群聊或多人", app.clone().unwrap_or_default()),
        };
        let mut content = format!("当前时间：{}（时区 {zone}）\n对话：「{}」（{place}）\n", self.day_label(now), title.as_deref().unwrap_or("未命名对话"));

        // Items with this person (or in this conversation) that are still open, and the ones the user threw away.
        let (clause, params) = match &counterpart {
            Some(c) => {
                let keys = self.counterpart_keys(&c.key)?;
                (format!("(counterpart IN ({}) OR conversation_id=?)", crate::db::placeholders(keys.len())), keys.iter().map(text).chain([text(conversation)]).collect::<Vec<_>>())
            }
            None => ("conversation_id=?".to_string(), vec![text(conversation)]),
        };
        let open = self.query_items(&format!("{clause} AND state='open' AND review<>'rejected' ORDER BY start_ms IS NULL, start_ms, created_ms DESC LIMIT {MAX_OPEN}"), &params)?;
        let ignored = self.query_items(&format!("{clause} AND review='rejected' AND updated_ms>={} ORDER BY updated_ms DESC LIMIT {MAX_OPEN}", now - IGNORED_DAYS * 86_400_000), &params)?;
        content += "\n未结条目（与这个对方，更新或完成时用 item_id）：\n";
        content += &if open.is_empty() { "（无）\n".to_string() } else { open.iter().map(|r| self.item_line(r) + "\n").collect::<String>() };
        if !ignored.is_empty() {
            content += "\n最近 30 天用户忽略的条目（不要再新增类似的）：\n";
            for r in &ignored {
                content += &format!("- {} | {}\n", r.start_ms.map(|t| self.day_label(t)).or_else(|| r.when_text.clone()).unwrap_or_else(|| "无时间".into()), r.title);
            }
        }

        let mut refs = BTreeMap::new();
        let mut n = 0;
        let mut render = |label: &str, rows: &[MessageRow], content: &mut String, refs: &mut BTreeMap<i64, (String, i64)>| {
            if rows.is_empty() {
                return;
            }
            *content += &format!("\n{label}\n");
            for m in rows {
                n += 1;
                let body: String = m.5.split_whitespace().collect::<Vec<_>>().join(" ");
                let body = if body.chars().count() > MESSAGE_CHARS { body.chars().take(MESSAGE_CHARS).collect::<String>() + "…" } else { body };
                *content += &format!("[{n}] {} {}：{body}\n", self.day_label(m.2), who(m));
                refs.insert(n, (m.1.clone(), m.2));
            }
        };
        render("上文（只用来理解，不抽取）：", &context, &mut content, &mut refs);
        render("新消息（从这里面抽取）：", &new, &mut content, &mut refs);
        Ok(Some(ExtractInput { content, refs, new_ids: new.iter().map(|m| m.1.clone()).collect(), max_seq }))
    }

    fn query_items(&self, tail: &str, params: &[crate::db::P]) -> Result<Vec<crate::items::ItemRow>> {
        self.db.query(&format!("SELECT {} FROM items WHERE {tail}", crate::items::ITEM_COLUMNS), params, crate::items::item_from)
    }

    /// Cache key: input message ids + the open-item snapshot (all of `content` before the messages) + prompt + model.
    pub(crate) fn extract_cache_key(&self, input: &ExtractInput) -> Result<String> {
        let snapshot = input.content.split("\n上文").next().unwrap_or("").split("\n新消息").next().unwrap_or("");
        // Times in the header lines change every minute; only the item lines belong in the key.
        let items: Vec<&str> = snapshot.lines().filter(|l| l.starts_with("- ")).collect();
        Ok(sha256_hex(&serde_json::to_vec(&json!([input.new_ids, items, PROMPT_VERSION, sha256_hex(self.extract_prompt().as_bytes()), self.extract_model()?]))?))
    }

    pub(crate) fn items_prepare(&self, job: &Value) -> Result<Value> {
        let conversation = job["input_ref"].as_str().ok_or_else(|| bad("缺少对话"))?;
        let c = self.model_config()?;
        let a = ["summary", "title"]
            .iter()
            .find_map(|role| c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == *role && a["model"].is_object()))
            .ok_or_else(|| bad("waiting_model"))?;
        let provider = c["providers"].as_array().into_iter().flatten().find(|p| p["provider_id"] == a["model"]["provider_id"] && p["enabled"] == true).ok_or_else(|| bad("waiting_model"))?;
        self.feature_check(FEATURE, &json!({"source": "*"}), Some(provider))?;
        let input = self.extract_input(conversation, self.extract_cursor(conversation)?)?.ok_or_else(|| bad("no_new_messages"))?;
        let key = self.extract_cache_key(&input)?;
        if self.automation_get(&format!("{CACHE}{conversation}"))?["key"] == json!(key) {
            return Err(bad("cache_hit"));
        }
        self.workflow_budget(&c, a, input.content.len())?;
        let prompt = self.extract_prompt();
        let refs: BTreeMap<String, Value> = input.refs.iter().map(|(n, (id, ms))| (n.to_string(), json!({"id": id, "ms": ms}))).collect();
        Ok(json!({
            "job": job, "speech": false, "step": FEATURE, "provider": provider, "assignment": a,
            "prompt": prompt, "content": input.content, "json_mode": true, "max_tokens": 2048, "repair_json": true,
            "refs": refs, "max_seq": input.max_seq, "cache_key": key,
            "source_revision": sha256_hex(input.content.as_bytes()),
            "config_revision": sha256_hex(&serde_json::to_vec(&c)?), "feature_revision": self.feature_settings()?["items"][FEATURE],
        }))
    }

    pub(crate) fn items_reply_allowed(&self, r: &Value) -> Result<()> {
        let conversation = r["job"]["input_ref"].as_str().unwrap_or("");
        if self.db.scalar("SELECT COUNT(*) FROM conversations WHERE conversation_id=?", &[text(conversation)])? == 0 {
            return Err(bad("source_changed"));
        }
        if sha256_hex(&serde_json::to_vec(&self.model_config()?)?) != r["config_revision"] || self.feature_settings()?["items"][FEATURE] != r["feature_revision"] {
            return Err(bad("source_or_configuration_changed"));
        }
        let job = self.job_command(&json!({"action":"get","id":r["job"]["id"]}))?;
        if job["state"] != "running" || job["attempt"] != r["job"]["attempt"] || job["owner"] != r["job"]["owner"] || job["lease_until_ms"].as_i64().unwrap_or(0) <= self.clock.now_ms() {
            return Err(TimelineError::Denied("stale_job_lease".into()));
        }
        Ok(())
    }

    pub(crate) fn items_complete(&self, r: &Value, output: &Value) -> Result<Value> {
        self.items_reply_allowed(r)?;
        let raw = output.as_str().ok_or_else(|| bad("invalid_output"))?;
        let parsed = parse_json_output(raw).ok_or_else(|| bad("invalid_output"))?;
        let conversation = r["job"]["input_ref"].as_str().unwrap_or("");
        let refs: BTreeMap<i64, (String, i64)> = r["refs"].as_object().into_iter().flatten().filter_map(|(n, v)| Some((n.parse().ok()?, (v["id"].as_str()?.to_string(), v["ms"].as_i64()?)))).collect();
        let model = r["assignment"]["model"]["model_id"].as_str().unwrap_or("");
        self.db.transaction(|| {
            let applied = self.items_apply_parsed(conversation, &refs, &parsed, model)?;
            self.automation_put(&format!("{CURSOR}{conversation}"), &r["max_seq"])?;
            self.automation_put(&format!("{CACHE}{conversation}"), &json!({"key": r["cache_key"]}))?;
            self.automation_put(&format!("{RAW}{conversation}"), &json!({"output": parsed, "applied": applied, "model": model, "prompt": sha256_hex(self.extract_prompt().as_bytes())}))?;
            self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=? WHERE job_id=?", &[text(format!("items:{conversation}")), text(r["job"]["id"].as_str().unwrap_or(""))])?;
            Ok(applied)
        })
    }

    fn items_apply_parsed(&self, conversation: &str, refs: &BTreeMap<i64, (String, i64)>, parsed: &Value, model: &str) -> Result<Value> {
        // Messages deleted while the model was thinking cannot be cited any more.
        let mut live = BTreeMap::new();
        for (n, (id, ms)) in refs {
            if self.db.scalar("SELECT COUNT(*) FROM messages WHERE message_id=?", &[text(id)])? > 0 {
                live.insert(*n, (id.clone(), *ms));
            }
        }
        let counterpart = self.conversation_counterpart(conversation)?;
        self.items_apply(&Apply { conversation, counterpart: counterpart.as_ref(), refs: &live, model, prompt_version: PROMPT_VERSION }, parsed)
    }

    /// What the model would read for the last `last` messages of a conversation (offline replay on a library copy).
    #[doc(hidden)]
    pub fn items_preview(&self, conversation: &str, last: i64) -> Result<Value> {
        let cursor = self.db.first("SELECT seq FROM messages WHERE conversation_id=? ORDER BY seq DESC LIMIT 1 OFFSET ?", &[text(conversation), int(last)], |r| r.int(0))?.unwrap_or(0);
        let input = self.extract_input(conversation, cursor)?.ok_or_else(|| bad("no_new_messages"))?;
        Ok(json!({"conversation_id": conversation, "prompt": self.extract_prompt(), "content": input.content, "cursor": cursor, "max_seq": input.max_seq, "cache_key": self.extract_cache_key(&input)?,
                  "refs": input.refs.iter().map(|(n, (id, _))| (n.to_string(), json!(id))).collect::<BTreeMap<_, _>>()}))
    }

    /// Applies a raw model reply for the messages `items_preview(conversation, last)` showed, as the job would.
    #[doc(hidden)]
    pub fn items_apply_raw(&self, conversation: &str, last: i64, raw: &str, model: &str) -> Result<Value> {
        let cursor = self.db.first("SELECT seq FROM messages WHERE conversation_id=? ORDER BY seq DESC LIMIT 1 OFFSET ?", &[text(conversation), int(last)], |r| r.int(0))?.unwrap_or(0);
        let input = self.extract_input(conversation, cursor)?.ok_or_else(|| bad("no_new_messages"))?;
        let parsed = parse_json_output(raw).ok_or_else(|| bad("invalid_output"))?;
        self.db.transaction(|| self.items_apply_parsed(conversation, &input.refs, &parsed, model))
    }

    /// Applies a parsed reply to explicit message numbers; for tests.
    #[doc(hidden)]
    pub fn items_apply_for_test(&self, conversation: &str, refs: &BTreeMap<i64, String>, parsed: &Value, model: &str) -> Result<Value> {
        let mut full = BTreeMap::new();
        for (n, id) in refs {
            let ms = self.db.first("SELECT sent_ms FROM messages WHERE message_id=?", &[text(id)], |r| r.int(0))?.ok_or_else(|| bad("unknown_message"))?;
            full.insert(*n, (id.clone(), ms));
        }
        self.db.transaction(|| self.items_apply_parsed(conversation, &full, parsed, model))
    }
}
