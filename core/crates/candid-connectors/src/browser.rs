//! Browser history connector (agent-platform-v1 §2.0, item 2). Built per the 2026-11-12 decision with import OFF:
//! describe / authorize / revoke / dry-run work, while sync never writes to the timeline — `import_enabled` cannot be
//! switched on in this version. The history database is copied (with its WAL) into a private temporary directory and
//! read there; the browser's own files are never opened for writing.
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const BROWSERS: &[&str] = &["safari", "chrome"];
/// Safari stores CFAbsoluteTime (seconds since 2001-01-01), Chrome WebKit time (µs since 1601-01-01).
const CF_EPOCH_MS: i64 = 978_307_200_000;
const WEBKIT_EPOCH_MS: i64 = 11_644_473_600_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    pub version: i64,
    pub browser: String,
    /// Chrome profile directory name (default `Default`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// History database override (tests use synthetic databases).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Always false in this version: the connector is built but importing is not enabled (user decision 2026-11-12).
    pub import_enabled: bool,
    pub backfill_days: Option<i64>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig { version: 1, browser: String::new(), profile: None, path: None, import_enabled: false, backfill_days: Some(90) }
    }
}

impl BrowserConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !BROWSERS.contains(&self.browser.as_str()) {
            return Err("unsupported_browser".into());
        }
        if self.import_enabled {
            return Err("import_not_enabled".into());
        }
        if self.backfill_days.is_some_and(|d| d < 1) {
            return Err("invalid_limits".into());
        }
        if self.profile.as_ref().is_some_and(|p| p.contains('/') || p.contains("..") || p.is_empty()) {
            return Err("invalid_profile".into());
        }
        Ok(())
    }

    pub fn history_path(&self) -> PathBuf {
        if let Some(p) = &self.path {
            return PathBuf::from(p);
        }
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        match self.browser.as_str() {
            "safari" => home.join("Library/Safari/History.db"),
            _ => home.join("Library/Application Support/Google/Chrome").join(self.profile.as_deref().unwrap_or("Default")).join("History"),
        }
    }
}

/// One visit as the timeline would see it (url without credentials, query or fragment).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Visit {
    pub url: String,
    pub title: Option<String>,
    pub visit_ms: i64,
    pub visit_count: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DryRun {
    pub format: String,
    pub visits: usize,
    pub distinct_urls: usize,
    pub domains: usize,
    pub first_ms: Option<i64>,
    pub last_ms: Option<i64>,
    pub skipped_urls: usize,
    /// Always false: nothing was or would be written in this version.
    pub would_import: bool,
}

/// `scheme://host[:port]/path` only; userinfo, query and fragment removed. Non-web schemes are skipped.
pub fn clean_url(raw: &str) -> Option<String> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = scheme.to_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let rest = &rest[..end];
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority).to_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}{path}"))
}

fn domain(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    host.split(':').next().unwrap_or(host)
}

struct TempCopy(PathBuf);
impl Drop for TempCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn io_code(e: &std::io::Error) -> String {
    match e.kind() {
        ErrorKind::PermissionDenied => "full_disk_access_required".into(),
        ErrorKind::NotFound => "history_not_found".into(),
        _ => "history_unreadable".into(),
    }
}

/// Copies the database and its `-wal` into a private directory (0700); the copy is deleted on drop.
fn copy_history(src: &Path, scratch: &Path) -> Result<(TempCopy, PathBuf), String> {
    fs::metadata(src).map_err(|e| io_code(&e))?;
    let dir = scratch.join(format!("browser-{}", candid_store::uuid_v4()));
    fs::create_dir_all(&dir).map_err(|_| "history_unreadable")?;
    let guard = TempCopy(dir.clone());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|_| "history_unreadable")?;
    let dst = dir.join("history.sqlite");
    fs::copy(src, &dst).map_err(|e| io_code(&e))?;
    let wal = PathBuf::from(format!("{}-wal", src.display()));
    if wal.exists() {
        fs::copy(&wal, dir.join("history.sqlite-wal")).map_err(|e| io_code(&e))?;
    }
    Ok((guard, dst))
}

fn columns(db: &Connection, table: &str) -> BTreeSet<String> {
    db.prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1)).map(|rows| rows.filter_map(|r| r.ok()).collect()))
        .unwrap_or_default()
}

fn require(db: &Connection, table: &str, cols: &[&str]) -> Result<(), String> {
    let have = columns(db, table);
    if cols.iter().all(|c| have.contains(*c)) {
        Ok(())
    } else {
        Err("unknown_history_schema".into())
    }
}

