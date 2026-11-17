//! Frozen v1 schema and synthetic rows exercise real table rebuilding and both sides of the commit boundary.
use candid_store::{
    testing::{Db, RowExt},
    *,
};
use std::{fs, path::PathBuf};
fn old_root() -> PathBuf {
    let root = fixtures::temp_root("v1-upgrade");
    fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join("timeline.sqlite"), false, true).unwrap();
    db.script(include_str!("../../../fixtures/schema-v1.sql"))
        .unwrap();
    db.script("PRAGMA foreign_keys=ON; INSERT INTO event_kinds VALUES('note','evidence',0,1,'Synthetic note'); INSERT INTO store_meta VALUES('ingest_seq_next','2'); PRAGMA user_version=1;").unwrap();
    let policy = json::canonical_of(&SourcePolicy::default());
    db.conn.execute("INSERT INTO sources VALUES('s','manual','SYNTHETIC',NULL,NULL,NULL,'{}',?,'active',NULL,NULL,1,1)",[policy]).unwrap();
    db.script("INSERT INTO threads VALUES('t','s','session','ai_session','SYNTHETIC',1,2,'[]','{}'); INSERT INTO events(event_id,source_id,source_key,kind,start_ms,end_ms,time_basis,thread_id,payload_version,payload_json,ingest_seq,ingested_ms) VALUES('e','s','original','note',1,2,'source','t',1,'{}',1,1); INSERT INTO text_chunks VALUES(1,'synthetic-hash','SYNTHETIC unchanged',19,NULL,1); INSERT INTO event_text VALUES('e','note',0,1,NULL,NULL,1,0); INSERT INTO text_fts(rowid,text) VALUES(1,'SYNTHETIC unchanged'); INSERT INTO meta_fts(rowid,title,app_name,url) VALUES(1,NULL,NULL,NULL);").unwrap();
    fs::write(
        root.join("store.json"),
        json::canonical_of(&StoreIdentity {
            store_id: "synthetic-fixed-store".into(),
            schema_version: 1,
            created_at: "2026-11-11T00:00:00Z".into(),
            created_by: "synthetic-v1".into(),
            layout: "timeline-root/1".into(),
        }),
    )
    .unwrap();
    root
}
fn open(root: &std::path::Path) -> Result<TimelineStore> {
    TimelineStore::open(root, "synthetic-v2", false, Clock::default())
}
#[test]
fn upgrade_preserves_identity_rows_references_and_repeated_open() {
    let root = old_root();
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert_eq!(store.identity.store_id, "synthetic-fixed-store");
    assert_eq!(
        store.event_id("s", "original").unwrap().as_deref(),
        Some("e")
    );
    assert_eq!(
        store
            .raw_db()
            .query("SELECT text FROM text_chunks", &[], |r| r.text(0))
            .unwrap(),
        vec!["SYNTHETIC unchanged"]
    );
    assert!(store.integrity_check(false).unwrap().ok);
    let c = agents::AgentConsent::new("pi");
    store.set_agent_consent(&c, false).unwrap();
    assert_eq!(
        store
            .raw_db()
            .scalar(
                "SELECT COUNT(*) FROM event_kinds WHERE kind IN ('ai.prompt','ai.work')",
                &[]
            )
            .unwrap(),
        2
    );
    drop(store);
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert!(store.integrity_check(false).unwrap().ok);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn ddl_failure_rolls_back_and_retry_preserves_v1_data() {
    let root = old_root();
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    db.execute("CREATE TABLE threads_v2(synthetic INTEGER)", &[])
        .unwrap();
    drop(db);
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), 1);
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM events WHERE event_id='e'", &[])
            .unwrap(),
        1
    );
    db.execute("DROP TABLE threads_v2", &[]).unwrap();
    drop(db);
    assert!(open(&root).unwrap().integrity_check(false).unwrap().ok);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn metadata_replace_failure_resumes_only_committed_migration() {
    let root = old_root();
    fs::create_dir(root.join(".store.json.tmp")).unwrap();
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), SCHEMA_VERSION);
    drop(db);
    fs::remove_dir(root.join(".store.json.tmp")).unwrap();
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert_eq!(
        store.event_id("s", "original").unwrap().as_deref(),
        Some("e")
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn unrelated_version_mismatch_is_refused_without_rebuilding() {
    let root = old_root();
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    db.execute("PRAGMA user_version=2", &[]).unwrap();
    drop(db);
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM events WHERE event_id='e'", &[])
            .unwrap(),
        1
    );
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

/// Frozen v3 schema (fixtures/schema-v3.sql) with synthetic rows, including a tombstone and the agent index.
fn v3_root() -> PathBuf {
    let root = fixtures::temp_root("v3-upgrade");
    fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join("timeline.sqlite"), false, true).unwrap();
    db.script(include_str!("../../../fixtures/schema-v3.sql")).unwrap();
    db.script("PRAGMA foreign_keys=ON; INSERT INTO event_kinds VALUES('note','evidence',0,1,'Synthetic note'); INSERT INTO event_kinds VALUES('mail.message','evidence',0,1,'Synthetic mail'); INSERT INTO store_meta VALUES('ingest_seq_next','2'); INSERT INTO store_meta VALUES('schema.upgrade','2:3'); PRAGMA user_version=3;").unwrap();
    let policy = json::canonical_of(&SourcePolicy::default());
    db.conn.execute("INSERT INTO sources VALUES('s','manual','SYNTHETIC',NULL,NULL,NULL,'{}',?,'active',NULL,NULL,1,1)", [policy]).unwrap();
    db.script("INSERT INTO threads VALUES('t','s','thread','mail_thread','SYNTHETIC',1,2,'[]','{}'); INSERT INTO events(event_id,source_id,source_key,kind,start_ms,end_ms,time_basis,thread_id,payload_version,payload_json,ingest_seq,ingested_ms) VALUES('e','s','original','note',1,2,'source','t',1,'{}',1,1); INSERT INTO text_chunks VALUES(1,'synthetic-hash','SYNTHETIC unchanged',19,NULL,1); INSERT INTO event_text VALUES('e','note',0,1,NULL,NULL,1,0); INSERT INTO text_fts(rowid,text) VALUES(1,'SYNTHETIC unchanged'); INSERT INTO meta_fts(rowid,title,app_name,url) VALUES(1,NULL,NULL,NULL); INSERT INTO tombstones VALUES('event','gone',5,'user');").unwrap();
    fs::write(
        root.join("store.json"),
        json::canonical_of(&StoreIdentity {
            store_id: "synthetic-v3-store".into(),
            schema_version: 3,
            created_at: "2026-11-12T00:00:00Z".into(),
            created_by: "synthetic-v3".into(),
            layout: "timeline-root/1".into(),
        }),
    )
    .unwrap();
    root
}

