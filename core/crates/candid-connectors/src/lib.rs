//! Ingest connectors in the Rust core (timeline-v1 §6.1): mail (Mail.app data and IMAP) and browser history.
//! Every connector writes only through `TimelineStore::ingest`. Network and file reads happen without holding the
//! store lock: plan (locked) → read (unlocked) → commit (locked, one transaction with the cursor).
pub mod browser;
pub mod mail;

use mail::config::MailConfig;
use candid_store::{ConnectorSource, IngestBatch, IngestCursor, SourcePolicy, TimelineError, TimelineStore};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const SOURCE_KINDS: [&str; 3] = ["mail.apple", "mail.imap", "browser.history"];
const DRY_RUN_LIMIT: usize = 5000;

/// A failed command: HTTP-ish status and a bounded code (no message text, no source content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub status: u16,
    pub code: String,
}

impl Failure {
    fn new(status: u16, code: impl Into<String>) -> Self {
        Failure { status, code: code.into() }
    }
}

impl From<TimelineError> for Failure {
    fn from(e: TimelineError) -> Self {
        let status = match e {
            TimelineError::UnknownSource(_) | TimelineError::NotFound(_) => 404,
            TimelineError::InvalidInput(_) => 400,
            TimelineError::SourceNotWritable(_) | TimelineError::Denied(_) => 409,
            _ => 500,
        };
        Failure::new(status, e.code())
    }
}

type Shared = Mutex<TimelineStore>;

fn lock(store: &Shared) -> std::sync::MutexGuard<'_, TimelineStore> {
    store.lock().unwrap_or_else(|p| p.into_inner())
}

// MARK: Secrets (process memory only)

static SECRETS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Replaces the in-memory credentials keyed by `secret_ref` (the App reads them from the Keychain). `null` removes.
pub fn set_secrets(values: &serde_json::Map<String, Value>) {
    let mut m = SECRETS.get_or_init(Default::default).lock().unwrap();
    for (k, v) in values {
        match v.as_str() {
            Some(s) if !s.is_empty() => {
                m.insert(k.clone(), s.to_string());
            }
            _ => {
                m.remove(k);
            }
        }
    }
}

fn secret(reference: &str) -> Option<String> {
    SECRETS.get_or_init(Default::default).lock().unwrap().get(reference).cloned()
}

// MARK: Describe

pub fn describe() -> Value {
    json!([
        {"kind": "mail.apple", "display_name": "邮件 App", "adapter": "apple_mail", "platform": "macos",
         "permissions": ["full_disk_access"], "event_kinds": ["mail.message"], "thread_kinds": ["mail_thread"],
         "formats": [mail::apple::FORMAT], "default_config": MailConfig::default(), "default_policy": default_policy("mail.apple"),
         "read_only": true, "import": "enabled_after_authorize"},
        {"kind": "mail.imap", "display_name": "IMAP 邮箱", "adapter": "imap", "platform": "all",
         "permissions": ["credentials:secret_ref"], "event_kinds": ["mail.message"], "thread_kinds": ["mail_thread"],
         "formats": [mail::imap::FORMAT], "default_config": MailConfig { imap: Some(Default::default()), ..Default::default() },
         "default_policy": default_policy("mail.imap"), "read_only": true, "import": "enabled_after_authorize",
         "notes": "Gmail: imap.gmail.com:993 with an app password"},
        {"kind": "browser.history", "display_name": "浏览器历史", "browsers": browser::BROWSERS,
         "permissions": ["full_disk_access (Safari)"], "event_kinds": ["browser.visit"], "thread_kinds": [],
         "formats": ["safari-history/1", "chrome-history/1"], "default_config": browser::BrowserConfig::default(),
         "default_policy": default_policy("browser.history"), "read_only": true, "import": "disabled"}
    ])
}

/// Mail text is stored for the user; external clients only see it when granted this source explicitly.
fn default_policy(kind: &str) -> SourcePolicy {
    SourcePolicy { store_text: kind != "browser.history", store_media: false, agent_visible: false, ..Default::default() }
}

// MARK: Status

