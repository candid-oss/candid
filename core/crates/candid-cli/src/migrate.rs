//! `candid backup`, `candid restore` and `candid move-root` (docs/data-migration.md). They work on the files
//! directly and only read through `candid-store`: the database snapshot is SQLite's `VACUUM INTO` on a read
//! connection, so a backup can run while the core is serving; restore and move-root take the data root's writer
//! lock (the same `flock` on `.lock` the core holds) and refuse while a core is running.

use candid_store::{Clock, TimelineStore, SCHEMA_VERSION};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

pub const FORMAT: &str = "candid-backup/1";
const MANIFEST: &str = "manifest.json";
const DEFAULTS_DOMAIN: &str = "io.github.candid-oss.candid";
/// App-side entries under the support directory that a backup carries (the rest is rebuilt, secret or a cache).
pub const APP_ENTRIES: &[&str] =
    &["capture", "conversations", "ai-providers", "MediaWorkspace", "pi/sessions", "pi/agent/models-store.json"];
/// How long restore and move-root wait for a core that is shutting down to release the data root.
const LOCK_WAIT: Duration = Duration::from_secs(15);

type Res<T> = std::result::Result<T, String>;

/// Runtime files, tokens and journals that never go into a backup.
fn skipped(name: &str) -> bool {
    [ "-wal", "-shm", "-journal", ".pid", ".lock", ".log", ".partial", ".pending"].iter().any(|s| name.ends_with(s))
        || name.contains(".log.")
        || matches!(name, ".DS_Store" | "connection.json" | "auth.json" | "tmp_uploads" | "staging")
}

pub struct Options {
    pub target: Option<PathBuf>,
    pub support: PathBuf,
    /// The support directory is the real one, so the App's UserDefaults domain belongs to it.
    pub real_support: bool,
    pub json: bool,
    pub no_media: bool,
    pub move_existing: bool,
    pub remove_old: bool,
}

pub fn default_support() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    home.join("Library/Application Support/Candid")
}

impl Options {
    pub fn parse(command: &str, args: &[String]) -> Res<Options> {
        let mut o = Options {
            target: None,
            support: default_support(),
            real_support: true,
            json: false,
            no_media: false,
            move_existing: false,
            remove_old: false,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--json" => o.json = true,
                "--no-media" if command == "backup" => o.no_media = true,
                "--move-existing" if command == "restore" => o.move_existing = true,
                "--remove-old" if command == "move-root" => o.remove_old = true,
                "--support" => {
                    o.support = absolute(Path::new(it.next().ok_or("--support 需要一个目录")?));
                    o.real_support = o.support == default_support();
                }
                flag if flag.starts_with("--") => return Err(format!("{command} 没有参数 {flag}")),
                path if o.target.is_none() => o.target = Some(absolute(Path::new(path))),
                extra => return Err(format!("多余的参数：{extra}")),
            }
        }
        Ok(o)
    }
}

/// Progress as JSON lines on stdout (`--json`, for the App) or a percentage on stderr.
pub struct Report {
    json: bool,
    phase: String,
    done: u64,
    total: u64,
    last: Option<Instant>,
}

impl Report {
    pub fn new(json: bool) -> Report {
        Report { json, phase: String::new(), done: 0, total: 0, last: None }
    }
    fn phase(&mut self, phase: &str, total: u64) {
        self.phase = phase.into();
        self.done = 0;
        self.total = total;
        self.emit(true);
    }
    fn add(&mut self, n: u64) {
        self.done += n;
        self.emit(false);
    }
    fn emit(&mut self, force: bool) {
        if !force && self.last.is_some_and(|t| t.elapsed() < Duration::from_millis(150)) && self.done < self.total {
            return;
        }
        self.last = Some(Instant::now());
        if self.json {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", json!({"phase": self.phase, "done": self.done, "total": self.total}));
            let _ = out.flush();
        } else {
            let pct = (self.done * 100).checked_div(self.total).unwrap_or(0);
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{} {pct:>3}%", phase_label(&self.phase));
            if self.done >= self.total {
                let _ = writeln!(err);
            }
        }
    }
}

fn phase_label(phase: &str) -> &str {
    match phase {
        "snapshot" => "数据库快照",
        "copy" => "复制文件",
        "verify" => "校验",
        "restore" => "恢复文件",
        "upgrade" => "打开并升级库",
        other => other,
    }
}

// MARK: Helpers

fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::from("/");
    for c in joined.components() {
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

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn mkdirs(dir: &Path) -> Res<()> {
    fs::create_dir_all(dir).map_err(|e| format!("无法创建 {}：{e}", dir.display()))?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    Ok(())
}

fn is_empty_dir(path: &Path) -> bool {
    fs::read_dir(path).map(|mut d| d.next().is_none()).unwrap_or(false)
}

fn sha256_file(path: &Path) -> Res<String> {
    let mut f = fs::File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Copies one file, hashing what it reads, into a destination that must not exist yet.
fn copy_hashed(src: &Path, dst: &Path, report: &mut Report) -> Res<(u64, String)> {
    if let Some(parent) = dst.parent() {
        mkdirs(parent)?;
    }
    let mut input = fs::File::open(src).map_err(|e| format!("无法读取 {}：{e}", src.display()))?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dst)
        .map_err(|e| format!("无法写入 {}：{e}", dst.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buf).map_err(|e| format!("读取 {} 失败：{e}", src.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        output.write_all(&buf[..n]).map_err(|e| format!("写入 {} 失败：{e}", dst.display()))?;
        total += n as u64;
        report.add(n as u64);
    }
    Ok((total, hex(&h.finalize())))
}

/// A read connection to a live WAL database, or to one whose writer is gone (no `-shm`, which a read-only
/// connection cannot create).
fn open_source(path: &Path) -> Res<Connection> {
    let probe = |c: &Connection| c.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())).is_ok();
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(c) = Connection::open_with_flags(path, flags) {
        if probe(&c) {
            return Ok(c);
        }
    }
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("无法打开 {}：{e}", path.display()))?;
    if probe(&c) {
        Ok(c)
    } else {
        Err(format!("{} 不是可读的 SQLite 库", path.display()))
    }
}

/// A consistent copy of a database, safe while another process writes it.
fn snapshot(src: &Path, dst: &Path) -> Res<()> {
    if let Some(parent) = dst.parent() {
        mkdirs(parent)?;
    }
    let conn = open_source(src)?;
    let _ = conn.busy_timeout(Duration::from_secs(30));
    conn.execute("VACUUM INTO ?1", [dst.to_string_lossy()]).map_err(|e| format!("快照 {} 失败：{e}", src.display()))?;
    let _ = fs::set_permissions(dst, fs::Permissions::from_mode(0o600));
    Ok(())
}

