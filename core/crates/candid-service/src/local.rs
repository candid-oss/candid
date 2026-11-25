//! The App's local channel (ADR-024 §3): HTTP/1.1 over a Unix domain socket in the run directory, socket 0600 in a
//! 0700 directory, so only this user's processes can connect. The App is the internal client (`transport =
//! 'internal'`, no token); a request that carries a Bearer token is answered as that client instead. Tool calls go
//! through the same scope filtering and `access_log` as the loopback HTTP.
//!
//! Only this channel serves (the loopback router refuses them with 403 `local_only`):
//!
//! | Method and path | Purpose |
//! |---|---|
//! | `GET /health` | process, store and endpoint identity |
//! | `GET /local/status` | sync phase, event count, last change, problems, last verification |
//! | `GET /local/events` | change feed: newline-delimited JSON (`hello`, `status`, `data`, `clients`) until either side closes |
//! | `GET /local/clients` | registered external clients with their recent use |
//! | `POST /local/clients` | `{"name"}` → token (shown once) and setup snippets for Claude Code, Codex and generic JSON |
//! | `POST /local/clients/revoke` | `{"client_id"}` |
//! | `POST /local/sync` | run a mirroring pass now (202; the result arrives as a `status` event) |
//! | `POST /local/verify` | `{"hash_blobs"}` → compare the desktop ledger with the timeline |
//! | `GET /local/sources`, `POST /local/sources` | list, or register once by kind and name, the App's own sources |
//! | `POST /local/ingest` | one `IngestBatch` (timeline-v1 §6.1) → ingest result; desktop batches only after the cutover |
//! | `POST /local/desktop/cutover` | import what is left of the desktop ledger, check it, then switch capture to direct writes |
//! | `GET /local/desktop/legacy-preferences` | the capture preferences kept in the ledger (read-only), for the first direct launch |
//! | `POST /local/desktop/delete` | `{"activity_id"}`, or `{"record_ids"\|"event_ids", "reason"}` → tombstones |
//! | `POST /local/desktop/retention` | `{"before_ms"}` → deletes older desktop records (reason `retention`) |
//! | `POST /local/desktop/free-space` | `{"target_bytes"}` → drops the oldest screenshot originals (over 7 days old; event, OCR and AX text stay) until desktop originals fit |
//! | `POST /v1/<tool>` | the query tools, and the App's own `app_*` read tools (internal client only) |
//! | `/local/pi/*`, agent tools | the attached Pi agent: run start/stop, model gateway, in-store writes ([`crate::pi`]) |

use crate::catalog;
use crate::http::{read_request, write_response, HttpRequest, HttpResponse, MAX_BODY};
use crate::router::{bearer, lock, run_tool, store_error, SharedStore};
use candid_store::{IngestBatch, SourcePolicy, TimelineStore, APP_TOOL_NAMES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Name of the App's internal client.
pub const APP_CLIENT_NAME: &str = "Candid App";
/// Longest socket path macOS accepts is 103 bytes; keep a margin.
pub const MAX_SOCKET_PATH: usize = 100;
const MAX_SUBSCRIBERS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub ok: bool,
    pub hashed: bool,
    /// Per ledger kind where counts differ, then hash mismatches and missing screenshots (at most a few lines).
    pub differences: Vec<String>,
    pub blobs_checked: usize,
    pub at_ms: i64,
}

/// What the App's 「时间线库」 group shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    /// `opening`, `syncing` (first pass), `ready`.
    pub phase: String,
    pub events: i64,
    pub last_sync_ms: Option<i64>,
    pub last_change: Option<String>,
    pub problems: Vec<String>,
    pub verification: Option<Verification>,
    /// A pass is running right now (also after the first one).
    pub syncing: bool,
    /// `ledger` (capture still waits; the ledger is mirrored) or `direct` (capture writes here; the ledger is a
    /// read-only backup).
    pub capture_mode: String,
    /// The record of the switch to direct capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutover: Option<Value>,
    /// Why the last switch attempt did not happen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutover_error: Option<String>,
}

impl Default for SyncStatus {
    fn default() -> Self {
        SyncStatus {
            phase: "opening".into(),
            events: 0,
            last_sync_ms: None,
            last_change: None,
            problems: vec![],
            verification: None,
            syncing: false,
            capture_mode: "ledger".into(),
            cutover: None,
            cutover_error: None,
        }
    }
}

/// Subscribers of `GET /local/events`. A subscriber that cannot take a line within a second is dropped.
/// Publishing `data` or `power` also wakes the background thread ([`crate::background`]); the thread itself
/// announces without waking.
#[derive(Default)]
pub struct Feed {
    subscribers: Mutex<Vec<UnixStream>>,
    seq: AtomicU64,
    wake: Arc<crate::background::Wake>,
}

