//! Reading excerpts straight from the accessibility text of `screen.text` events (no model, no screenshot): the page
//! text a browser or document app exposes. One excerpt per (app, URL or title) visit; a visit ends when the page was
//! not seen for 30 minutes. Repeated paragraphs (the same page captured again and again) are dropped with
//! `screen_cleanup`, and each excerpt keeps the events that contributed lines in `excerpt_sources`.
//!
//! Apps whose AX tree does not hold the content (chat clients, whose text is a list of bubbles) still go through the
//! screenshot excerpt (`screen_excerpt`); AI chat apps and sites are left out on purpose, since an AI's answers are
//! not stored in the readable layer; private browsing windows are left out.

use crate::curated::PROJECTION;
use crate::db::{int, opt_text, text, RowExt};
use crate::screen_cleanup::{clean, ScreenFrame};
use crate::{sha256_hex, Result, TimelineStore};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Apps whose AX tree carries the page or document body as text.
const AX_APPS: &[&str] = &[
    "Google Chrome", "Safari", "Arc", "Microsoft Edge", "Firefox", "Brave Browser", "Chromium", "Vivaldi", "Opera",
    "Notion", "Obsidian", "WPS Office", "预览", "Pages", "Microsoft Word", "Typora", "Bear",
];
/// Sites that are AI conversations; their replies stay out.
const AI_HOSTS: &[&str] = &["chatgpt.com", "chat.openai.com", "claude.ai", "gemini.google.com", "chat.deepseek.com", "grok.com", "kimi.com", "doubao.com", "poe.com", "perplexity.ai", "copilot.microsoft.com"];
const VISIT_GAP_MS: i64 = 30 * 60 * 1000;
const MAX_EXCERPT_CHARS: usize = 20_000;
/// A new visit needs this much text: shorter ones were mostly redirects, error stubs and empty app shells.
const MIN_EXCERPT_CHARS: usize = 80;
/// Pages with nothing to read: blank tabs, search result lists, sign-in and account pages.
const BLANK_TITLES: &[&str] = &["新标签页", "New Tab", "无标题", "Untitled"];

fn not_reading(title: &str, url: Option<&str>) -> bool {
    if BLANK_TITLES.contains(&title.trim()) {
        return true;
    }
    let Some(url) = url else { return false };
    let rest = url.split("://").nth(1).unwrap_or("");
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let host = host.to_ascii_lowercase();
    let search = (host.contains("google.") && path.starts_with("/search")) || (host.ends_with("bing.com") && path.starts_with("/search"))
        || (host.ends_with("baidu.com") && (path.starts_with("/s?") || path.starts_with("/s/")))
        || (host.ends_with("duckduckgo.com") && path.contains("q="));
    let account = host.starts_with("accounts.") || host.starts_with("myaccount.") || host.starts_with("login.") || host.starts_with("passport.")
        || ["/login", "/signin", "/sign-in", "/oauth", "/sso", "/auth"].iter().any(|p| path.starts_with(p));
    search || account
}

/// A line that is only a link (AX exposes hrefs of link lists) carries no reading.
fn bare_link(line: &str) -> bool {
    let l = line.trim();
    (l.starts_with("http://") || l.starts_with("https://")) && !l.contains(char::is_whitespace)
}

pub(crate) fn wants(app: &str) -> bool {
    AX_APPS.contains(&app)
}

/// A frame with what the excerpt row needs besides the text.
struct Frame {
    frame: ScreenFrame,
    url: Option<String>,
    title: Option<String>,
}

/// The window title without the app suffix and the browser's status badges.
fn clean_title(title: &str, app: &str) -> Option<String> {
    let parts: Vec<&str> = title
        .split(" - ")
        .filter(|p| *p != app && !p.contains("内存用量") && !p.contains("正在录音") && !p.contains("正在使用") && !p.trim_end().ends_with(" MB") && !p.trim_end().ends_with(" GB"))
        .collect();
    Some(parts.join(" - ")).filter(|t| !t.trim().is_empty())
}