fn needs(s: &ConnectorSource) -> Option<&'static str> {
    match s.last_error.as_deref() {
        Some("full_disk_access_required") => Some("full_disk_access"),
        Some("waiting_authorization") | Some("imap_auth_failed") => Some("credentials"),
        Some("unknown_mail_version") | Some("unknown_mail_schema") | Some("unknown_history_schema") => Some("unsupported_format"),
        _ => None,
    }
}

fn source_json(s: &ConnectorSource) -> Value {
    json!({
        "source_id": s.source_id, "kind": s.kind, "display_name": s.display_name, "account_ref": s.account_ref,
        "secret_ref": s.secret_ref, "has_secret": s.secret_ref.as_deref().map(|r| secret(r).is_some()),
        "config": s.config, "policy": s.policy, "status": s.status, "last_sync_ms": s.last_sync_ms,
        "last_error": s.last_error, "needs": needs(s), "events": s.events,
    })
}

pub fn list(store: &TimelineStore) -> Result<Value, Failure> {
    Ok(json!({"sources": store.connector_sources(&SOURCE_KINDS)?.iter().map(source_json).collect::<Vec<_>>()}))
}

// MARK: Commands

fn parse_mail(kind: &str, v: &Value) -> Result<MailConfig, Failure> {
    let c: MailConfig = serde_json::from_value(v.clone()).map_err(|_| Failure::new(400, "invalid_config"))?;
    c.validate(kind).map_err(|code| Failure::new(400, code))?;
    Ok(c)
}

fn parse_browser(v: &Value) -> Result<browser::BrowserConfig, Failure> {
    let c: browser::BrowserConfig = serde_json::from_value(v.clone()).map_err(|_| Failure::new(400, "invalid_config"))?;
    c.validate().map_err(|code| Failure::new(400, code))?;
    Ok(c)
}

fn validated(kind: &str, config: &Value) -> Result<Value, Failure> {
    match kind {
        "mail.apple" | "mail.imap" => Ok(serde_json::to_value(parse_mail(kind, config)?).unwrap()),
        "browser.history" => Ok(serde_json::to_value(parse_browser(config)?).unwrap()),
        _ => Err(Failure::new(400, "unsupported_kind")),
    }
}

fn same_target(kind: &str, existing: &ConnectorSource, config: &Value, account_ref: Option<&str>) -> bool {
    match kind {
        "mail.apple" => true,
        "mail.imap" => existing.account_ref.as_deref() == account_ref && existing.config["imap"]["host"] == config["imap"]["host"],
        _ => existing.config["browser"] == config["browser"] && existing.config["profile"] == config["profile"] && existing.config["path"] == config["path"],
    }
}

/// `{action, …}` from the App's local channel. Never returns message or history content — only counts and codes.
pub fn command(store: &Shared, v: &Value, scratch: &Path) -> Result<Value, Failure> {
    let action = v["action"].as_str().unwrap_or("");
    match action {
        "describe" => Ok(json!({"connectors": describe()})),
        "list" => list(&lock(store)),
        "secrets" => {
            let values = v["secrets"].as_object().ok_or_else(|| Failure::new(400, "invalid_secrets"))?;
            set_secrets(values);
            Ok(json!({"saved": values.len()}))
        }
        "authorize" => authorize(store, v),
        "update" => update(store, v),
        "revoke" => {
            let id = v["source_id"].as_str().ok_or_else(|| Failure::new(400, "source_id"))?;
            let s = lock(store);
            let src = s.connector_source(id)?.filter(|c| SOURCE_KINDS.contains(&c.kind.as_str())).ok_or_else(|| Failure::new(404, "unknown_source"))?;
            s.set_source_status(&src.source_id, "revoked", None)?;
            let deleted = if v["delete_stored"].as_bool().unwrap_or(false) { s.delete_source_data(&src.source_id, "source_revoked")?.events } else { 0 };
            Ok(json!({"revoked": true, "deleted_events": deleted}))
        }
        "dry_run" => dry_run(store, v, scratch),
        "mail_scope" => mail_scope(store, v),
        "mail_recent" => mail_recent(store, v),
        "mail_body" => mail_body(store, v),
        "set_level" => set_level(store, v),
        "relevel" => relevel(store, v),
        "sync" => {
            let only = v["source_id"].as_str();
            Ok(serde_json::to_value(sync_all(store, only, scratch)).unwrap())
        }
        _ => Err(Failure::new(400, "unknown_action")),
    }
}

