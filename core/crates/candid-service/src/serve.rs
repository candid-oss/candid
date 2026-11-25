//! The service without a user interface: opens a data root, serves the query tools on 127.0.0.1, writes
//! `endpoint.json` and the command-line token, and optionally mirrors the desktop ledger and the Hub media library
//! on an interval. As the App's helper (`--socket`) it also serves the local channel ([`crate::local`]).

use crate::endpoint::{RunDirectory, ServiceEndpoint};
use crate::http::{Handler, LoopbackServer};
use crate::local::{self, Core, LocalServer, SyncStatus};
use crate::router::{Router, SharedStore};
use candid_store::{ClientScope, Clock, ScopeTime, TimelineStore};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub const CLI_CLIENT_NAME: &str = "本机命令行";

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub root: PathBuf,
    pub run_dir: PathBuf,
    pub port: u16,
    pub workers: usize,
    pub created_by: String,
    /// `<dir>/ledger.sqlite` is mirrored when it exists.
    pub ledger: Option<PathBuf>,
    /// `<dir>/memory.sqlite` and `<dir>/vault/media_timeline` are mirrored when they exist.
    pub hub: Option<PathBuf>,
    pub sync_interval: Duration,
    /// The App's local channel (Unix domain socket); none for a plain headless service.
    pub socket: Option<PathBuf>,
    /// Exit when this process is no longer our parent (the App that launched us).
    pub parent_pid: Option<i32>,
    /// Command path for the setup snippets; defaults to this executable.
    pub cli_path: Option<String>,
    /// `--power ac|battery`: a declared power state for a core run without the App (never goes stale). None
    /// leaves the state unknown until a platform report arrives; unknown counts as battery.
    pub power: Option<bool>,
}

impl ServeOptions {
    pub fn new(root: impl Into<PathBuf>, run_dir: impl Into<PathBuf>) -> ServeOptions {
        ServeOptions {
            root: root.into(),
            run_dir: run_dir.into(),
            port: 0,
            workers: 2,
            created_by: format!("candid/{}", crate::VERSION),
            ledger: None,
            hub: None,
            sync_interval: Duration::from_secs(30),
            socket: None,
            parent_pid: None,
            cli_path: None,
            power: None,
        }
    }
}

/// Text and derived content for the last 90 days; no screenshots or audio, no proposals until approval exists.
pub fn default_scope() -> ClientScope {
    ClientScope {
        time: ScopeTime { max_days_back: Some(90), ..Default::default() },
        text: true,
        media: false,
        derived: true,
        propose: false,
        ..Default::default()
    }
}

/// The 「本机命令行」 client: its token sits in the run directory (0600). A missing or stale token is replaced and
/// the old registration revoked.
pub fn ensure_cli_token(store: &TimelineStore, run_dir: &Path) -> std::io::Result<()> {
    let file = RunDirectory::cli_token_file(run_dir);
    if let Ok(token) = std::fs::read_to_string(&file) {
        let token = token.trim();
        if !token.is_empty() && store.client_id_for_token(token).ok().flatten().is_some() {
            return Ok(());
        }
    }
    let io = |e: candid_store::TimelineError| std::io::Error::other(e.message());
    for c in store.clients().map_err(io)? {
        if c.transport == "cli" && c.revoked_ms.is_none() {
            store.revoke_client(&c.client_id).map_err(io)?;
        }
    }
    let reg = store.register_client(CLI_CLIENT_NAME, "cli", &default_scope(), None).map_err(io)?;
    if let Some(token) = reg.token {
        RunDirectory::write_private(token.as_bytes(), &file)?;
    }
    Ok(())
}

/// One mirroring pass; returns short count-only lines for the log.
pub fn sync_once(store: &TimelineStore, ledger: Option<&Path>, hub: Option<&Path>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(l) = ledger.filter(|l| l.join("ledger.sqlite").exists()) {
        match store.sync_desktop_ledger(l, None) {
            Ok(r) => lines.push(format!(
                "桌面：+{} −{} 修订 {} 未变 {} 冲突 {} 失败 {}",
                r.inserted,
                r.deleted,
                r.revised,
                r.unchanged,
                r.conflicts,
                r.failures.len()
            )),
            Err(e) => lines.push(format!("桌面：{}", e.code())),
        }
    }
    if let Some(h) = hub.filter(|h| h.join("memory.sqlite").exists()) {
        match store.migrate_media(h) {
            Ok(_) => lines.push("资料：Rust 迁移核对完成，旧库保留".into()),
            Err(e) => lines.push(format!("资料：{}",e.message())),
        }
    }
    match store.sync_agents() {
        Ok(v) => lines.push(format!(
            "AI：+{} 修订 {} 问题 {}",
            v["inserted"],
            v["revised"],
            v["issues"].as_array().map(Vec::len).unwrap_or(0)
        )),
        Err(e) => lines.push(format!("AI：{}", e.code())),
    };
    lines
}