impl TimelineStore {
    /// Projects a batch of `screen.text` rows: (event_id, start_ms, end_ms, ingested_ms, source_id, app, title, payload).
    #[allow(clippy::type_complexity)]
    pub(crate) fn project_screen_text(&self, rows: &[(String, Option<i64>, Option<i64>, i64, String, Option<String>, Option<String>, String)]) -> Result<usize> {
        let mut groups: BTreeMap<(String, String), Vec<Frame>> = BTreeMap::new();
        for (event_id, start, end, ingested, source_id, app, title, payload) in rows {
            let Some(app) = app.as_deref().filter(|a| AX_APPS.contains(a)) else { continue };
            let raw_title = title.as_deref().unwrap_or("");
            if raw_title.contains("无痕") || raw_title.contains("Incognito") || raw_title.contains("Private Browsing") || raw_title.contains("隐私浏览") {
                continue;
            }
            let p: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
            if p["text_status"] == "no_text" || p["text_status"] == "disabled" {
                continue;
            }
            let object = &p["record_evidence"]["object"];
            let url = object["key"].as_str().filter(|k| ["url", "path"].contains(&object["basis"].as_str().unwrap_or("")) && (k.starts_with("http://") || k.starts_with("https://") || k.starts_with("file://"))).map(String::from);
            if url.as_deref().is_some_and(|u| AI_HOSTS.iter().any(|h| u.split('/').nth(2).is_some_and(|host| host == *h || host.ends_with(&format!(".{h}"))))) {
                continue;
            }
            if not_reading(&clean_title(raw_title, app).unwrap_or_default(), url.as_deref()) {
                continue;
            }
            let body = self.field_text(event_id, "body")?;
            if body.is_empty() {
                continue;
            }
            let at = start.unwrap_or(*ingested);
            let key = url.clone().unwrap_or_else(|| format!("{app}\u{1f}{}", clean_title(raw_title, app).unwrap_or_default()));
            groups.entry((app.to_string(), key.clone())).or_default().push(Frame {
                frame: ScreenFrame { event_id: event_id.clone(), source_id: source_id.clone(), revision: String::new(), start_ms: at, end_ms: end.unwrap_or(at), app: app.to_string(), window: key, text: body, field: "body".into() },
                url,
                title: clean_title(raw_title, app),
            });
        }
        let mut written = 0;
        for ((app, key), mut frames) in groups {
            frames.sort_by(|a, b| (a.frame.start_ms, &a.frame.event_id).cmp(&(b.frame.start_ms, &b.frame.event_id)));
            let mut run: Vec<Frame> = vec![];
            for f in frames {
                if run.last().is_some_and(|l| f.frame.start_ms - l.frame.end_ms.max(l.frame.start_ms) > VISIT_GAP_MS) {
                    written += self.write_visit(&app, &key, std::mem::take(&mut run))?;
                }
                run.push(f);
            }
            if !run.is_empty() {
                written += self.write_visit(&app, &key, run)?;
            }
        }
        Ok(written)
    }