/// Takes the data root's writer lock, waiting a little for a core that is shutting down.
fn lock_root(root: &Path) -> Res<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join(".lock"))
        .map_err(|e| format!("无法打开 {}：{e}", root.join(".lock").display()))?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        // SAFETY: flock on a descriptor owned by `file`; closing the file releases it.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        if Instant::now() >= deadline {
            return Err("Candid 的核心正在使用这个库。请先退出 Candid App，或在 App 的 设置 → 数据 里操作。".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Files under `dir` (or `dir` itself), skipping runtime files and symbolic links, as (source, relative path).
fn walk(dir: &Path, rel: &str, skip: &dyn Fn(&str) -> bool, out: &mut Vec<(PathBuf, String)>) {
    let Ok(meta) = fs::symlink_metadata(dir) else { return };
    if meta.is_file() {
        out.push((dir.to_path_buf(), rel.to_string()));
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if skip(&name) {
            continue;
        }
        walk(&e.path(), &format!("{rel}/{name}"), skip, out);
    }
}

fn defaults(args: &[&str]) -> bool {
    std::process::Command::new("/usr/bin/defaults")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn read_json(path: &Path) -> Res<Value> {
    let data = fs::read(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    serde_json::from_slice(&data).map_err(|e| format!("{} 不是有效的 JSON：{e}", path.display()))
}

fn created_by() -> String {
    format!("candid/{}", candid_service::VERSION)
}

// MARK: backup

pub fn backup(o: &Options, report: &mut Report) -> Res<Value> {
    let dest = o.target.clone().ok_or("用法：candid backup <目标目录> [--no-media]")?;
    let root = o.support.join("timeline");
    if !root.join("timeline.sqlite").is_file() || !root.join("store.json").is_file() {
        return Err(format!("没有找到时间线库：{}", root.display()));
    }
    let out = if !dest.exists() || is_empty_dir(&dest) { dest } else { dest.join(format!("candid-backup-{}", stamp())) };
    let partial = PathBuf::from(format!("{}.partial", out.display()));
    if out.exists() && !is_empty_dir(&out) || partial.exists() {
        return Err(format!("{} 已存在", if partial.exists() { &partial } else { &out }.display()));
    }
    mkdirs(&partial)?;
    let result = write_backup(o, &root, &partial, report);
    if result.is_err() {
        let _ = fs::remove_dir_all(&partial);
    }
    let mut summary = result?;
    if out.exists() {
        fs::remove_dir(&out).map_err(|e| format!("无法替换空目录 {}：{e}", out.display()))?;
    }
    fs::rename(&partial, &out).map_err(|e| format!("无法完成备份：{e}"))?;
    summary["path"] = json!(out.display().to_string());
    Ok(summary)
}

fn write_backup(o: &Options, root: &Path, partial: &Path, report: &mut Report) -> Res<Value> {
    let started = Instant::now();
    report.phase("snapshot", 1);
    let snap_rel = "timeline/timeline.sqlite";
    snapshot(&root.join("timeline.sqlite"), &partial.join(snap_rel))?;
    report.add(1);
    let snap = Connection::open_with_flags(partial.join(snap_rel), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法读取快照：{e}"))?;
    let schema: i64 = snap.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let events: i64 = snap.query_row("SELECT count(*) FROM events", [], |r| r.get(0)).unwrap_or(0);
    let identity = read_json(&root.join("store.json"))?;

    // What to copy, from the snapshot's point of view: originals deleted after it are simply not needed.
    let mut items: Vec<(PathBuf, String)> = vec![(root.join("store.json"), "timeline/store.json".into())];
    walk(&root.join("prompts"), "timeline/prompts", &skipped, &mut items);
    if !o.no_media {
        let mut seen = BTreeSet::new();
        for sql in ["SELECT rel_path FROM blobs", "SELECT DISTINCT blob_path FROM media_objects"] {
            let Ok(mut stmt) = snap.prepare(sql) else { continue };
            let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.flatten().collect();
            for rel in rows {
                if safe_rel(&rel) && seen.insert(rel.clone()) {
                    items.push((root.join(&rel), format!("timeline/{rel}")));
                }
            }
        }
    }
    drop(snap);
    for entry in APP_ENTRIES {
        walk(&o.support.join(entry), &format!("app/{entry}"), &skipped, &mut items);
    }

    let mut files = vec![];
    let mut missing = vec![];
    let mut locked_dbs: Vec<Value> = vec![];
    let total: u64 = items.iter().filter_map(|(p, _)| fs::metadata(p).ok()).map(|m| m.len()).sum();
    report.phase("copy", total);
    for (src, rel) in &items {
        if !src.is_file() {
            missing.push(rel.clone());
            continue;
        }
        let dst = partial.join(rel);
        let (bytes, sha) = if rel.ends_with(".sqlite") {
            // Secondary databases (the old media Hub) may be held under an exclusive lock by their own process
            // and are already mirrored into the timeline library: skip them with a reason instead of failing.
            if let Err(error) = snapshot(src, &dst) {
                let _ = fs::remove_file(&dst);
                locked_dbs.push(json!({"path": rel, "error": error}));
                continue;
            }
            report.add(fs::metadata(src).map(|m| m.len()).unwrap_or(0));
            (fs::metadata(&dst).map(|m| m.len()).unwrap_or(0), sha256_file(&dst)?)
        } else {
            copy_hashed(src, &dst, report)?
        };
        files.push(json!({"path": rel, "bytes": bytes, "sha256": sha}));
    }
    files.insert(0, json!({"path": snap_rel, "bytes": fs::metadata(partial.join(snap_rel)).map(|m| m.len()).unwrap_or(0), "sha256": sha256_file(&partial.join(snap_rel))?}));
    if o.real_support {
        let plist = partial.join(format!("defaults/{DEFAULTS_DOMAIN}.plist"));
        mkdirs(plist.parent().unwrap_or(partial))?;
        if defaults(&["export", DEFAULTS_DOMAIN, &plist.to_string_lossy()]) {
            let rel = format!("defaults/{DEFAULTS_DOMAIN}.plist");
            files.push(json!({"path": rel, "bytes": fs::metadata(&plist).map(|m| m.len()).unwrap_or(0), "sha256": sha256_file(&plist)?}));
        } else {
            let _ = fs::remove_dir(plist.parent().unwrap_or(partial));
        }
    }
    let bytes: u64 = files.iter().filter_map(|f| f["bytes"].as_u64()).sum();
    let manifest = json!({
        "format": FORMAT,
        "created_at": chrono::Local::now().to_rfc3339(),
        "candid_version": candid_service::VERSION,
        "schema_version": schema,
        "store_id": identity["store_id"],
        "media": !o.no_media,
        "events": events,
        "bytes": bytes,
        "files": files,
        "missing": missing,
        "skipped": locked_dbs,
    });
    fs::write(partial.join(MANIFEST), candid_store::json::pretty(&manifest)).map_err(|e| format!("无法写入清单：{e}"))?;
    // SAFETY: sync(2) has no preconditions; one flush for the whole tree instead of an fsync per file.
    unsafe { libc::sync() };
    Ok(json!({
        "files": manifest["files"].as_array().map(|a| a.len()).unwrap_or(0),
        "bytes": bytes,
        "events": events,
        "schema_version": schema,
        "missing": missing.len(),
        "skipped": locked_dbs,
        "seconds": started.elapsed().as_secs_f64(),
    }))
}

fn safe_rel(rel: &str) -> bool {
    !rel.is_empty() && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)))
}

// MARK: restore

pub fn restore(o: &Options, report: &mut Report) -> Res<Value> {
    let archive = o.target.clone().ok_or("用法：candid restore <备份目录> [--move-existing]")?;
    let manifest = read_json(&archive.join(MANIFEST)).map_err(|e| format!("这不是 Candid 备份：{e}"))?;
    if manifest["format"] != FORMAT {
        return Err(format!("不认识的备份格式：{}", manifest["format"]));
    }
    let schema = manifest["schema_version"].as_i64().ok_or("清单缺少 schema_version")?;
    if schema > SCHEMA_VERSION {
        return Err(format!("这份备份来自更新的 Candid（库版本 {schema}，当前支持 {SCHEMA_VERSION}），请先升级 Candid"));
    }
    let files: Vec<(String, u64, String)> = manifest["files"]
        .as_array()
        .ok_or("清单缺少 files")?
        .iter()
        .map(|f| (f["path"].as_str().unwrap_or("").to_string(), f["bytes"].as_u64().unwrap_or(0), f["sha256"].as_str().unwrap_or("").to_string()))
        .collect();
    for (rel, _, _) in &files {
        let ok = safe_rel(rel)
            && (rel.starts_with("timeline/") || rel.starts_with("defaults/") || app_entry(rel).is_some());
        if !ok {
            return Err(format!("清单里有不允许的路径：{rel}"));
        }
    }
    if !files.iter().any(|(r, _, _)| r == "timeline/timeline.sqlite") || !files.iter().any(|(r, _, _)| r == "timeline/store.json") {
        return Err("备份里没有时间线库".into());
    }

    // Everything is checked before anything on disk changes.
    report.phase("verify", files.iter().map(|f| f.1).sum());
    for (rel, bytes, sha) in &files {
        if &sha256_file(&archive.join(rel))? != sha {
            return Err(format!("备份已损坏：{rel} 的哈希不符"));
        }
        report.add(*bytes);
    }

    let support = &o.support;
    let timeline = support.join("timeline");
    let _lock = if timeline.join("store.json").exists() { Some(lock_root(&timeline)?) } else { None };

    let mut entries: BTreeSet<&str> = BTreeSet::new();
    entries.insert("timeline");
    for (rel, _, _) in &files {
        if let Some(e) = app_entry(rel) {
            entries.insert(e);
        }
    }
    let has_defaults = o.real_support && files.iter().any(|(r, _, _)| r.starts_with("defaults/"));
    let mut conflicts: Vec<&str> =
        entries.iter().copied().filter(|e| occupied(&support.join(e))).collect();
    let defaults_exist = has_defaults && defaults(&["read", DEFAULTS_DOMAIN]);
    if (!conflicts.is_empty() || defaults_exist) && !o.move_existing {
        let mut list: Vec<String> = conflicts.iter().map(|e| support.join(e).display().to_string()).collect();
        if defaults_exist {
            list.push(format!("UserDefaults（{DEFAULTS_DOMAIN}）"));
        }
        return Err(format!("目标处已有数据，没有恢复：\n  {}\n加 --move-existing 会先把它们移到 {}，不会覆盖。",
            list.join("\n  "), support.join("before-restore-<时间>").display()));
    }
    let aside = support.join(format!("before-restore-{}", stamp()));
    let mut moved = vec![];
    conflicts.sort();
    for e in &conflicts {
        moved.push(move_aside(&support.join(e), &aside.join(e))?);
    }
    if defaults_exist {
        mkdirs(&aside)?;
        let saved = aside.join(format!("{DEFAULTS_DOMAIN}.plist"));
        if !defaults(&["export", DEFAULTS_DOMAIN, &saved.to_string_lossy()]) {
            return Err("无法先保存现有的 UserDefaults，没有恢复".into());
        }
        moved.push(saved.display().to_string());
    }

    report.phase("restore", files.iter().map(|f| f.1).sum());
    for (rel, _, sha) in &files {
        let dst = if let Some(rest) = rel.strip_prefix("timeline/") {
            timeline.join(rest)
        } else if let Some(rest) = rel.strip_prefix("app/") {
            support.join(rest)
        } else {
            continue;
        };
        let (_, got) = copy_hashed(&archive.join(rel), &dst, report)
            .map_err(|e| format!("{e}\n恢复中断；原有数据在 {}", aside.display()))?;
        if &got != sha {
            return Err(format!("复制 {rel} 时内容变了，恢复中断；原有数据在 {}", aside.display()));
        }
    }
    if has_defaults {
        let plist = archive.join(format!("defaults/{DEFAULTS_DOMAIN}.plist"));
        if !defaults(&["import", DEFAULTS_DOMAIN, &plist.to_string_lossy()]) {
            return Err("UserDefaults 导入失败（其余数据已恢复）".into());
        }
    }
    drop(_lock);
    // SAFETY: as in backup.
    unsafe { libc::sync() };

    // Opening the store the normal way runs the schema upgrade chain and the startup recovery.
    report.phase("upgrade", 1);
    let mut store = TimelineStore::open(&timeline, &created_by(), false, Clock::default())
        .map_err(|e| format!("恢复的库打不开：{}", e.message()))?;
    let missing = store.recovery.missing_blob_files.len();
    store.close();
    drop(store);
    report.add(1);
    Ok(json!({
        "files": files.len(),
        "bytes": files.iter().map(|f| f.1).sum::<u64>(),
        "schema_from": schema,
        "schema_to": SCHEMA_VERSION,
        "moved_aside": moved,
        "missing_originals": missing,
        "media": manifest["media"],
    }))
}

/// The App entry a backup path belongs to (`app/capture/x` → `capture`).
fn app_entry(rel: &str) -> Option<&'static str> {
    let rest = rel.strip_prefix("app/")?;
    APP_ENTRIES.iter().copied().find(|e| rest == *e || rest.starts_with(&format!("{e}/")))
}