fn authorize(store: &Shared, v: &Value) -> Result<Value, Failure> {
    let kind = v["kind"].as_str().ok_or_else(|| Failure::new(400, "kind"))?;
    let mut config = validated(kind, &v["config"])?;
    let account_ref = v["account_ref"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let secret_ref = v["secret_ref"].as_str().map(str::trim).filter(|s| !s.is_empty());
    if kind == "mail.imap" {
        let login = account_ref.ok_or_else(|| Failure::new(400, "account_ref_required"))?;
        if secret_ref.is_none() {
            return Err(Failure::new(400, "secret_ref_required"));
        }
        // The login is the user's own address; it decides direction like the configured self addresses.
        let mut c: MailConfig = serde_json::from_value(config.clone()).unwrap();
        if login.contains('@') && !c.self_addresses.iter().any(|a| a.eq_ignore_ascii_case(login)) {
            c.self_addresses.push(login.to_lowercase());
        }
        config = serde_json::to_value(c).unwrap();
    } else if secret_ref.is_some() {
        return Err(Failure::new(400, "secret_ref_not_used"));
    }
    let name = v["display_name"].as_str().map(String::from).unwrap_or_else(|| match kind {
        "mail.apple" => "邮件 App".into(),
        "mail.imap" => format!("IMAP {}", account_ref.unwrap_or("")),
        _ => format!("{} 历史", config["browser"].as_str().unwrap_or("")),
    });
    let s = lock(store);
    let existing = s.connector_sources(&[kind])?.into_iter().find(|e| same_target(kind, e, &config, account_ref));
    let id = match existing {
        Some(e) if e.status != "revoked" => return Err(Failure::new(409, "already_authorized")),
        Some(e) => {
            // Re-authorising a revoked connection keeps its identity and stored history.
            s.set_source_config(&e.source_id, &config, Some(&name))?;
            s.set_source_status(&e.source_id, "active", None)?;
            e.source_id
        }
        None => s.register_source_with(kind, &name, &default_policy(kind), None, account_ref, secret_ref, &config, None)?,
    };
    let probe = probe_source(&s, &id);
    drop(s);
    Ok(json!({"source_id": id, "probe": probe}))
}

fn update(store: &Shared, v: &Value) -> Result<Value, Failure> {
    let id = v["source_id"].as_str().ok_or_else(|| Failure::new(400, "source_id"))?;
    let s = lock(store);
    let src = s.connector_source(id)?.filter(|c| SOURCE_KINDS.contains(&c.kind.as_str())).ok_or_else(|| Failure::new(404, "unknown_source"))?;
    if src.status == "revoked" {
        return Err(Failure::new(409, "source_revoked"));
    }
    let config = validated(&src.kind, &v["config"])?;
    if src.kind == "mail.imap" && config["imap"]["host"] != src.config["imap"]["host"] {
        return Err(Failure::new(409, "account_change_requires_new_source"));
    }
    let scope_changed = ["accounts", "mailboxes", "backfill_days"].iter().any(|k| config[k] != src.config[k]);
    s.set_source_config(id, &config, v["display_name"].as_str())?;
    // Whether AI (the attached agent, MCP clients) may read this source; the App itself always can.
    if let Some(visible) = v["agent_visible"].as_bool() {
        let mut policy = src.policy.clone();
        policy.agent_visible = visible;
        s.update_policy(id, &policy)?;
    }
    if scope_changed && src.kind.starts_with("mail.") {
        // A wider scope must revisit rows the cursor already passed; stored messages are de-duplicated by key.
        s.ingest(&IngestBatch { source_id: id.into(), cursor_after: Some(IngestCursor::new(mail::CURSOR_STREAM, "")), ..Default::default() })?;
    }
    Ok(json!({"saved": true, "rescan": scope_changed}))
}

/// Access check without reading content; records a changed error code on the source.
fn probe_source(s: &TimelineStore, id: &str) -> Value {
    let Ok(Some(src)) = s.connector_source(id) else { return json!({"ok": false, "code": "unknown_source"}) };
    let result: Result<Value, String> = match src.kind.as_str() {
        "mail.apple" => serde_json::from_value::<MailConfig>(src.config.clone()).map_err(|_| "invalid_config".to_string()).and_then(|c| mail::apple::probe(&c).map(|layout| json!({"layout": layout}))),
        "mail.imap" => match src.secret_ref.as_deref().and_then(secret) {
            None => Err("waiting_authorization".into()),
            Some(_) => Ok(json!({"deferred": "first_sync"})),
        },
        _ => serde_json::from_value::<browser::BrowserConfig>(src.config.clone())
            .map_err(|_| "invalid_config".to_string())
            .and_then(|c| std::fs::metadata(c.history_path()).map(|_| json!({})).map_err(|e| if e.kind() == std::io::ErrorKind::PermissionDenied { "full_disk_access_required".into() } else { "history_not_found".into() })),
    };
    match result {
        Ok(v) => {
            let _ = s.note_source_error(id, None);
            json!({"ok": true, "detail": v})
        }
        Err(code) => {
            let _ = s.note_source_error(id, Some(&code));
            json!({"ok": false, "code": code})
        }
    }
}

// MARK: Sync

#[derive(Clone, Debug, Default, Serialize)]
pub struct SourceOutcome {
    pub source_id: String,
    pub kind: String,
    /// `synced`, `skipped` (revoked/paused/import disabled) or `error`.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The error code changed this pass (callers report it once, not every pass).
    pub newly_failing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<mail::Stats>,
    pub inserted: usize,
    pub revised: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncSummary {
    pub sources: Vec<SourceOutcome>,
}

impl SyncSummary {
    pub fn changed(&self) -> bool {
        self.sources.iter().any(|s| s.inserted + s.revised + s.removed > 0)
    }
    /// One problem line per source whose failure is new this pass.
    pub fn new_problems(&self) -> Vec<String> {
        self.sources.iter().filter(|s| s.newly_failing).map(|s| format!("{}：{}", s.kind, s.code.as_deref().unwrap_or("error"))).collect()
    }
}

struct Plan {
    src: ConnectorSource,
    config: MailConfig,
    cursor: Option<String>,
    known: mail::Known,
    salt: String,
    now_ms: i64,
}

fn plan(s: &TimelineStore, src: ConnectorSource, now_ms: i64) -> Result<Plan, String> {
    let config: MailConfig = serde_json::from_value(src.config.clone()).map_err(|_| "invalid_config")?;
    config.validate(&src.kind)?;
    let cursor = s.cursor(&src.source_id, mail::CURSOR_STREAM).map_err(|e| e.code())?;
    let known = s
        .stored_source_events(&src.source_id, "mail.message")
        .map_err(|e| e.code())?
        .into_iter()
        .map(|(k, p, t)| (k, (p, t)))
        .collect();
    let salt = s.store_secret("connectors.address_salt").map_err(|e| e.code())?;
    Ok(Plan { src, config, cursor, known, salt, now_ms })
}

fn read(p: &Plan, limit: usize) -> Result<mail::Scan, String> {
    match p.src.kind.as_str() {
        "mail.apple" => mail::apple::scan(&p.config, p.cursor.as_deref(), &p.known, limit, p.now_ms),
        _ => {
            let password = p.src.secret_ref.as_deref().and_then(secret).ok_or("waiting_authorization")?;
            let login = p.src.account_ref.clone().ok_or("account_ref_required")?;
            mail::imap::scan(&p.config, &mail::imap::Login { login: &login, password: &password }, p.cursor.as_deref(), &p.known, limit, p.now_ms)
        }
    }
}

fn adapter_name(kind: &str) -> &'static str {
    if kind == "mail.apple" {
        "apple_mail"
    } else {
        "imap"
    }
}

fn sync_mail(store: &Shared, src: ConnectorSource) -> SourceOutcome {
    let mut out = SourceOutcome { source_id: src.source_id.clone(), kind: src.kind.clone(), ..Default::default() };
    let fail = |store: &Shared, out: &mut SourceOutcome, code: String| {
        out.newly_failing = lock(store).note_source_error(&out.source_id, Some(&code)).unwrap_or(false);
        out.state = "error".into();
        out.code = Some(code);
    };
    let now = lock(store).clock.now_ms();
    let planned = plan(&lock(store), src, now);
    let p = match planned {
        Ok(p) => p,
        Err(code) => {
            fail(store, &mut out, code);
            return out;
        }
    };
    if !p.config.import_confirmed {
        // Scope and preview come first: nothing is read until the user confirms (config `import_confirmed`).
        out.state = "skipped".into();
        out.code = Some("awaiting_confirmation".into());
        return out;
    }
    let scan = match read(&p, p.config.batch_limit) {
        Ok(s) => s,
        Err(code) => {
            fail(store, &mut out, code);
            return out;
        }
    };
    let (batch, stats) = mail::build(&p.src.source_id, adapter_name(&p.src.kind), &p.config, &p.salt, &scan, &p.known);
    let s = lock(store);
    // Revoked or changed while reading: the ingest refuses non-active sources, and a stale plan is retried next pass.
    match s.ingest(&batch) {
        Ok(r) => {
            out.state = "synced".into();
            out.inserted = r.inserted;
            out.revised = r.revised;
            out.removed = r.removed;
            out.newly_failing = false;
            let _ = s.note_source_error(&out.source_id, None);
        }
        Err(TimelineError::SourceNotWritable(_)) => out.state = "skipped".into(),
        Err(e) => {
            drop(s);
            fail(store, &mut out, e.code());
            return out;
        }
    }
    out.stats = Some(stats);
    out
}

/// One pass over every active connector source (or one). Browser history is visited but never imported.
/// Every source now (commands, tests). Failures still count toward the background backoff.
pub fn sync_all(store: &Shared, only: Option<&str>, scratch: &Path) -> SyncSummary {
    sync_with(store, only, scratch, false)
}

/// The background pass: a source that keeps failing is skipped until its backoff has passed.
pub fn sync_due(store: &Shared, scratch: &Path) -> SyncSummary {
    sync_with(store, None, scratch, true)
}

fn sync_with(store: &Shared, only: Option<&str>, scratch: &Path, respect_backoff: bool) -> SyncSummary {
    let _ = scratch;
    let sources = match lock(store).connector_sources(&SOURCE_KINDS) {
        Ok(s) => s,
        Err(_) => return SyncSummary::default(),
    };
    let mut summary = SyncSummary::default();
    for src in sources {
        if only.is_some_and(|o| o != src.source_id) {
            continue;
        }
        if src.status == "revoked" || src.status == "paused" {
            summary.sources.push(SourceOutcome { source_id: src.source_id, kind: src.kind, state: "skipped".into(), ..Default::default() });
            continue;
        }
        if src.kind == "browser.history" {
            // Built, import off (2026-11-12): the sync path never writes; it does not even read the history.
            summary.sources.push(SourceOutcome { source_id: src.source_id, kind: src.kind, state: "skipped".into(), code: Some("import_disabled".into()), ..Default::default() });
            continue;
        }
        // A source that keeps failing (no disk access, wrong password, server down) waits longer before each
        // background retry: 30 s, 1, 2, 4 … up to 30 min. Explicit syncs always try at once.
        let now = std::time::Instant::now();
        if respect_backoff {
            if let Some((_, next)) = backoff().lock().unwrap().get(&src.source_id) {
                if now < *next {
                    summary.sources.push(SourceOutcome { source_id: src.source_id, kind: src.kind, state: "skipped".into(), code: Some("retry_later".into()), ..Default::default() });
                    continue;
                }
            }
        }
        let id = src.source_id.clone();
        let outcome = sync_mail(store, src);
        let mut map = backoff().lock().unwrap();
        if outcome.state == "error" {
            let fails = map.get(&id).map(|e| e.0 + 1).unwrap_or(1);
            let wait = std::time::Duration::from_secs((30u64 << (fails - 1).min(6)).min(1800));
            map.insert(id, (fails, now + wait));
        } else {
            map.remove(&id);
        }
        drop(map);
        summary.sources.push(outcome);
    }
    summary
}

/// Consecutive failures and the earliest next attempt per source (process memory; a restart retries at once).
fn backoff() -> &'static Mutex<HashMap<String, (u32, std::time::Instant)>> {
    static BACKOFF: OnceLock<Mutex<HashMap<String, (u32, std::time::Instant)>>> = OnceLock::new();
    BACKOFF.get_or_init(Default::default)
}