    /// One visit's frames: new paragraphs are appended to the visit's excerpt (created if this is a new visit).
    fn write_visit(&self, app: &str, key: &str, run: Vec<Frame>) -> Result<usize> {
        let first = run[0].frame.start_ms;
        let last_end = run.iter().map(|f| f.frame.end_ms.max(f.frame.start_ms)).max().unwrap_or(first);
        let url = run.iter().rev().find_map(|f| f.url.clone());
        let title = run.iter().rev().find_map(|f| f.title.clone());
        let open = self.db.first(
            "SELECT excerpt_id,end_ms,text FROM excerpts WHERE key=? AND excerpt_id GLOB 'ax:*' ORDER BY start_ms DESC LIMIT 1",
            &[text(key)],
            |r| (r.text(0), r.int(1), r.text(2)),
        )?;
        let open = open.filter(|(_, end, _)| first - *end <= VISIT_GAP_MS);
        let mut seen: HashSet<String> = open.as_ref().map(|(_, _, t)| t.lines().map(|l| l.trim().to_string()).collect()).unwrap_or_default();
        let mut paragraphs: Vec<String> = vec![];
        let mut contributors: Vec<String> = vec![];
        let frames = run.into_iter().map(|f| f.frame).collect();
        for episode in clean(frames, &HashSet::new()) {
            for line in episode.lines.iter().filter(|l| l.is_new && l.weight >= 0.5) {
                let t = line.text.trim().to_string();
                if t.is_empty() || bare_link(&t) || !seen.insert(t.clone()) {
                    continue;
                }
                paragraphs.push(t);
                if !contributors.contains(&line.event_id) {
                    contributors.push(line.event_id.clone());
                }
            }
        }
        let now = self.clock.now_ms();
        match open {
            Some((id, _, old)) => {
                self.db.execute("UPDATE excerpts SET end_ms=MAX(end_ms,?), title=COALESCE(title,?) WHERE excerpt_id=?", &[int(last_end), opt_text(title.as_deref()), text(&id)])?;
                let room = MAX_EXCERPT_CHARS.saturating_sub(old.chars().count());
                if paragraphs.is_empty() || room == 0 {
                    return Ok(0);
                }
                let added: String = paragraphs.join("\n").chars().take(room).collect();
                self.db.execute("UPDATE excerpts SET text=text||char(10)||? WHERE excerpt_id=?", &[text(&added), text(&id)])?;
                for event in &contributors {
                    self.db.execute("INSERT OR IGNORE INTO excerpt_sources(excerpt_id,event_id) SELECT ?,event_id FROM events WHERE event_id=?", &[text(&id), text(event)])?;
                }
                Ok(1)
            }
            None => {
                // A page that showed a line or two (a redirect, a bare URL) is not worth an excerpt yet.
                if paragraphs.iter().map(|p| p.chars().count()).sum::<usize>() < MIN_EXCERPT_CHARS {
                    return Ok(0);
                }
                let body: String = paragraphs.join("\n").chars().take(MAX_EXCERPT_CHARS).collect();
                let id = format!("ax:{}", &sha256_hex(contributors[0].as_bytes())[..32]);
                self.db.execute(
                    "INSERT OR IGNORE INTO excerpts(excerpt_id,key,app,title,url,start_ms,end_ms,text,producer,created_ms) VALUES(?,?,?,?,?,?,?,?,?,?)",
                    &[text(&id), text(key), text(app), opt_text(title.as_deref()), opt_text(url.as_deref()), int(first), int(last_end), text(&body), text(PROJECTION), int(now)],
                )?;
                for event in &contributors {
                    self.db.execute("INSERT OR IGNORE INTO excerpt_sources(excerpt_id,event_id) SELECT ?,event_id FROM events WHERE event_id=?", &[text(&id), text(event)])?;
                }
                if self.db.scalar("SELECT COUNT(*) FROM excerpt_sources WHERE excerpt_id=?", &[text(&id)])? == 0 {
                    self.db.execute("DELETE FROM excerpts WHERE excerpt_id=?", &[text(&id)])?;
                    return Ok(0);
                }
                Ok(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_without_reading_are_skipped() {
        assert!(not_reading("新标签页", None));
        assert!(not_reading("x", Some("https://www.google.com/search?q=hasselblad")));
        assert!(not_reading("x", Some("https://www.google.com.hk/search?q=a")));
        assert!(not_reading("x", Some("https://accounts.google.com/v3/signin")));
        assert!(not_reading("x", Some("https://example.com/login?next=/")));
        assert!(!not_reading("大疆收购哈苏原因", Some("https://www.zhihu.com/question/1")));
        assert!(!not_reading("Docs", Some("https://www.google.com/maps/place/x")));
        assert!(bare_link("https://inspect.aisi.org.uk/evals"));
        assert!(!bare_link("see https://a.b for details"));
    }
}
