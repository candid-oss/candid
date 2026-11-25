//! `candid backup` → `candid restore` round trips and `candid move-root`, on a synthetic library with media in a
//! temporary support directory (never the real one: every call passes `--support`).
use candid_store::fixtures;
use candid_store::testing::Db;
use candid_store::{json, SourcePolicy, StoreIdentity, TimelineStore};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn dir(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("candid-migrate-{label}-{}", candid_store::uuid_v4()));
    fs::create_dir_all(&d).unwrap();
    d
}

fn candid(args: &[&str], support: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_candid")).args(args).arg("--support").arg(support).output().unwrap()
}

fn ok(out: &Output) -> Value {
    assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let last = String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or("").to_string();
    serde_json::from_str(&last).unwrap()
}

/// A support directory with the world fixture (events and staged blobs), a media original, a prompt override and
/// App files, including ones a backup must leave behind.
fn library() -> PathBuf {
    let support = dir("src");
    let root = support.join("timeline");
    let mut store = TimelineStore::open(&root, "test", true, fixtures::test_clock()).unwrap();
    fixtures::load_world(&store, &fixtures::fixture_path("world.json")).unwrap();
    store.close();
    drop(store);
    fs::create_dir_all(root.join("prompts")).unwrap();
    fs::write(root.join("prompts/item-extract.md"), "SYNTHETIC prompt").unwrap();
    let app = [
        ("capture/preferences.json", "{\"a\":1}"),
        ("conversations/one.json", "{\"m\":[]}"),
        ("ai-providers/config.json", "{\"roles\":[]}"),
        ("MediaWorkspace/Recordings/r1/session.json", "{}"),
        ("MediaWorkspace/connection.json", "{\"token\":\"secret\"}"),
        ("MediaWorkspace/hub.pid", "1"),
        ("pi/sessions/s.jsonl", "{}\n"),
        ("pi/agent/models-store.json", "{}"),
        ("pi/agent/auth.json", "{\"token\":\"secret\"}"),
        ("run/cli.token", "secret"),
        ("components/pi/index.ts", "x"),
    ];
    for (rel, body) in app {
        fs::create_dir_all(support.join(rel).parent().unwrap()).unwrap();
        fs::write(support.join(rel), body).unwrap();
    }
    fs::create_dir_all(support.join("MediaWorkspace/Hub")).unwrap();
    let hub = Db::open(&support.join("MediaWorkspace/Hub/memory.sqlite"), false, true).unwrap();
    hub.script("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES(42);").unwrap();
    drop(hub);
    support
}

/// Every regular file under `root` (except the lock and SQLite side files) with its SHA-256.
fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                stack.push(p);
            } else if !(name == ".lock" || name.ends_with("-wal") || name.ends_with("-shm")) {
                out.insert(p.strip_prefix(root).unwrap().display().to_string(), candid_store::sha256_hex_file(&p).unwrap());
            }
        }
    }
    out
}

fn counts(root: &Path) -> (i64, i64) {
    let db = Db::open(&root.join("timeline.sqlite"), true, false).unwrap();
    (db.scalar("SELECT count(*) FROM events", &[]).unwrap(), db.scalar("SELECT count(*) FROM blobs", &[]).unwrap())
}