// MARK: Dry run

fn dry_run(store: &Shared, v: &Value, scratch: &Path) -> Result<Value, Failure> {
    // Either an existing source, or a kind + config before authorising (nothing is registered or written).
    let (src, kind) = match v["source_id"].as_str() {
        Some(id) => {
            let src = lock(store).connector_source(id)?.filter(|c| SOURCE_KINDS.contains(&c.kind.as_str())).ok_or_else(|| Failure::new(404, "unknown_source"))?;
            if src.status == "revoked" {
                return Err(Failure::new(409, "source_revoked"));
            }
            let kind = src.kind.clone();
            (Some(src), kind)
        }
        None => (None, v["kind"].as_str().ok_or_else(|| Failure::new(400, "kind"))?.to_string()),
    };
    // A `config` in the request previews that scope instead of the stored one (nothing is saved).
    let override_config = v.get("config").is_some_and(|c| !c.is_null());
    let config = match &src {
        Some(s) if !override_config => s.config.clone(),
        _ => validated(&kind, &v["config"])?,
    };
    let now = lock(store).clock.now_ms();
    if kind == "browser.history" {
        let c = parse_browser(&config)?;
        return browser::dry_run(&c, scratch, now).map(|r| json!({"kind": kind, "result": r})).map_err(|code| Failure::new(409, code));
    }
    let mail_config = parse_mail(&kind, &config)?;
    let src = src.unwrap_or_else(|| ConnectorSource {
        source_id: String::new(),
        kind: kind.clone(),
        display_name: String::new(),
        account_ref: v["account_ref"].as_str().map(String::from),
        secret_ref: v["secret_ref"].as_str().map(String::from),
        config: config.clone(),
        policy: default_policy(&kind),
        status: "active".into(),
        last_sync_ms: None,
        last_error: None,
        events: 0,
    });
    let (cursor, known, salt) = if src.source_id.is_empty() {
        (None, mail::Known::new(), String::new())
    } else {
        let s = lock(store);
        let known = s.stored_source_events(&src.source_id, "mail.message")?.into_iter().map(|(k, p, t)| (k, (p, t))).collect();
        // A scope under review starts from the beginning: the stored cursor belongs to the saved scope.
        let cursor = if override_config { None } else { s.cursor(&src.source_id, mail::CURSOR_STREAM)? };
        (cursor, known, s.store_secret("connectors.address_salt")?)
    };
    // Counts only: headers are decoded, nothing is stored, and no salt is created for an unregistered source.
    // Previewing a scope counts everything in it, stored or not, and says how much is already stored; otherwise
    // the preview of an account already imported read 0 (only new mail was counted).
    let stored = if override_config { known.clone() } else { mail::Known::new() };
    let p = Plan { config: mail_config, cursor, known: if override_config { mail::Known::new() } else { known }, salt, now_ms: now, src };
    let limit = v["limit"].as_u64().map(|l| l as usize).unwrap_or(DRY_RUN_LIMIT).clamp(1, 5000);
    let scan = read(&p, limit).map_err(|code| Failure::new(409, code))?;
    let already = scan.new.iter().filter(|r| stored.contains_key(&r.key)).count();
    let (batch, stats) = mail::build_with("dry-run", adapter_name(&kind), &p.config, &p.salt, &scan, &p.known, true);
    Ok(json!({"kind": kind, "format": scan.format, "would_insert": batch.events.len(), "would_remove": batch.removed_source_keys.len(),
              "would_revise": batch.payload_revisions.len(), "threads": batch.threads.len(), "stats": stats, "already_stored": already}))
}

