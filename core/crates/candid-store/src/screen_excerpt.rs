//! Screenshots into the AI-readable layer (docs/plans/ai-v1-plan.md §3): pick the screenshots that show new text,
//! let the `vision` model copy out the main content, and write chat bubbles as messages and reading as excerpts.
//!
//! 1. **Keyframes** (`excerpt_scan`, rules only): per window, a screenshot is sent only when its OCR lines differ
//!    from the last screenshot sent for that window by at least two meaningful lines, or, without OCR, when a tiny
//!    grayscale thumbnail of it differs from the last one sent by a few percent of its cells. Interface text that never
//!    changes, scrolling by a line, and repeated identical frames cost nothing. A daily cap bounds the calls.
//! 2. **Excerpt** (`screen.excerpt` job): screenshot + OCR text as a reference + the prompt in
//!    `core/prompts/screen-excerpt.md` (a local copy at `<root>/prompts/screen-excerpt.md` overrides it, for tuning).
//! 3. **Merge** (`excerpt_complete`): chat messages are aligned with the end of the conversation so far and only
//!    the new tail is appended; an excerpt nearly identical to the last one of its page is skipped. Every row keeps
//!    the screenshot as its source, so deleting the screenshot removes it.
//!
//! The model's raw output is kept as `screen.excerpt/1:<screenshot>` so prompt changes can be compared.

use crate::curated::NewMessage;
use crate::db::{int, opt_text, text, RowExt};
use crate::{sha256_hex, Result, TimelineError, TimelineStore};
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) const FEATURE: &str = "screen-excerpt";
pub(crate) const JOB_KIND: &str = "screen.excerpt";
const PROMPT: &str = include_str!("../../../prompts/screen-excerpt.md");
const PROMPT_VERSION: &str = "screen-excerpt/1";
const CURSOR: &str = "excerpt.cursor/1";
const BATCH: i64 = 500;
/// Without OCR: share of thumbnail cells that must change (a new chat bubble is a few percent of the screen).
const MIN_THUMB_CHANGE: f64 = 0.015;
const MIN_NEW_LINES: usize = 2;
/// Chat lines whose top is below this (0–1000 of the screenshot height) sit in the input box.
const INPUT_BAND_TOP: i64 = 830;
const MIN_NEW_CHARS: usize = 6;

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

/// A line worth comparing: trimmed, inner whitespace collapsed; clock times, bare numbers and single characters
/// carry no content.
fn meaningful(line: &str) -> Option<String> {
    let s: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = s.chars().count();
    if chars < 2 || s.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '/' || c == '-' || c == '.' || c == ' ') {
        return None;
    }
    Some(s)
}

fn line_hashes(ocr: &str) -> Vec<(String, usize)> {
    ocr.lines().filter_map(meaningful).map(|l| (sha256_hex(l.as_bytes())[..16].to_string(), l.chars().count())).collect()
}

/// Character-bigram Dice similarity of two normalized texts, 0–1.
pub(crate) fn similarity(a: &str, b: &str) -> f64 {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<Vec<_>>();
    let (a, b) = (norm(a), norm(b));
    if a == b {
        return 1.0;
    }
    if a.len() < 2 || b.len() < 2 {
        return 0.0;
    }
    let grams = |v: &[char]| v.windows(2).map(|w| (w[0], w[1])).collect::<Vec<_>>();
    let (ga, mut gb) = (grams(&a), grams(&b));
    let total = ga.len() + gb.len();
    let mut hits = 0;
    for g in ga {
        if let Some(i) = gb.iter().position(|x| *x == g) {
            gb.swap_remove(i);
            hits += 1;
        }
    }
    2.0 * hits as f64 / total as f64
}

/// A chat title without a trailing member count, e.g. 「群名（63）」 → 「群名」: the count changes, the chat does not.
pub(crate) fn chat_title(title: &str) -> String {
    let t = title.trim();
    let stripped = t.strip_suffix(')').or_else(|| t.strip_suffix('）')).and_then(|rest| {
        let open = rest.rfind(['(', '（'])?;
        rest[open..].trim_start_matches(['(', '（']).chars().all(|c| c.is_ascii_digit()).then(|| rest[..open].trim_end())
    });
    stripped.filter(|s| !s.is_empty()).unwrap_or(t).to_string()
}

