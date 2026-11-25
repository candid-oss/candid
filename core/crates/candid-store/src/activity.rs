//! Summarize a screen episode from cited messages, excerpts, or scoped OCR observations.

use crate::screen_cleanup::ScreenEpisode;
use crate::db::{int, text, RowExt, P};
use crate::{sha256_hex, Result, TimelineStore};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

const PROMPT: &str = include_str!("../../../prompts/activity-block.md");
pub(crate) const PROMPT_VERSION: &str = "activity-block/2";
/// Curated rows a little outside the block still belong to it (a message is stamped at its screenshot's time).
const SLACK_BEFORE_MS: i64 = 5_000;
const SLACK_AFTER_MS: i64 = 60_000;
const MAX_MESSAGES: usize = 40;
const MAX_EXCERPTS: usize = 6;
const MESSAGE_CHARS: usize = 200;
const EXCERPT_CHARS: usize = 400;
const MAX_CHARS: usize = 6000;
const MAX_OCR_FRAMES: usize = 3;

pub(crate) struct ActivityInput {
    /// What the model reads (the user message).
    pub content: String,
    pub has_curated: bool,
    pub hash: String,
    /// Short reference (`m1`, `x2`) → message or excerpt id.
    pub refs: BTreeMap<String, String>,
    pub conversations: Vec<String>,
    pub excerpts: Vec<String>,
    pub evidence: Vec<String>,
    pub stats: Vec<(String, i64)>,
}

fn clip(s: &str, n: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        flat
    } else {
        flat.chars().take(n).collect::<String>() + "…"
    }
}