// MARK: Levels, live bodies

fn mail_source(s: &TimelineStore, id: &str) -> Result<(ConnectorSource, MailConfig), Failure> {
    let src = s.connector_source(id)?.filter(|c| c.kind.starts_with("mail.")).ok_or_else(|| Failure::new(404, "unknown_source"))?;
    let config: MailConfig = serde_json::from_value(src.config.clone()).map_err(|_| Failure::new(400, "invalid_config"))?;
    Ok((src, config))
}

/// Current copies of messages from the source. Only Mail.app can be read live; IMAP bodies are not fetched on demand.
fn read_live(src: &ConnectorSource, config: &MailConfig, keys: &std::collections::BTreeSet<String>) -> Result<std::collections::BTreeMap<String, Option<mail::RawMail>>, Failure> {
    if src.kind != "mail.apple" {
        return Err(Failure::new(409, "live_read_unavailable"));
    }
    mail::apple::read_live(config, keys).map_err(|code| Failure::new(409, code))
}

fn event_row(e: &candid_store::MailEventRef) -> Value {
    let p = &e.payload;
    json!({
        "event_id": e.event_id, "source_id": e.source_id, "thread_key": e.thread_key, "title": e.title, "start_ms": e.start_ms,
        "level": p["level"], "level_basis": p["level_basis"], "important_candidate": p["important_candidate"].as_bool().unwrap_or(false),
        "direction": p["direction"], "from_name": p["from"]["name"], "body_status": p["body_status"], "mailboxes": p["mailboxes"],
    })
}