#[test]
fn v3_upgrade_adds_connector_kinds_quoted_text_and_source_deleted() {
    let root = v3_root();
    let store = open(&root).unwrap();
    assert_eq!((store.identity.schema_version, store.identity.store_id.as_str()), (SCHEMA_VERSION, "synthetic-v3-store"));
    let db = store.raw_db();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), SCHEMA_VERSION);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM tombstones WHERE entity_id='gone' AND reason='user'", &[]).unwrap(), 1);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM event_kinds WHERE kind='browser.visit'", &[]).unwrap(), 1);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='sources_agent'", &[]).unwrap(), 1);
    assert_eq!(store.event_id("s", "original").unwrap().as_deref(), Some("e"));
    assert!(store.integrity_check(false).unwrap().ok);
    // New values are accepted after the upgrade.
    for kind in ["mail.apple", "browser.history"] {
        store.register_source(kind, "SYNTHETIC", &SourcePolicy::default()).unwrap();
    }
    let mut e = IngestEvent::new("m", "mail.message", Some(10));
    e.time_basis = "source".into();
    e.payload = serde_json::json!({"direction": "in"});
    e.text = vec![IngestText::new("quoted", "> SYNTHETIC quoted")];
    store.ingest(&IngestBatch::events("s", vec![e])).unwrap();
    let id = store.event_id("s", "m").unwrap().unwrap();
    store.delete_events(&[id], "source_deleted").unwrap();
    drop(store);
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert!(store.integrity_check(false).unwrap().ok);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn v3_upgrade_failure_rolls_back_and_retries() {
    let root = v3_root();
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    db.execute("CREATE TABLE tombstones_v4(synthetic INTEGER)", &[]).unwrap();
    drop(db);
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), 3);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM sources WHERE source_id='s'", &[]).unwrap(), 1);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM event_text WHERE event_id='e'", &[]).unwrap(), 1);
    assert!(db.execute("INSERT INTO sources VALUES('x','mail.apple','X',NULL,NULL,NULL,'{}','{}','active',NULL,NULL,1,1)", &[]).is_err(), "v3 constraint kept");
    db.execute("DROP TABLE tombstones_v4", &[]).unwrap();
    drop(db);
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert!(store.integrity_check(false).unwrap().ok);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn v3_upgrade_resumes_after_interrupted_metadata_write() {
    let root = v3_root();
    fs::create_dir(root.join(".store.json.tmp")).unwrap();
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), SCHEMA_VERSION);
    drop(db);
    fs::remove_dir(root.join(".store.json.tmp")).unwrap();
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert_eq!(store.event_id("s", "original").unwrap().as_deref(), Some("e"));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