impl Feed {
    fn line(&self, kind: &str) -> Vec<u8> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut out = serde_json::to_vec(&json!({"type": kind, "seq": seq})).unwrap_or_default();
        out.push(b'\n');
        out
    }

    fn subscribe(&self, mut stream: UnixStream) {
        let mut subs = self.subscribers.lock().unwrap_or_else(|p| p.into_inner());
        if subs.len() >= MAX_SUBSCRIBERS {
            write_response(&mut stream, &HttpResponse::error(503, "busy", "订阅数已满。"));
            return;
        }
        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
        let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n";
        let hello = self.line("hello");
        if stream.write_all(head).and_then(|_| stream.write_all(&hello)).and_then(|_| stream.flush()).is_ok() {
            subs.push(stream);
        }
    }

    /// Sends `{"type": kind, "seq": n}` to every subscriber; `data` wakes the background thread and `power` makes it
    /// look at waiting jobs again.
    pub fn publish(&self, kind: &str) {
        match kind {
            "data" => self.wake.wake(),
            "power" => self.wake.invalidate(),
            _ => {}
        }
        self.announce(kind);
    }

    /// The background thread's wake signal.
    pub fn wake(&self) -> &Arc<crate::background::Wake> {
        &self.wake
    }

    /// Sends `{"type": kind, "seq": n}` without waking anything (the background thread reporting its own work).
    pub fn announce(&self, kind: &str) {
        let line = self.line(kind);
        let mut subs = self.subscribers.lock().unwrap_or_else(|p| p.into_inner());
        subs.retain_mut(|s| s.write_all(&line).and_then(|_| s.flush()).is_ok());
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().map(|s| s.len()).unwrap_or(0)
    }

    pub fn close_all(&self) {
        for s in self.subscribers.lock().unwrap_or_else(|p| p.into_inner()).drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// A request for the sync thread: run a pass now.
#[derive(Default)]
pub struct SyncTrigger {
    requested: Mutex<bool>,
    signal: Condvar,
}

impl SyncTrigger {
    pub fn request(&self) {
        *self.requested.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.signal.notify_all();
    }

    /// Waits until requested or timed out. The stop path sets `stop` and calls `request` to wake us.
    pub fn wait(&self, timeout: Duration, stop: &AtomicBool) {
        let deadline = Instant::now() + timeout;
        let mut requested = self.requested.lock().unwrap_or_else(|p| p.into_inner());
        while !*requested && !stop.load(Ordering::SeqCst) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            requested = self.signal.wait_timeout(requested, left).unwrap_or_else(|p| p.into_inner()).0;
        }
        *requested = false;
    }
}

/// Everything the local channel and the sync thread share.
pub struct Core {
    pub store: SharedStore,
    pub status: Mutex<SyncStatus>,
    pub feed: Feed,
    pub trigger: SyncTrigger,
    pub internal_client: String,
    /// Path written into the setup snippets (`Contents/Helpers/candid` inside the App).
    pub cli_path: String,
    pub ledger: Option<PathBuf>,
    pub hub: Option<PathBuf>,
    pub root: PathBuf,
    pub store_id: String,
    pub endpoint: Mutex<String>,
    pub socket: PathBuf,
    pub parent_pid: Option<i32>,
    pub started: Instant,
    /// Held while a cutover runs, so two requests do not race.
    pub cutover_lock: Mutex<()>,
}

/// Returns memory freed by a mirroring pass or a verification to the system, so the idle helper does not keep the
/// first sync's peak (macOS keeps freed pages in the malloc zones otherwise).
pub fn release_memory() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means all zones; goal 0 means as much as possible.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The text the App shows after registering a client: the same three snippets the Swift service produced.
pub fn setup_snippets(cli: &str, token: &str) -> (String, String, String) {
    let slug = "candid";
    (
        format!("claude mcp add {slug} --scope user --env CANDID_TOKEN={token} -- \"{cli}\" mcp"),
        format!("[mcp_servers.{slug}]\ncommand = \"{cli}\"\nargs = [\"mcp\"]\nenv = {{ CANDID_TOKEN = \"{token}\" }}"),
        format!("{{\"mcpServers\": {{\"{slug}\": {{\"command\": \"{cli}\", \"args\": [\"mcp\"], \"env\": {{\"CANDID_TOKEN\": \"{token}\"}}}}}}}}"),
    )
}

fn ok_json(value: &Value) -> HttpResponse {
    HttpResponse { status: 200, body: serde_json::to_vec(value).unwrap_or_default() }
}

fn decode<T: for<'de> Deserialize<'de>>(request: &HttpRequest, what: &str) -> Result<T, HttpResponse> {
    let body: &[u8] = if request.body.is_empty() { b"{}" } else { &request.body };
    serde_json::from_slice(body).map_err(|e| HttpResponse::error(400, "invalid_input", &format!("参数不符合 {what} 的格式：{}", crate::router::describe(&e))))
}

#[derive(Deserialize)]
struct NewClient {
    name: String,
    #[serde(default)]
    agents: Vec<String>,
}

#[derive(Deserialize)]
struct ClientRef {
    client_id: String,
}

#[derive(Deserialize)]
struct VerifyRequest {
    #[serde(default = "yes")]
    hash_blobs: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct NewSource {
    kind: String,
    display_name: String,
    #[serde(default)]
    policy: Option<SourcePolicy>,
}

impl Core {
    /// Wakes the background thread and has it look at waiting jobs again. For changes outside the library (power
    /// state, authorization, an explicit request); writes to the library wake it on their own.
    pub fn wake_background(&self) {
        self.feed.wake.invalidate();
    }