/// The newest stored mail (metadata only: no body text) for the App's list.
fn mail_recent(store: &Shared, v: &Value) -> Result<Value, Failure> {
    let limit = v["limit"].as_u64().unwrap_or(50) as usize;
    let rows = lock(store).mail_recent(v["source_id"].as_str(), limit)?;
    Ok(json!({"messages": rows.iter().map(event_row).collect::<Vec<_>>()}))
}

/// One message's body read live from the source; nothing is stored or indexed.
fn mail_body(store: &Shared, v: &Value) -> Result<Value, Failure> {
    let id = v["event_id"].as_str().ok_or_else(|| Failure::new(400, "event_id"))?;
    let (e, src, config) = {
        let s = lock(store);
        let e = s.mail_event(id)?.ok_or_else(|| Failure::new(404, "unknown_event"))?;
        let (src, config) = mail_source(&s, &e.source_id)?;
        (e, src, config)
    };
    let keys = std::collections::BTreeSet::from([e.source_key.clone()]);
    let live = read_live(&src, &config, &keys)?;
    let body = mail::live_body(live.get(&e.source_key).and_then(|r| r.as_ref()));
    Ok(json!({"message": event_row(&e), "body": body}))
}

/// Raises or lowers one message, or its whole thread, by hand. A manual level is never overridden by rules.
fn set_level(store: &Shared, v: &Value) -> Result<Value, Failure> {
    use mail::config::Level;
    let id = v["event_id"].as_str().ok_or_else(|| Failure::new(400, "event_id"))?;
    let level = match v["level"].as_str() {
        Some("metadata") => Level::Metadata,
        Some("normal") => Level::Normal,
        Some("important") => Level::Important,
        _ => return Err(Failure::new(400, "invalid_level")),
    };
    let whole_thread = match v["scope"].as_str().unwrap_or("message") {
        "message" => false,
        "thread" => true,
        _ => return Err(Failure::new(400, "invalid_scope")),
    };
    let (events, src, config) = {
        let s = lock(store);
        let e = s.mail_event(id)?.ok_or_else(|| Failure::new(404, "unknown_event"))?;
        let (src, config) = mail_source(&s, &e.source_id)?;
        let events = match (&e.thread_key, whole_thread) {
            (Some(t), true) => s.mail_thread_events(&e.source_id, t)?,
            _ => vec![e],
        };
        (events, src, config)
    };
    let adds_body: Vec<&candid_store::MailEventRef> = events.iter().filter(|e| level >= Level::Normal && mail::level_of(&e.payload) < Level::Normal).collect();
    let live = if adds_body.is_empty() {
        Default::default()
    } else {
        read_live(&src, &config, &adds_body.iter().map(|e| e.source_key.clone()).collect())?
    };
    let (mut changes, mut deleted, mut unreadable) = (Vec::new(), 0usize, 0usize);
    for e in &events {
        let old = mail::level_of(&e.payload);
        let crosses = (old >= Level::Normal) != (level >= Level::Normal);
        let text = if !crosses {
            None
        } else if level < Level::Normal {
            Some(mail::text_for(level, &Default::default()).into_iter().chain(e.title.as_deref().map(|t| candid_store::IngestText::new("subject", t))).collect())
        } else {
            match live.get(&e.source_key) {
                Some(Some(raw)) => match raw.bytes.as_deref().and_then(mail::mime::parse) {
                    Some(p) => Some(mail::text_for(level, &p)),
                    None => {
                        unreadable += 1;
                        continue;
                    }
                },
                _ => {
                    deleted += 1;
                    continue;
                }
            }
        };
        let basis_unchanged = e.payload["level_basis"] == "manual";
        if old == level && basis_unchanged {
            continue;
        }
        changes.push((e.source_key.clone(), mail::leveled_payload(&e.payload, level, "manual", false, false), text));
    }
    if changes.is_empty() && deleted > 0 && !whole_thread {
        return Err(Failure::new(409, "source_deleted"));
    }
    let changed = lock(store).apply_mail_levels(&src.source_id, &changes)?;
    Ok(json!({"changed": changed, "source_deleted": deleted, "unreadable": unreadable, "level": level.as_str()}))
}

