//! Port of Swift `MigrationTests` and `LiveSyncTests`. The ledger and Hub fixtures are written by
//! `candid_store::fixtures` in the same shapes the Swift production writers produce.
mod common;
use common::*;
use candid_store::fixtures::*;
use candid_store::testing::{Db, RowExt};
use candid_store::*;
use std::collections::BTreeSet;
use std::path::Path;

fn file_hash(p: &Path) -> String {
    sha256_hex_file(p).unwrap_or_else(|_| "missing".into())
}

struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key_of(fx: &Fx, event_id: &str) -> Option<String> {
    fx.store
        .raw_db()
        .first("SELECT source_key FROM events WHERE event_id=?", &[testing::rusqlite::types::Value::Text(event_id.into())], |r| r.text(0))
        .unwrap()
}

#[test]
fn desktop_ledger_migrates_strictly_read_only_idempotently_and_verifiably() {
    let ledger = make_synthetic_ledger().unwrap();
    let _c = Cleanup(ledger.root.clone());
    let fx = Fx::make("mig-ledger");
    let db_before = file_hash(&ledger.root.join("ledger.sqlite"));
    let image_hash = sha256_hex(&ledger.image);
    let original_blob = ledger.root.join(format!("blobs/{}/{image_hash}.jpg", &image_hash[..2]));
    let blob_before = file_hash(&original_blob);

    let report = fx.store.migrate_desktop_ledger(&ledger.root, None, 3, None).unwrap();
    assert!(report.completed && report.failures.is_empty(), "{report:?}");
    assert_eq!(report.inserted, ledger.records.len() + 1); // + one OCR perception event
    let verify = fx.store.verify_desktop_ledger_migration(&ledger.root, None, true).unwrap();
    assert!(verify.ok(), "{verify:?}");
    assert_eq!(verify.blobs_checked, 1);
    assert_eq!(verify.counts_by_kind["context"].store, 3);
    assert_eq!(verify.counts_by_kind["screenshot"].store, 1);
    assert_eq!(verify.counts_by_kind["recognition"].store, 1);

    // mapping details
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events e JOIN events p ON p.event_id=e.parent_event_id WHERE e.source_key='rec-shot' AND p.source_key='rec-win'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE source_key='rec-prot' AND sensitivity='protected'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_text t JOIN events e USING(event_id) WHERE e.source_key='rec-prot'"), 0);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='coverage.gap' AND payload_json LIKE '%\"paused\"%'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='system.state' AND payload_json LIKE '%\"wake\"%'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_text t JOIN events e USING(event_id) WHERE e.kind='image.text' AND t.field='ocr'"), 1);
    // is_new is recomputed in time order: ctx2 repeats the first paragraph
    let flags = fx
        .store
        .raw_db()
        .query(
            "SELECT t.is_new FROM event_text t JOIN events e USING(event_id) WHERE e.source_key IN ('rec-ctx1','rec-ctx2') AND t.field='body' ORDER BY e.start_ms, t.ord",
            &[],
            |r| r.int(0),
        )
        .unwrap();
    assert_eq!(flags, vec![1, 1, 0, 1]);

    // §9 "抽样做时间线和搜索对照": the migrated store answers the same questions as the ledger
    let ledger_db = Db::open(&ledger.root.join("ledger.sqlite"), true, false).unwrap();
    let ledger_hits: BTreeSet<String> =
        ledger_db.query("SELECT id FROM records WHERE instr(search_text, '预算') > 0", &[], |r| r.text(0)).unwrap().into_iter().collect();
    let hits = fx.store.search(&SearchRequest { q: Some("预算".into()), attention: Some("all".into()), limit: Some(50), ..Default::default() }, &fx.client).unwrap();
    let store_keys: BTreeSet<String> = hits.hits.iter().filter_map(|h| key_of(&fx, &h.event_id)).collect();
    assert!(ledger_hits.is_subset(&store_keys), "ledger finds {:?} that the store misses", ledger_hits.difference(&store_keys));
    assert!(store_keys.contains("ledger-recognition:rec-shot:ocr")); // OCR text is searchable too
    let timeline = fx
        .store
        .timeline(
            &TimelineRequest {
                start: Some("2026-09-01".into()),
                end: Some("2026-10-30".into()),
                kinds: Some(vec!["app.focus".into(), "window.focus".into(), "screen.text".into(), "screen.shot".into()]),
                limit: Some(50),
                ..Default::default()
            },
            &fx.client,
        )
        .unwrap();
    let mut ledger_order: Vec<(f64, String)> = ledger
        .records
        .iter()
        .filter(|r| ["application", "window", "context", "screenshot"].contains(&r.kind))
        .map(|r| (r.unix, r.id.clone()))
        .collect();
    ledger_order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let store_order: Vec<String> = timeline.entries.iter().filter_map(|e| e.event.as_ref().and_then(|ev| key_of(&fx, &ev.event_id))).collect();
    assert_eq!(store_order, ledger_order.into_iter().map(|o| o.1).collect::<Vec<_>>());

    // re-run: nothing new
    let counts = fx.store.table_counts().unwrap();
    let again = fx.store.migrate_desktop_ledger(&ledger.root, None, 3, None).unwrap();
    assert!(again.inserted == 0 && again.completed);
    assert_eq!(fx.store.table_counts().unwrap(), counts);
    // the ledger was never modified
    drop(ledger_db);
    assert_eq!(file_hash(&ledger.root.join("ledger.sqlite")), db_before);
    assert_eq!(file_hash(&original_blob), blob_before);
    assert!(fx.store.integrity_check(true).unwrap().ok);
}