/// Reads visits at or after `since_ms` from a private copy. Unknown layouts are refused, never guessed.
pub fn read_visits(config: &BrowserConfig, scratch: &Path, since_ms: Option<i64>) -> Result<(String, Vec<Visit>), String> {
    config.validate()?;
    let (_guard, copy) = copy_history(&config.history_path(), scratch)?;
    // The copy is ours: opened read-write only so SQLite can fold the copied WAL; the original is untouched.
    let db = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|_| "history_unreadable")?;
    let (format, sql, since) = match config.browser.as_str() {
        "safari" => {
            require(&db, "history_items", &["id", "url", "visit_count"])?;
            require(&db, "history_visits", &["id", "history_item", "visit_time", "title"])?;
            (
                "safari-history/1",
                "SELECT i.url, v.title, v.visit_time, i.visit_count FROM history_visits v JOIN history_items i ON i.id=v.history_item WHERE v.visit_time >= ?1 ORDER BY v.visit_time, v.id",
                rusqlite::types::Value::Real(since_ms.map(|s| (s - CF_EPOCH_MS) as f64 / 1000.0).unwrap_or(f64::MIN)),
            )
        }
        _ => {
            require(&db, "urls", &["id", "url", "title", "visit_count"])?;
            require(&db, "visits", &["id", "url", "visit_time"])?;
            (
                "chrome-history/1",
                "SELECT u.url, u.title, v.visit_time, u.visit_count FROM visits v JOIN urls u ON u.id=v.url WHERE v.visit_time >= ?1 ORDER BY v.visit_time, v.id",
                rusqlite::types::Value::Integer(since_ms.map(|s| (s + WEBKIT_EPOCH_MS) * 1000).unwrap_or(i64::MIN)),
            )
        }
    };
    let safari = config.browser == "safari";
    let mut stmt = db.prepare(sql).map_err(|_| "unknown_history_schema")?;
    let rows = stmt
        .query_map([since], |r| {
            let t: f64 = match r.get_ref(2)? {
                rusqlite::types::ValueRef::Integer(i) => i as f64,
                rusqlite::types::ValueRef::Real(f) => f,
                _ => f64::NAN,
            };
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, t, r.get::<_, Option<i64>>(3)?))
        })
        .map_err(|_| "history_unreadable")?;
    let mut out = Vec::new();
    for row in rows {
        let (url, title, t, count) = row.map_err(|_| "history_unreadable")?;
        if !t.is_finite() {
            continue;
        }
        let visit_ms = if safari { CF_EPOCH_MS + (t * 1000.0) as i64 } else { (t as i64) / 1000 - WEBKIT_EPOCH_MS };
        out.push(Visit { url: url.unwrap_or_default(), title: title.filter(|t| !t.is_empty()), visit_ms, visit_count: count.unwrap_or(0) });
    }
    Ok((format.into(), out))
}

/// Counts only (no URL, title or domain names leave this function).
pub fn dry_run(config: &BrowserConfig, scratch: &Path, now_ms: i64) -> Result<DryRun, String> {
    let since = config.backfill_days.map(|d| now_ms - d * 86_400_000);
    let (format, visits) = read_visits(config, scratch, since)?;
    let mut urls = BTreeSet::new();
    let mut domains = BTreeSet::new();
    let mut r = DryRun { format, ..Default::default() };
    for v in &visits {
        match clean_url(&v.url) {
            Some(u) => {
                domains.insert(domain(&u).to_string());
                urls.insert(u);
                r.visits += 1;
                r.first_ms = Some(r.first_ms.map_or(v.visit_ms, |f: i64| f.min(v.visit_ms)));
                r.last_ms = Some(r.last_ms.map_or(v.visit_ms, |l: i64| l.max(v.visit_ms)));
            }
            None => r.skipped_urls += 1,
        }
    }
    r.distinct_urls = urls.len();
    r.domains = domains.len();
    Ok(r)
}

/// The `browser.visit` events this connector would produce once importing is enabled. Used by tests and the dry
/// run's mapping check; nothing calls ingest with them in this version.
pub fn map_visits(browser: &str, visits: &[Visit]) -> Vec<candid_store::IngestEvent> {
    visits
        .iter()
        .filter_map(|v| {
            let url = clean_url(&v.url)?;
            let mut e = candid_store::IngestEvent::new(&format!("visit:{}:{}", v.visit_ms, candid_store::sha256_hex(&url)), "browser.visit", Some(v.visit_ms));
            e.time_basis = "source".into();
            e.url = Some(url);
            e.title = v.title.clone();
            e.payload = serde_json::json!({"browser": browser, "visit_count": v.visit_count});
            Some(e)
        })
        .collect()
}