/// Frozen v4 schema (fixtures/schema-v4.sql) with synthetic derived rows: a memory with its source, a speaker
/// correction and a queued job.
fn v4_root() -> PathBuf {
    let root = fixtures::temp_root("v4-upgrade");
    fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join("timeline.sqlite"), false, true).unwrap();
    db.script(include_str!("../../../fixtures/schema-v4.sql")).unwrap();
    db.script("PRAGMA foreign_keys=ON; INSERT INTO event_kinds VALUES('note','evidence',0,1,'Synthetic note'); INSERT INTO store_meta VALUES('ingest_seq_next','2'); INSERT INTO store_meta VALUES('schema.upgrade','3:4'); PRAGMA user_version=4;").unwrap();
    let policy = json::canonical_of(&SourcePolicy::default());
    db.conn.execute("INSERT INTO sources VALUES('s','manual','SYNTHETIC',NULL,NULL,NULL,'{}',?,'active',NULL,NULL,1,1)", [policy]).unwrap();
    db.script("INSERT INTO events(event_id,source_id,source_key,kind,start_ms,end_ms,time_basis,payload_version,payload_json,ingest_seq,ingested_ms) VALUES('e','s','original','note',1000,2000,'source',1,'{}',1,1);
      INSERT INTO meta_fts(rowid,title,app_name,url) VALUES(1,NULL,NULL,NULL);
      INSERT INTO memories VALUES('m',1,'commitment','SYNTHETIC send the quote',NULL,NULL,'candidate','rules/1',NULL,NULL,NULL,5);
      INSERT INTO memory_sources VALUES('m',1,'event','e',NULL,NULL);
      INSERT INTO corrections VALUES('c','speaker','spk-1','relabel','{\"person\":\"p\"}',6);
      INSERT INTO jobs(job_id,kind,input_ref,config_version,state,created_ms,updated_ms) VALUES('j','workflow.title','media-1','fixed/1:x','queued',7,7);").unwrap();
    fs::write(
        root.join("store.json"),
        json::canonical_of(&StoreIdentity {
            store_id: "synthetic-v4-store".into(),
            schema_version: 4,
            created_at: "2026-11-13T00:00:00Z".into(),
            created_by: "synthetic-v4".into(),
            layout: "timeline-root/1".into(),
        }),
    )
    .unwrap();
    root
}