/// Something is there that a restore would otherwise write over: a file, or a non-empty directory.
fn occupied(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => fs::read_dir(path).map(|d| d.flatten().any(|e| e.file_name() != ".lock")).unwrap_or(true),
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Moves existing data out of the way. A data root that was moved elsewhere (a symbolic link) keeps its link; its
/// contents are set aside next to it on the same volume, and an empty directory takes their place.
fn move_aside(path: &Path, to: &Path) -> Res<String> {
    let is_link = fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if is_link {
        let real = fs::canonicalize(path).map_err(|e| format!("无法解析 {}：{e}", path.display()))?;
        let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let aside = real.with_file_name(format!("{name}.before-restore-{}", stamp()));
        fs::rename(&real, &aside).map_err(|e| format!("无法移开 {}：{e}", real.display()))?;
        mkdirs(&real)?;
        return Ok(aside.display().to_string());
    }
    if let Some(parent) = to.parent() {
        mkdirs(parent)?;
    }
    fs::rename(path, to).map_err(|e| format!("无法移开 {}：{e}", path.display()))?;
    Ok(to.display().to_string())
}

// MARK: move-root

pub fn move_root(o: &Options, report: &mut Report) -> Res<Value> {
    let new = o.target.clone().ok_or("用法：candid move-root <新位置> [--remove-old]")?;
    let link = o.support.join("timeline");
    let meta = fs::symlink_metadata(&link).map_err(|_| format!("没有找到时间线库：{}", link.display()))?;
    let is_link = meta.file_type().is_symlink();
    let source = if is_link { fs::canonicalize(&link).map_err(|e| format!("{} 指向的位置不可用：{e}", link.display()))? } else { link.clone() };
    if !source.join("timeline.sqlite").is_file() {
        return Err(format!("{} 里没有时间线库", source.display()));
    }
    let moving_back = new == link;
    if moving_back && !is_link {
        return Err("数据已经在这个位置".into());
    }
    if !moving_back {
        if new.exists() && !is_empty_dir(&new) {
            return Err(format!("{} 不是空目录", new.display()));
        }
        let parent = new.parent().ok_or("新位置无效")?;
        if !parent.is_dir() {
            return Err(format!("{} 不存在（外置盘是否已连接？）", parent.display()));
        }
        let real_new = fs::canonicalize(parent).map_err(|e| e.to_string())?.join(new.file_name().unwrap_or_default());
        if real_new == source || real_new.starts_with(&source) || source.starts_with(&real_new) || real_new.starts_with(&link) {
            return Err("新位置不能和现有数据重叠".into());
        }
    }
    let started = Instant::now();
    let lock = lock_root(&source)?;
    // With the core stopped, fold the WAL into the database so the copy is one self-contained file.
    if let Ok(c) = Connection::open_with_flags(source.join("timeline.sqlite"), OpenFlags::SQLITE_OPEN_READ_WRITE) {
        let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    let dest = if moving_back { link.with_file_name(format!("timeline.incoming-{}", stamp())) } else { new.clone() };
    let mut items = vec![];
    walk(&source, "", &|n: &str| n == ".lock" || n.ends_with("-shm") || n == ".DS_Store", &mut items);
    let total: u64 = items.iter().filter_map(|(p, _)| fs::metadata(p).ok()).map(|m| m.len()).sum();
    mkdirs(&dest)?;
    let copy = (|| -> Res<Vec<(PathBuf, String)>> {
        report.phase("copy", total);
        let mut copied = vec![];
        for (src, rel) in &items {
            let target = dest.join(rel.trim_start_matches('/'));
            let (_, sha) = copy_hashed(src, &target, report)?;
            copied.push((target, sha));
        }
        // SAFETY: as in backup; the verify pass below should read what reached the disk.
        unsafe { libc::sync() };
        report.phase("verify", total);
        for (target, sha) in &copied {
            if &sha256_file(target)? != sha {
                return Err(format!("{} 复制后校验不符", target.display()));
            }
            report.add(fs::metadata(target).map(|m| m.len()).unwrap_or(0));
        }
        Ok(copied)
    })();
    let copied = match copy {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_dir_all(&dest);
            return Err(format!("{e}\n没有移动，数据仍在 {}", source.display()));
        }
    };

    // Swap: the old directory is renamed (kept), the support directory's `timeline` becomes or follows the link.
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let old = source.with_file_name(format!("{name}.moved-{}", stamp()));
    fs::rename(&source, &old).map_err(|e| format!("无法改名 {}：{e}", source.display()))?;
    drop(lock);
    let swap = (|| -> std::io::Result<()> {
        if is_link {
            fs::remove_file(&link)?;
        }
        if moving_back {
            fs::rename(&dest, &link)
        } else {
            std::os::unix::fs::symlink(&new, &link)
        }
    })();
    if let Err(e) = swap {
        let _ = fs::rename(&old, &source);
        return Err(format!("无法切换到新位置：{e}；数据仍在 {}", source.display()));
    }
    let mut store = TimelineStore::open(&link, &created_by(), false, Clock::default())
        .map_err(|e| format!("新位置的库打不开：{}；原数据保留在 {}", e.message(), old.display()))?;
    store.close();
    drop(store);
    let removed = o.remove_old && fs::remove_dir_all(&old).is_ok();
    Ok(json!({
        "from": source.display().to_string(),
        "to": if moving_back { link.display().to_string() } else { new.display().to_string() },
        "link": if moving_back { Value::Null } else { json!(link.display().to_string()) },
        "old": if removed { Value::Null } else { json!(old.display().to_string()) },
        "files": copied.len(),
        "bytes": total,
        "seconds": started.elapsed().as_secs_f64(),
    }))
}

/// Entry point from `main`: runs one command and prints its outcome (a JSON line with `--json`).
pub fn run(command: &str, args: &[String]) -> i32 {
    let opts = match Options::parse(command, args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut report = Report::new(opts.json);
    let result = match command {
        "backup" => backup(&opts, &mut report),
        "restore" => restore(&opts, &mut report),
        _ => move_root(&opts, &mut report),
    };
    match result {
        Ok(mut summary) => {
            if opts.json {
                summary["ok"] = json!(true);
                println!("{summary}");
            } else {
                println!("{}", candid_store::json::pretty(&summary));
            }
            0
        }
        Err(e) => {
            if opts.json {
                println!("{}", json!({"ok": false, "error": e}));
            }
            let _ = writeln!(std::io::stderr(), "{e}");
            1
        }
    }
}
