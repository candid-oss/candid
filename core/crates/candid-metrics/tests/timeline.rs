use chrono::NaiveDate;
use chrono_tz::Asia::Singapore;
use candid_metrics::{timeline::Snapshot, Dataset};
use candid_store::{
    fixtures::temp_root, sha256_hex, Clock, IngestBatch, IngestBlob, IngestEvent, IngestText,
    SourcePolicy, TimelineStore,
};
use std::process::Command;

#[test]
fn core_ingestion_to_offline_snapshot_is_repeatable_private_and_read_only() {
    let d: Dataset =
        serde_json::from_str(include_str!("../../../fixtures/metrics/ideal-day.json")).unwrap();
    let root = temp_root("metrics");
    let store = TimelineStore::open(&root, "metrics-test", true, Clock::default()).unwrap();
    let source = store
        .register_source("mac.desktop", "synthetic", &SourcePolicy::new(true, true))
        .unwrap();
    let mut events = Vec::new();
    for (i, r) in d.text_records.iter().enumerate() {
        let mut e = IngestEvent::new(&format!("reading-{i}"), "screen.text", Some(r.at_ms));
        e.app_bundle_id = Some("synthetic.private.app".into());
        e.title = Some("PRIVATE TITLE MUST NOT LEAK".into());
        e.text = vec![IngestText::new("body", &r.text)];
        events.push(e);
    }
    for (i, s) in d.shots.as_ref().unwrap().iter().enumerate() {
        let path = store.staging_directory().join(format!("frame-{i}"));
        let bytes = b"synthetic image";
        std::fs::write(&path, bytes).unwrap();
        let mut e = IngestEvent::new(&format!("frame-{i}"), "screen.shot", Some(s.at_ms));
        e.blobs = vec![IngestBlob::new(path.to_str().unwrap(), &sha256_hex(bytes))];
        events.push(e);
    }
    let batch = IngestBatch::events(&source, events);
    assert_eq!(store.ingest(&batch).unwrap().inserted, 269);
    // Idempotent retries produce the same measurement input.
    for event in &batch.events {
        for blob in &event.blobs {
            std::fs::write(&blob.staging_path, b"synthetic image").unwrap();
        }
    }
    assert_eq!(store.ingest(&batch).unwrap().duplicates, 269);
    drop(store);
    let path = root.join("timeline.sqlite");
    let before = std::fs::read(&path).unwrap();
    let first = Snapshot::read(&path, 1794240000000, 1794326400000).unwrap();
    let date = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
    let report = first.measure(date, end, Singapore).unwrap();
    assert_eq!(report.counts["desktop_events"], 269);
    let extended = first
        .measure(
            date,
            NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
            Singapore,
        )
        .unwrap();
    assert_eq!(extended.days[1].observed_ms, 0);
    assert!(extended.days[1].metrics.values().all(|m| m.value.is_none()));
    assert_eq!(
        report.days[0].metrics["screenshot_bytes"].value,
        Some(2250.)
    );
    assert_eq!(
        report.days[0].metrics["screenshot_unique_bytes"].value,
        Some(15.)
    );
    assert_eq!(
        report.days[0].metrics["legacy_app_reference_redundancy"].value,
        Some(118. / 119.)
    );
    assert_eq!(report.days[0].metrics["redundancy"].value, None);
    assert_eq!(report.days[0].metrics["text_coverage"].value, None);
    assert_eq!(
        report,
        Snapshot::read(&path, 1794240000000, 1794326400000)
            .unwrap()
            .measure(date, end, Singapore)
            .unwrap()
    );
    assert_eq!(before, std::fs::read(&path).unwrap());
    let out = Command::new(env!("CARGO_BIN_EXE_candid-metrics"))
        .args([
            "--timeline",
            path.to_str().unwrap(),
            "--start",
            "2026-11-10",
            "--end",
            "2026-11-11",
            "--timezone",
            "Asia/Singapore",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = String::from_utf8(out.stdout).unwrap();
    for private in [
        "PRIVATE TITLE",
        "synthetic.private.app",
        "张伟",
        "OCR O",
        "通知 1",
    ] {
        assert!(!out.contains(private));
    }
    assert_eq!(before, std::fs::read(&path).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn unknown_schema_missing_database_and_invalid_cli_are_rejected_without_creation() {
    let root = temp_root("metrics-reject");
    std::fs::create_dir_all(&root).unwrap();
    let missing = root.join("absent.sqlite");
    assert!(Snapshot::read(&missing, 0, 1).is_err());
    assert!(!missing.exists());
    let path = root.join("future.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=5").unwrap();
    drop(db);
    assert!(Snapshot::read(&path, 0, 1).is_err());
    for args in [
        vec!["--start", "2026-11-10"],
        vec!["--private", "PRIVATE SECRET"],
        vec!["--timezone", "PRIVATE SECRET"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_candid-metrics"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE SECRET"));
        assert!(out.stdout.is_empty());
    }
    std::fs::remove_dir_all(root).unwrap();
}


#[test]
fn v1_and_upgraded_v2_have_identical_read_only_desktop_metrics() {
    use candid_store::{json, StoreIdentity};
    let root = temp_root("metrics-upgrade");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("timeline.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../../../fixtures/schema-v1.sql")).unwrap();
    db.execute_batch("PRAGMA user_version=1; INSERT INTO event_kinds VALUES('screen.text','evidence',0,1,'Synthetic text'); INSERT INTO store_meta VALUES('ingest_seq_next','2'); INSERT INTO sources(source_id,kind,display_name,policy_json,status,created_ms,updated_ms) VALUES('s','mac.desktop','SYNTHETIC','{}','active',1,1); INSERT INTO events(event_id,source_id,source_key,kind,start_ms,time_basis,payload_version,payload_json,ingest_seq,ingested_ms) VALUES('e','s','text','screen.text',1794240001000,'source',1,'{}',1,1794240001000); INSERT INTO text_chunks VALUES(1,'synthetic','SYNTHETIC text',14,NULL,1); INSERT INTO event_text VALUES('e','body',0,1,NULL,NULL,1,0); INSERT INTO text_fts(rowid,text) VALUES(1,'SYNTHETIC text');").unwrap();
    drop(db);
    std::fs::write(root.join("store.json"),json::canonical_of(&StoreIdentity {store_id:"synthetic-metrics-upgrade".into(),schema_version:1,created_at:"2026-11-10T00:00:00Z".into(),created_by:"synthetic".into(),layout:"timeline-root/1".into()})).unwrap();
    let start=NaiveDate::from_ymd_opt(2026,11,10).unwrap();
    let end=NaiveDate::from_ymd_opt(2026,11,11).unwrap();
    let measure = || Snapshot::read(&path,1794240000000,1794326400000).unwrap().measure(start,end,Singapore).unwrap();
    let bytes=std::fs::read(&path).unwrap();
    let v1=measure();
    assert_eq!(std::fs::read(&path).unwrap(),bytes);
    let store=TimelineStore::open(&root,"synthetic",false,Clock::default()).unwrap();
    assert_eq!(store.identity.schema_version,candid_store::SCHEMA_VERSION);
    // New AI records must not change this desktop-only baseline.
    let source=store.register_source("agent.transcript","SYNTHETIC AI",&SourcePolicy::default()).unwrap();
    let mut e=IngestEvent::new("synthetic-prompt","ai.prompt",Some(1794240001000));
    e.payload=serde_json::json!({"agent":"codex","session_ref":"agent:codex:synthetic","message_ref":"synthetic-prompt","origin":"transcript"});
    e.text=vec![IngestText::new("prompt","SYNTHETIC AI must not enter desktop metrics")];
    assert_eq!(store.ingest(&IngestBatch::events(&source,vec![e])).unwrap().inserted,1);
    drop(store);
    let bytes=std::fs::read(&path).unwrap();
    assert_eq!(measure(),v1);
    assert_eq!(std::fs::read(&path).unwrap(),bytes);
    assert_eq!(rusqlite::Connection::open(&path).unwrap().query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),candid_store::SCHEMA_VERSION);
    std::fs::remove_dir_all(root).unwrap();
}