    pub fn status(&self) -> SyncStatus {
        self.status.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Records the event count while the caller still holds the store, so a slow pass that counted earlier cannot
    /// overwrite a newer count (a mirroring pass releases the store while mail syncs; an ingest may land meanwhile).
    fn record_event_count(&self, store: &TimelineStore) {
        if let Ok(n) = store.event_count() {
            self.status.lock().unwrap_or_else(|p| p.into_inner()).events = n;
        }
    }

    pub fn update_status(&self, change: impl FnOnce(&mut SyncStatus)) {
        change(&mut self.status.lock().unwrap_or_else(|p| p.into_inner()));
        self.feed.publish("status");
    }

    /// Compares the desktop ledger with the timeline record by record (counts per kind, screenshot references, and
    /// with `hash_blobs` every original's hash). The images are hashed without holding the store.
    pub fn verify(&self, hash_blobs: bool) -> Option<Verification> {
        let ledger = self.ledger.as_ref().filter(|l| l.join("ledger.sqlite").exists())?;
        let v = self.check_ledger(ledger, hash_blobs);
        self.update_status(|s| s.verification = Some(v.clone()));
        release_memory();
        Some(v)
    }

    fn check_ledger(&self, ledger: &Path, hash_blobs: bool) -> Verification {
        let check = lock(&self.store).check_desktop_ledger(ledger, None);
        match check {
            Ok(mut c) => {
                if hash_blobs {
                    c.hash_files();
                }
                let v = c.verification;
                let mut differences: Vec<String> = v
                    .counts_by_kind
                    .iter()
                    .filter(|(_, c)| c.ledger != c.store)
                    .map(|(k, c)| format!("{k}：账本 {}，时间线 {}", c.ledger, c.store))
                    .collect();
                differences.extend(v.blob_mismatches.iter().take(5).cloned());
                if !v.missing_events.is_empty() {
                    differences.push(format!("{} 张截图未进入时间线", v.missing_events.len()));
                }
                Verification { ok: v.ok(), hashed: hash_blobs, differences, blobs_checked: v.blobs_checked, at_ms: now_ms() }
            }
            Err(e) => Verification { ok: false, hashed: hash_blobs, differences: vec![e.message()], blobs_checked: 0, at_ms: now_ms() },
        }
    }

    /// Loads the capture mode and the cutover record into the status (at start).
    pub fn load_capture_mode(&self) {
        let (direct, record) = {
            let store = lock(&self.store);
            (store.desktop_direct().unwrap_or(false), store.desktop_cutover().ok().flatten())
        };
        let verification = record.as_ref().and_then(|r| r.get("verification")).and_then(|v| serde_json::from_value::<Verification>(v.clone()).ok());
        self.update_status(|s| {
            s.capture_mode = if direct { "direct" } else { "ledger" }.into();
            s.cutover = record;
            if verification.is_some() {
                s.verification = verification;
            }
        });
    }

    /// The switch to direct capture (timeline-v1 §9.1): mirrors what is left of the ledger until a pass adds
    /// nothing, checks every record and screenshot hash, and only then records `direct`, after which the ledger is
    /// no longer mirrored. A failed check leaves the mode as it was and says why. Without a ledger (a new install)
    /// capture is direct from the start.
    pub fn cutover(&self) -> Result<Value, (Value, String)> {
        let _guard = self.cutover_lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(record) = lock(&self.store).desktop_cutover().ok().flatten().filter(|_| self.status().capture_mode == "direct") {
            return Ok(record);
        }
        let started = Instant::now();
        let Some(ledger) = self.ledger.clone().filter(|l| l.join("ledger.sqlite").exists()) else {
            let record = json!({"at_ms": now_ms(), "duration_ms": 0, "ledger": Value::Null, "records": 0, "screenshots": 0});
            return self.finish_cutover(record);
        };
        self.update_status(|s| s.syncing = true);
        let mut problems = Vec::new();
        for _ in 0..5 {
            let pass = lock(&self.store).sync_desktop_ledger(&ledger, None);
            match pass {
                Ok(r) => {
                    problems = r.failures.iter().take(5).map(|f| format!("桌面：{f}")).collect();
                    if r.inserted + r.deleted + r.revised == 0 {
                        break;
                    }
                }
                Err(e) => {
                    problems = vec![format!("桌面：{}", e.message())];
                    break;
                }
            }
        }
        let v = self.check_ledger(&ledger, true);
        self.record_event_count(&lock(&self.store));
        self.update_status(|s| {
            s.syncing = false;
            s.verification = Some(v.clone());
        });
        release_memory();
        if !v.ok {
            let mut reasons = v.differences.clone();
            reasons.extend(problems);
            let reason = format!("未切换到直写：{}", if reasons.is_empty() { "核对未通过".to_string() } else { reasons.join("；") });
            self.update_status(|s| s.cutover_error = Some(reason.clone()));
            self.feed.publish("data");
            return Err((serde_json::to_value(&v).unwrap_or(Value::Null), reason));
        }
        let records: i64 = {
            let store = lock(&self.store);
            store.check_desktop_ledger(&ledger, None).map(|c| c.verification.counts_by_kind.iter().filter(|(k, _)| k.as_str() != "recognition").map(|(_, c)| c.ledger).sum()).unwrap_or(0)
        };
        let record = json!({
            "at_ms": now_ms(),
            "duration_ms": started.elapsed().as_millis() as i64,
            "ledger": ledger.display().to_string(),
            "records": records,
            "screenshots": v.blobs_checked,
            "verification": v,
        });
        self.finish_cutover(record)
    }

    fn finish_cutover(&self, record: Value) -> Result<Value, (Value, String)> {
        if let Err(e) = lock(&self.store).mark_desktop_direct(&record) {
            let reason = format!("未切换到直写：{}", e.message());
            self.update_status(|s| s.cutover_error = Some(reason.clone()));
            return Err((Value::Null, reason));
        }
        self.update_status(|s| {
            s.capture_mode = "direct".into();
            s.cutover = Some(record.clone());
            s.cutover_error = None;
        });
        self.feed.publish("data");
        Ok(record)
    }

    /// One mirroring pass with the App's status bookkeeping; the first pass of a process also verifies (with hashes
    /// when the library was empty before it).
    pub fn sync_pass(&self) {
        let first_pass = self.status().last_sync_ms.is_none();
        self.update_status(|s| {
            s.syncing = true;
            if s.phase != "ready" {
                s.phase = "syncing".into();
            }
        });
        let mut problems = Vec::new();
        let mut changes = Vec::new();
        let empty_before = {
            let store = lock(&self.store);
            let before = store.event_count().unwrap_or(0);
            let mirror_ledger = !store.desktop_direct().unwrap_or(false);
            if let Some(l) = self.ledger.as_ref().filter(|l| mirror_ledger && l.join("ledger.sqlite").exists()) {
                match store.sync_desktop_ledger(l, None) {
                    Ok(r) => {
                        problems.extend(r.failures.iter().take(5).map(|f| format!("桌面：{f}")));
                        if r.changed() {
                            changes.push(format!("桌面 +{}{}", r.inserted, if r.deleted > 0 { format!(" −{}", r.deleted) } else { String::new() }));
                        }
                    }
                    Err(e) => problems.push(format!("桌面：{}", e.message())),
                }
            }
            if let Err(e)=store.prepare_media(self.hub.as_deref()){problems.push(format!("资料：{}",e.message()));}
            match store.sync_agents() {
                Ok(r) => {
                    if r["inserted"].as_u64().unwrap_or(0) + r["revised"].as_u64().unwrap_or(0) > 0
                    {
                        changes.push(format!("AI +{} 修订 {}", r["inserted"], r["revised"]));
                    }
                    if r["issues"].as_array().is_some_and(|a| !a.is_empty()) {
                        problems.push("AI 会话：部分文件格式不可读".into());
                    }
                }
                Err(e) => problems.push(format!("AI 会话：{}", e.code())),
            }
            before == 0
        };
        // Mail (and browser history, import off) read without the store lock; each source commits in one batch.
        let scratch = lock(&self.store).staging_directory();
        let connectors = candid_connectors::sync_due(&self.store, &scratch);
        if connectors.changed() {
            let (i, r, d) = connectors.sources.iter().fold((0, 0, 0), |a, s| (a.0 + s.inserted, a.1 + s.revised, a.2 + s.removed));
            changes.push(format!("邮件 +{i} 修订 {r} 删除 {d}"));
        }
        problems.extend(connectors.new_problems());
        self.record_event_count(&lock(&self.store));
        let changed = !changes.is_empty();
        self.update_status(|s| {
            s.last_sync_ms = Some(now_ms());
            if changed {
                s.last_change = Some(changes.join("，"));
            }
            s.problems = problems;
            s.phase = "ready".into();
            s.syncing = false;
        });
        if changed {
            self.feed.publish("data");
        }
        if first_pass && self.status().capture_mode != "direct" {
            self.verify(empty_before);
        }
        // Access counts shown next to each client move with every external query.
        self.feed.publish("clients");
        release_memory();
    }

    fn health(&self) -> Value {
        json!({
            "ok": true,
            "version": crate::VERSION,
            "pid": std::process::id(),
            "parent_pid": self.parent_pid,
            "store_id": self.store_id,
            "root": self.root.display().to_string(),
            "staging_dir": self.root.join("staging").display().to_string(),
            "endpoint": *self.endpoint.lock().unwrap_or_else(|p| p.into_inner()),
            "socket": self.socket.display().to_string(),
            "uptime_ms": self.started.elapsed().as_millis() as u64,
            "subscribers": self.feed.subscriber_count(),
        })
    }

    fn status_json(&self) -> Value {
        let mut v = serde_json::to_value(self.status()).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut v {
            map.insert("endpoint".into(), json!(*self.endpoint.lock().unwrap_or_else(|p| p.into_inner())));
            map.insert("root".into(), json!(self.root.display().to_string()));
        }
        v
    }

    /// Answers one local-channel request (everything except the change feed, which keeps the connection).
    pub fn handle(&self, request: &HttpRequest) -> HttpResponse {
        let path = request.path.split('?').next().unwrap_or("").to_string();
        let method = request.method.as_str();
        if request.body.len() > MAX_BODY {
            return HttpResponse::error(413, "too_large", "请求体超过 1 MiB。");
        }
        if let Some(response) = crate::pi::route(self, request) {
            return response;
        }
        if let Some(response) = crate::agenda::route(self, request) {
            return response;
        }
        if let Some(name) = path.strip_prefix("/v1/").filter(|n| APP_TOOL_NAMES.contains(n)) {
            if method != "POST" {
                return HttpResponse::error(405, "method", "请用 POST /v1/<工具名>。");
            }
            if !bearer(request).is_empty() {
                return HttpResponse::error(403, "internal_only", "这个工具只供 Candid App 自己使用。");
            }
            return run_tool(&lock(&self.store), name, &request.body, &self.internal_client);
        }
        if let Some(name) = path.strip_prefix("/v1/") {
            let Some(tool) = catalog::tool(name) else {
                return HttpResponse::error(404, "unknown_tool", &format!("没有这个工具：{path}"));
            };
            if method != "POST" {
                return HttpResponse::error(405, "method", "请用 POST /v1/<工具名>。");
            }
            let store = lock(&self.store);
            let token = bearer(request);
            let client = if token.is_empty() { Some(self.internal_client.clone()) } else { store.client_id_for_token(&token).ok().flatten() };
            let Some(client) = client else {
                return HttpResponse::error(401, "unauthorized", "访问令牌无效或已撤销。");
            };
            return run_tool(&store, tool.name, &request.body, &client);
        }
        if path.starts_with("/local/agents") && !bearer(request).is_empty() {
            return HttpResponse::error(403, "internal_only", "会话设置只供 App 使用。");
        }
        if path == "/local/voiceprints" {
            if request.method != "POST" { return HttpResponse::error(405,"method","请使用 POST"); }
            if !bearer(request).is_empty() { return HttpResponse::error(403,"internal_only","此操作只供 App 使用。"); }
            let v: Value = match serde_json::from_slice(&request.body) { Ok(v)=>v, Err(_)=>return HttpResponse::error(400,"json","请求无效") };
            return match crate::voiceprints::command(&self.store,&v) { Ok(v)=>ok_json(&v), Err(e)=>HttpResponse::error(409,"voiceprint_failed",&e) };
        }
        if path == "/local/automation" || path == "/local/screen" {
            if method != "POST" {return HttpResponse::error(405,"method","请用 POST。");}
            if !bearer(request).is_empty() {return HttpResponse::error(403,"internal_only","此操作只供 App 使用。");}
            let v:Value=match decode(request,"技能管理"){Ok(v)=>v,Err(e)=>return e};
            let store=lock(&self.store);
            let result=if path=="/local/screen"{store.screen_command(&v)}else{store.automation_command(&v)};
            return match result {Ok(result)=>{
                if !["status","detail","heartbeat","evaluation_scope"].contains(&v["action"].as_str().unwrap_or("status")) && !(v["action"]=="claim" && result.is_null()) {self.feed.publish("data");}
                ok_json(&result)
            },Err(e)=>store_error(&e)};
        }
        if ["/local/models", "/local/workflows", "/local/models/authorize", "/local/models/complete", "/local/compose"].contains(&path.as_str()) {
            if method != "POST" {return HttpResponse::error(405,"method","请用 POST。");}
            if !bearer(request).is_empty(){return HttpResponse::error(403,"internal_only","此操作只供 App 使用。");}
            let v:Value=match decode(request,"固定流程"){Ok(v)=>v,Err(e)=>return e};
            if path=="/local/models/authorize" {crate::workflows::authorize(&v);self.wake_background();return ok_json(&json!({"authorized":true}));}
            if path=="/local/compose" {
                // Draft from the user's history (plan M3): the store builds the request, the call runs unlocked.
                let prepared=match lock(&self.store).compose_prepare(&v){Ok(p)=>p,Err(e)=>return store_error(&e)};
                return match crate::workflows::complete(&self.store,&prepared){
                    Ok(out)=>ok_json(&json!({"draft":out["text"].as_str().unwrap_or("").trim(),"context":prepared["context"],"call_id":out["call_id"]})),
                    Err(e)=>HttpResponse::error(409,"model_call_failed",&e),
                };
            }
            if path=="/local/models/complete" {return match crate::workflows::complete(&self.store,&v){Ok(v)=>ok_json(&v),Err(e)=>HttpResponse::error(409,"model_call_failed",&e)};}
            let result=if path=="/local/models"{lock(&self.store).model_command(&v)}else{lock(&self.store).workflow_command(&v)};
            return match result {Ok(v)=>{self.feed.publish("data");ok_json(&v)},Err(e)=>store_error(&e)};
        }
        if ["/local/media", "/local/jobs", "/local/records"].contains(&path.as_str()) {
            if method != "POST" {return HttpResponse::error(405,"method","请用 POST。");}
            let token=bearer(request);
            let store=lock(&self.store);
            let v:Value=match decode(request,"本机管理") {Ok(v)=>v,Err(e)=>return e};
            if !token.is_empty() {
                let client=match store.client_id_for_token(&token) {Ok(Some(c))=>c,_=>return HttpResponse::error(401,"unauthorized","访问令牌无效。")};
                if path!="/local/records" || v["action"].as_str().unwrap_or("query")!="query" {return HttpResponse::error(403,"internal_only","此操作只供 App 使用。");}
                return match store.record_management(&v,&client) {Ok(v)=>ok_json(&v),Err(e)=>store_error(&e)};
            }
            let result=match path.as_str() {"/local/media"=>store.media_command(&v),"/local/jobs"=>store.job_command(&v),_=>store.record_management(&v,&self.internal_client)};
            drop(store);
            return match result {Ok(v)=>{self.feed.publish("data");ok_json(&v)},Err(e)=>store_error(&e)};
        }
        if path == "/local/connectors" {
            if !bearer(request).is_empty() {
                return HttpResponse::error(403, "internal_only", "连接设置只供 App 使用。");
            }
            let v: Value = match method {
                "GET" => json!({"action": "list"}),
                "POST" => match decode(request, "连接器") {
                    Ok(v) => v,
                    Err(e) => return e,
                },
                _ => return HttpResponse::error(405, "method", "请用 GET 或 POST。"),
            };
            let scratch = lock(&self.store).staging_directory();
            let mutating = ["authorize", "update", "revoke", "sync", "secrets", "set_level", "relevel"].contains(&v["action"].as_str().unwrap_or(""));
            return match candid_connectors::command(&self.store, &v, &scratch) {
                Ok(out) => {
                    if mutating {
                        self.trigger.request();
                        self.feed.publish("data");
                    }
                    ok_json(&out)
                }
                Err(f) => HttpResponse::error(f.status, &f.code, &f.code),
            };
        }
        match (method, path.as_str()) {

            ("GET", "/local/agents") => match lock(&self.store).agent_settings() {Ok(v)=>ok_json(&v),Err(e)=>store_error(&e)},
            ("POST", "/local/agents/consent") => {
                #[derive(Deserialize)] struct Settings { consent:candid_store::agents::AgentConsent, #[serde(default)] delete_stored:bool }
                let req:Settings=match decode(request,"会话授权") {Ok(r)=>r,Err(e)=>return e};
                let result=lock(&self.store).set_agent_consent(&req.consent,req.delete_stored);
                match result {Ok(())=>{self.trigger.request();self.feed.publish("data");ok_json(&json!({"saved":true}))},Err(e)=>store_error(&e)}
            },
            ("POST", "/local/agents/query") => {
                let req:candid_store::agents::AgentRequest=match decode(request,"会话查询"){Ok(r)=>r,Err(e)=>return e};
                match lock(&self.store).agent_call(&req,&self.internal_client){Ok(v)=>ok_json(&v),Err(e)=>store_error(&e)}
            },
            ("GET", "/health") => ok_json(&self.health()),
            ("GET", "/local/status") => ok_json(&self.status_json()),
            ("GET", "/local/clients") => match lock(&self.store).clients() {
                Ok(c) => ok_json(&json!({ "clients": c })),
                Err(e) => store_error(&e),
            },
            ("POST", "/local/clients") => {
                let req: NewClient = match decode(request, "登记访问方") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                let name = req.name.trim();
                if name.is_empty() || name.chars().count() > 80 {
                    return HttpResponse::error(400, "invalid_input", "名称不能为空，也不能超过 80 个字。");
                }
                if req.agents.iter().any(|a| !candid_store::agents::AGENTS.contains(&a.as_str())) {
                    return HttpResponse::error(400, "invalid_input", "不支持的 AI 会话来源。");
                }
                let mut scope = crate::serve::default_scope();
                scope.agents = req.agents;
                let reg = match lock(&self.store).register_client(name, "mcp", &scope, None) {
                    Ok(r) => r,
                    Err(e) => return store_error(&e),
                };
                self.feed.publish("clients");
                let token = reg.token.unwrap_or_default();
                let (claude_code, codex, generic) = setup_snippets(&self.cli_path, &token);
                ok_json(&json!({
                    "client_id": reg.client_id, "name": name, "token": token,
                    "claude_code": claude_code, "codex": codex, "json": generic,
                }))
            }
            ("POST", "/local/clients/revoke") => {
                let req: ClientRef = match decode(request, "撤销访问方") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                let store = lock(&self.store);
                let clients = match store.clients() {
                    Ok(c) => c,
                    Err(e) => return store_error(&e),
                };
                let Some(client) = clients.iter().find(|c| c.client_id == req.client_id) else {
                    return HttpResponse::error(404, "not_found", "没有这个访问方。");
                };
                if client.transport == "cli" {
                    return HttpResponse::error(409, "managed_client", "「本机命令行」由核心自动管理，不能撤销。");
                }
                if let Err(e) = store.revoke_client(&req.client_id) {
                    return store_error(&e);
                }
                drop(store);
                self.feed.publish("clients");
                ok_json(&json!({"revoked": req.client_id}))
            }
            ("POST", "/local/sync") => {
                self.trigger.request();
                HttpResponse { status: 202, body: br#"{"queued":true}"#.to_vec() }
            }
            ("POST", "/local/verify") => {
                let req: VerifyRequest = match decode(request, "核对") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                match self.verify(req.hash_blobs) {
                    Some(v) => ok_json(&serde_json::to_value(v).unwrap_or(Value::Null)),
                    None => HttpResponse::error(409, "no_ledger", "没有可核对的桌面账本。"),
                }
            }
            ("GET", "/local/sources") => match lock(&self.store).sources() {
                Ok(s) => ok_json(&json!({ "sources": s })),
                Err(e) => store_error(&e),
            },
            ("POST", "/local/sources") => {
                let req: NewSource = match decode(request, "登记来源") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                register_source_once(&lock(&self.store), &req.kind, &req.display_name, &req.policy.unwrap_or_default())
            }
            ("POST", "/local/ingest") => {
                let batch: IngestBatch = match decode(request, "入库批次") {
                    Ok(b) => b,
                    Err(e) => return e,
                };
                let result = {
                    let store = lock(&self.store);
                    let desktop = store.sources().map(|s| s.iter().any(|s| s.source_id == batch.source_id && s.kind == "mac.desktop")).unwrap_or(false);
                    if desktop && !store.desktop_direct().unwrap_or(false) {
                        return HttpResponse::error(409, "cutover_pending", "桌面采集尚未切换到直写，请先完成切换。");
                    }
                    let result = store.ingest(&batch);
                    if result.as_ref().is_ok_and(|r| r.inserted + r.tombstoned > 0) {
                        self.record_event_count(&store);
                    }
                    result
                };
                match result {
                    Ok(r) => {
                        if r.inserted + r.tombstoned + r.threads_inserted + r.threads_updated > 0 {
                            self.feed.publish("status");
                            self.feed.publish("data");
                        }
                        ok_json(&serde_json::to_value(r).unwrap_or(Value::Null))
                    }
                    Err(e) => store_error(&e),
                }
            }
            ("POST", "/local/desktop/cutover") => match self.cutover() {
                Ok(record) => ok_json(&json!({"mode": "direct", "cutover": record})),
                Err((verification, reason)) => HttpResponse {
                    status: 409,
                    body: serde_json::to_vec(&json!({"error": {"code": "cutover_failed", "message": reason}, "mode": "ledger", "verification": verification}))
                        .unwrap_or_default(),
                },
            },
            ("GET", "/local/desktop/legacy-preferences") => {
                let Some(ledger) = self.ledger.as_ref().filter(|l| l.join("ledger.sqlite").exists()) else {
                    return HttpResponse::error(404, "no_ledger", "没有原桌面账本。");
                };
                match TimelineStore::legacy_desktop_preferences(ledger) {
                    Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
                        Ok(v) => ok_json(&json!({"preferences": v})),
                        Err(_) => HttpResponse::error(422, "unreadable", "原账本里的采集设置无法解析。"),
                    },
                    Ok(None) => HttpResponse::error(404, "not_found", "原账本里没有采集设置。"),
                    Err(e) => store_error(&e),
                }
            }
            ("POST", "/local/desktop/delete") => {
                let req: DesktopDelete = match decode(request, "删除") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                let reason = req.reason.clone().unwrap_or_else(|| "user".into());
                if reason != "user" && reason != "superseded" {
                    return HttpResponse::error(400, "invalid_input", "reason 只能是 user 或 superseded。");
                }
                let outcome = {
                    let store = lock(&self.store);
                    if let Some(activity) = &req.activity_id {
                        store.delete_desktop_activity(activity)
                    } else {
                        let mut ids = match store.desktop_event_ids(&req.record_ids) {
                            Ok(ids) => ids,
                            Err(e) => return store_error(&e),
                        };
                        match store.desktop_owned_events(&req.event_ids) {
                            Ok(more) => ids.extend(more),
                            Err(e) => return store_error(&e),
                        }
                        store.delete_events(&ids, &reason).map(|r| r.events)
                    }
                };
                self.after_delete(outcome)
            }
            ("POST", "/local/desktop/retention") => {
                let req: DesktopRetention = match decode(request, "保留期") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                let outcome = lock(&self.store).delete_desktop_before(req.before_ms);
                self.after_delete(outcome)
            }
            ("POST", "/local/desktop/free-space") => {
                let req: candid_store::ScreenshotSpaceRequest = match decode(request, "腾出空间") {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                let outcome = lock(&self.store).free_screenshot_space(&req);
                match outcome {
                    Ok(report) => {
                        if report.screenshots > 0 {
                            self.feed.publish("data");
                        }
                        ok_json(&json!(report))
                    }
                    Err(e) => store_error(&e),
                }
            }
            (_, "/local/agents" | "/local/agents/consent" | "/local/agents/query" | "/health" | "/local/status" | "/local/clients" | "/local/clients/revoke" | "/local/sync" | "/local/verify" | "/local/sources" | "/local/ingest" | "/local/events"
                | "/local/desktop/cutover" | "/local/desktop/legacy-preferences" | "/local/desktop/delete" | "/local/desktop/retention" | "/local/desktop/free-space") => {
                HttpResponse::error(405, "method", "方法不对。")
            }
            _ => HttpResponse::error(404, "not_found", &format!("没有这个接口：{path}")),
        }
    }
}

#[derive(Deserialize)]
struct DesktopDelete {
    #[serde(default)]
    activity_id: Option<String>,
    #[serde(default)]
    record_ids: Vec<String>,
    #[serde(default)]
    event_ids: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct DesktopRetention {
    before_ms: i64,
}

impl Core {
    fn after_delete(&self, outcome: candid_store::Result<usize>) -> HttpResponse {
        match outcome {
            Ok(n) => {
                if n > 0 {
                    self.record_event_count(&lock(&self.store));
                    self.feed.publish("status");
                    self.feed.publish("data");
                }
                ok_json(&json!({"deleted": n}))
            }
            Err(e) => store_error(&e),
        }
    }
}

fn register_source_once(store: &TimelineStore, kind: &str, name: &str, policy: &SourcePolicy) -> HttpResponse {
    let existing = match store.sources() {
        Ok(s) => s.into_iter().find(|s| s.kind == kind && s.display_name == name && s.status != "revoked"),
        Err(e) => return store_error(&e),
    };
    if let Some(s) = existing {
        return ok_json(&json!({"source_id": s.source_id, "created": false}));
    }
    match store.register_source(kind, name, policy) {
        Ok(id) => ok_json(&json!({"source_id": id, "created": true})),
        Err(e) => store_error(&e),
    }
}

/// Serves one accepted connection.
fn serve_connection(core: &Core, mut stream: UnixStream) {
    let request = match read_request(&mut stream) {
        None => return,
        Some(Ok(r)) => r,
        Some(Err(response)) => return write_response(&mut stream, &response),
    };
    if request.path.split('?').next() == Some("/local/events") && request.method == "GET" {
        let _ = stream.set_read_timeout(None);
        core.feed.subscribe(stream);
        return;
    }
    let response = core.handle(&request);
    write_response(&mut stream, &response);
}

/// Creates the socket's directory 0700 when missing and refuses a directory others can write or another user owns.
fn private_parent(path: &Path) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        return Ok(());
    }
    let meta = std::fs::metadata(dir)?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if meta.uid() != uid {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} 不属于当前用户", dir.display())));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Binds the socket 0600. A socket file nobody answers on is stale and replaced; a live one means another core is
/// running (`AddrInUse`).
pub fn bind_private(path: &Path) -> std::io::Result<UnixListener> {
    if path.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("套接字路径过长（{} 字节）", path.as_os_str().len())));
    }
    private_parent(path)?;
    if std::fs::symlink_metadata(path).is_ok() {
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "另一个核心进程正在运行"));
        }
        std::fs::remove_file(path)?;
    }
    // No umask juggling (it is process-wide): the directory is already 0700, so nobody else can reach the socket in
    // the moment before it is narrowed to 0600.
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// The Unix-socket server: one accept thread and a fixed pool of workers.
pub struct LocalServer {
    pub path: PathBuf,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    workers: Vec<JoinHandle<()>>,
    inode: u64,
}