#[test]
fn interrupted_migration_resumes_without_duplicates() {
    let ledger = make_synthetic_ledger().unwrap();
    let _c = Cleanup(ledger.root.clone());
    let root = temp_root("mig-resume");
    let _r = Cleanup(root.clone());
    let store = open(&root);
    let partial = store.migrate_desktop_ledger(&ledger.root, None, 2, Some(2)).unwrap();
    assert!(!partial.completed && partial.inserted == 4);
    assert!(!store.verify_desktop_ledger_migration(&ledger.root, None, true).unwrap().ok());
    // simulate an app restart as well
    let sid = partial.source_id.clone();
    drop(store);
    let reopened = open(&root);
    let rest = reopened.migrate_desktop_ledger(&ledger.root, Some(&sid), 2, None).unwrap();
    assert!(rest.completed && rest.inserted == ledger.records.len() + 1 - 4, "{rest:?}");
    assert_eq!(rest.conflicts, 0);
    assert!(reopened.verify_desktop_ledger_migration(&ledger.root, None, true).unwrap().ok());
}

#[test]
fn tampered_ledger_image_is_reported_not_imported_and_fixable_on_rerun() {
    let ledger = make_synthetic_ledger().unwrap();
    let _c = Cleanup(ledger.root.clone());
    let fx = Fx::make("mig-tamper");
    let hash = sha256_hex(&ledger.image);
    let file = ledger.root.join(format!("blobs/{}/{hash}.jpg", &hash[..2]));
    std::fs::write(&file, b"corrupted").unwrap();
    let report = fx.store.migrate_desktop_ledger(&ledger.root, None, 200, None).unwrap();
    assert!(!report.completed && !report.failures.is_empty() && report.skipped >= 1);
    assert_eq!(fx.store.event_id(&report.source_id, "rec-shot").unwrap(), None);
    std::fs::write(&file, &ledger.image).unwrap(); // repaired original
    assert!(fx.store.migrate_desktop_ledger(&ledger.root, None, 200, None).unwrap().completed);
    assert!(fx.store.verify_desktop_ledger_migration(&ledger.root, None, true).unwrap().ok());
}

#[test]
fn unsupported_ledger_version_is_refused() {
    let fx = Fx::make("mig-badver");
    let dir = temp_root("badledger");
    let _c = Cleanup(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("ledger.sqlite"), false, true).unwrap();
    db.execute("PRAGMA user_version=9", &[]).unwrap();
    drop(db);
    assert!(fx.store.migrate_desktop_ledger(&dir, None, 200, None).is_err());
}