pub struct Service {
    pub store: SharedStore,
    pub endpoint: ServiceEndpoint,
    pub core: Arc<Core>,
    run_dir: PathBuf,
    server: LoopbackServer,
    local: Option<LocalServer>,
    stop: Arc<AtomicBool>,
    sync: Option<JoinHandle<()>>,
    workflows: Option<JoinHandle<()>>,
    convert: Option<JoinHandle<()>>,
}

fn utc_seconds_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Why the service could not start; `Busy` means another core already holds the data root or the socket.
#[derive(Debug)]
pub enum StartError {
    Busy(String),
    Failed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Busy(m) | StartError::Failed(m) => f.write_str(m),
        }
    }
}

impl Service {
    pub fn start(opts: &ServeOptions, log: impl Fn(&str) + Send + Sync + 'static) -> Result<Service, String> {
        Self::start_checked(opts, log).map_err(|e| e.to_string())
    }

    /// Opens the data root, binds the local socket (when configured) and the loopback HTTP, writes `endpoint.json`
    /// and the command-line token, then starts the mirroring thread. The socket is bound before any thread starts.
    pub fn start_checked(opts: &ServeOptions, log: impl Fn(&str) + Send + Sync + 'static) -> Result<Service, StartError> {
        let failed = StartError::Failed;
        let store = TimelineStore::open(&opts.root, &opts.created_by, true, Clock::default()).map_err(|e| match e {
            candid_store::TimelineError::Locked => StartError::Busy(e.message()),
            other => StartError::Failed(other.message()),
        })?;
        if let Err(e)=store.prepare_media(opts.hub.as_deref()){log(&format!("资料暂不可用：{}",e.message()));}
        let listener = match &opts.socket {
            Some(path) => Some(local::bind_private(path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    StartError::Busy(format!("{}：{e}", path.display()))
                } else {
                    StartError::Failed(format!("本地通道 {}：{e}", path.display()))
                }
            })?),
            None => None,
        };
        let store_id = store.identity.store_id.clone();
        let internal = store.internal_client(local::APP_CLIENT_NAME, &ClientScope::unbounded()).map_err(|e| failed(e.message()))?;
        let shared: SharedStore = Arc::new(Mutex::new(store));
        let router = Router::new(shared.clone());
        let handler: Handler = Arc::new(move |req, port| router.handle(req, port));
        let server = LoopbackServer::start(opts.port, opts.workers, handler).map_err(|e| failed(format!("无法监听 127.0.0.1:{}：{e}", opts.port)))?;
        let endpoint = ServiceEndpoint {
            url: format!("http://127.0.0.1:{}", server.port),
            pid: std::process::id() as i32,
            store_id: store_id.clone(),
            started_at: utc_seconds_now(),
        };
        let core = Arc::new(Core {
            store: shared.clone(),
            status: Mutex::new(SyncStatus::default()),
            feed: Default::default(),
            trigger: Default::default(),
            internal_client: internal,
            cli_path: opts.cli_path.clone().unwrap_or_else(|| {
                std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "candid".into())
            }),
            ledger: opts.ledger.clone(),
            hub: opts.hub.clone(),
            root: opts.root.clone(),
            store_id,
            endpoint: Mutex::new(endpoint.url.clone()),
            socket: opts.socket.clone().unwrap_or_default(),
            parent_pid: opts.parent_pid,
            started: std::time::Instant::now(),
            cutover_lock: Mutex::new(()),
            background: Mutex::new(None),
        });
        core.load_capture_mode();
        let data = serde_json::to_vec(&endpoint).map_err(|e| failed(e.to_string()))?;
        RunDirectory::write_private(&data, &RunDirectory::endpoint_file(&opts.run_dir)).map_err(|e| failed(format!("endpoint.json：{e}")))?;
        {
            let store = shared.lock().map_err(|_| failed("store lock".into()))?;
            ensure_cli_token(&store, &opts.run_dir).map_err(|e| failed(format!("cli.token：{e}")))?;
            store.power_declare(opts.power).map_err(|e| failed(format!("电源状态：{}", e.message())))?;
            let events = store.event_count().unwrap_or(0);
            core.update_status(|s| s.events = events);
        }
        // With no legacy mirrors the opened library is ready immediately; AI refresh reports `syncing`
        // independently, preserving the existing startup contract while still scheduling AI-only libraries.
        if opts.ledger.is_none() && opts.hub.is_none() {core.update_status(|s|s.phase="ready".into());}
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(log);
        let sync = {
            let (core, stop, interval, log) = (core.clone(), stop.clone(), opts.sync_interval, log.clone());
            Some(
                std::thread::Builder::new()
                    .name("candid-sync".into())
                    .spawn(move || {
                        crate::scheduling::background();
                        while !stop.load(Ordering::SeqCst) {
                            let before = core.status();
                            core.sync_pass();
                            let s = core.status();
                            // Counts only, and only when something changed, so the App's log stays small.
                            if before.last_sync_ms.is_none() || s.events != before.events || !s.problems.is_empty() {
                                log(&format!("同步：事件 {}{}{}", s.events, s.last_change.map(|c| format!("，{c}")).unwrap_or_default(),
                                    if s.problems.is_empty() { String::new() } else { format!("，问题 {}", s.problems.len()) }));
                            }
                            core.trigger.wait(interval, &stop);
                        }
                    })
                    .map_err(|e| failed(e.to_string()))?,
            )
        };
        // Served last: the socket is bound (connections wait in its backlog), but the App only gets answers once the
        // endpoint, the command-line token and the first status exist.
        let local = match (listener, &opts.socket) {
            (Some(l), Some(path)) => Some(LocalServer::start(l, path, opts.workers, core.clone()).map_err(|e| failed(e.to_string()))?),
            _ => None,
        };
        let workflow_store=shared.clone();let workflow_stop=stop.clone();let workflow_core=core.clone();
        let workflows = Some(std::thread::Builder::new().name("candid-workflows".into()).spawn(move || {
            crate::scheduling::background();
            let mut worker = crate::workflows::Worker::default();
            let mut compactor = crate::audio_compaction::Compactor::default();
            while !workflow_stop.load(Ordering::SeqCst) {
                let before = jobs_revision(&workflow_store);
                worker.tick(&workflow_store, &|| workflow_core.feed.publish("data"));
                compactor.tick(&workflow_store);
                if jobs_revision(&workflow_store) != before { workflow_core.feed.publish("data"); }
                if !workflow_stop.load(Ordering::SeqCst) { std::thread::park_timeout(Duration::from_secs(2)); }
            }
        }).map_err(|e| failed(e.to_string()))?);
        if let Some(w) = &workflows { *core.background.lock().unwrap_or_else(|p| p.into_inner()) = Some(w.thread().clone()); }
        let convert = crate::library_conversion::start(shared.clone(), stop.clone()).ok();
        Ok(Service { workflows, convert, store: shared, endpoint, core, run_dir: opts.run_dir.clone(), server, local, stop, sync })
    }

    pub fn port(&self) -> u16 {
        self.server.port
    }

    /// Stops serving (a running mirroring pass finishes first), removes the endpoint and socket this process wrote,
    /// and releases the data root.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.core.trigger.request();
        if let Some(w) = &self.workflows { w.thread().unpark(); }
        if let Some(s) = self.sync.take() {
            let _ = s.join();
        }
        if let Some(w)=self.workflows.take(){let _=w.join();}
        if let Some(c) = self.convert.take() {
            c.thread().unpark();
            let _ = c.join();
        }
        self.core.feed.close_all();
        if let Some(mut l) = self.local.take() {
            l.stop();
        }
        self.server.stop();
        if let Ok(current) = RunDirectory::read_endpoint(&self.run_dir) {
            if current.pid == std::process::id() as i32 && current.url == self.endpoint.url {
                let _ = std::fs::remove_file(RunDirectory::endpoint_file(&self.run_dir));
            }
        }
        if let Ok(mut s) = self.store.lock() {
            s.close();
        }
    }
}

fn jobs_revision(store: &SharedStore) -> Option<i64> {
    store.lock().ok().and_then(|s| s.background_revision("jobs").ok())
}
