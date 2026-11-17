//! Port of Swift `StoreRootTests`.
mod common;
use common::*;
use candid_store::fixtures::temp_root;
use candid_store::testing::RowExt;
use candid_store::*;
use std::sync::Arc;

#[test]
fn create_writes_identity_and_full_schema() {
    let fx = Fx::make("create");
    let identity: StoreIdentity = serde_json::from_slice(&std::fs::read(fx.root.join("store.json")).unwrap()).unwrap();
    assert_eq!(identity.layout, "timeline-root/1");
    assert_eq!(identity.schema_version, SCHEMA_VERSION);
    assert_eq!(identity.store_id.len(), 36);
    for dir in ["blobs/sha256", "staging", "exports", "logs"] {
        assert!(fx.root.join(dir).exists(), "{dir}");
    }
    let tables = fx.texts("SELECT name FROM sqlite_master WHERE type IN ('table') ORDER BY name");
    for t in [
        "sources", "source_cursors", "events", "event_kinds", "ingest_conflicts", "threads", "blobs", "event_blobs", "text_chunks", "event_text", "text_fts",
        "meta_fts", "episodes", "episode_evidence", "memories", "memory_sources", "corrections", "ai_providers", "ai_models", "ai_roles", "ai_calls", "clients",
        "access_log", "proposals", "jobs", "changes", "tombstones", "store_meta",
    ] {
        assert!(tables.contains(&t.to_string()), "missing table {t}");
    }
    let strict = fx.texts("SELECT name FROM pragma_table_list WHERE type='table' AND strict=0 AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%_fts%'");
    assert!(strict.is_empty(), "non-STRICT tables: {strict:?}");
    assert_eq!(fx.scalar("PRAGMA foreign_keys"), 1);
    assert_eq!(fx.texts("PRAGMA journal_mode"), vec!["wal"]);
    assert!(fx.store.integrity_check(false).unwrap().ok);
    // store.json is the Swift encoder's sorted-key form
    let raw = std::fs::read_to_string(fx.root.join("store.json")).unwrap();
    assert!(raw.starts_with("{\"created_at\":"), "{raw}");
}