#[test]
fn hub_media_and_observations_migrate_with_mapping_and_idempotency() {
    let hub = make_synthetic_hub().unwrap();
    let _c = Cleanup(hub.root.clone());
    let fx = Fx::make("mig-hub");
    let report = fx.store.migrate_hub_media(&hub.db, &hub.vault, None, 100, None).unwrap();
    assert!(report.completed, "{:?}", report.failures);
    let kinds = fx.store.raw_db().query("SELECT kind, COUNT(*) FROM events GROUP BY kind ORDER BY kind", &[], |r| (r.text(0), r.int(1))).unwrap();
    assert_eq!(kinds.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(), vec!["audio.recording", "image.capture", "speech.segment"]);
    assert_eq!(kinds.iter().map(|k| k.1).collect::<Vec<_>>(), vec![1, 1, 2]);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE source_key='media:m3'"), 0); // deleted in the Hub
    let imported = fx.store.raw_db().first("SELECT start_ms,time_basis FROM events WHERE source_key='media:m2'", &[], |r| (r.opt_int(0), r.text(1))).unwrap().unwrap();
    assert!(imported.0.is_none() && imported.1 == "imported");
    assert_eq!(fx.scalar("SELECT start_ms FROM events WHERE source_key='media:m1'"), 1_793_408_400_123); // 2026-10-31T01:00:00.123Z
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM events s JOIN events a ON a.event_id=s.parent_event_id WHERE s.kind='speech.segment' AND a.source_key='media:m1'"),
        2
    );
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='speech.segment' AND payload_json LIKE '%pending%'"), 0);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM threads WHERE kind='recording'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM blobs"), 2);
    let again = fx.store.migrate_hub_media(&hub.db, &hub.vault, None, 100, None).unwrap();
    assert!(again.inserted == 0 && again.completed);

    let obs = fx.store.migrate_hub_observations(&hub.db, None, 500, None).unwrap();
    assert!(obs.inserted == 2 && obs.completed);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE source_key='dev/boot1/1' AND kind='marker'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE source_key='dev/boot1/2' AND kind='system.state' AND start_ms IS NULL AND time_basis='imported'"), 1);
    assert_eq!(fx.store.migrate_hub_observations(&hub.db, None, 500, None).unwrap().inserted, 0);
    assert!(fx.store.integrity_check(true).unwrap().ok);
}

#[test]
fn hub_missing_original_is_reported_and_skipped() {
    let hub = make_synthetic_hub().unwrap();
    let _c = Cleanup(hub.root.clone());
    let fx = Fx::make("mig-hub-missing");
    for f in std::fs::read_dir(hub.vault.join("blobs")).unwrap().flatten() {
        if f.path().extension().is_some_and(|e| e == "jpg") {
            std::fs::remove_file(f.path()).unwrap();
        }
    }
    let report = fx.store.migrate_hub_media(&hub.db, &hub.vault, None, 100, None).unwrap();
    assert!(!report.completed && report.failures.len() == 1);
    assert!(fx.store.event_id(&report.source_id, "media:m1").unwrap().is_some());
}

// MARK: LiveSync

#[test]
fn desktop_ledger_sync_imports_once_then_follows_new_deleted_and_rerun_records() {
    let ledger = make_synthetic_ledger().unwrap();
    let _c = Cleanup(ledger.root.clone());
    let fx = Fx::make("sync-ledger");
    let first = fx.store.sync_desktop_ledger(&ledger.root, None).unwrap();
    assert!(first.failures.is_empty(), "{:?}", first.failures);
    assert_eq!(first.inserted, ledger.records.len() + 1); // + the OCR result
    assert!(fx.store.verify_desktop_ledger_migration(&ledger.root, Some(&first.source_id), true).unwrap().ok());
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events e JOIN events p ON p.event_id=e.parent_event_id WHERE e.source_key='rec-shot' AND p.source_key='rec-win'"), 1);

    let quiet = fx.store.sync_desktop_ledger(&ledger.root, None).unwrap();
    assert!(quiet.inserted == 0 && quiet.deleted == 0 && quiet.revised == 0 && !quiet.changed());

    // New activity, a deletion and a re-run recognition.
    let db = ledger_db(&ledger.root).unwrap();
    let new = LedgerRecord {
        id: "rec-new".into(),
        unix: LEDGER_T0 + 200.0,
        kind: "context",
        app_name: "Notes".into(),
        bundle_id: "com.example.notes".into(),
        window_title: "预算草稿".into(),
        run_id: "synthetic-run".into(),
        trigger: Some("timer".into()),
        context: Some(serde_json::json!({"readableText": "新的合成段落：季度复盘", "textStatus": "ok"})),
        activity_id: Some("act-1".into()),
        ..Default::default()
    };
    ledger_append(&db, &new).unwrap();
    ledger_delete(&db, "rec-mark", LEDGER_T0 + 300.0).unwrap();
    ledger_save_recognition(&db, "rec-shot", "ocr", "synthetic-ocr/2", "OCR 第二次 预算表 修订", LEDGER_T0 + 500.0).unwrap();
    drop(db);

    let next = fx.store.sync_desktop_ledger(&ledger.root, None).unwrap();
    assert!(next.failures.is_empty(), "{:?}", next.failures);
    assert_eq!(next.inserted, 2); // the new context and the new OCR run
    assert!(next.deleted == 1 && next.revised == 1, "{next:?}");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE source_key='rec-mark'"), 0);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='image.text'"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_text t JOIN text_chunks c USING(chunk_id) WHERE c.text LIKE '%第二次%'"), 1);

    // A deleted record never comes back, and nothing repeats.
    assert!(!fx.store.sync_desktop_ledger(&ledger.root, None).unwrap().changed());
    assert!(fx.store.integrity_check(true).unwrap().ok);
}