impl TimelineStore {
    pub(crate) fn activity_prompt(&self) -> String {
        std::fs::read_to_string(self.root.join("prompts").join("activity-block.md")).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| PROMPT.to_string())
    }

    /// The model that would write the block, as `provider/model` (or null without one); part of the cache key.
    pub(crate) fn activity_model(&self) -> Result<Value> {
        let c = self.model_config()?;
        Ok(["episode", "summary", "title"]
            .iter()
            .find_map(|role| c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == *role && a["model"].is_object()).map(|a| a["model"].clone()))
            .unwrap_or(Value::Null))
    }

    /// Foreground time per app inside `[start, end]` from `app.focus` events, longest first, in whole minutes.
    fn activity_stats(&self, sources: &[String], start: i64, end: i64) -> Result<Vec<(String, i64)>> {
        let mut totals: BTreeMap<String, i64> = BTreeMap::new();
        for source in sources {
            let rows = self.db.query(
                "SELECT COALESCE(app_name,app_bundle_id,'?'),start_ms,end_ms FROM events WHERE source_id=? AND kind='app.focus' AND start_ms IS NOT NULL AND start_ms>=? AND start_ms<=? ORDER BY start_ms",
                &[text(source), int(start - 3_600_000), int(end)],
                |r| (r.text(0), r.int(1), r.opt_int(2)),
            )?;
            for (i, (app, s, e)) in rows.iter().enumerate() {
                let next = rows.get(i + 1).map(|n| n.1);
                let stop = e.unwrap_or_else(|| next.unwrap_or(end).min(s + 300_000));
                let (from, to) = ((*s).max(start), stop.min(end));
                if to > from {
                    *totals.entry(app.clone()).or_default() += to - from;
                }
            }
        }
        let mut out: Vec<(String, i64)> = totals.into_iter().map(|(a, ms)| (a, (ms + 30_000) / 60_000)).filter(|(_, m)| *m > 0).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out.truncate(5);
        Ok(out)
    }

    /// The model input of one block: its messages (with conversation and sender), excerpts (title + truncated body)
    /// and app-time statistics. A cross-app block gets only evidence tied to its observed frames.
    pub(crate) fn activity_input(&self, ep: &ScreenEpisode) -> Result<ActivityInput> {
        let sources: Vec<String> = ep.frames.iter().map(|f| f.source_id.clone()).collect::<HashSet<_>>().into_iter().collect();
        let (from, to) = (ep.start_ms - SLACK_BEFORE_MS, ep.end_ms + SLACK_AFTER_MS);
        let marks = crate::db::placeholders(sources.len());
        let apps: BTreeMap<_, _> = ep.frames.iter().map(|f| (&f.app, &f.window)).collect();
        let app_filter = crate::db::placeholders(apps.len());
        let cross_app = apps.len() > 1;
        let frame_json = json!(ep.frames.iter().map(|f| &f.event_id).collect::<Vec<_>>()).to_string();
        let src_params = || -> Vec<P> { sources.iter().map(text).collect() };
        // Every source event of the row must be readable here: right source, not protected.
        let clean = |table: &str, key: &str, alias: &str| {
            let related = if cross_app {
                " AND EXISTS (SELECT 1 FROM json_each(?) ids JOIN events f ON f.event_id=ids.value
                  WHERE f.event_id=e.event_id OR f.parent_event_id=e.event_id OR f.event_id=e.parent_event_id)"
            } else { "" };
            format!("NOT EXISTS (SELECT 1 FROM {table} s JOIN events e ON e.event_id=s.event_id WHERE s.{key}={alias}.{key} AND (e.sensitivity='protected' OR e.source_id NOT IN ({marks})))
              AND EXISTS (SELECT 1 FROM {table} s JOIN events e ON e.event_id=s.event_id WHERE s.{key}={alias}.{key}{related})")
        };
        let evidence_params = || {
            let mut params: Vec<P> = src_params();
            if cross_app {
                params.push(text(&frame_json));
            }
            params.extend(apps.keys().map(|app| text(*app)));
            params
        };
        let mut params: Vec<P> = vec![int(from), int(to)];
        params.extend(evidence_params());
        let messages = self.db.query(
            &format!("SELECT m.message_id,m.conversation_id,m.sent_ms,m.actor,m.sender,m.text,c.title FROM messages m JOIN conversations c ON c.conversation_id=m.conversation_id WHERE m.sent_ms>=? AND m.sent_ms<=? AND {} AND c.app IN ({app_filter}) ORDER BY m.sent_ms,m.seq", clean("message_sources", "message_id", "m")),
            &params,
            |r| (r.text(0), r.text(1), r.int(2), r.text(3), r.opt_text(4), r.text(5), r.opt_text(6)),
        )?;
        let mut params: Vec<P> = vec![int(to), int(from)];
        params.extend(evidence_params());
        let excerpts = self.db.query(
            &format!("SELECT x.excerpt_id,x.title,x.url,x.text FROM excerpts x WHERE x.start_ms<=? AND x.end_ms>=? AND {} AND x.app IN ({app_filter}) ORDER BY x.start_ms,x.seq", clean("excerpt_sources", "excerpt_id", "x")),
            &params,
            |r| (r.text(0), r.opt_text(1), r.opt_text(2), r.text(3)),
        )?;
        let stats = self.activity_stats(&sources, ep.start_ms, ep.end_ms)?;

        let minutes = ((ep.end_ms - ep.start_ms) as f64 / 60_000.0).round().max(1.0) as i64;
        let hm = |ms: i64| self.clock.iso(ms)[11..16].to_string();
        let mut content = format!("时段：{}–{}（约 {} 分钟）\n主窗口：{} · {}\n", hm(ep.start_ms), hm(ep.end_ms), minutes, ep.app, ep.window);
        if !stats.is_empty() {
            content += &format!("前台应用时长：{}\n", stats.iter().map(|(a, m)| format!("{a} {m} 分钟")).collect::<Vec<_>>().join("；"));
        }
        if cross_app {
            content += "同一对象的窗口：";
            content += &apps.iter().map(|(app, title)| format!("{} · {}", clip(app, 40), clip(title, 80))).collect::<Vec<_>>().join(" / ");
            content.push('\n');
        }
        let mut refs = BTreeMap::new();
        let mut conversations = Vec::new();
        let mut kept_excerpts = Vec::new();
        if !messages.is_empty() {
            content += "\n聊天消息：\n";
            // The latest messages matter most; keep them in time order.
            let skip = messages.len().saturating_sub(MAX_MESSAGES);
            for (i, (id, conversation, at, actor, sender, body, title)) in messages.iter().skip(skip).enumerate() {
                let r = format!("m{}", i + 1);
                let who = match (actor.as_str(), sender.as_deref().filter(|s| !s.is_empty())) {
                    ("self", _) => "我".to_string(),
                    (_, Some(s)) => s.to_string(),
                    ("unknown", _) => "发言人未确认".to_string(),
                    _ => "对方".to_string(),
                };
                content += &format!("[{r}] {} 「{}」{who}：{}\n", hm(*at), title.as_deref().unwrap_or("未命名对话"), clip(body, MESSAGE_CHARS));
                refs.insert(r, id.clone());
                if !conversations.contains(conversation) {
                    conversations.push(conversation.clone());
                }
            }
        }
        if !excerpts.is_empty() {
            content += "\n阅读摘录：\n";
            for (i, (id, title, url, body)) in excerpts.iter().take(MAX_EXCERPTS).enumerate() {
                let r = format!("x{}", i + 1);
                let head = title.as_deref().filter(|s| !s.is_empty()).or(url.as_deref()).unwrap_or("无标题");
                content += &format!("[{r}] 《{}》{}\n", clip(head, 80), clip(body, EXCERPT_CHARS));
                refs.insert(r, id.clone());
                kept_excerpts.push(id.clone());
            }
        }
        if refs.is_empty() {
            let frames: Vec<_> = ep.frames.iter().filter(|f| f.field == "ocr" && matches!(f.app.as_str(), "微信" | "WeChat")).collect();
            let indices = [0, frames.len() / 2, frames.len().saturating_sub(1)];
            let mut picked = HashSet::new();
            let mut seen = HashSet::new();
            for index in indices.into_iter().filter(|i| picked.insert(*i)).take(MAX_OCR_FRAMES) {
                let Some(frame) = frames.get(index) else { continue };
                let (_, body) = self.screen_frame_body(&frame.event_id)?;
                let visible = self.chat_ocr_preview(&frame.event_id, &body)?.into_iter()
                    .filter(|line| line.chars().count() >= 4 && seen.insert(line.clone()))
                    .take(10).map(|line| clip(&line, 100)).collect::<Vec<_>>();
                if visible.is_empty() {
                    continue;
                }
                let r = format!("o{}", refs.len() + 1);
                content += &format!("\n[{r}] {} 屏幕可见文字，未确认收发：{}\n", hm(frame.start_ms), visible.join(" / "));
                refs.insert(r, frame.event_id.clone());
            }
        }
        let mut evidence = HashSet::new();
        let visible_content: String = content.chars().take(MAX_CHARS).collect();
        if visible_content.len() < content.len() {
            content = visible_content + "…";
        }
        refs.retain(|r, _| content.contains(&format!("[{r}]")));
        for (kind, id) in &refs {
            let sql = match kind.as_bytes()[0] {
                b'm' => "SELECT event_id FROM message_sources WHERE message_id=?",
                b'x' => "SELECT event_id FROM excerpt_sources WHERE excerpt_id=?",
                _ => {
                    evidence.insert(id.clone());
                    continue;
                }
            };
            evidence.extend(self.db.query(sql, &[text(id)], |r| r.text(0))?);
        }
        let mut evidence: Vec<_> = evidence.into_iter().collect();
        evidence.sort();
        let has_curated = !refs.is_empty();
        let hash = sha256_hex(&serde_json::to_vec(&json!([&content, &refs, &evidence]))?);
        Ok(ActivityInput { hash, content, has_curated, refs, conversations, excerpts: kept_excerpts, evidence, stats })
    }

    /// Cache key of a block's summary: same input, prompt and model → same answer, no new call.
    pub(crate) fn activity_cache_key(&self, input: &ActivityInput) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(&json!([input.hash, PROMPT_VERSION, sha256_hex(self.activity_prompt().as_bytes()), self.activity_model()?]))?))
    }

    /// Rule title and summary from the statistics alone, for a block the model never reads.
    pub(crate) fn activity_rule_text(&self, ep: &ScreenEpisode, input: &ActivityInput) -> (String, String) {
        let minutes = ((ep.end_ms - ep.start_ms) as f64 / 60_000.0).round().max(1.0) as i64;
        let hm = |ms: i64| self.clock.iso(ms)[11..16].to_string();
        let mut summary = format!("{}–{} 在 {} 的「{}」约 {} 分钟。", hm(ep.start_ms), hm(ep.end_ms), ep.app, ep.window, minutes);
        if input.stats.len() > 1 {
            summary += &format!("前台应用：{}。", input.stats.iter().map(|(a, m)| format!("{a} {m} 分钟")).collect::<Vec<_>>().join("、"));
        }
        (format!("{} · {}", ep.app, ep.window), summary)
    }

    /// What the model would read for block `id`, plus the rule text; for offline replay on a library copy.
    #[doc(hidden)]
    pub fn activity_preview(&self, id: &str) -> Result<Value> {
        let data = self.automation_get(&format!("screen.input/1:{id}"))?;
        let ep = self.screen_episode_light(&data)?;
        let input = self.activity_input(&ep)?;
        let (title, summary) = self.activity_rule_text(&ep, &input);
        Ok(json!({"id": id, "has_curated": input.has_curated, "content": input.content, "refs": input.refs, "cache_key": self.activity_cache_key(&input)?, "rule_title": title, "rule_summary": summary, "prompt": self.activity_prompt()}))
    }
}