#[test]
fn v4_upgrade_keeps_derived_rows_and_accepts_the_new_layer() {
    let root = v4_root();
    let store = open(&root).unwrap();
    assert_eq!((store.identity.schema_version, store.identity.store_id.as_str()), (SCHEMA_VERSION, "synthetic-v4-store"));
    let db = store.raw_db();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), SCHEMA_VERSION);
    // Old rows survive the table rebuilds, with the memory still pointing at its source.
    assert_eq!(db.scalar("SELECT COUNT(*) FROM memories m JOIN memory_sources s USING(memory_id,rev) WHERE m.kind='commitment' AND s.ref_id='e'", &[]).unwrap(), 1);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM corrections WHERE target_type='speaker'", &[]).unwrap(), 1);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs WHERE job_id='j' AND feature IS NULL", &[]).unwrap(), 1);
    assert!(store.integrity_check(false).unwrap().ok);
    // New kinds and tables are accepted.
    db.script("INSERT INTO memories VALUES('r',1,'reference','SYNTHETIC leave reply',NULL,NULL,'confirmed','user',NULL,NULL,NULL,8);
      INSERT INTO memories VALUES('d',1,'decision','SYNTHETIC ship on Friday',NULL,NULL,'candidate','rules/1',NULL,NULL,NULL,8);
      INSERT INTO corrections VALUES('c2','digest','day:2026-11-13','edit_text','{}',9);
      INSERT INTO entities VALUES('p','person','SYNTHETIC Zhang','confirmed',NULL,9,9);
      INSERT INTO entity_aliases VALUES('p','voice-7','voice','user');
      INSERT INTO entity_mentions VALUES('p','event','e',NULL,NULL,'rules/1',NULL);
      INSERT INTO digests VALUES('day:1970-01-01',1,'day',0,86400000,'{}','draft','rules/1',NULL,NULL,NULL,10);").unwrap();
    assert!(db.execute("INSERT INTO memories VALUES('x',1,'gossip','x',NULL,NULL,'candidate','r',NULL,NULL,NULL,1)", &[]).is_err());
    assert!(db.execute("INSERT INTO entity_aliases VALUES('p','x','fingerprint','user')", &[]).is_err());
    assert_eq!(db.scalar("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('digests_time','entity_aliases_lookup','entity_mentions_ref')", &[]).unwrap(), 3);
    // A queued job records the feature it belongs to.
    let job = store.job_command(&serde_json::json!({"action":"enqueue","kind":"workflow.summary","input_ref":"media-2","config_version":"fixed/1:y"})).unwrap();
    assert_eq!(job["feature"], "recording");
    drop(store);
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert!(store.integrity_check(false).unwrap().ok);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn v4_upgrade_failure_rolls_back_and_retries() {
    let root = v4_root();
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    db.execute("CREATE TABLE entities(synthetic INTEGER)", &[]).unwrap();
    drop(db);
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), 4);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM memories WHERE memory_id='m'", &[]).unwrap(), 1);
    assert!(db.execute("INSERT INTO memories VALUES('r',1,'reference','x',NULL,NULL,'confirmed','user',NULL,NULL,NULL,8)", &[]).is_err(), "v4 constraint kept");
    assert_eq!(db.scalar("SELECT COUNT(*) FROM pragma_table_info('jobs') WHERE name='feature'", &[]).unwrap(), 0);
    db.execute("DROP TABLE entities", &[]).unwrap();
    drop(db);
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert_eq!(store.raw_db().scalar("SELECT COUNT(*) FROM memory_sources WHERE memory_id='m'", &[]).unwrap(), 1);
    assert!(store.integrity_check(false).unwrap().ok);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn v4_upgrade_resumes_after_interrupted_metadata_write() {
    let root = v4_root();
    fs::create_dir(root.join(".store.json.tmp")).unwrap();
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), SCHEMA_VERSION);
    drop(db);
    fs::remove_dir(root.join(".store.json.tmp")).unwrap();
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

