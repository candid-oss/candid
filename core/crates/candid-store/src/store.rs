//! The single owner of a data root (§2). All reads and writes go through [`TimelineStore`].

use crate::clock::Clock;
use crate::db::{int, opt_text, text, Db, RowExt};
use crate::ids::{sha256_hex, sha256_hex_file, token_hex, uuid_v4, uuid_v7};
use crate::json;
use crate::schema::{self, LAYOUT, SCHEMA_VERSION, SOURCE_KINDS};
use crate::types::{ClientScope, SourceInfo, SourcePolicy, StoreIdentity};
use crate::{Result, TimelineError};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};

/// Holds the exclusive single-writer lock on a data root for the life of the store.
pub(crate) struct RootLock {
    file: Option<fs::File>,
}

impl RootLock {
    fn acquire(path: &Path) -> Result<RootLock> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|_| TimelineError::InvalidStore("无法创建锁文件".into()))?;
        // SAFETY: flock on a valid descriptor owned by `file`.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(TimelineError::Locked);
        }
        Ok(RootLock { file: Some(file) })
    }

    fn release(&mut self) {
        if let Some(f) = self.file.take() {
            // SAFETY: as above; closing the file also releases the lock.
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

impl Drop for RootLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Points inside ingest where tests can simulate a crash (`TimelineError::SimulatedCrash` leaves files in place).
pub type IngestHook = Box<dyn Fn(&str) -> Result<()> + Send>;

#[derive(Default)]
pub struct TestHooks {
    pub ingest: Option<IngestHook>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub staging_files_removed: usize,
    pub orphan_blob_files_removed: usize,
    pub missing_blob_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityReport {
    pub ok: bool,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClientInfo {
    pub client_id: String,
    pub name: String,
    pub transport: String,
    pub scope: ClientScope,
    pub created_ms: i64,
    pub revoked_ms: Option<i64>,
    pub last_access_ms: Option<i64>,
    pub calls_7d: i64,
    pub denied_7d: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRegistration {
    pub client_id: String,
    /// Token plaintext is returned once; only its hash is stored.
    pub token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub seq: i64,
    pub entity: String,
    pub id: String,
    pub op: String,
}

pub(crate) type AgentProbe = Box<dyn Fn() -> (bool, Option<String>) + Send>;

pub struct TimelineStore {
    pub root: PathBuf,
    pub identity: StoreIdentity,
    pub recovery: RecoveryReport,
    pub clock: Clock,
    pub(crate) db: Db,
    lock: RootLock,
    pub(crate) hooks: TestHooks,
    pub(crate) agent_probe: AgentProbe,
    pub(crate) media_locks: std::cell::RefCell<std::collections::BTreeMap<String,Db>>,
    pub(crate) workflow_scan_stamp: std::cell::Cell<Option<(i64, i64)>>,
    pub(crate) desktop_read_cache: std::cell::RefCell<crate::desktop_read_cache::DesktopReadCache>,
    pub(crate) media_projection_stamp: std::cell::Cell<Option<i64>>,
    pub(crate) media_cleanup_stamp: std::cell::Cell<Option<(i64, i64)>>,
    pub(crate) agent_file_stamps: std::cell::RefCell<std::collections::HashMap<String,(u64,Option<std::time::SystemTime>)>>,
}

/// `.`/`..` removed without resolving symlinks (Foundation's `standardizedFileURL`).
pub(crate) fn standardize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
    };
    let mut out = PathBuf::from("/");
    for c in absolute.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(p) => out.push(p),
            _ => {}
        }
    }
    out
}

pub(crate) fn mkdir_private(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn iso_utc_seconds(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc.timestamp_millis_opt(ms).single().unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

impl TimelineStore {
    pub const SUPPORTED_SCHEMA_VERSION: i64 = SCHEMA_VERSION;

    /// Creates the data root when absent (and `create` is true), otherwise validates and reopens it.
    pub fn open(root: impl AsRef<Path>, created_by: &str, create: bool, clock: Clock) -> Result<TimelineStore> {
        let root = standardize(root.as_ref());
        let store_json = root.join("store.json");
        let db_path = root.join("timeline.sqlite");
        if root.exists() && !root.is_dir() {
            return Err(TimelineError::InvalidStore("路径不是目录".into()));
        }
        let has_json = store_json.exists();
        let has_db = db_path.exists();
        if !has_json && !has_db && !create {
            return Err(TimelineError::InvalidStore("数据根目录不存在或为空".into()));
        }
        if has_json != has_db {
            return Err(TimelineError::InvalidStore(
                if has_json { "缺少 timeline.sqlite，不会自动重建" } else { "缺少 store.json，不会自动重建" }.into(),
            ));
        }
        mkdir_private(&root)?;
        let lock = RootLock::acquire(&root.join(".lock"))?;

        let mut identity: StoreIdentity;
        let db: Db;
        if has_json {
            let data = fs::read(&store_json)?;
            identity = serde_json::from_slice(&data).map_err(|e| TimelineError::InvalidStore(format!("store.json: {e}")))?;
            if identity.layout != LAYOUT {
                return Err(TimelineError::InvalidStore(format!("未知布局 {}", identity.layout)));
            }
            if identity.schema_version > SCHEMA_VERSION {
                return Err(TimelineError::NewerSchema { found: identity.schema_version, supported: SCHEMA_VERSION });
            }
            db = Db::open(&db_path, false, false)?;
            let version = db.scalar("PRAGMA user_version", &[])?;
            if version > SCHEMA_VERSION {
                return Err(TimelineError::NewerSchema { found: version, supported: SCHEMA_VERSION });
            }
            crate::schema_upgrade::upgrade(&db, &root, &mut identity, version)?;
        } else {
            let now = clock.now_ms();
            identity = StoreIdentity {
                store_id: uuid_v4(),
                schema_version: SCHEMA_VERSION,
                created_at: iso_utc_seconds(now),
                created_by: created_by.into(),
                layout: LAYOUT.into(),
            };
            db = Db::open(&db_path, false, true)?;
            db.execute("PRAGMA journal_mode=WAL", &[])?;
            schema::create(&db)?;
            // store.json is written last, so a crash mid-creation leaves an inconsistent root that is refused, not half-used.
            let temp = root.join(".store.json.tmp");
            fs::write(&temp, json::canonical_of(&identity))?;
            fs::rename(&temp, &store_json)?;
        }
        db.execute("PRAGMA journal_mode=WAL", &[])?;
        db.execute("PRAGMA synchronous=FULL", &[])?;
        db.execute("PRAGMA foreign_keys=ON", &[])?;
        db.execute("PRAGMA busy_timeout=5000", &[])?;
        for dir in ["blobs/sha256", "staging", "exports", "logs"] {
            mkdir_private(&root.join(dir))?;
        }
        let _ = fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600));
        let recovery = Self::recover(&root, &db)?;
        let store = TimelineStore {
            root,
            identity,
            recovery,
            clock,
            db,
            lock,
            hooks: TestHooks::default(),
            media_locks: Default::default(),
            workflow_scan_stamp: Default::default(),
            desktop_read_cache: Default::default(),
            media_projection_stamp: Default::default(),
            media_cleanup_stamp: Default::default(),
            agent_file_stamps: Default::default(),
            agent_probe: Box::new(|| (false, Some("not_configured".into()))),
        };
        // Pre-v5 workflow drafts move into the derived tables once; a failure rolls back and retries next open.
        let _ = store.migrate_workflow_drafts();
        Ok(store)
    }

    /// Releases the writer lock. The store should be dropped afterwards.
    pub fn close(&mut self) {
        self.lock.release();
    }

    pub fn set_hooks(&mut self, hooks: TestHooks) {
        self.hooks = hooks;
    }

    pub fn set_agent_probe(&mut self, probe: impl Fn() -> (bool, Option<String>) + Send + 'static) {
        self.agent_probe = Box::new(probe);
    }

    pub fn staging_directory(&self) -> PathBuf {
        self.root.join("staging")
    }

    pub fn database_path(&self) -> PathBuf {
        self.root.join("timeline.sqlite")
    }

    /// Raw connection for tests and fixtures in this workspace.
    #[doc(hidden)]
    pub fn raw_db(&self) -> &Db {
        &self.db
    }

    // MARK: Startup recovery (§6.2)

    /// Staging holds only uncommitted batches, so it is emptied. Blob files that no row references
    /// (a crash between the file move and the commit) are removed; rows whose file is gone are reported.
    fn recover(root: &Path, db: &Db) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        if let Ok(entries) = fs::read_dir(root.join("staging")) {
            for entry in entries.flatten() {
                let p = entry.path();
                let removed = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                if removed.is_ok() || !p.exists() {
                    report.staging_files_removed += 1;
                }
            }
        }
        let known: HashSet<String> = db.query("SELECT rel_path FROM blobs", &[], |r| r.text(0))?.into_iter().collect();
        let blob_root = root.join("blobs/sha256");
        let mut stack = vec![blob_root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let p = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(p);
                } else if ft.is_file() {
                    let relative = p.strip_prefix(root).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
                    if !known.contains(&relative) {
                        let _ = fs::remove_file(&p);
                        report.orphan_blob_files_removed += 1;
                    }
                }
            }
        }
        for path in &known {
            if !root.join(path).exists() {
                report.missing_blob_files.push(path.clone());
            }
        }
        report.missing_blob_files.sort();
        Ok(report)
    }

    // MARK: Integrity (on demand)

    /// `PRAGMA integrity_check`, foreign keys, FTS consistency, and blob files. `verify_blob_hashes` re-hashes every original.
    pub fn integrity_check(&self, verify_blob_hashes: bool) -> Result<IntegrityReport> {
        let mut issues = Vec::new();
        let result = self.db.query("PRAGMA integrity_check", &[], |r| r.text(0))?;
        if result != vec!["ok".to_string()] {
            issues.extend(result.into_iter().map(|r| format!("integrity_check: {r}")));
        }
        let fk = self.db.query("PRAGMA foreign_key_check", &[], |r| {
            format!("{} rowid {} → {}", r.text(0), r.opt_int(1).map(|n| n.to_string()).unwrap_or_else(|| "?".into()), r.text(2))
        })?;
        issues.extend(fk.into_iter().map(|r| format!("foreign_key: {r}")));
        if let Err(e) = self.db.execute("INSERT INTO text_fts(text_fts) VALUES('integrity-check')", &[]) {
            issues.push(format!("text_fts: {}", e.message()));
        }
        let version = self.db.scalar("PRAGMA user_version", &[])?;
        if version != self.identity.schema_version {
            issues.push(format!("user_version {version} ≠ store.json {}", self.identity.schema_version));
        }
        let blobs = self.db.query("SELECT sha256, rel_path, byte_count FROM blobs", &[], |r| (r.text(0), r.text(1), r.int(2)))?;
        for (sha, rel, size) in blobs {
            let path = self.root.join(&rel);
            let Ok(meta) = fs::metadata(&path) else {
                issues.push(format!("blob 缺失: {sha}"));
                continue;
            };
            if meta.len() as i64 != size {
                issues.push(format!("blob 大小不一致: {sha}"));
            }
            if verify_blob_hashes {
                if sha256_hex_file(&path).ok().as_deref() != Some(sha.as_str()) {
                    issues.push(format!("blob hash 不一致: {sha}"));
                } else {
                    self.db.execute("UPDATE blobs SET verified_ms=? WHERE sha256=?", &[int(self.clock.now_ms()), text(&sha)])?;
                }
            }
        }
        Ok(IntegrityReport { ok: issues.is_empty(), issues })
    }

    // MARK: Sources

    #[allow(clippy::too_many_arguments)]
    pub fn register_source_with(
        &self,
        kind: &str,
        display_name: &str,
        policy: &SourcePolicy,
        device_id: Option<&str>,
        account_ref: Option<&str>,
        secret_ref: Option<&str>,
        config: &Value,
        id: Option<&str>,
    ) -> Result<String> {
        if !SOURCE_KINDS.contains(&kind) {
            return Err(TimelineError::InvalidInput(format!("未知来源种类 {kind}")));
        }
        let now = self.clock.now_ms();
        let source_id = id.map(String::from).unwrap_or_else(|| uuid_v7(Some(now)));
        let policy_json = json::canonical_of(policy);
        self.db.transaction(|| {
            self.db.execute(
                "INSERT INTO sources(source_id,kind,display_name,device_id,account_ref,secret_ref,config_json,policy_json,status,created_ms,updated_ms)
                VALUES(?,?,?,?,?,?,?,?,'active',?,?)",
                &[
                    text(&source_id),
                    text(kind),
                    text(display_name),
                    opt_text(device_id),
                    opt_text(account_ref),
                    opt_text(secret_ref),
                    text(json::canonical(config)),
                    text(policy_json.clone()),
                    int(now),
                    int(now),
                ],
            )?;
            self.record_change("source", &source_id, "insert")
        })?;
        Ok(source_id)
    }

    pub fn register_source(&self, kind: &str, display_name: &str, policy: &SourcePolicy) -> Result<String> {
        self.register_source_with(kind, display_name, policy, None, None, None, &serde_json::json!({"version": 1}), None)
    }

    pub fn set_source_status(&self, source_id: &str, status: &str, error: Option<&str>) -> Result<()> {
        if !["active", "paused", "error", "revoked"].contains(&status) {
            return Err(TimelineError::InvalidInput(format!("状态 {status}")));
        }
        self.db.transaction(|| {
            let old = self.db.first("SELECT status FROM sources WHERE source_id=?", &[text(source_id)], |r| r.text(0))?;
            if old.as_deref().is_some_and(|old| (old == "revoked") != (status == "revoked")) {
                self.voiceprint_authorization_advance(source_id)?;
            }

            self.db.execute(
                "UPDATE sources SET status=?, last_error=?, updated_ms=? WHERE source_id=?",
                &[text(status), opt_text(error), int(self.clock.now_ms()), text(source_id)],
            )?;
            if self.db.changes() == 0 {
                return Err(TimelineError::UnknownSource(source_id.into()));
            }
            self.record_change("source", source_id, "update")
        })
    }

    pub fn update_policy(&self, source_id: &str, policy: &SourcePolicy) -> Result<()> {
        let policy_json = json::canonical_of(policy);
        self.db.transaction(|| {
            let old = self.db.first("SELECT policy_json FROM sources WHERE source_id=?", &[text(source_id)], |r| r.text(0))?;
            if old.as_ref().is_some_and(|old| *old != policy_json) {
                self.voiceprint_authorization_advance(source_id)?;
            }

            self.db.execute(
                "UPDATE sources SET policy_json=?, updated_ms=? WHERE source_id=?",
                &[text(policy_json.clone()), int(self.clock.now_ms()), text(source_id)],
            )?;
            if self.db.changes() == 0 {
                return Err(TimelineError::UnknownSource(source_id.into()));
            }
            self.record_change("source", source_id, "update")
        })
    }

    pub fn sources(&self) -> Result<Vec<SourceInfo>> {
        self.db.query(
            "SELECT source_id,kind,display_name,device_id,status,last_sync_ms,last_error,policy_json FROM sources ORDER BY created_ms, source_id",
            &[],
            |r| SourceInfo {
                source_id: r.text(0),
                kind: r.text(1),
                display_name: r.text(2),
                device_id: r.opt_text(3),
                status: r.text(4),
                last_sync_ms: r.opt_int(5),
                last_error: r.opt_text(6),
                policy: serde_json::from_str(&r.text(7)).unwrap_or_default(),
            },
        )
    }

    /// Records a successful check of a source (new data or not) and clears an earlier sync error.
    pub(crate) fn touch_source_sync(&self, source_id: &str) -> Result<()> {
        self.db.execute(
            "UPDATE sources SET last_sync_ms=?, status=CASE WHEN status='error' THEN 'active' ELSE status END, last_error=CASE WHEN status='error' THEN NULL ELSE last_error END WHERE source_id=?",
            &[int(self.clock.now_ms()), text(source_id)],
        )
    }

    /// When the source last wrote new data into the timeline, from the events themselves (no extra column). Deleted
    /// events do not count; `None` when the source has no events.
    pub fn last_ingest_ms(&self, source_id: &str) -> Result<Option<i64>> {
        self.db.first("SELECT ingested_ms FROM events WHERE source_id=? ORDER BY ingest_seq DESC LIMIT 1", &[text(source_id)], |r| r.int(0))
    }

    pub fn cursor(&self, source_id: &str, stream: &str) -> Result<Option<String>> {
        self.db.first("SELECT cursor FROM source_cursors WHERE source_id=? AND stream=?", &[text(source_id), text(stream)], |r| r.text(0))
    }

    // MARK: Clients

    pub fn register_client(&self, name: &str, transport: &str, scope: &ClientScope, id: Option<&str>) -> Result<ClientRegistration> {
        if !["mcp", "http", "cli", "internal"].contains(&transport) {
            return Err(TimelineError::InvalidInput(format!("transport {transport}")));
        }
        let client_id = id.map(String::from).unwrap_or_else(|| uuid_v7(Some(self.clock.now_ms())));
        let token = (transport != "internal").then(token_hex);
        self.db.execute(
            "INSERT INTO clients(client_id,name,transport,token_hash,scope_json,created_ms) VALUES(?,?,?,?,?,?)",
            &[
                text(&client_id),
                text(name),
                text(transport),
                opt_text(token.as_ref().map(sha256_hex)),
                text(json::canonical_of(scope)),
                int(self.clock.now_ms()),
            ],
        )?;
        Ok(ClientRegistration { client_id, token })
    }

    pub fn update_scope(&self, client_id: &str, scope: &ClientScope) -> Result<()> {
        self.db.execute("UPDATE clients SET scope_json=? WHERE client_id=?", &[text(json::canonical_of(scope)), text(client_id)])?;
        if self.db.changes() == 0 {
            return Err(TimelineError::UnknownClient(client_id.into()));
        }
        Ok(())
    }

    pub fn revoke_client(&self, client_id: &str) -> Result<()> {
        self.db.execute(
            "UPDATE clients SET revoked_ms=? WHERE client_id=? AND revoked_ms IS NULL",
            &[int(self.clock.now_ms()), text(client_id)],
        )
    }

    /// Registered clients with their recent use, newest first; revoked ones are listed so the history stays visible.
    pub fn clients(&self) -> Result<Vec<ClientInfo>> {
        let since = self.clock.now_ms() - 7 * 86_400_000;
        self.db.query(
            "SELECT c.client_id,c.name,c.transport,c.scope_json,c.created_ms,c.revoked_ms,
                   (SELECT MAX(at_ms) FROM access_log a WHERE a.client_id=c.client_id),
                   (SELECT COUNT(*) FROM access_log a WHERE a.client_id=c.client_id AND a.at_ms>=?),
                   (SELECT COUNT(*) FROM access_log a WHERE a.client_id=c.client_id AND a.at_ms>=? AND a.denied IS NOT NULL)
            FROM clients c WHERE c.transport<>'internal' ORDER BY c.created_ms DESC",
            &[int(since), int(since)],
            |r| ClientInfo {
                client_id: r.text(0),
                name: r.text(1),
                transport: r.text(2),
                scope: serde_json::from_str(&r.text(3)).unwrap_or_default(),
                created_ms: r.int(4),
                revoked_ms: r.opt_int(5),
                last_access_ms: r.opt_int(6),
                calls_7d: r.int(7),
                denied_7d: r.int(8),
            },
        )
    }

    /// The App's own access (`transport='internal'`, no token): one live registration per name, created on first use
    /// and never listed by [`TimelineStore::clients`]. Its calls are scoped and audited like any other client's.
    pub fn internal_client(&self, name: &str, scope: &ClientScope) -> Result<String> {
        let existing = self.db.first(
            "SELECT client_id FROM clients WHERE transport='internal' AND name=? AND revoked_ms IS NULL ORDER BY created_ms LIMIT 1",
            &[text(name)],
            |r| r.text(0),
        )?;
        match existing {
            Some(id) => Ok(id),
            None => Ok(self.register_client(name, "internal", scope, None)?.client_id),
        }
    }

    /// Resolves a bearer token to a client (transport hook).
    pub fn client_id_for_token(&self, token: &str) -> Result<Option<String>> {
        self.db.first("SELECT client_id FROM clients WHERE token_hash=? AND revoked_ms IS NULL", &[text(sha256_hex(token))], |r| r.text(0))
    }

    // MARK: Internals shared by ingest, deletion and queries

    pub(crate) fn record_change(&self, entity: &str, id: &str, op: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO changes(at_ms,entity,entity_id,op) VALUES(?,?,?,?)",
            &[int(self.clock.now_ms()), text(entity), text(id), text(op)],
        )
    }

    pub(crate) fn meta_value(&self, key: &str) -> Result<Option<String>> {
        self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(key)], |r| r.text(0))
    }

    pub(crate) fn next_ingest_seq(&self, count: usize) -> Result<i64> {
        let next = self.meta_value("ingest_seq_next")?.and_then(|v| v.parse::<i64>().ok()).unwrap_or(1);
        self.db.execute("UPDATE store_meta SET value=? WHERE key='ingest_seq_next'", &[text((next + count as i64).to_string())])?;
        Ok(next)
    }

    /// Highest assigned ingest_seq (the snapshot position for paging).
    pub(crate) fn current_seq(&self) -> Result<i64> {
        Ok(self.meta_value("ingest_seq_next")?.and_then(|v| v.parse::<i64>().ok()).unwrap_or(1) - 1)
    }

    pub fn changes(&self, after: i64, limit: i64) -> Result<Vec<Change>> {
        self.db.query(
            "SELECT seq,entity,entity_id,op FROM changes WHERE seq>? ORDER BY seq LIMIT ?",
            &[int(after), int(limit)],
            |r| Change { seq: r.int(0), entity: r.text(1), id: r.text(2), op: r.text(3) },
        )
    }

    pub fn event_id(&self, source_id: &str, source_key: &str) -> Result<Option<String>> {
        self.db.first("SELECT event_id FROM events WHERE source_id=? AND source_key=?", &[text(source_id), text(source_key)], |r| r.text(0))
    }

    pub fn table_counts(&self) -> Result<BTreeMap<String, i64>> {
        let mut out = BTreeMap::new();
        for table in ["events", "threads", "text_chunks", "event_text", "blobs", "event_blobs", "ingest_conflicts", "tombstones"] {
            out.insert(table.to_string(), self.db.scalar(&format!("SELECT COUNT(*) FROM {table}"), &[])?);
        }
        Ok(out)
    }

    pub fn event_count(&self) -> Result<i64> {
        self.db.scalar("SELECT COUNT(*) FROM events", &[])
    }
}