fn same_message(a: &(String, String), b: &(String, String)) -> bool {
    a.0 == b.0 && similarity(&a.1, &b.1) >= 0.85
}

/// How many messages at the start of `frame` are already the last messages of `known`; `None` when the frame
/// matches somewhere but not at the end (a scrolled-back view, nothing to append).
pub(crate) fn new_from(known: &[(String, String)], frame: &[(String, String)]) -> Option<usize> {
    if known.is_empty() || frame.is_empty() {
        return Some(0);
    }
    let mut best: Option<(usize, bool)> = None; // (matched length, reaches the end of known)
    for start in 0..known.len() {
        let mut n = 0;
        while start + n < known.len() && n < frame.len() && same_message(&known[start + n], &frame[n]) {
            n += 1;
        }
        if n == 0 {
            continue;
        }
        let at_end = start + n == known.len();
        if best.is_none_or(|(m, e)| n > m || (n == m && at_end && !e)) {
            best = Some((n, at_end));
        }
    }
    match best {
        None => Some(0),
        Some((n, true)) => Some(n),
        Some((n, false)) if n == frame.len() => None,
        // The frame starts inside the history and runs past it: append what follows the matched part.
        Some((n, false)) => Some(n),
    }
}

impl TimelineStore {
    fn excerpt_sources(&self) -> Result<Vec<String>> {
        let config = self.feature_settings()?;
        let p = &config["items"][FEATURE];
        if p["enabled"] != true {
            return Ok(vec![]);
        }
        let allowed = p["sources"].as_array().cloned().unwrap_or_default();
        Ok(self
            .sources()?
            .into_iter()
            .filter(|s| s.policy.agent_visible && s.policy.store_text && s.status != "revoked" && s.kind == "mac.desktop" && allowed.iter().any(|id| *id == "*" || *id == s.source_id))
            .map(|s| s.source_id)
            .collect())
    }