impl LocalServer {
    pub fn start(listener: UnixListener, path: &Path, workers: usize, core: Arc<Core>) -> std::io::Result<LocalServer> {
        let inode = std::fs::metadata(path).map(|m| m.ino()).unwrap_or(0);
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<UnixStream>();
        let rx = Arc::new(Mutex::new(rx));
        let mut pool = Vec::new();
        for i in 0..workers.max(1) {
            let (rx, core) = (rx.clone(), core.clone());
            pool.push(std::thread::Builder::new().name(format!("candid-local-{i}")).spawn(move || loop {
                let next = rx.lock().map(|r| r.recv());
                match next {
                    Ok(Ok(stream)) => serve_connection(&core, stream),
                    _ => break,
                }
            })?);
        }
        let stop_flag = stop.clone();
        // Waits in poll(2) with a timeout rather than a blocking accept: a stop must not depend on reaching the socket
        // file, which may already be gone (a deleted run directory) when the App goes away.
        let fd = listener.as_raw_fd();
        let accept = std::thread::Builder::new().name("candid-local-accept".into()).spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                // SAFETY: one valid pollfd for the listener, which this thread owns.
                let ready = unsafe { libc::poll(&mut pfd, 1, 500) };
                if ready <= 0 || stop_flag.load(Ordering::SeqCst) {
                    continue;
                }
                if let Ok((s, _)) = listener.accept() {
                    if tx.send(s).is_err() {
                        break;
                    }
                }
            }
        })?;
        Ok(LocalServer { path: path.to_path_buf(), stop, accept: Some(accept), workers: pool, inode })
    }

    /// Stops accepting, finishes running requests and removes the socket file if it is still ours.
    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = UnixStream::connect(&self.path); // wakes the accept loop at once when the file is still there
        if let Some(a) = self.accept.take() {
            let _ = a.join();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        if std::fs::metadata(&self.path).map(|m| m.ino()).ok() == Some(self.inode) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A minimal client for the local channel (tests and scripts; the App has its own in Swift).
pub fn local_request(socket: &Path, method: &str, path: &str, body: &[u8]) -> std::io::Result<(u16, Vec<u8>)> {
    use std::io::Read;
    let mut s = UnixStream::connect(socket)?;
    s.set_read_timeout(Some(Duration::from_secs(120)))?;
    let head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| std::io::Error::other("bad response"))?;
    let status = String::from_utf8_lossy(&raw[..split]).split(' ').nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    Ok((status, raw[split + 4..].to_vec()))
}

/// The default socket for a run directory.
pub fn default_socket(run_dir: &Path) -> PathBuf {
    run_dir.join("core.sock")
}
