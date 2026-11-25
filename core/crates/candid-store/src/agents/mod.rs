//! Local transcript adapters. Source files are opened only after consent is checked. Replies never persist.
mod access;
mod formats;
mod sync;
mod checkpoints;
pub(crate) use checkpoints::{CheckpointDelta, CHECKPOINT_DDL};
#[cfg(test)]
mod checkpoint_tests;
use crate::{Result, TimelineError};
pub use access::AgentRequest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const AGENTS: [&str; 3] = ["codex", "claude_code", "pi"];
/// File checkpoints written by an older parser are replayed once from the start (existing activity is matched,
/// not duplicated). /3: Codex `item_completed` UserMessage sends; /4: Claude Code `origin.kind = human`
/// prompts (both 2026-11-12).
pub(crate) const CHECKPOINT_FORMAT: &str = "agent-source-checkpoint/4";
fn yes() -> bool {
    true
}
fn one() -> i64 {
    1
}
fn name_only() -> String {
    "name_only".into()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Exclude {
    pub projects: Vec<String>,
    pub sessions: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Content {
    pub user: bool,
    pub assistant: bool,
    pub tool_calls: String,
    pub tool_output: bool,
    pub reasoning: bool,
}
impl Default for Content {
    fn default() -> Self {
        Self {
            user: yes(),
            assistant: yes(),
            tool_calls: name_only(),
            tool_output: false,
            reasoning: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AgentConsent {
    pub version: i64,
    pub agent: String,
    pub read: bool,
    pub search: bool,
    pub activity: bool,
    /// None means unlimited; 4000 remains a proposal, not a confirmed product limit.
    pub prompt_max_chars: Option<usize>,
    pub paths: Vec<String>,
    pub exclude: Exclude,
    pub content: Content,
}
impl Default for AgentConsent {
    fn default() -> Self {
        Self {
            version: one(),
            agent: String::new(),
            read: false,
            search: false,
            activity: false,
            prompt_max_chars: None,
            paths: vec![],
            exclude: Exclude::default(),
            content: Content::default(),
        }
    }
}
impl AgentConsent {
    pub fn new(agent: &str) -> Self {
        Self {
            agent: agent.into(),
            ..Default::default()
        }
    }
    fn validate(&self) -> Result<()> {
        if !AGENTS.contains(&self.agent.as_str())
            || self.version != 1
            || self.paths.len() > 32
            || !["none", "name_only"].contains(&self.content.tool_calls.as_str())
            || self.prompt_max_chars == Some(0)
        {
            return Err(TimelineError::InvalidInput("不支持的会话设置".into()));
        }
        if !self.read && (self.search || self.activity) {
            return Err(TimelineError::Denied("read_required".into()));
        }
        Ok(())
    }
    pub(crate) fn roots(&self) -> Vec<PathBuf> {
        let p = if self.paths.is_empty() {
            vec![default_path(&self.agent).into()]
        } else {
            self.paths.clone()
        };
        p.iter().map(|p| expand(p)).collect()
    }
    fn excluded(&self, key: &str, project: Option<&str>) -> bool {
        self.exclude
            .sessions
            .iter()
            .any(|s| s == key || s == &format!("agent:{}:{key}", self.agent))
            || project.is_some_and(|p| {
                self.exclude
                    .projects
                    .iter()
                    .any(|e| below(p, &expand(e).to_string_lossy()))
            })
    }
    fn require_read(&self) -> Result<()> {
        if self.read {
            Ok(())
        } else {
            Err(TimelineError::Denied("read_required".into()))
        }
    }
}
fn default_path(agent: &str) -> &'static str {
    match agent {
        "codex" => "~/.codex/sessions",
        "claude_code" => "~/.claude/projects",
        _ => "~/.pi/agent/sessions",
    }
}
fn expand(s: &str) -> PathBuf {
    if let Some(p) = s.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(p)
    } else {
        PathBuf::from(s)
    }
}
fn below(p: &str, root: &str) -> bool {
    !root.is_empty() && (p == root || p.starts_with(&(root.trim_end_matches('/').to_owned() + "/")))
}
pub fn description(agent: &str) -> Value {
    json!({"agent":agent,"displayName":match agent {"codex"=>"Codex","claude_code"=>"Claude Code",_=>"pi"},"defaultPaths":[default_path(agent)],"formatVersions":[match agent {"codex"=>"codex-rollout/1","claude_code"=>"claude-code-jsonl/1",_=>"pi-session/3"}],"supportsActivity":true})
}
fn failure(code: &str) -> TimelineError {
    TimelineError::InvalidInput(code.into())
}
fn files(c: &AgentConsent) -> Result<Vec<PathBuf>> { Ok(file_page(c)?.0) }
/// The last walk of an agent's folders: every folder it read with that folder's modification time, the transcript
/// files found, and whether the walk hit its cap.
type Listing = (Vec<(PathBuf, Option<std::time::SystemTime>)>, Vec<PathBuf>, bool);
fn listings() -> &'static std::sync::Mutex<std::collections::HashMap<String, Listing>> {
    static L: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Listing>>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}
fn modified(p: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}
fn max_depth(c: &AgentConsent) -> usize {
    if c.agent == "codex" { 8 } else { 2 }
}
/// Transcript files under the consented folders, newest first. Enumeration itself is bounded, including
/// non-transcript entries and directories. A folder's modification time moves when an entry is added, removed or
/// renamed in it, so while no folder read last time has moved the previous listing is reused without reading any
/// directory (appends to a file are found by its own size and time, see `sync_agent`).
fn file_page(c: &AgentConsent) -> Result<(Vec<PathBuf>, bool)> {
    c.require_read()?;
    // Folders below the known layout (Claude Code keeps subagent sessions one level down) are not the person's
    // conversations: they are skipped, never a reason to stop the whole source.
    fn visit(p: &Path, depth: usize, max: usize, out: &mut Vec<PathBuf>, dirs: &mut Vec<(PathBuf, Option<std::time::SystemTime>)>, remaining: &mut usize, limited: &mut bool) -> Result<()> {
        if depth > max { return Ok(()); }
        // Taken before reading, so an entry added meanwhile moves it again for the next pass.
        dirs.push((p.to_path_buf(), modified(p)));
        if !p.exists() { return Ok(()); }
        for e in fs::read_dir(p).map_err(|_| failure("unreadable"))? {
            if *remaining == 0 { *limited = true; break; }
            *remaining -= 1;
            let e = e.map_err(|_| failure("unreadable"))?;
            let ft = e.file_type().map_err(|_| failure("unreadable"))?;
            if ft.is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
                visit(&e.path(), depth + 1, max, out, dirs, remaining, limited)?;
            } else if ft.is_file() && e.path().extension().is_some_and(|x| x == "jsonl") { out.push(e.path()); }
        }
        Ok(())
    }
    let roots = c.roots();
    let key = format!("{}\u{1f}{:?}", c.agent, roots);
    let cached = listings().lock().unwrap_or_else(|p| p.into_inner()).get(&key)
        .filter(|(dirs, ..)| dirs.iter().all(|(d, m)| modified(d) == *m))
        .map(|(_, files, limited)| (files.clone(), *limited));
    let (mut out, limited) = match cached {
        Some(listing) => listing,
        None => {
            // Listing is cheap; the cap only guards against pointing a source at a huge tree by mistake.
            let mut out = vec![]; let mut dirs = vec![]; let mut remaining = 50_000; let mut limited = false;
            for root in roots.iter().take(32) { visit(root, 1, max_depth(c), &mut out, &mut dirs, &mut remaining, &mut limited)?; }
            limited |= roots.len() > 32;
            out.sort(); out.dedup();
            listings().lock().unwrap_or_else(|p| p.into_inner()).insert(key, (dirs, out.clone(), limited));
            (out, limited)
        }
    };
    // Each file's time is read once (a plain sort reads it on every comparison).
    out.sort_by_cached_key(|p| std::cmp::Reverse(modified(p)));
    Ok((out, limited))
}
/// Whether `path` is a transcript file the walk in [`file_page`] would list for this consent.
fn listed(c: &AgentConsent, path: &Path) -> bool {
    if path.extension().is_none_or(|x| x != "jsonl") {
        return false;
    }
    c.roots().iter().any(|root| {
        let Ok(rest) = path.strip_prefix(root) else { return false };
        let parts: Vec<_> = rest.components().collect();
        // depth 1 is the root itself: a file directly in it has one component, and so on.
        !parts.is_empty() && parts.len() <= max_depth(c) && parts[..parts.len() - 1].iter().all(|p| !p.as_os_str().to_string_lossy().starts_with('.'))
    })
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Usage {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
}
impl Usage {
    fn add(&mut self, u: &Self) {
        self.input += u.input;
        self.output += u.output;
        self.cache_read += u.cache_read;
        self.cache_write += u.cache_write;
        self.reasoning += u.reasoning;
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Entry {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_file: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    source_line: usize,
    id: String,
    /// Monotonic source order only; ts remains the unchanged source timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_hash: String,
    ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<i64>,
    kind: String,
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    original_chars: usize,
    #[serde(default, skip_serializing_if = "is_false")]
    truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    succeeded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}
fn is_zero(value: &usize) -> bool { *value == 0 }
fn is_false(value: &bool) -> bool { !*value }
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Session {
    #[serde(default)]
    checkpoint_format: String,
    key: String,
    project: Option<String>,
    title: Option<String>,
    model: Option<String>,
    entries: Vec<Entry>,
    #[serde(default, skip_serializing_if = "is_false")]
    entries_external: bool,
    #[serde(skip)]
    saved_entries: std::collections::BTreeMap<(String, String), Entry>,
    #[serde(skip)]
    limited: bool,
    offset: u64,
    lines: usize,
    prefix_hash: String,
    #[serde(default)]
    order_watermark: Option<i64>,
    #[serde(default)]
    fallback_counts: std::collections::BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "is_false")]
    counts_external: bool,
    #[serde(skip)]
    saved_counts: std::collections::BTreeMap<String, usize>,
    #[serde(skip)]
    reset_counts: bool,
    /// Codex thread an agent created (`thread_source` other than `user`): its "user" messages are another
    /// agent's instructions, never the person's own words.
    #[serde(default)]
    agent_thread: bool,
}
impl Session {
    /// Stable timestamp ordering keeps the source's order for simultaneous entries.
    /// A late historical fragment must not replace newer session metadata.
    fn merge_fragment(&mut self, fragment: &Session) {
        self.checkpoint_format = fragment.checkpoint_format.clone();
        self.agent_thread |= fragment.agent_thread;
        let latest = |s: &Session| s.entries.iter().map(|e| e.ts).max();
        if latest(fragment) >= latest(self) {
            if fragment.project.is_some() { self.project = fragment.project.clone(); }
            if fragment.title.is_some() { self.title = fragment.title.clone(); }
            if fragment.model.is_some() { self.model = fragment.model.clone(); }
        }
        let mut positions: std::collections::HashMap<_, _> = self.entries.iter().enumerate()
            .map(|(i, e)| ((e.id.clone(), e.kind.clone()), i)).collect();
        for e in &fragment.entries {
            let key = (e.id.clone(), e.kind.clone());
            if let Some(&i) = positions.get(&key) {
                let old = &mut self.entries[i];
                if !old.source_file.is_empty() && old.source_file != e.source_file && old.source_hash != e.source_hash { self.limited = true; }
                *old = e.clone();
            } else {
                positions.insert(key, self.entries.len());
                self.entries.push(e.clone());
            }
        }
        // Source timestamps can regress within a file. Preserve that file's
        // causal sequence while placing independent historical fragments.
        self.entries.sort_by(|a,b| (a.order_ms.unwrap_or(a.ts), &a.source_file, a.source_line).cmp(&(b.order_ms.unwrap_or(b.ts), &b.source_file, b.source_line)));
        // Live pagination must also detect edits to any fragment, not only the first file.
        let identities: Vec<_> = self.entries.iter().map(|e| (&e.id, &e.kind, &e.source_hash)).collect();
        self.prefix_hash = crate::ids::sha256_hex(crate::json::canonical_of(&identities));
        self.limited |= fragment.limited;
    }

    fn reference(&self, agent: &str) -> String {
        format!("agent:{agent}:{}", self.key)
    }
}

/// Same secret families as the former Swift connector; no dependency or source text in errors.
pub fn redact(s: &str) -> String {
    let mut s = s.to_owned();
    let mut scan=0;
    while let Some(relative)=s[scan..].find("-----BEGIN ") {
        let start=scan+relative;
        let Some(close)=s[start+11..].find("-----") else {break;};
        let header_end=start+11+close+5;
        if !s[start..header_end].contains("PRIVATE KEY") {scan=header_end;continue;}
        let end=s[header_end..].find("-----END ").and_then(|n|s[header_end+n+9..].find("-----").map(|m|header_end+n+9+m+5)).unwrap_or(s.len());
        s.replace_range(start..end,"[REDACTED]");scan=start+"[REDACTED]".len();
    }
    let mut out = String::new();
    let mut token = String::new();
    let mut bearer = false;
    let flush = |t: &mut String, out: &mut String, b: &mut bool| {
        if t.is_empty() {
            return;
        }
        let secret = (*b && t.len() >= 24)
            || [
                ("sk-", 16),
                ("AKIA", 16),
                ("ghp_", 30),
                ("gho_", 30),
                ("ghu_", 30),
                ("ghs_", 30),
                ("ghr_", 30),
                ("github_pat_", 30),
                ("xoxb-", 10),
                ("xoxp-", 10),
                ("xoxa-", 10),
                ("xoxr-", 10),
                ("xoxs-", 10),
                ("AIza", 30),
            ]
            .iter()
            .any(|(p, n)| t.starts_with(p) && t.len() >= p.len() + n);
        *b = t.eq_ignore_ascii_case("bearer");
        out.push_str(if secret { "[REDACTED]" } else { t });
        t.clear();
    };
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || "-._~+/=".contains(ch) {
            token.push(ch);
        } else {
            flush(&mut token, &mut out, &mut bearer);
            out.push(ch);
            if !ch.is_whitespace() {
                bearer = false;
            }
        }
    }
    flush(&mut token, &mut out, &mut bearer);
    out
}