    fn excerpt_prompt(&self) -> String {
        std::fs::read_to_string(self.root.join("prompts").join("screen-excerpt.md")).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| PROMPT.to_string())
    }

    fn excerpt_version(&self) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(&json!([PROMPT_VERSION, sha256_hex(self.excerpt_prompt().as_bytes()), self.model_config()?, self.feature_settings()?["items"][FEATURE]]))?))
    }

    /// The OCR of a screenshot: its event id, text and whether it exists yet.
    fn shot_ocr(&self, shot: &str) -> Result<Option<(String, String)>> {
        let Some(id) = self.db.first(
            "SELECT event_id FROM events WHERE parent_event_id=? AND kind='image.text' AND json_extract(payload_json,'$.method')='ocr' ORDER BY ingest_seq DESC LIMIT 1",
            &[text(shot)],
            |r| r.text(0),
        )?
        else {
            return Ok(None);
        };
        let body = self.db.query("SELECT c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id WHERE t.event_id=? ORDER BY t.field,t.ord", &[text(&id)], |r| r.text(0))?.join("\n");
        Ok(Some((id, body)))
    }

    fn shot_blob(&self, shot: &str) -> Result<Option<(String, String)>> {
        self.db.first(
            "SELECT b.rel_path,b.mime_type FROM event_blobs eb JOIN blobs b ON b.sha256=eb.sha256 WHERE eb.event_id=? AND eb.role='original' AND b.mime_type LIKE 'image/%' LIMIT 1",
            &[text(shot)],
            |r| (r.text(0), r.text(1)),
        )
    }

    fn shot_thumbnail(&self, shot: &str) -> Result<Option<Vec<u8>>> {
        let Some((path, mime)) = self.shot_blob(shot)? else { return Ok(None) };
        if mime != "image/webp" {
            return Ok(None);
        }
        Ok(std::fs::read(self.root.join(path)).ok().and_then(|b| crate::webp::luma_thumbnail(&b)))
    }

    /// OCR lines of a screenshot with their boxes `(left, right, top, text)` on a 0–1000 grid, when the OCR kept
    /// boxes (`image.text` payload `boxes`, one per line).
    pub(crate) fn shot_ocr_lines(&self, shot: &str) -> Result<Vec<(i64, i64, i64, String)>> {
        let Some(id) = self.db.first(
            "SELECT event_id FROM events WHERE parent_event_id=? AND kind='image.text' AND json_extract(payload_json,'$.method')='ocr' ORDER BY ingest_seq DESC LIMIT 1",
            &[text(shot)],
            |r| r.text(0),
        )?
        else {
            return Ok(vec![]);
        };
        let payload: Value = self.db.first("SELECT payload_json FROM events WHERE event_id=?", &[text(&id)], |r| r.text(0))?.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or(Value::Null);
        let Some(boxes) = payload["boxes"].as_array() else { return Ok(vec![]) };
        let body = self.db.query("SELECT c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id WHERE t.event_id=? AND t.field='ocr' ORDER BY t.ord", &[text(&id)], |r| r.text(0))?.join("\n");
        let lines: Vec<&str> = body.split('\n').collect();
        if lines.len() != boxes.len() {
            return Ok(vec![]);
        }
        Ok(lines
            .iter()
            .zip(boxes)
            .filter_map(|(l, b)| {
                let (x, y, w) = (b[0].as_i64()?, b[1].as_i64()?, b[2].as_i64()?);
                Some((x, x + w, y, l.to_string()))
            })
            .collect())
    }

    /// Finds screenshots with new text and queues them. Called from the background tick; cheap when idle.
    pub fn excerpt_scan(&self) -> Result<Value> {
        let sources = self.excerpt_sources()?;
        if sources.is_empty() {
            return Ok(json!({"changed": false, "disabled": true}));
        }
        let key = "excerpt.automatic-since/1";
        let since = match self.automation_get(key)?.as_i64() {
            Some(v) => v,
            None => {
                let now = self.clock.now_ms();
                self.automation_put(key, &json!(now))?;
                now
            }
        };
        let cursor: i64 = self.automation_get(CURSOR)?.as_i64().unwrap_or(0);
        let mut sql = format!(
            "SELECT event_id,start_ms,app_name,title,ingest_seq FROM events WHERE kind='screen.shot' AND ingest_seq>? AND sensitivity<>'protected' AND COALESCE(start_ms,ingested_ms)>=? AND source_id IN ({})",
            crate::db::placeholders(sources.len())
        );
        sql += " ORDER BY ingest_seq LIMIT ?";
        let mut params = vec![int(cursor), int(since)];
        params.extend(sources.iter().map(text));
        params.push(int(BATCH));
        let shots = self.db.query(&sql, &params, |r| (r.text(0), r.opt_int(1), r.opt_text(2), r.opt_text(3), r.int(4)))?;
        let version = self.excerpt_version()?;
        let (mut last, mut queued) = (cursor, 0);
        for (shot, start, app, title, seq) in shots {
            last = seq;
            let window = format!("excerpt.window/1:{}", sha256_hex(format!("{}\u{1f}{}", app.as_deref().unwrap_or(""), title.as_deref().unwrap_or("")).as_bytes()));
            let state = self.automation_get(&window)?;
            // With OCR, compare meaningful lines; without it (OCR off or not run), compare tiny thumbnails.
            let (fresh, lines, thumb) = match self.shot_ocr(&shot)? {
                Some((_, ocr)) => {
                    let current = line_hashes(&ocr);
                    let previous: HashSet<String> = state["lines"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
                    let new: Vec<&(String, usize)> = current.iter().filter(|(h, _)| !previous.contains(h)).collect();
                    let fresh = new.len() >= MIN_NEW_LINES && new.iter().map(|(_, n)| n).sum::<usize>() >= MIN_NEW_CHARS;
                    (fresh, json!(current.iter().map(|(h, _)| h.as_str()).take(600).collect::<Vec<_>>()), Value::Null)
                }
                None => {
                    let Some(thumb) = self.shot_thumbnail(&shot)? else { continue };
                    use base64::Engine;
                    let engine = base64::engine::general_purpose::STANDARD;
                    let previous = state["thumb"].as_str().and_then(|t| engine.decode(t).ok()).unwrap_or_default();
                    (crate::webp::thumbnail_change(&previous, &thumb) >= MIN_THUMB_CHANGE, Value::Null, json!(engine.encode(&thumb)))
                }
            };
            // The daily limit is applied when a job is picked up (`feature_budget`), so frames over the limit wait in
            // the queue and are dropped by `expire_stale_jobs` after a day, instead of being lost here.
            if !fresh {
                continue;
            }
            self.job_command(&json!({"action":"enqueue","kind":JOB_KIND,"feature":FEATURE,"input_ref":shot,"config_version":version}))?;
            self.automation_put(&window, &json!({"lines": lines, "thumb": thumb, "at": start}))?;
            queued += 1;
        }
        if last != cursor {
            self.automation_put(CURSOR, &json!(last))?;
        }
        Ok(json!({"changed": queued > 0, "queued": queued}))
    }

    pub(crate) fn excerpt_prepare(&self, job: &Value) -> Result<Value> {
        let shot = job["input_ref"].as_str().ok_or_else(|| bad("缺少截图"))?;
        let source = self.db.first("SELECT source_id FROM events WHERE event_id=? AND kind='screen.shot'", &[text(shot)], |r| r.text(0))?.ok_or_else(|| bad("source_changed"))?;
        if !self.excerpt_sources()?.contains(&source) {
            return Err(TimelineError::Denied("feature_disabled".into()));
        }
        let c = self.model_config()?;
        let a = c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == "vision" && a["model"].is_object()).ok_or_else(|| bad("waiting_model"))?;
        let provider = c["providers"].as_array().into_iter().flatten().find(|p| p["provider_id"] == a["model"]["provider_id"] && p["enabled"] == true).ok_or_else(|| bad("waiting_model"))?;
        self.feature_check(FEATURE, &json!({"source": source}), Some(provider))?;
        if !crate::feature_settings::is_loopback(provider["base_url"].as_str().unwrap_or("")) && !self.automation_config()?.remote_sources.contains(&source) {
            return Err(TimelineError::Denied("waiting_authorization".into()));
        }
        let parts = self.excerpt_parts(shot)?;
        let bytes = std::fs::read(parts["image_path"].as_str().unwrap_or("")).map_err(|_| bad("source_changed"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(bad("input_limit"));
        }
        use base64::Engine;
        let image = format!("data:{};base64,{}", parts["mime"].as_str().unwrap_or("image/webp"), base64::engine::general_purpose::STANDARD.encode(&bytes));
        let context = parts["context"].as_str().unwrap_or("").to_string();
        let prompt = self.excerpt_prompt();
        let schema = json!({"type":"object","properties":{
            "kind":{"type":"string","enum":["chat","doc","web","other"]},
            "title":{"type":"string"},
            "messages":{"type":"array","items":{"type":"object","properties":{"from":{"type":"string","enum":["me","other"]},"sender":{"type":"string"},"text":{"type":"string"}},"required":["from","sender","text"],"additionalProperties":false}},
            "excerpt":{"type":"string"}},"required":["kind","title","messages","excerpt"],"additionalProperties":false});
        self.workflow_budget(&c, a, prompt.len() + context.len())?;
        Ok(json!({
            "job": job, "speech": false, "step": FEATURE, "provider": provider, "assignment": a,
            "prompt": prompt, "content": context, "json_mode": true, "response_schema": schema, "max_tokens": 4096,
            "messages": [{"role":"system","content":prompt},{"role":"user","content":[{"type":"text","text":context},{"type":"image_url","image_url":{"url":image}}]}],
            "source_revision": shot, "config_revision": sha256_hex(&serde_json::to_vec(&c)?), "feature_revision": self.feature_settings()?["items"][FEATURE],
        }))
    }

    /// What the model sees for screenshot `shot`: the prompt, the text part (window, URL, OCR reference) and the
    /// image file. Shared by the job and by offline replays.
    pub fn excerpt_parts(&self, shot: &str) -> Result<Value> {
        let (app, title, url) = self
            .db
            .first("SELECT app_name,title,url FROM events WHERE event_id=? AND kind='screen.shot'", &[text(shot)], |r| (r.opt_text(0), r.opt_text(1), r.opt_text(2)))?
            .ok_or_else(|| bad("source_changed"))?;
        let (path, mime) = self.shot_blob(shot)?.ok_or_else(|| bad("source_changed"))?;
        let boxed = self.shot_ocr_lines(shot)?;
        let (label, ocr) = if boxed.is_empty() {
            ("OCR 参考（含侧栏等噪声，仅用于认字）", self.shot_ocr(shot)?.map(|o| o.1).unwrap_or_default())
        } else {
            (
                "OCR 参考（每行前面是 [左边缘-右边缘,顶部]，坐标按截图宽高归一到 0–1000；含侧栏等噪声，仅用于认字和判断位置）",
                boxed.iter().map(|(a, b, y, t)| format!("[{a}-{b},{y}] {t}")).collect::<Vec<_>>().join("\n"),
            )
        };
        let ocr: String = ocr.chars().take(6000).collect();
        let context = format!(
            "应用：{}\n窗口标题：{}\n{}{label}：\n{}",
            app.as_deref().unwrap_or(""),
            title.as_deref().unwrap_or(""),
            url.as_deref().map(|u| format!("网址：{u}\n")).unwrap_or_default(),
            ocr
        );
        Ok(json!({"prompt": self.excerpt_prompt(), "context": context, "image_path": self.root.join(path), "mime": mime, "app": app, "title": title}))
    }

    /// Parses a model's raw reply for `shot` and merges it, as the job does (offline replays).
    pub fn excerpt_apply_raw(&self, shot: &str, raw: &str, model: &str) -> Result<Value> {
        let parsed = parse_json_output(raw).ok_or_else(|| bad("invalid_output"))?;
        self.db.transaction(|| self.excerpt_write(shot, &parsed, model))
    }

    pub(crate) fn excerpt_reply_allowed(&self, r: &Value) -> Result<()> {
        let shot = r["job"]["input_ref"].as_str().unwrap_or("");
        if self.db.scalar("SELECT COUNT(*) FROM events WHERE event_id=?", &[text(shot)])? == 0 {
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

    pub(crate) fn excerpt_complete(&self, r: &Value, output: &Value) -> Result<Value> {
        self.excerpt_reply_allowed(r)?;
        let raw = output.as_str().ok_or_else(|| bad("invalid_output"))?;
        let parsed = parse_json_output(raw).ok_or_else(|| bad("invalid_output"))?;
        let shot = r["job"]["input_ref"].as_str().unwrap();
        let model = r["assignment"]["model"]["model_id"].as_str().unwrap_or("");
        let result = self.db.transaction(|| {
            let written = self.excerpt_write(shot, &parsed, model)?;
            self.automation_put(&format!("screen.excerpt/1:{shot}"), &json!({"output": parsed, "written": written, "model": model, "prompt": sha256_hex(self.excerpt_prompt().as_bytes())}))?;
            self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=? WHERE job_id=?", &[text(format!("excerpt:{shot}")), text(r["job"]["id"].as_str().unwrap_or(""))])?;
            Ok(written)
        })?;
        Ok(result)
    }

    /// Writes one parsed excerpt for screenshot `shot`; also used by tests and replays.
    pub fn excerpt_write(&self, shot: &str, out: &Value, model: &str) -> Result<Value> {
        let (start, app, wtitle, url) = self
            .db
            .first("SELECT COALESCE(start_ms,ingested_ms),app_name,title,url FROM events WHERE event_id=?", &[text(shot)], |r| (r.int(0), r.opt_text(1), r.opt_text(2), r.opt_text(3)))?
            .ok_or_else(|| bad("source_changed"))?;
        let producer = format!("{PROMPT_VERSION}:{model}");
        let title = out["title"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from).or(wtitle.clone());
        match out["kind"].as_str().unwrap_or("other") {
            "chat" => {
                let frame: Vec<(String, String, Option<String>)> = out["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let body = m["text"].as_str()?.trim();
                        if body.is_empty() {
                            return None;
                        }
                        let actor = if m["from"] == "me" { "self" } else { "other" };
                        let sender = m["sender"].as_str().map(str::trim).filter(|s| !s.is_empty() && actor == "other").map(String::from);
                        Some((actor.to_string(), body.to_string(), sender))
                    })
                    .collect();
                if frame.is_empty() {
                    return Ok(json!({"kind": "chat", "appended": 0}));
                }
                // Models are unreliable about left and right; where OCR boxes exist, sides and order come from them.
                let frame = place_by_geometry(frame, title.as_deref().unwrap_or(""), &self.shot_ocr_lines(shot)?);
                let app_name = app.clone().unwrap_or_default();
                let conversation_id = format!("chat:{}", &sha256_hex(format!("{app_name}\u{1f}{}", chat_title(title.as_deref().unwrap_or(""))).as_bytes())[..24]);
                let known: Vec<(String, String)> = self
                    .db
                    .query("SELECT actor,text FROM (SELECT actor,text,sent_ms,seq FROM messages WHERE conversation_id=? ORDER BY sent_ms DESC,seq DESC LIMIT 60) ORDER BY sent_ms,seq", &[text(&conversation_id)], |r| (r.text(0), r.text(1)))?;
                let pairs: Vec<(String, String)> = frame.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect();
                let Some(skip) = new_from(&known, &pairs) else {
                    return Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": 0, "note": "scrolled_back"}));
                };
                if skip >= frame.len() {
                    return Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": 0}));
                }
                // New messages are stamped at the screenshot, in order, after anything already there.
                // Frames overlap and the model copies the same bubble slightly differently each time: anything that
                // matches one of the recent messages is a repeat, wherever the alignment put it.
                let frame: Vec<_> = frame.into_iter().enumerate().filter(|(i, (a, b, _))| *i >= skip && !known.iter().any(|k| same_message(k, &(a.clone(), b.clone())))).map(|(_, m)| m).collect();
                if frame.is_empty() {
                    return Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": 0}));
                }
                let skip = 0;
                let floor = self.db.scalar("SELECT COALESCE(MAX(sent_ms),0) FROM messages WHERE conversation_id=?", &[text(&conversation_id)])?;
                let base = start.max(floor + 1);
                self.upsert_conversation(&conversation_id, "chat", app.as_deref(), title.as_deref(), base, &producer)?;
                let mut appended = 0;
                for (i, (actor, body, sender)) in frame.into_iter().enumerate().skip(skip) {
                    appended += self.insert_message(&NewMessage {
                        message_id: format!("sx:{}", &sha256_hex(format!("{shot}\u{1f}{i}").as_bytes())[..32]),
                        conversation_id: &conversation_id,
                        sent_ms: base + i as i64,
                        actor: &actor,
                        sender,
                        text: body,
                        producer: &producer,
                        sources: vec![shot.to_string()],
                    })?;
                }
                self.db.execute("UPDATE conversations SET last_ms=MAX(last_ms,?) WHERE conversation_id=?", &[int(base + 1000), text(&conversation_id)])?;
                Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": appended}))
            }
            kind @ ("doc" | "web") => {
                let body = out["excerpt"].as_str().unwrap_or("").trim();
                if body.chars().count() < 20 {
                    return Ok(json!({"kind": kind, "written": 0}));
                }
                let key = url.clone().unwrap_or_else(|| format!("{}\u{1f}{}", app.as_deref().unwrap_or(""), title.as_deref().unwrap_or("")));
                let last = self.db.first("SELECT excerpt_id,text FROM excerpts WHERE key=? ORDER BY start_ms DESC LIMIT 1", &[text(&key)], |r| (r.text(0), r.text(1)))?;
                if let Some((id, old)) = &last {
                    if similarity(old, body) >= 0.9 {
                        self.db.execute("UPDATE excerpts SET end_ms=MAX(end_ms,?) WHERE excerpt_id=?", &[int(start), text(id)])?;
                        self.db.execute("INSERT OR IGNORE INTO excerpt_sources(excerpt_id,event_id) VALUES(?,?)", &[text(id), text(shot)])?;
                        return Ok(json!({"kind": kind, "written": 0, "extended": id}));
                    }
                }
                let id = format!("ex:{}", &sha256_hex(shot.as_bytes())[..32]);
                self.db.execute(
                    "INSERT OR IGNORE INTO excerpts(excerpt_id,key,app,title,url,start_ms,end_ms,text,producer,created_ms) VALUES(?,?,?,?,?,?,?,?,?,?)",
                    &[text(&id), text(&key), opt_text(app.as_deref()), opt_text(title.as_deref()), opt_text(url.as_deref()), int(start), int(start), text(body), text(&producer), int(self.clock.now_ms())],
                )?;
                self.db.execute("INSERT OR IGNORE INTO excerpt_sources(excerpt_id,event_id) VALUES(?,?)", &[text(&id), text(shot)])?;
                Ok(json!({"kind": kind, "written": 1, "excerpt_id": id}))
            }
            _ => Ok(json!({"kind": "other"})),
        }
    }
}

/// Assigns each message a side and an order from the OCR line it matches. The conversation title's line marks the
/// left edge of the main area (a sidebar with the same name lies further left and is ignored): other people's
/// bubbles start next to it, the user's own are far to the right. Messages without a confident match keep the
/// model's side and stay after the message before them.
pub(crate) fn place_by_geometry(frame: Vec<(String, String, Option<String>)>, title: &str, ocr: &[(i64, i64, i64, String)]) -> Vec<(String, String, Option<String>)> {
    if ocr.is_empty() {
        return frame;
    }
    let best = |text: &str, pool: &[&(i64, i64, i64, String)]| -> Option<(i64, i64, i64)> {
        let top = pool.iter().map(|l| similarity(&l.3, text)).fold(0.0, f64::max);
        if top < 0.6 {
            return None;
        }
        // Among equally good lines, the rightmost: sidebars and their previews are on the left.
        pool.iter().filter(|l| similarity(&l.3, text) >= top - 0.05).max_by_key(|l| l.1).map(|l| (l.0, l.1, l.2))
    };
    let all: Vec<&(i64, i64, i64, String)> = ocr.iter().collect();
    let title = chat_title(title);
    // The header is the rightmost line naming the chat; a sidebar entry with the same name lies to its left.
    let left = if title.is_empty() { None } else { ocr.iter().filter(|l| similarity(&chat_title(&l.3), &title) >= 0.8).map(|l| l.0).max() };
    let pool: Vec<&(i64, i64, i64, String)> = match left {
        Some(l) => ocr.iter().filter(|o| o.0 >= l - 50).collect(),
        None => all.clone(),
    };
    let placed: Vec<_> = frame
        .into_iter()
        .map(|m| { let b = best(&m.1, &pool); (m, b) })
        // Text in the input box at the bottom is being typed, not sent.
        .filter(|(_, b)| b.is_none_or(|b| b.2 < INPUT_BAND_TOP))
        .collect();
    let hits: Vec<(i64, i64, i64)> = placed.iter().filter_map(|(_, b)| *b).collect();
    let side = |b: (i64, i64, i64)| -> Option<&'static str> {
        match left {
            Some(l) => Some(if b.0 < l + 100 { "other" } else { "self" }),
            None if hits.len() >= 2 => {
                let lo = hits.iter().map(|h| h.0).min()?;
                let hi = hits.iter().map(|h| h.1).max()?;
                // Left-aligned lines are other people's; only lines clearly off the left edge and reaching the
                // right one are the user's (when every message is on one side, they all start at the left edge).
                Some(if b.0 <= lo + 30 { "other" } else if b.1 >= hi - 30 && b.0 > lo + 100 { "self" } else { "other" })
            }
            None => None,
        }
    };
    let mut keyed = vec![];
    let mut last = -1.0;
    for (i, ((actor, body, sender), b)) in placed.into_iter().enumerate() {
        let actor = b.and_then(side).map(String::from).unwrap_or(actor);
        let sender = if actor == "self" { None } else { sender };
        let y = b.map(|b| b.2 as f64).unwrap_or(last + 0.5);
        last = y;
        keyed.push((y, i, (actor, body, sender)));
    }
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|k| k.2).collect()
}