/// Re-levels stored mail under the current rules. Rules need the sender and recipients, which are not stored, so
/// each message is read from the source again (in memory); mail the source no longer has is left to the normal sync.
fn relevel(store: &Shared, v: &Value) -> Result<Value, Failure> {
    let only = v["source_id"].as_str();
    let sources: Vec<ConnectorSource> = lock(store).connector_sources(&SOURCE_KINDS)?.into_iter().filter(|s| s.kind.starts_with("mail.") && s.status != "revoked" && only.is_none_or(|o| o == s.source_id)).collect();
    if only.is_some() && sources.is_empty() {
        return Err(Failure::new(404, "unknown_source"));
    }
    let mut out = Vec::new();
    for src in sources {
        let config: MailConfig = serde_json::from_value(src.config.clone()).map_err(|_| Failure::new(400, "invalid_config"))?;
        if src.kind != "mail.apple" {
            out.push(json!({"source_id": src.source_id, "state": "unsupported"}));
            continue;
        }
        let (known, salt) = {
            let s = lock(store);
            let known: mail::Known = s.stored_source_events(&src.source_id, "mail.message")?.into_iter().map(|(k, p, t)| (k, (p, t))).collect();
            (known, s.store_secret("connectors.address_salt")?)
        };
        let keys = known.keys().cloned().collect();
        let live = read_live(&src, &config, &keys)?;
        let (changes, stats) = mail::relevel(&config, &salt, &known, &live);
        let batch: Vec<_> = changes.into_iter().map(|c| (c.source_key, c.payload, c.text)).collect();
        lock(store).apply_mail_levels(&src.source_id, &batch)?;
        out.push(json!({"source_id": src.source_id, "state": "done", "stats": stats}));
    }
    Ok(json!({"sources": out}))
}

/// Accounts and folders of the local Mail app, for the scope picker (counts only, no message content).
fn mail_scope(store: &Shared, v: &Value) -> Result<Value, Failure> {
    let config = match v["source_id"].as_str() {
        Some(id) => mail_source(&lock(store), id)?.1,
        None => parse_mail("mail.apple", &v["config"])?,
    };
    mail::apple::scope_options(&config).map_err(|code| Failure::new(409, code))
}
