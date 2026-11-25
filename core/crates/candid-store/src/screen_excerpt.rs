//! Screenshots into the AI-readable layer (docs/plans/ai-v1-plan.md §3): pick the screenshots that show new text,
//! let the `vision` model copy out the main content, and write chat bubbles as messages and reading as excerpts.
//!
//! 1. **Keyframes** (`excerpt_scan`, rules only): per window, a screenshot is sent only when its OCR lines differ
//!    from the last screenshot sent for that window by at least two meaningful lines, or, without OCR, when a tiny
//!    grayscale thumbnail of it differs from the last one sent by a few percent of its cells. Interface text that never
//!    changes, scrolling by a line, and repeated identical frames cost nothing. A daily cap bounds the calls.
//! 2. **Excerpt** (`screen.excerpt` job): one representative frame or a short sequence of changed chat frames.
//!    Chat OCR hints come from the message pane when its layout is recognizable.
//! 3. **Merge** (`excerpt_complete`): chat messages are aligned with the end of the conversation so far and only
//!    the new tail is appended; an excerpt nearly identical to the last one of its page is skipped. Every row keeps
//!    the screenshot as its source, so deleting the screenshot removes it.
//!
//! The model's raw output is kept as `screen.excerpt/1:<screenshot>` so prompt changes can be compared.

use crate::curated::NewMessage;
use crate::db::{int, opt_text, text, RowExt};
use crate::{sha256_hex, Result, TimelineError, TimelineStore};
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) const FEATURE: &str = "screen-excerpt";
pub(crate) const JOB_KIND: &str = "screen.excerpt";
const PROMPT: &str = include_str!("../../../prompts/screen-excerpt.md");
const PROMPT_VERSION: &str = "screen-excerpt/1";
const CHAT_PROMPT: &str = include_str!("../../../prompts/screen-chat.md");
const CHAT_WAIT_MS: i64 = 30_000;
const CHAT_FRAMES: usize = 3;
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

pub(crate) fn chat_app(app: &str) -> bool {
    matches!(app, "微信" | "WeChat" | "QQ")
}

type OcrLine = (i64, i64, i64, String);

fn ocr_line(x: i64, y: i64, width: i64, body: &str) -> Option<OcrLine> {
    let right = x.checked_add(width)?;
    ((0..=1000).contains(&x) && (0..=1000).contains(&y) && (x..=1000).contains(&right))
        .then(|| (x, right, y, body.to_string()))
}

/// Infer the left edge from the chat header. This applies only to the desktop WeChat layout.
fn wechat_pane(ocr: &[OcrLine]) -> Option<(i64, i64)> {
    let left = ocr.iter()
        .find(|(x, _, y, t)| (180..=550).contains(x) && *y < 70 && meaningful(t).is_some())?.0;
    let bottom = if ocr.iter().any(|(x, _, y, t)| *x > 780 && *y > 900 && matches!(t.trim(), "发送" | "發送" | "Send")) {
        810
    } else {
        INPUT_BAND_TOP
    };
    Some((left, bottom))
}

/// (left edge, bottom) of the message pane for the chat layouts we know; `None` elsewhere.
fn chat_pane(app: &str, ocr: &[OcrLine]) -> Option<(i64, i64)> {
    match app {
        "微信" | "WeChat" => wechat_pane(ocr),
        // Desktop QQ: conversation list up to ~27% of the width, composer below ~80% of the height. Only when the
        // chat header is where the layout puts it.
        "QQ" => ocr_header(app, ocr).map(|_| (272, 800)),
        _ => None,
    }
}

fn in_chat_pane(line: &OcrLine, left: i64, bottom: i64) -> bool {
    line.0 >= left && (80..bottom).contains(&line.2)
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

/// Comparison key of a chat title: member count dropped, width and case folded, spaces and punctuation removed, and
/// the OCR look-alikes l/I folded together, so 「Al Study Group 2026 （8）」 and 「AI Study Group 2026(8)」 meet.
pub(crate) fn chat_key(title: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = title.nfkc().collect();
    chat_title(&folded).chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).map(|c| if c == 'l' { 'i' } else { c }).collect()
}

/// The chat name printed above the message pane, read from OCR (desktop WeChat and QQ layouts on the 0–1000 grid).
fn ocr_header(app: &str, lines: &[OcrLine]) -> Option<String> {
    let (x0, x1, y0, y1) = match app {
        "微信" | "WeChat" => (180, 550, 0, 70),
        "QQ" => (250, 600, 40, 130),
        _ => return None,
    };
    lines.iter().filter(|l| (x0..=x1).contains(&l.0) && (y0..y1).contains(&l.2) && meaningful(&l.3).is_some())
        .min_by_key(|l| (l.2, l.0)).map(|l| l.3.trim().to_string())
}