#[test]
fn sync_after_one_shot_migration_adds_nothing_and_replaces_old_recognition_keys() {
    let ledger = make_synthetic_ledger().unwrap();
    let _c = Cleanup(ledger.root.clone());
    let fx = Fx::make("sync-after-mig");
    assert!(fx.store.migrate_desktop_ledger(&ledger.root, None, 200, None).unwrap().completed);
    let sync = fx.store.sync_desktop_ledger(&ledger.root, None).unwrap();
    // The migration keyed the OCR result without a run stamp; the first sync replaces it with the stamped key once.
    assert!(sync.inserted == 1 && sync.revised == 1 && sync.unchanged >= ledger.records.len(), "{sync:?}");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='image.text'"), 1);
    assert!(!fx.store.sync_desktop_ledger(&ledger.root, None).unwrap().changed());
}

#[test]
fn hub_media_sync_follows_new_transcripts_and_deletions() {
    let hub = make_synthetic_hub().unwrap();
    let _c = Cleanup(hub.root.clone());
    let fx = Fx::make("sync-hub");
    let first = fx.store.sync_hub_media(&hub.db, &hub.vault, None).unwrap();
    assert!(first.failures.is_empty(), "{:?}", first.failures);
    assert_eq!(first.inserted, 4); // recording + 2 segments + image
    assert!(!fx.store.sync_hub_media(&hub.db, &hub.vault, None).unwrap().changed());

    // The image gets text later; the recording is deleted in the library.
    let db = Db::open(&hub.db, false, false).unwrap();
    db.execute("INSERT INTO media_timeline_recognition VALUES('m2','fast','{\"text\":\"合成照片里的白板文字\"}','','2026-11-01T00:02:00+00:00')", &[]).unwrap();
    db.execute("INSERT INTO media_timeline_deleted VALUES('m1')", &[]).unwrap();
    let next = fx.store.sync_hub_media(&hub.db, &hub.vault, None).unwrap();
    assert!(next.failures.is_empty(), "{:?}", next.failures);
    assert_eq!(next.inserted, 1);
    assert_eq!(next.deleted, 3); // the recording and its two segments
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind IN ('audio.recording','speech.segment')"), 0);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='image.text'"), 1);

    // A changed result replaces the earlier text.
    db.execute("UPDATE media_timeline_recognition SET result='{\"text\":\"合成白板文字（重新识别）\"}' WHERE media_id='m2'", &[]).unwrap();
    let revised = fx.store.sync_hub_media(&hub.db, &hub.vault, None).unwrap();
    assert!(revised.revised == 1 && revised.inserted == 1, "{revised:?}");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='image.text'"), 1);
    assert!(fx.store.integrity_check(true).unwrap().ok);
}

#[test]
fn idle_wal_database_opens_read_only() {
    // The Hub keeps its library in WAL mode; when the Hub is not running there is no -shm file to share.
    let root = temp_root("wal");
    let _c = Cleanup(root.clone());
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("hub.sqlite");
    {
        let writer = Db::open(&path, false, true).unwrap();
        writer.execute("PRAGMA journal_mode=WAL", &[]).unwrap();
        writer.execute("CREATE TABLE t(x INTEGER)", &[]).unwrap();
        writer.execute("INSERT INTO t VALUES(42)", &[]).unwrap();
        writer.execute("PRAGMA wal_checkpoint(TRUNCATE)", &[]).unwrap();
    }
    // What a cleanly stopped writer leaves behind: the database file only.
    let _ = std::fs::remove_file(format!("{}-wal", path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    let reader = Db::open(&path, true, false).unwrap();
    assert_eq!(reader.scalar("SELECT x FROM t", &[]).unwrap(), 42);
    assert!(reader.execute("INSERT INTO t VALUES(1)", &[]).is_err());
}
