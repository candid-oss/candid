//! Browser history connector with synthetic Safari and Chrome databases. Import stays off: nothing reaches the
//! timeline even when the source is authorised and synced.
mod common;
use common::*;
use candid_connectors::browser::{clean_url, map_visits, read_visits, BrowserConfig};
use candid_connectors::sync_all;
use rusqlite::Connection;
use serde_json::json;
use std::path::{Path, PathBuf};

const CF: f64 = 978_307_200.0;

fn safari(dir: &Path) -> PathBuf {
    let p = dir.join("History.db");
    let db = Connection::open(&p).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE history_items(id INTEGER PRIMARY KEY, url TEXT NOT NULL UNIQUE, domain_expansion TEXT, visit_count INTEGER NOT NULL);
         CREATE TABLE history_visits(id INTEGER PRIMARY KEY, history_item INTEGER NOT NULL, visit_time REAL NOT NULL, title TEXT, load_successful BOOLEAN DEFAULT 1);",
    )
    .unwrap();
    let t = |ms: i64| ms as f64 / 1000.0 - CF;
    db.execute("INSERT INTO history_items VALUES(1,'https://user:pw@docs.example.test/a?token=secret#frag',NULL,3)", []).unwrap();
    db.execute("INSERT INTO history_items VALUES(2,'https://news.example.test/',NULL,1)", []).unwrap();
    db.execute("INSERT INTO history_items VALUES(3,'file:///Users/someone/private.txt',NULL,1)", []).unwrap();
    for (id, item, ms, title) in [(1, 1, NOW - 3_600_000, "合成文档"), (2, 1, NOW - 7_200_000, "合成文档"), (3, 2, NOW - 60_000, "News"), (4, 3, NOW - 1000, "local"), (5, 2, NOW - 200 * 86_400_000, "old")] {
        db.execute("INSERT INTO history_visits(id,history_item,visit_time,title) VALUES(?1,?2,?3,?4)", rusqlite::params![id, item, t(ms), title]).unwrap();
    }
    // Leave data in the WAL: the reader must copy and fold it, not miss it.
    std::mem::forget(db);
    p
}

fn chrome(dir: &Path) -> PathBuf {
    let p = dir.join("History");
    let db = Connection::open(&p).unwrap();
    db.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR, visit_count INTEGER DEFAULT 0, typed_count INTEGER DEFAULT 0, last_visit_time INTEGER, hidden INTEGER DEFAULT 0);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER NOT NULL, visit_time INTEGER NOT NULL, from_visit INTEGER, transition INTEGER DEFAULT 0);",
    )
    .unwrap();
    let t = |ms: i64| (ms + 11_644_473_600_000) * 1000;
    db.execute("INSERT INTO urls VALUES(1,'http://Example.TEST:8080/path?q=1','合成页面',2,0,0,0)", []).unwrap();
    db.execute("INSERT INTO urls VALUES(2,'chrome://settings','设置',1,0,0,0)", []).unwrap();
    for (id, url, ms) in [(1, 1, NOW - 10_000), (2, 1, NOW - 20_000), (3, 2, NOW - 30_000)] {
        db.execute("INSERT INTO visits(id,url,visit_time) VALUES(?1,?2,?3)", rusqlite::params![id, url, t(ms)]).unwrap();
    }
    p
}

#[test]
fn urls_lose_credentials_query_and_fragment() {
    assert_eq!(clean_url("https://user:pw@Docs.Example.TEST/a?token=1#x").as_deref(), Some("https://docs.example.test/a"));
    assert_eq!(clean_url("http://example.test").as_deref(), Some("http://example.test/"));
    assert_eq!(clean_url("file:///etc/passwd"), None);
    assert_eq!(clean_url("chrome://settings"), None);
}