#[test]
fn reopen_keeps_identity_and_data() {
    let root = temp_root("reopen");
    let mut first = open(&root);
    let id = first.identity.store_id.clone();
    let source = first.register_source("manual", "n", &SourcePolicy::default()).unwrap();
    first.ingest(&IngestBatch::events(&source, vec![note("a", Some(1))])).unwrap();
    first.close();
    drop(first);
    let second = open(&root);
    assert_eq!(second.identity.store_id, id);
    assert_eq!(second.table_counts().unwrap()["events"], 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn second_open_while_held_is_refused() {
    let fx = Fx::make("lock");
    assert_eq!(try_open(&fx.root).err(), Some(TimelineError::Locked));
}

#[test]
fn newer_schema_is_refused_for_writing() {
    let root = temp_root("newer");
    let store = open(&root);
    store.raw_db().execute("PRAGMA user_version=99", &[]).unwrap();
    drop(store);
    assert_eq!(try_open(&root).err(), Some(TimelineError::NewerSchema { found: 99, supported: SCHEMA_VERSION }));
    // store.json newer than the program as well
    let fresh = temp_root("newer-json");
    drop(open(&fresh));
    let url = fresh.join("store.json");
    let mut identity: StoreIdentity = serde_json::from_slice(&std::fs::read(&url).unwrap()).unwrap();
    identity.schema_version = 99;
    std::fs::write(&url, json::canonical_of(&identity)).unwrap();
    assert_eq!(try_open(&fresh).err(), Some(TimelineError::NewerSchema { found: 99, supported: SCHEMA_VERSION }));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&fresh);
}

#[test]
fn half_present_root_is_never_rebuilt() {
    let root = temp_root("half");
    drop(open(&root));
    std::fs::remove_file(root.join("timeline.sqlite")).unwrap();
    assert!(try_open(&root).is_err());
    assert!(!root.join("timeline.sqlite").exists());
    let absent = temp_root("absent");
    assert!(TimelineStore::open(&absent, "t", false, Clock::default()).is_err());
    assert!(!absent.exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn mismatched_versions_are_refused() {
    let root = temp_root("mismatch");
    let s = open(&root);
    s.raw_db().execute("PRAGMA user_version=0", &[]).unwrap();
    drop(s);
    assert!(try_open(&root).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn startup_recovery_clears_staging_and_orphan_blobs_but_keeps_referenced() {
    let root = temp_root("recover");
    let first = open(&root);
    let source = first.register_source("mac.desktop", "m", &SourcePolicy::new(true, true)).unwrap();
    let (path, sha) = write_staging(&first, b"kept", "jpg");
    first.ingest(&IngestBatch::events(&source, vec![shot("s", 1, &path, &sha)])).unwrap();
    let kept = first.raw_db().query("SELECT rel_path FROM blobs", &[], |r| r.text(0)).unwrap()[0].clone();
    drop(first);
    // simulate crash leftovers
    std::fs::write(root.join("staging/leftover.tmp"), b"half").unwrap();
    let orphan_dir = root.join("blobs/sha256/ab");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(orphan_dir.join(format!("ab{}.jpg", "0".repeat(62))), b"orphan").unwrap();
    let reopened = open(&root);
    assert_eq!(reopened.recovery.staging_files_removed, 1);
    assert_eq!(reopened.recovery.orphan_blob_files_removed, 1);
    assert!(root.join(&kept).exists());
    assert!(reopened.integrity_check(true).unwrap().ok);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn crash_between_file_move_and_commit_leaves_no_partial_state() {
    let root = temp_root("crash");
    let mut store = open(&root);
    let source = store.register_source("mac.desktop", "Synthetic Mac", &SourcePolicy::new(true, true)).unwrap();
    let (path, sha) = write_staging(&store, b"shot", "jpg");
    store.set_hooks(TestHooks {
        ingest: Some(Box::new(|point| if point == "beforeCommit" { Err(TimelineError::SimulatedCrash) } else { Ok(()) })),
    });
    let batch = |p: &str, h: &str| IngestBatch {
        on_missing_parent: None,
        merge_text: None, removed_source_keys: vec![], payload_revisions: vec![],
        source_id: source.clone(),
        cursor_after: Some(IngestCursor::new("s", "1")),
        threads: vec![],
        events: vec![shot("k", 5, p, h)],
    };
    assert_eq!(store.ingest(&batch(&path, &sha)).err(), Some(TimelineError::SimulatedCrash));
    // the process "died": file is on disk, transaction never committed
    let jpgs = |root: &std::path::Path| blob_files(root).iter().filter(|p| p.extension().is_some_and(|e| e == "jpg")).count();
    assert_eq!(jpgs(&root), 1);
    assert_eq!(store.table_counts().unwrap()["events"], 0);
    assert_eq!(store.cursor(&source, "s").unwrap(), None);
    store.close();
    drop(store);
    let reopened = open(&root);
    assert_eq!(reopened.recovery.orphan_blob_files_removed, 1);
    assert_eq!(jpgs(&root), 0);
    // retry from the original cursor succeeds
    let (path2, sha2) = write_staging(&reopened, b"shot", "jpg");
    assert_eq!(reopened.ingest(&batch(&path2, &sha2)).unwrap().inserted, 1);
    assert_eq!(reopened.cursor(&source, "s").unwrap().as_deref(), Some("1"));
    assert!(reopened.integrity_check(true).unwrap().ok);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn integrity_check_reports_tampered_blob_and_missing_file() {
    let fx = Fx::make("tamper");
    let (path, sha) = write_staging(&fx.store, b"original", "jpg");
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![shot("k", 1, &path, &sha)])).unwrap();
    let rel = fx.texts("SELECT rel_path FROM blobs")[0].clone();
    std::fs::write(fx.root.join(&rel), b"changed").unwrap();
    assert!(!fx.store.integrity_check(true).unwrap().ok);
    std::fs::remove_file(fx.root.join(&rel)).unwrap();
    assert!(fx.store.integrity_check(false).unwrap().issues.iter().any(|i| i.starts_with("blob 缺失")));
}

#[test]
fn clock_is_shareable_between_threads() {
    let clock = Arc::new(candid_store::fixtures::test_clock());
    let c2 = clock.clone();
    assert_eq!(std::thread::spawn(move || c2.now_ms()).join().unwrap(), clock.now_ms());
}