/// Accepts plain JSON, or JSON inside a fenced code block, as small local models often return.
pub(crate) fn parse_json_output(raw: &str) -> Option<Value> {
    let s = raw.trim();
    let s = s.strip_prefix("```json").or_else(|| s.strip_prefix("```")).map(|t| t.trim_end().trim_end_matches("```")).unwrap_or(s).trim();
    let v: Value = serde_json::from_str(s).ok().or_else(|| {
        let (a, b) = (s.find('{')?, s.rfind('}')?);
        serde_json::from_str(&s[a..=b]).ok()
    })?;
    v.is_object().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(a: &str, t: &str) -> (String, String) {
        (a.into(), t.into())
    }

    #[test]
    fn aligns_frames_with_the_conversation_end() {
        let known = vec![m("other", "周四可以开会吗"), m("self", "可以，三点"), m("other", "好的")];
        // The frame repeats the last two and adds one.
        assert_eq!(new_from(&known, &[m("self", "可以，三点"), m("other", "好的"), m("self", "合同我晚点发你")]), Some(2));
        // Small OCR differences still match.
        assert_eq!(new_from(&known, &[m("self", "可以, 三点"), m("other", "好的"), m("other", "收到")]), Some(2));
        // A scrolled-back view adds nothing.
        assert_eq!(new_from(&known, &[m("other", "周四可以开会吗"), m("self", "可以，三点")]), None);
        // Nothing in common: everything is new.
        assert_eq!(new_from(&known, &[m("other", "另一个话题")]), Some(0));
        assert_eq!(new_from(&[], &[m("other", "x")]), Some(0));
    }

    #[test]
    fn keyframe_lines_ignore_times_and_numbers() {
        assert_eq!(meaningful("  12:47 "), None);
        assert_eq!(meaningful("3"), None);
        assert_eq!(meaningful("周四 三点"), Some("周四 三点".into()));
        assert!(similarity("周四三点可以，我把合同发你。", "周四三点可以,我把合同发你") > 0.85);
    }

    #[test]
    fn geometry_decides_sides_and_order() {
        // A sidebar on the left repeats the chat name and a message preview.
        let ocr = vec![
            (114, 273, 469, "陈凉靓".to_string()),
            (114, 273, 490, "framework是可以干很多事啊".to_string()),
            (311, 360, 25, "陈凉靓".to_string()),
            (376, 420, 70, "啥".to_string()),
            (854, 912, 145, "一个框架".to_string()),
            (681, 914, 449, "不是要部署一个需要offload的模型么".to_string()),
            (374, 559, 759, "framework是可以干很多事啊".to_string()),
        ];
        let m = |a: &str, t: &str| (a.to_string(), t.to_string(), None);
        // The model got every side wrong and the order mixed up.
        let out = place_by_geometry(vec![m("other", "framework是可以干很多事啊"), m("other", "一个框架"), m("self", "啥"), m("other", "不是要部署一个需要offload的模型么")], "陈凉靓", &ocr);
        let got: Vec<(&str, &str)> = out.iter().map(|(a, t, _)| (a.as_str(), t.as_str())).collect();
        assert_eq!(got, [("other", "啥"), ("self", "一个框架"), ("self", "不是要部署一个需要offload的模型么"), ("other", "framework是可以干很多事啊")]);
        // Without OCR the model's answer stands.
        assert_eq!(place_by_geometry(vec![m("self", "x")], "t", &[]), vec![m("self", "x")]);
    }

    #[test]
    fn chat_titles_drop_member_counts() {
        assert_eq!(chat_title("25北邮河套（4）"), "25北邮河套");
        assert_eq!(chat_title("咪咪应援会 (63)"), "咪咪应援会");
        assert_eq!(chat_title("项目(二期)"), "项目(二期)");
        assert_eq!(chat_title("（3）"), "（3）");
    }

    #[test]
    fn parses_fenced_json() {
        assert_eq!(parse_json_output("```json\n{\"kind\":\"other\"}\n```").unwrap()["kind"], "other");
        assert_eq!(parse_json_output("好的：{\"kind\":\"chat\"}").unwrap()["kind"], "chat");
        assert!(parse_json_output("no").is_none());
    }
}