type ChatMessage = (String, String, Option<String>);

fn same_message(a: &ChatMessage, b: &ChatMessage) -> bool {
    let sim = similarity(&a.1, &b.1);
    let same_side = a.0 == b.0 && !matches!((&a.2, &b.2), (Some(left), Some(right)) if left != right);
    // A model's guess at whose bubble it is flips between frames; a long text does not repeat by chance, so it is
    // the same message whoever it was attributed to. Short ones (「收到」) do repeat, so they keep the side check.
    (same_side && sim >= 0.85) || (sim >= 0.9 && a.1.chars().filter(|c| c.is_alphanumeric()).count() >= 12)
}

/// How many messages at the start of `frame` are already the last messages of `known`; `None` when the frame
/// matches somewhere but not at the end (a scrolled-back view, nothing to append).
fn new_from(known: &[ChatMessage], frame: &[ChatMessage]) -> Option<usize> {
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
        Ok(sha256_hex(&serde_json::to_vec(&json!([PROMPT_VERSION, sha256_hex(self.excerpt_prompt().as_bytes()), sha256_hex(self.chat_excerpt_prompt().as_bytes()), self.model_config()?, self.feature_settings()?["items"][FEATURE]]))?))
    }

    fn chat_excerpt_prompt(&self) -> String {
        std::fs::read_to_string(self.root.join("prompts/screen-chat.md"))
            .ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| CHAT_PROMPT.to_string())
    }

    /// The OCR of a screenshot: its event id, text and whether it exists yet.
    fn shot_ocr(&self, shot: &str) -> Result<Option<(String, String)>> {
        let Some(id) = self.db.first(
            "SELECT event_id FROM events WHERE parent_event_id=? AND kind='image.text' AND sensitivity<>'protected' AND json_extract(payload_json,'$.method')='ocr' ORDER BY ingest_seq DESC LIMIT 1",
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
    pub(crate) fn shot_ocr_lines(&self, shot: &str) -> Result<Vec<OcrLine>> {
        let Some((id, body)) = self.shot_ocr(shot)? else { return Ok(vec![]) };
        self.ocr_lines(&id, &body)
    }

    fn ocr_lines(&self, id: &str, body: &str) -> Result<Vec<OcrLine>> {
        let payload: Value = self.db.first("SELECT payload_json FROM events WHERE event_id=?", &[text(id)], |r| r.text(0))?.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or(Value::Null);
        if let Some(boxes) = payload["boxes"].as_array().filter(|b| b.len() == body.split('\n').count()) {
            return Ok(body.split('\n').zip(boxes).filter_map(|(line, b)| {
                ocr_line(b[0].as_i64()?, b[1].as_i64()?, b[2].as_i64()?, line)
            }).collect());
        }
        // The macOS app stores the complete recognition result as a derived blob.
        let Some(result_ref) = payload["result_ref"].as_str() else { return Ok(vec![]) };
        let blobs = self.db.query(
            "SELECT b.rel_path,b.mime_type FROM event_blobs eb JOIN blobs b USING(sha256)
             WHERE eb.event_id=? AND eb.role='derived' AND b.mime_type IN ('application/json','application/json+deflate') AND b.byte_count<=1048576",
            &[text(id)],
            |r| (r.text(0), r.text(1)),
        )?;
        let Some(result) = blobs.into_iter().find_map(|(path, mime)| {
            let bytes = std::fs::read(self.root.join(path)).ok()?;
            let (json, _) = crate::json_blobs::logical_bytes(bytes, &mime).ok()?;
            if sha256_hex(&json) != result_ref {
                return None;
            }
            serde_json::from_slice::<Value>(&json).ok()
        }) else { return Ok(vec![]) };
        Ok(result["ocrLines"].as_array().into_iter().flatten().filter_map(|line| {
            ocr_line(line["x"].as_i64()?, line["y"].as_i64()?, line["width"].as_i64()?, line["text"].as_str()?)
        }).collect())
    }

    pub(crate) fn chat_ocr_preview(&self, id: &str, body: &str) -> Result<Vec<String>> {
        let mut lines = self.ocr_lines(id, body)?;
        let Some((left, bottom)) = wechat_pane(&lines) else { return Ok(vec![]) };
        lines.sort_by_key(|l| (l.2, l.0));
        Ok(lines.into_iter().filter(|l| in_chat_pane(l, left, bottom))
            .filter_map(|l| meaningful(&l.3)).collect())
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
            "SELECT event_id,COALESCE(start_ms,ingested_ms),app_name,title,ingest_seq,source_id,json_extract(payload_json,'$.legacy.activity_id'),parent_event_id
             FROM events WHERE kind='screen.shot' AND ingest_seq>? AND sensitivity<>'protected' AND COALESCE(start_ms,ingested_ms)>=? AND source_id IN ({})",
            crate::db::placeholders(sources.len())
        );
        sql += " ORDER BY ingest_seq LIMIT ?";
        let mut params = vec![int(cursor), int(since)];
        params.extend(sources.iter().map(text));
        params.push(int(BATCH));
        let shots = self.db.query(&sql, &params, |r| (r.text(0), r.int(1), r.text(2), r.text(3), r.int(4), r.text(5), r.opt_text(6), r.opt_text(7)))?;
        let version = self.excerpt_version()?;
        let batching = self.automation_get("excerpt.chat-batch/1")? != json!(false);
        let (mut last, mut queued) = (cursor, 0);
        for (shot, start, app, title, seq, source, activity, parent) in shots {
            last = seq;
            if crate::curated_screen::wants(&app) && self.db.scalar(
                "SELECT COUNT(*) FROM excerpt_sources WHERE event_id=?", &[opt_text(parent)],
            )? > 0 {
                continue;
            }
            let window = format!("excerpt.window/2:{}", sha256_hex(&serde_json::to_vec(&json!([source, app, title, activity]))?));
            let state = self.automation_get(&window)?;
            let chat = chat_app(&app);
            let encoder = base64::engine::general_purpose::STANDARD;
            let previous_thumb = state["thumb"].as_str().and_then(|thumb| encoder.decode(thumb).ok()).unwrap_or_default();
            let (fresh, lines, thumb) = match self.shot_ocr(&shot)? {
                Some((id, ocr)) if !ocr.trim().is_empty() => {
                    let mut thumbnail = None;
                    let current = if matches!(app.as_str(), "微信" | "WeChat") {
                        let boxes = self.ocr_lines(&id, &ocr)?;
                        wechat_pane(&boxes).map(|(left, bottom)| {
                            let body = boxes.iter().filter(|l| in_chat_pane(l, left, bottom))
                                .map(|l| l.3.as_str()).collect::<Vec<_>>().join("\n");
                            if let Some(thumb) = self.shot_thumbnail(&shot)? {
                                let width = crate::webp::THUMB_W;
                                let height = crate::webp::THUMB_H;
                                let x = left as usize * width / 1000;
                                thumbnail = Some(thumb.chunks_exact(width).skip(height * 8 / 100)
                                    .take((bottom as usize - 80) * height / 1000)
                                    .flat_map(|row| row[x..].iter().copied()).collect::<Vec<_>>());
                            }
                            Ok::<_, TimelineError>(line_hashes(&body))
                        }).transpose()?.unwrap_or_else(|| line_hashes(&ocr))
                    } else {
                        line_hashes(&ocr)
                    };
                    let previous: HashSet<&str> = state["lines"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    let new: Vec<_> = current.iter().filter(|(h, _)| !previous.contains(h.as_str())).collect();
                    // Even a single short chat reply can be a real state change.
                    let image_changed = thumbnail.as_ref().is_some_and(|thumb| {
                        (previous_thumb.is_empty() && state.is_null())
                            || (previous_thumb.len() == thumb.len()
                                && crate::webp::thumbnail_change(&previous_thumb, thumb) >= 0.04)
                    });
                    let fresh = if chat { !new.is_empty() || image_changed } else {
                        new.len() >= MIN_NEW_LINES && new.iter().map(|(_, n)| n).sum::<usize>() >= MIN_NEW_CHARS
                    };
                    (fresh, json!(current.iter().map(|(h, _)| h.as_str()).take(600).collect::<Vec<_>>()), json!(thumbnail.map(|thumb| encoder.encode(thumb))))
                }
                _ => {
                    let Some(thumb) = self.shot_thumbnail(&shot)? else { continue };
                    (crate::webp::thumbnail_change(&previous_thumb, &thumb) >= MIN_THUMB_CHANGE, Value::Null, json!(encoder.encode(&thumb)))
                }
            };
            // The daily limit is applied when a job is picked up (`feature_budget`), so frames over the limit wait in
            // the queue and are dropped by `expire_stale_jobs` after a day, instead of being lost here.
            if !fresh {
                continue;
            }
            // Several changed chat frames are read in one call unless switched off (store_meta `excerpt.chat-batch/1`
            // = false): one frame per call keeps the task simple for a small model.
            let has_image = self.shot_blob(&shot)?.is_some() && batching;
            let inserted = self.db.transaction(|| {
                let now = self.clock.now_ms();
                let candidate = state["job"].as_str().filter(|_| chat && has_image && state["version"] == version
                    && state["start"].as_i64().is_some_and(|old| (old..=old + CHAT_WAIT_MS).contains(&start)));
                if let Some(job) = candidate {
                    if self.db.scalar("SELECT COUNT(*) FROM jobs WHERE job_id=? AND state='queued' AND attempts=0 AND next_run_ms>?",
                        &[text(job), int(now)])? > 0 {
                        let mut pending = self.automation_get(&format!("excerpt.batch/2:{job}"))?;
                        if let Some(frames) = pending.as_array_mut().filter(|f| f.len() < CHAT_FRAMES) {
                            if !frames.iter().any(|f| f == &shot) {
                                frames.push(json!(shot));
                            }
                            if frames.len() == CHAT_FRAMES {
                                self.db.execute("UPDATE jobs SET next_run_ms=? WHERE job_id=?", &[int(now), text(job)])?;
                            }
                            self.automation_put(&format!("excerpt.batch/2:{job}"), &pending)?;
                            self.automation_put(&window, &json!({"lines":lines,"thumb":thumb,"job":job,"start":state["start"],"version":version}))?;
                            self.automation_put(CURSOR, &json!(seq))?;
                            return Ok(false);
                        }
                    }
                }
                let delay_until = (chat && has_image).then_some(now + CHAT_WAIT_MS);
                let job = self.enqueue_job(JOB_KIND, &shot, &version, Some(FEATURE.into()), delay_until)?;
                if chat && has_image {
                    self.automation_put(&format!("excerpt.batch/2:{job}"), &json!([shot]))?;
                }
                self.automation_put(&window, &json!({"lines":lines,"thumb":thumb,"job":job,"start":start,"version":version}))?;
                self.automation_put(CURSOR, &json!(seq))?;
                Ok(true)
            })?;
            queued += usize::from(inserted);
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
        let batch = self.automation_get(&format!("excerpt.batch/2:{}", job["id"].as_str().unwrap_or("")))?;
        let frames: Vec<&str> = match batch.as_array() {
            Some(frames) => frames.iter().map(|f| f.as_str().ok_or_else(|| bad("invalid_input"))).collect::<Result<_>>()?,
            None => vec![shot],
        };
        if frames.first() != Some(&shot) || frames.len() > CHAT_FRAMES {
            return Err(bad("invalid_input"));
        }
        let use_chat_prompt = batch.is_array();
        let prompt = if use_chat_prompt { self.chat_excerpt_prompt() } else { self.excerpt_prompt() };
        let mut parts = Vec::new();
        let mut context = String::new();
        let mut revisions = Vec::new();
        let mut image_bytes = 0;
        let mut seen = HashSet::new();
        for (index, &frame) in frames.iter().enumerate() {
            let revision = self.excerpt_frame_revision(frame, &source)?;
            revisions.push(revision);
            let input = if use_chat_prompt {
                let meta = self.db.first("SELECT app_name,title FROM events WHERE event_id=?", &[text(frame)], |r| (r.text(0), r.text(1)))?
                    .ok_or_else(|| bad("source_changed"))?;
                let mut hints = Vec::new();
                if matches!(meta.0.as_str(), "微信" | "WeChat") {
                    let boxes = self.shot_ocr_lines(frame)?;
                    if let Some((left, bottom)) = wechat_pane(&boxes) {
                        hints.extend(boxes.iter().filter(|l| in_chat_pane(l, left, bottom))
                            .filter(|l| {
                                let key: String = l.3.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
                                key.chars().count() >= 4 && seen.insert(key)
                            }).take(12).map(|l| l.3.chars().take(120).collect::<String>()));
                    }
                }
                format!("第 {} 帧，应用 {}，窗口 {}\n聊天区 OCR 校对提示\n{}", index + 1, meta.0, meta.1, hints.join("\n"))
            } else {
                self.excerpt_parts(frame)?["context"].as_str().unwrap_or("").to_string()
            };
            context.push_str(&input);
            context.push('\n');
            parts.push(json!({"type":"text","text":input}));
            let (path, mime) = self.shot_blob(frame)?.ok_or_else(|| bad("source_changed"))?;
            let path = self.root.join(path);
            let size = std::fs::metadata(&path).map_err(|_| bad("source_changed"))?.len();
            image_bytes += size;
            if image_bytes > 16 * 1024 * 1024 {
                return Err(bad("input_limit"));
            }
            let bytes = std::fs::read(path).map_err(|_| bad("source_changed"))?;
            let (bytes, mime) = crate::webp::for_model(bytes, &mime, crate::feature_settings::is_loopback(provider["base_url"].as_str().unwrap_or("")));
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            parts.push(json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{encoded}")}}));
        }
        let mut message = json!({"type":"object","properties":{
            "from":{"type":"string","enum":["me","other"]},"sender":{"type":"string"},"text":{"type":"string"}
        },"required":["from","sender","text"],"additionalProperties":false});
        if use_chat_prompt {
            message["properties"]["from"]["enum"] = json!(["me","other","unknown"]);
            message["properties"]["frame"] = json!({"type":"integer","minimum":1,"maximum":frames.len()});
            message["required"].as_array_mut().unwrap().push(json!("frame"));
        }
        let kinds = if use_chat_prompt { json!(["chat","other"]) } else { json!(["chat","doc","web","other"]) };
        let schema = json!({"type":"object","properties":{
            "kind":{"type":"string","enum":kinds},"title":{"type":"string"},
            "messages":{"type":"array","items":message},"excerpt":{"type":"string"}
        },"required":["kind","title","messages","excerpt"],"additionalProperties":false});
        self.workflow_budget(&c, a, prompt.len() + context.len())?;
        Ok(json!({
            "job": job, "speech": false, "step": FEATURE, "provider": provider, "assignment": a,
            "prompt": prompt, "content": context, "json_mode": true, "response_schema": schema, "max_tokens": 4096,
            "messages": [{"role":"system","content":prompt},{"role":"user","content":parts}],
            "frames":frames,"frame_revisions":revisions,"source":source,"chat_batch":use_chat_prompt,
            "source_revision": shot, "config_revision": sha256_hex(&serde_json::to_vec(&c)?), "feature_revision": self.feature_settings()?["items"][FEATURE],
        }))
    }

    fn excerpt_frame_revision(&self, frame: &str, source: &str) -> Result<String> {
        let row = self.db.first(
            "SELECT e.app_name,e.app_bundle_id,e.title,s.policy_json FROM events e JOIN sources s USING(source_id)
             WHERE e.event_id=? AND e.source_id=? AND e.kind='screen.shot' AND e.sensitivity<>'protected'",
            &[text(frame), text(source)], |r| (r.text(0), r.text(1), r.text(2), r.text(3)),
        )?.ok_or_else(|| bad("source_changed"))?;
        let policy: crate::SourcePolicy = serde_json::from_str(&row.3)?;
        if policy.exclude.apps.iter().any(|a| a == &row.0 || a == &row.1)
            || !policy.exclude.threads.is_empty()
            || !policy.exclude.patterns.is_empty() {
            return Err(TimelineError::Denied("source_unavailable".into()));
        }
        Ok(sha256_hex(&serde_json::to_vec(&json!([self.fingerprint_stored(frame)?, row.3]))?))
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
        let source = r["source"].as_str().ok_or_else(|| bad("source_changed"))?;
        if !self.excerpt_sources()?.iter().any(|s| s == source) {
            return Err(TimelineError::Denied("source_unavailable".into()));
        }
        if !crate::feature_settings::is_loopback(r["provider"]["base_url"].as_str().unwrap_or(""))
            && !self.automation_config()?.remote_sources.iter().any(|s| s == source) {
            return Err(TimelineError::Denied("waiting_authorization".into()));
        }
        let frames = r["frames"].as_array().ok_or_else(|| bad("source_changed"))?;
        if frames.first().and_then(Value::as_str) != Some(shot) {
            return Err(bad("source_changed"));
        }
        for (index, frame) in frames.iter().enumerate() {
            let revision = self.excerpt_frame_revision(frame.as_str().ok_or_else(|| bad("source_changed"))?, source)?;
            if r["frame_revisions"][index] != revision {
                return Err(bad("source_changed"));
            }
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
            let written = if r["chat_batch"] == true {
                let frames = r["frames"].as_array().ok_or_else(|| bad("invalid_input"))?;
                self.write_chat_batch(frames, &parsed, model)?
            } else {
                self.excerpt_write(shot, &parsed, model)?
            };
            self.automation_put(&format!("screen.excerpt/1:{shot}"), &json!({
                "output": parsed, "written": written, "frames": r["frames"], "model": model,
                "prompt": sha256_hex(r["prompt"].as_str().unwrap_or("").as_bytes()),
            }))?;
            self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=? WHERE job_id=?", &[text(format!("excerpt:{shot}")), text(r["job"]["id"].as_str().unwrap_or(""))])?;
            Ok(written)
        })?;
        Ok(result)
    }

    fn write_chat_batch(&self, frames: &[Value], output: &Value, model: &str) -> Result<Value> {
        if output["kind"] == "other" {
            return Ok(json!({"kind":"other"}));
        }
        if output["kind"] != "chat" || output["title"].as_str().is_none_or(|t| t.trim().is_empty()) {
            return Err(bad("invalid_output"));
        }
        let mut by_frame = vec![Vec::new(); frames.len()];
        // One malformed message (a frame number out of range, an unknown side) costs that message, not the batch.
        for message in output["messages"].as_array().ok_or_else(|| bad("invalid_output"))? {
            let Some(number) = message["frame"].as_u64().filter(|n| (1..=frames.len() as u64).contains(n)) else { continue };
            if !matches!(message["from"].as_str(), Some("me" | "other" | "unknown")) || !message["text"].is_string() {
                continue;
            }
            by_frame[number as usize - 1].push(message);
        }
        let mut count = 0;
        for (frame, messages) in frames.iter().zip(by_frame) {
            if messages.is_empty() {
                continue;
            }
            let written = self.excerpt_write(frame.as_str().ok_or_else(|| bad("invalid_input"))?,
                &json!({"kind":"chat","title":output["title"],"messages":messages}), model)?;
            count += written["appended"].as_u64().unwrap_or(0);
        }
        Ok(json!({"kind":"chat","appended":count}))
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
                        let actor = match m["from"].as_str() { Some("me") => "self", Some("unknown") => "unknown", _ => "other" };
                        let sender = m["sender"].as_str().map(str::trim).filter(|s| !s.is_empty() && actor == "other").map(String::from);
                        Some((actor.to_string(), body.to_string(), sender))
                    })
                    .collect();
                if frame.is_empty() {
                    return Ok(json!({"kind": "chat", "appended": 0}));
                }
                // Models are unreliable about left and right; where OCR boxes exist, sides and order come from them.
                let lines = self.shot_ocr_lines(shot)?;
                let pane = chat_pane(app.as_deref().unwrap_or(""), &lines);
                let image = match (&pane, app.as_deref()) {
                    (Some(_), Some("微信" | "WeChat")) => self.shot_blob(shot)?.and_then(|(path, _)| std::fs::read(self.root.join(path)).ok()).and_then(|b| crate::webp::decode_rgb(&b)),
                    _ => None,
                };
                let frame = place_by_geometry(frame, title.as_deref().unwrap_or(""), &lines, pane, &|b| image.as_ref().is_some_and(|img| green_bubble(img, b)));
                let app_name = app.clone().unwrap_or_default();
                let tail = |id: &str| self.db.query("SELECT actor,text,sender FROM (SELECT actor,text,sender,sent_ms,seq FROM messages WHERE conversation_id=? ORDER BY sent_ms DESC,seq DESC LIMIT 60) ORDER BY sent_ms,seq", &[text(id)], |r| (r.text(0), r.text(1), r.opt_text(2)));
                // Which chat this is: the header OCR read, else the model's title. The app's own name (WeChat's and
                // QQ's window title) identifies nothing.
                let placeholder = |t: &str| chat_key(t).is_empty() || chat_key(t) == chat_key(&app_name);
                let named = ocr_header(&app_name, &lines).or_else(|| out["title"].as_str().map(str::trim).map(String::from)).filter(|t| !placeholder(t));
                let recent = self.db.query(
                    "SELECT conversation_id,COALESCE(title,''),last_ms FROM conversations WHERE kind='chat' AND app IS ? ORDER BY last_ms DESC LIMIT 500",
                    &[opt_text(app.as_deref())], |r| (r.text(0), r.text(1), r.int(2)),
                )?;
                let (conversation_id, title) = match &named {
                    Some(t) => {
                        let key = chat_key(t);
                        let same = recent.iter().find(|(_, other, _)| chat_key(other) == key)
                            .or_else(|| recent.iter().find(|(_, other, _)| similarity(&chat_key(other), &key) >= 0.85));
                        match same {
                            Some((id, _, _)) => (id.clone(), Some(t.clone())),
                            None => (format!("chat:{}", &sha256_hex(format!("{app_name}\u{1f}{key}").as_bytes())[..24]), Some(t.clone())),
                        }
                    }
                    // No name on screen: only a frame that continues the chat seen last is kept.
                    None => {
                        let last = recent.iter().find(|(_, _, at)| start - *at <= 15 * 60_000);
                        match last {
                            Some((id, other, _)) if new_from(&tail(id)?, &frame).is_some_and(|k| k > 0) => (id.clone(), Some(other.clone())),
                            _ => return Ok(json!({"kind": "chat", "appended": 0, "note": "untitled"})),
                        }
                    }
                };
                let known: Vec<ChatMessage> = tail(&conversation_id)?;
                let Some(skip) = new_from(&known, &frame) else {
                    return Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": 0, "note": "scrolled_back"}));
                };
                if skip >= frame.len() {
                    return Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": 0}));
                }
                // New messages are stamped at the screenshot, in order, after anything already there.
                // Frames overlap and the model copies the same bubble slightly differently each time: anything that
                // matches one of the recent messages is a repeat, wherever the alignment put it.
                let frame: Vec<_> = frame.into_iter().skip(skip).filter(|m| !known.iter().any(|k| same_message(k, m))).collect();
                if frame.is_empty() {
                    return Ok(json!({"kind": "chat", "conversation_id": conversation_id, "appended": 0}));
                }
                let floor = self.db.scalar("SELECT COALESCE(MAX(sent_ms),0) FROM messages WHERE conversation_id=?", &[text(&conversation_id)])?;
                let base = start.max(floor + 1);
                self.upsert_conversation(&conversation_id, "chat", app.as_deref(), title.as_deref(), base, &producer)?;
                let mut appended = 0;
                for (i, (actor, body, sender)) in frame.into_iter().enumerate() {
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
/// Desktop WeChat draws the user's own text bubbles green (light: about RGB 159,240,161; dark mode is a deeper
/// green). A pixel just left of an OCR line, inside its bubble, says whose bubble it is; a white or grey one says
/// nothing (other people's bubbles, and pictures, are both light).
fn green_bubble(img: &(usize, usize, Vec<u8>), b: (i64, i64, i64)) -> bool {
    let (w, h, px) = img;
    let x = (b.0 - 6).clamp(0, 999) as usize * w / 1000;
    let y = (b.2 + 10).clamp(0, 999) as usize * h / 1000;
    let i = (y * w + x) * 3;
    let Some(p) = px.get(i..i + 3) else { return false };
    let (r, g, bl) = (p[0] as i32, p[1] as i32, p[2] as i32);
    g > 150 && g - r > 40 && g - bl > 40
}

fn place_by_geometry(frame: Vec<ChatMessage>, title: &str, ocr: &[OcrLine], pane: Option<(i64, i64)>, own: &dyn Fn((i64, i64, i64)) -> bool) -> Vec<ChatMessage> {
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
    let left = pane.map(|(left, _)| left).or_else(|| {
        ocr.iter().filter(|l| !title.is_empty() && l.2 < 80 && similarity(&chat_title(&l.3), &title) >= 0.8).map(|l| l.0).max()
    });
    let pool: Vec<&(i64, i64, i64, String)> = match left {
        Some(l) => ocr.iter().filter(|o| o.0 >= l - 50).collect(),
        None => all.clone(),
    };
    let mut used = HashSet::new();
    let placed: Vec<_> = frame.into_iter().filter_map(|m| {
        if m.1 == "[图片]" {
            return Some((m, None));
        }
        let message_pool: Vec<_> = pool.iter().copied().filter(|l| {
            pane.is_none_or(|(left, bottom)| in_chat_pane(l, left, bottom))
                && !used.contains(&(l.0, l.1, l.2))
        }).collect();
        if let Some(b) = best(&m.1, &message_pool) {
            if b.2 >= pane.map(|(_, bottom)| bottom).unwrap_or(INPUT_BAND_TOP) {
                return None;
            }
            used.insert(b);
            // The model's text is kept: OCR places a bubble but is no better at characters (Apple Vision drops 「的」
            // from Chinese lines consistently, whatever the scale or language settings).
            return Some((m, Some(b)));
        }
        // A line seen only in a sidebar, composer, or an already matched bubble is not new evidence.
        if pane.is_some() && all.iter().any(|l| similarity(&l.3, &m.1) >= 0.9) {
            return None;
        }
        // A long bubble wraps over several OCR lines, so no single line matches; its text must still be in the pane.
        // Text that is nowhere in the pane was misread or made up (a group name read as a message, say).
        let placeholder = m.1.starts_with('[') && m.1.ends_with(']');
        if let (Some((left, bottom)), false) = (pane, placeholder) {
            let in_pane: Vec<&str> = ocr.iter().filter(|o| in_chat_pane(o, left, bottom)).map(|o| o.3.as_str()).collect();
            if coverage(&m.1, &in_pane) < 0.5 {
                return None;
            }
        }
        Some((m, None))
    }).collect();
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
        // Colour first (a green bubble is the user's), then the side of the pane, then the model's guess.
        let actor = match b {
            Some(b) if own(b) => "self".to_string(),
            _ => b.and_then(side).map(String::from).unwrap_or(actor),
        };
        let sender = if actor == "self" { None } else { sender };
        let y = b.map(|b| b.2 as f64).unwrap_or(last + 0.5);
        last = y;
        keyed.push((y, i, (actor, body, sender)));
    }
    // Keep a picture or another unmatched bubble in the order reported by the model.
    if hits.len() == keyed.len() {
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    keyed.into_iter().map(|k| k.2).collect()
}

/// Share of a message's character pairs found in the given OCR lines.
fn coverage(text: &str, lines: &[&str]) -> f64 {
    let pairs = |s: &str| {
        let v: Vec<char> = s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
        v.windows(2).map(|w| (w[0], w[1])).collect::<HashSet<_>>()
    };
    let want = pairs(text);
    if want.is_empty() {
        return 1.0;
    }
    let have: HashSet<_> = lines.iter().flat_map(|l| pairs(l)).collect();
    want.iter().filter(|p| have.contains(p)).count() as f64 / want.len() as f64
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

    fn m(a: &str, t: &str) -> ChatMessage {
        (a.into(), t.into(), None)
    }

    #[test]
    fn chat_titles_meet_despite_ocr_variants() {
        // Seen on one group within an hour: OCR and model readings differ in l/I, spacing, width and member count.
        let k = chat_key("AI Study Group 2026 (8)");
        for v in ["AlStudy Group 2026（8）", "Al Study Group 2026 （8）", "AI Study Group 2026"] {
            assert_eq!(chat_key(v), k, "{v}");
        }
        assert_ne!(chat_key("sunny day"), chat_key("读书会（6）"));
        assert!(similarity(&chat_key("sunny days"), &chat_key("sunny dayz")) >= 0.85);
        let header = vec![(20, 300, 12, "搜索".to_string()), (310, 600, 40, "周末羽毛球（4）".to_string()), (310, 500, 300, "收到".to_string())];
        assert_eq!(ocr_header("微信", &header).as_deref(), Some("周末羽毛球（4）"));
        assert_eq!(ocr_header("Chrome", &header), None);
    }

    #[test]
    fn green_bubbles_are_the_users() {
        // 10×10 image: white, with a green bubble on the right half from row 4 down.
        let mut px = vec![255u8; 10 * 10 * 3];
        for y in 4..10 {
            for x in 5..10 {
                px[(y * 10 + x) * 3..(y * 10 + x) * 3 + 3].copy_from_slice(&[159, 240, 161]);
            }
        }
        let img = (10, 10, px);
        assert!(green_bubble(&img, (600, 900, 500)));
        assert!(!green_bubble(&img, (100, 400, 500)));
        assert!(!green_bubble(&img, (600, 900, 100)));
        let ocr = vec![(600, 900, 500, "我发的".to_string()), (100, 400, 600, "对方发的".to_string())];
        let out = place_by_geometry(vec![m("other", "我发的"), m("other", "对方发的")], "t", &ocr, None, &|b| green_bubble(&img, b));
        assert_eq!(out[0].0, "self");
    }

    #[test]
    fn long_messages_dedupe_across_side_flips() {
        let long = "我觉得关键是先把需求说清楚，然后再讨论排期和分工的事情";
        assert!(same_message(&m("other", long), &m("self", long)));
        assert!(!same_message(&m("other", "收到"), &m("self", "收到")));
        assert!(same_message(&m("other", "收到"), &m("other", "收到")));
    }

    #[test]
    fn wrapped_bubbles_are_found_and_made_up_ones_are_not() {
        let lines = ["这个版本先跑通了主流程，接下来要把导入和导出的格式统一一下，然", "后补上失败重试和日志"];
        assert!(coverage("这个版本先跑通了主流程，接下来要把导入和导出的格式统一一下，然后补上失败重试和日志", &lines) > 0.9);
        assert!(coverage("周来今天开全吗", &["周末今天开会吗 (1200)", "请先看置顶公告再提问"]) < 0.5);
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
        let out = place_by_geometry(vec![m("other", "framework是可以干很多事啊"), m("other", "一个框架"), m("self", "啥"), m("other", "不是要部署一个需要offload的模型么")], "陈凉靓", &ocr, None, &|_| false);
        let got: Vec<(&str, &str)> = out.iter().map(|(a, t, _)| (a.as_str(), t.as_str())).collect();
        assert_eq!(got, [("other", "啥"), ("self", "一个框架"), ("self", "不是要部署一个需要offload的模型么"), ("other", "framework是可以干很多事啊")]);
        // Without OCR the model's answer stands.
        assert_eq!(place_by_geometry(vec![m("self", "x")], "t", &[], None, &|_| false), vec![m("self", "x")]);
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