// Reconstruct the v5 schema from a synthetic upgraded fixture, removing only the v6 accelerators.
fn v5_root() -> PathBuf {
    let root = v4_root();
    drop(open(&root).unwrap());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    for name in db.query("SELECT name FROM sqlite_master WHERE type='trigger' AND (name GLOB 'background_*' OR name GLOB 'projection_*')", &[], |r| r.text(0)).unwrap() {
        db.script(&format!("DROP TRIGGER {name}")).unwrap();
    }
    db.script("DROP TABLE agent_session_entries; DROP TABLE agent_file_counts; DROP INDEX events_projection_owner; DROP TABLE background_revisions; DROP INDEX events_source_key; DROP INDEX jobs_pending; DROP INDEX media_parts_recording; PRAGMA user_version=5; UPDATE store_meta SET value='4:5' WHERE key='schema.upgrade';").unwrap();
    let mut identity: StoreIdentity = serde_json::from_slice(&fs::read(root.join("store.json")).unwrap()).unwrap();
    identity.schema_version = 5;
    fs::write(root.join("store.json"), json::canonical_of(&identity)).unwrap();
    root
}

#[test]
fn v5_performance_upgrade_is_atomic_and_resumes_after_metadata_interruption() {
    let root = v5_root();
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    db.script("CREATE TABLE events_source_key(synthetic INTEGER)").unwrap();
    drop(db);
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), 5);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='background_revisions'", &[]).unwrap(), 0);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM events WHERE event_id='e'", &[]).unwrap(), 1);
    db.script("DROP TABLE events_source_key").unwrap();
    drop(db);
    fs::create_dir(root.join(".store.json.tmp")).unwrap();
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), SCHEMA_VERSION);
    drop(db);
    fs::remove_dir(root.join(".store.json.tmp")).unwrap();
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert_eq!(store.event_id("s", "original").unwrap().as_deref(), Some("e"));
    assert!(store.integrity_check(false).unwrap().ok);
    drop(store);
    assert!(open(&root).unwrap().integrity_check(false).unwrap().ok);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deleting_an_event_clears_its_mentions_and_the_digests_covering_it() {
    let root = v4_root();
    let store = open(&root).unwrap();
    let db = store.raw_db();
    db.script("INSERT INTO entities VALUES('p','person','SYNTHETIC Zhang','confirmed',NULL,9,9);
      INSERT INTO entity_mentions VALUES('p','event','e',NULL,NULL,'rules/1',NULL);
      INSERT INTO digests VALUES('day:covering',1,'day',0,86400000,'{}','draft','rules/1',NULL,NULL,NULL,10);
      INSERT INTO digests VALUES('day:covering',2,'day',0,86400000,'{}','confirmed','rules/1',NULL,NULL,NULL,11);
      INSERT INTO digests VALUES('day:other',1,'day',86400000,172800000,'{}','draft','rules/1',NULL,NULL,NULL,10);").unwrap();
    let report = store.delete_events(&["e".to_string()], "user").unwrap();
    assert_eq!((report.events, report.memories, report.mentions, report.digests), (1, 1, 1, 1));
    assert_eq!(db.scalar("SELECT COUNT(*) FROM digests", &[]).unwrap(), 1, "only the day that did not cover the event stays");
    assert_eq!(db.scalar("SELECT COUNT(*) FROM entities WHERE entity_id='p'", &[]).unwrap(), 1, "the person stays");
    assert!(store.is_tombstoned("digest", "day:covering").unwrap());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn v6_audio_source_upgrade_rolls_back_and_restores_invalidation_triggers() {
    let root = v5_root();
    drop(open(&root).unwrap());
    // Existing sources/foreign keys and the v6 triggers must survive the new CHECK constraint.
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    strip_v8(&db);
    db.script("PRAGMA user_version=6; UPDATE store_meta SET value='5:6' WHERE key='schema.upgrade'; CREATE TABLE sources_v7(synthetic INTEGER)").unwrap();
    let mut identity: StoreIdentity = serde_json::from_slice(&fs::read(root.join("store.json")).unwrap()).unwrap();
    identity.schema_version = 6;
    fs::write(root.join("store.json"), json::canonical_of(&identity)).unwrap();
    drop(db);
    assert!(open(&root).is_err());
    let db = Db::open(&root.join("timeline.sqlite"), false, false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version", &[]).unwrap(), 6);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM sources WHERE source_id='s'", &[]).unwrap(), 1);
    db.script("DROP TABLE sources_v7").unwrap();
    drop(db);
    fs::create_dir(root.join(".store.json.tmp")).unwrap();
    assert!(open(&root).is_err());
    fs::remove_dir(root.join(".store.json.tmp")).unwrap();
    let store = open(&root).unwrap();
    assert_eq!(store.identity.schema_version, SCHEMA_VERSION);
    assert_eq!(store.event_id("s", "original").unwrap().as_deref(), Some("e"));
    assert!(store.integrity_check(false).unwrap().ok);
    let db = store.raw_db();
    let before = db.scalar("SELECT revision FROM background_revisions WHERE domain='workflows'", &[]).unwrap();
    db.script("UPDATE sources SET status='paused' WHERE source_id='s'").unwrap();
    assert!(db.scalar("SELECT revision FROM background_revisions WHERE domain='workflows'", &[]).unwrap() > before);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}


fn strip_v8(db: &Db) {
    for name in db.query("SELECT name FROM sqlite_master WHERE type='trigger' AND name GLOB 'projection_*'",&[],|r|r.text(0)).unwrap() {
        db.script(&format!("DROP TRIGGER {name}")).unwrap();
    }
    db.script("DROP TABLE agent_session_entries; DROP TABLE agent_file_counts; DROP INDEX events_projection_owner; DELETE FROM background_revisions WHERE domain='media_projection'").unwrap();
}

#[test]
fn v7_checkpoint_upgrade_is_atomic_preserves_inline_history_and_resumes_metadata() {
    let root=v4_root();drop(open(&root).unwrap());
    let db=Db::open(&root.join("timeline.sqlite"),false,false).unwrap();
    strip_v8(&db);
    db.script("PRAGMA user_version=7; INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','6:7'); INSERT INTO source_cursors VALUES('s','session:SYNTHETIC','{\"entries\":[\"SYNTHETIC history\"]}',1); CREATE TABLE agent_session_entries(synthetic INTEGER)").unwrap();
    let mut identity:StoreIdentity=serde_json::from_slice(&fs::read(root.join("store.json")).unwrap()).unwrap();identity.schema_version=7;
    fs::write(root.join("store.json"),json::canonical_of(&identity)).unwrap();drop(db);
    assert!(open(&root).is_err());
    let db=Db::open(&root.join("timeline.sqlite"),false,false).unwrap();
    assert_eq!(db.scalar("PRAGMA user_version",&[]).unwrap(),7);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM background_revisions WHERE domain='media_projection'",&[]).unwrap(),0);
    db.script("DROP TABLE agent_session_entries").unwrap();drop(db);
    fs::create_dir(root.join(".store.json.tmp")).unwrap();assert!(open(&root).is_err());
    fs::remove_dir(root.join(".store.json.tmp")).unwrap();
    let store=open(&root).unwrap();assert_eq!(store.identity.schema_version,8);
    assert!(store.cursor("s","session:SYNTHETIC").unwrap().unwrap().contains("SYNTHETIC history"));
    assert!(store.integrity_check(false).unwrap().ok);drop(store);fs::remove_dir_all(root).unwrap();
}