#[test]
fn backup_then_restore_round_trips_the_library_and_leaves_secrets_behind() {
    let src = library();
    let (events, blobs) = counts(&src.join("timeline"));
    assert!(events > 0 && blobs > 0, "the fixture should have events and blobs");
    let dest = dir("backup");
    let summary = ok(&candid(&["backup", dest.to_str().unwrap(), "--json"], &src));
    // An empty destination is used as is.
    assert_eq!(summary["path"], dest.display().to_string());
    let manifest: Value = serde_json::from_slice(&fs::read(dest.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["format"], "candid-backup/1");
    assert_eq!(manifest["schema_version"], candid_store::SCHEMA_VERSION);
    assert_eq!(manifest["missing"].as_array().unwrap().len(), 0);
    for leaked in ["app/pi/agent/auth.json", "app/MediaWorkspace/connection.json", "app/MediaWorkspace/hub.pid", "app/run/cli.token"] {
        assert!(!dest.join(leaked).exists(), "{leaked} must not be backed up");
    }
    assert!(!dest.join("app/components").exists());

    let target = dir("dst");
    let restored = ok(&candid(&["restore", dest.to_str().unwrap(), "--json"], &target));
    assert_eq!(restored["schema_to"], candid_store::SCHEMA_VERSION);
    assert_eq!(restored["missing_originals"], 0);
    assert_eq!(counts(&target.join("timeline")), (events, blobs));
    // Originals, prompts and App files come back byte for byte.
    let (a, b) = (tree(&src.join("timeline")), tree(&target.join("timeline")));
    for (rel, sha) in &a {
        if rel.starts_with("blobs/") || rel.starts_with("prompts/") || rel.starts_with("media/") || rel == "store.json" {
            assert_eq!(b.get(rel), Some(sha), "{rel}");
        }
    }
    for rel in ["capture/preferences.json", "conversations/one.json", "ai-providers/config.json", "MediaWorkspace/Recordings/r1/session.json", "pi/sessions/s.jsonl", "pi/agent/models-store.json"] {
        assert_eq!(fs::read(src.join(rel)).unwrap(), fs::read(target.join(rel)).unwrap(), "{rel}");
    }
    assert!(!target.join("pi/agent/auth.json").exists() && !target.join("run").exists());
    let hub = Db::open(&target.join("MediaWorkspace/Hub/memory.sqlite"), true, false).unwrap();
    assert_eq!(hub.scalar("SELECT x FROM t", &[]).unwrap(), 42);

    // Restoring over existing data is refused and changes nothing; --move-existing sets it aside first.
    let before = tree(&target);
    let refused = candid(&["restore", dest.to_str().unwrap()], &target);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--move-existing"));
    assert_eq!(tree(&target), before);
    let again = ok(&candid(&["restore", dest.to_str().unwrap(), "--move-existing", "--json"], &target));
    let moved = again["moved_aside"].as_array().unwrap();
    assert!(moved.iter().any(|m| m.as_str().unwrap().ends_with("timeline")));
    assert_eq!(counts(&target.join("timeline")), (events, blobs));
    let aside = PathBuf::from(moved[0].as_str().unwrap()).parent().unwrap().to_path_buf();
    assert_eq!(counts(&aside.join("timeline")), (events, blobs));

    // A non-empty destination gets a dated folder inside it; --no-media carries the database only.
    let lean = ok(&candid(&["backup", dest.to_str().unwrap(), "--no-media", "--json"], &src));
    let lean_path = PathBuf::from(lean["path"].as_str().unwrap());
    assert!(lean_path.starts_with(&dest) && lean_path != dest);
    assert!(!lean_path.join("timeline/blobs").exists());
    let lean_target = dir("lean");
    let r = ok(&candid(&["restore", lean_path.to_str().unwrap(), "--json"], &lean_target));
    assert_eq!(r["missing_originals"].as_u64().unwrap() as i64, blobs);
    assert_eq!(counts(&lean_target.join("timeline")).0, events);
    for d in [src, dest, target, lean_target] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn restore_checks_everything_before_touching_the_disk() {
    let src = library();
    let dest = dir("backup");
    ok(&candid(&["backup", dest.to_str().unwrap(), "--json"], &src));

    // A tampered original: nothing is written.
    let manifest: Value = serde_json::from_slice(&fs::read(dest.join("manifest.json")).unwrap()).unwrap();
    let blob = manifest["files"].as_array().unwrap().iter().find(|f| f["path"].as_str().unwrap().contains("/blobs/")).unwrap();
    let blob_path = dest.join(blob["path"].as_str().unwrap());
    let original = fs::read(&blob_path).unwrap();
    fs::write(&blob_path, b"tampered").unwrap();
    let target = dir("dst");
    let out = candid(&["restore", dest.to_str().unwrap()], &target);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("哈希不符"));
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    fs::write(&blob_path, original).unwrap();

    // A backup from a newer schema is refused.
    let mut newer = manifest.clone();
    newer["schema_version"] = Value::from(candid_store::SCHEMA_VERSION + 1);
    fs::write(dest.join("manifest.json"), serde_json::to_vec(&newer).unwrap()).unwrap();
    let out = candid(&["restore", dest.to_str().unwrap()], &target);
    assert!(String::from_utf8_lossy(&out.stderr).contains("更新的 Candid"));
    fs::write(dest.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();

    // Paths outside the backup layout are refused.
    let mut escape = manifest.clone();
    escape["files"].as_array_mut().unwrap().push(serde_json::json!({"path": "app/../../evil", "bytes": 0, "sha256": ""}));
    fs::write(dest.join("manifest.json"), serde_json::to_vec(&escape).unwrap()).unwrap();
    let out = candid(&["restore", dest.to_str().unwrap()], &target);
    assert!(String::from_utf8_lossy(&out.stderr).contains("不允许的路径"));
    assert!(fs::read_dir(&target).unwrap().next().is_none());

    // A running core holds the root: restore with --move-existing still refuses to take it.
    fs::write(dest.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let mut held = TimelineStore::open(src.join("timeline"), "test", false, fixtures::test_clock()).unwrap();
    let out = candid(&["restore", dest.to_str().unwrap(), "--move-existing"], &src);
    assert!(String::from_utf8_lossy(&out.stderr).contains("正在使用"), "{}", String::from_utf8_lossy(&out.stderr));
    // ...while a backup works alongside it.
    let live = dir("live");
    ok(&candid(&["backup", live.to_str().unwrap(), "--json"], &src));
    held.close();
    for d in [src, dest, target, live] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn an_older_backup_is_upgraded_on_restore() {
    let support = dir("v4");
    let root = support.join("timeline");
    fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join("timeline.sqlite"), false, true).unwrap();
    db.script(include_str!("../../../fixtures/schema-v4.sql")).unwrap();
    db.script("PRAGMA foreign_keys=ON; INSERT INTO event_kinds VALUES('note','evidence',0,1,'Synthetic note'); INSERT INTO store_meta VALUES('ingest_seq_next','2'); INSERT INTO store_meta VALUES('schema.upgrade','3:4'); PRAGMA user_version=4;").unwrap();
    let policy = json::canonical_of(&SourcePolicy::default());
    db.conn.execute("INSERT INTO sources VALUES('s','manual','SYNTHETIC',NULL,NULL,NULL,'{}',?,'active',NULL,NULL,1,1)", [policy]).unwrap();
    db.script("INSERT INTO events(event_id,source_id,source_key,kind,start_ms,end_ms,time_basis,payload_version,payload_json,ingest_seq,ingested_ms) VALUES('e','s','original','note',1000,2000,'source',1,'{}',1,1);
      INSERT INTO meta_fts(rowid,title,app_name,url) VALUES(1,NULL,NULL,NULL);").unwrap();
    drop(db);
    let identity = StoreIdentity {
        store_id: "synthetic-v4-store".into(),
        schema_version: 4,
        created_at: "2026-11-13T00:00:00Z".into(),
        created_by: "synthetic-v4".into(),
        layout: "timeline-root/1".into(),
    };
    fs::write(root.join("store.json"), json::canonical_of(&identity)).unwrap();
    let dest = dir("backup");
    ok(&candid(&["backup", dest.to_str().unwrap(), "--json"], &support));
    let target = dir("dst");
    let r = ok(&candid(&["restore", dest.to_str().unwrap(), "--json"], &target));
    assert_eq!(r["schema_from"], 4);
    let db = Db::open(&target.join("timeline/timeline.sqlite"), true, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), candid_store::SCHEMA_VERSION);
    assert_eq!(db.scalar("SELECT count(*) FROM events", &[]).unwrap(), 1);
    for d in [support, dest, target] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn move_root_relocates_the_library_behind_a_link_and_back() {
    let support = library();
    let root = support.join("timeline");
    let before = tree(&root);
    let (events, blobs) = counts(&root);
    let external = dir("external");
    let new = external.join("Candid timeline");
    let r = ok(&candid(&["move-root", new.to_str().unwrap(), "--json"], &support));
    assert!(fs::symlink_metadata(&root).unwrap().file_type().is_symlink());
    assert_eq!(fs::canonicalize(&root).unwrap(), fs::canonicalize(&new).unwrap());
    let old = PathBuf::from(r["old"].as_str().unwrap());
    assert!(old.join("timeline.sqlite").exists(), "the old copy is kept without --remove-old");
    let blob_files = |t: &BTreeMap<String, String>| t.iter().filter(|(k, _)| k.starts_with("blobs/")).map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>();
    assert_eq!(blob_files(&tree(&new)), blob_files(&before));
    assert_eq!(counts(&root), (events, blobs));
    // The core opens it through the link.
    let mut store = TimelineStore::open(&root, "test", false, fixtures::test_clock()).unwrap();
    // ...and while it runs, moving is refused.
    let busy = candid(&["move-root", external.join("other").to_str().unwrap()], &support);
    assert!(String::from_utf8_lossy(&busy.stderr).contains("正在使用"));
    store.close();
    drop(store);
    // A non-empty destination is refused.
    fs::create_dir_all(external.join("full")).unwrap();
    fs::write(external.join("full/x"), "x").unwrap();
    assert_eq!(candid(&["move-root", external.join("full").to_str().unwrap()], &support).status.code(), Some(1));

    // Back to the default place, removing the external copy.
    let back = ok(&candid(&["move-root", root.to_str().unwrap(), "--remove-old", "--json"], &support));
    assert!(back["old"].is_null());
    assert!(fs::symlink_metadata(&root).unwrap().is_dir());
    assert!(!new.exists());
    assert_eq!(counts(&root), (events, blobs));
    assert_eq!(blob_files(&tree(&root)), blob_files(&before));
    for d in [support, external] {
        let _ = fs::remove_dir_all(d);
    }
}