#[test]
fn safari_and_chrome_read_from_a_private_copy_with_times_and_counts() {
    let dir = temp("browser-read");
    let scratch = dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let s = safari(&dir);
    let before = std::fs::read(&s).unwrap();
    let cfg = BrowserConfig { browser: "safari".into(), path: Some(s.to_string_lossy().into()), ..Default::default() };
    let (format, visits) = read_visits(&cfg, &scratch, Some(NOW - 90 * 86_400_000)).unwrap();
    assert_eq!(format, "safari-history/1");
    assert_eq!(visits.len(), 4);
    assert_eq!(visits[0].visit_ms, NOW - 7_200_000);
    assert_eq!(visits[0].visit_count, 3);
    assert_eq!(std::fs::read(&s).unwrap(), before, "original untouched");
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "temporary copy removed");
    let events = map_visits("safari", &visits);
    assert_eq!(events.len(), 3, "file:// skipped");
    assert_eq!(events[0].url.as_deref(), Some("https://docs.example.test/a"));
    assert_eq!((events[0].kind.as_str(), events[0].payload["visit_count"].as_i64()), ("browser.visit", Some(3)));
    let c = chrome(&dir);
    let cfg = BrowserConfig { browser: "chrome".into(), path: Some(c.to_string_lossy().into()), ..Default::default() };
    let (format, visits) = read_visits(&cfg, &scratch, None).unwrap();
    assert_eq!((format.as_str(), visits.len()), ("chrome-history/1", 3));
    assert_eq!(visits.iter().map(|v| v.visit_ms).max(), Some(NOW - 10_000));
    assert_eq!(map_visits("chrome", &visits)[0].url.as_deref(), Some("http://example.test:8080/path"));
}

#[test]
fn authorize_dry_run_and_sync_never_import() {
    let dir = temp("browser-flow");
    let st = store(&dir);
    let s = safari(&dir);
    let config = json!({"version": 1, "browser": "safari", "path": s.to_string_lossy()});
    // Enabling import is refused in this version.
    let mut on = config.clone();
    on["import_enabled"] = json!(true);
    assert_eq!(cmd_err(&st, &dir, json!({"action": "authorize", "kind": "browser.history", "config": on})), "import_not_enabled");
    let dry = cmd(&st, &dir, json!({"action": "dry_run", "kind": "browser.history", "config": config}));
    assert_eq!(dry["result"], json!({"format": "safari-history/1", "visits": 3, "distinct_urls": 2, "domains": 2, "first_ms": NOW - 7_200_000, "last_ms": NOW - 60_000, "skipped_urls": 1, "would_import": false}));
    let r = cmd(&st, &dir, json!({"action": "authorize", "kind": "browser.history", "config": config}));
    assert_eq!(r["probe"]["ok"], true);
    let sid = r["source_id"].as_str().unwrap().to_string();
    let out = sync_all(&st, None, &dir).sources.remove(0);
    assert_eq!((out.state.as_str(), out.code.as_deref(), out.inserted), ("skipped", Some("import_disabled"), 0));
    assert_eq!(count(&st, "SELECT COUNT(*) FROM events"), 0);
    assert_eq!(count(&st, "SELECT COUNT(*) FROM source_cursors"), 0);
    let mut upd = config.clone();
    upd["import_enabled"] = json!(true);
    assert_eq!(cmd_err(&st, &dir, json!({"action": "update", "source_id": sid, "config": upd})), "import_not_enabled");
    let dry = cmd(&st, &dir, json!({"action": "dry_run", "source_id": sid}));
    assert_eq!(dry["result"]["visits"], 3);
    cmd(&st, &dir, json!({"action": "revoke", "source_id": sid}));
    assert_eq!(cmd_err(&st, &dir, json!({"action": "dry_run", "source_id": sid})), "source_revoked");
    assert_eq!(sync_all(&st, None, &dir).sources[0].state, "skipped");
}

#[test]
fn unknown_history_schema_and_missing_permission_are_refused() {
    let dir = temp("browser-refuse");
    let p = dir.join("History.db");
    Connection::open(&p).unwrap().execute_batch("CREATE TABLE history_items(id INTEGER PRIMARY KEY, address TEXT);").unwrap();
    let cfg = BrowserConfig { browser: "safari".into(), path: Some(p.to_string_lossy().into()), ..Default::default() };
    assert_eq!(read_visits(&cfg, &dir, None).unwrap_err(), "unknown_history_schema");
    let locked = dir.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    let file = locked.join("History.db");
    let good = dir.join("good");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::copy(safari(&good), &file).unwrap();
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let cfg = BrowserConfig { browser: "safari".into(), path: Some(file.to_string_lossy().into()), ..Default::default() };
    let code = read_visits(&cfg, &dir, None).unwrap_err();
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(code, "full_disk_access_required");
    assert!(BrowserConfig { browser: "firefox".into(), ..Default::default() }.validate().is_err());
}
