//! Synthetic regression cases for idle work, permissions, retry deadlines and indexed lookups.
mod common;
use common::Fx;
use candid_store::{testing::RowExt, *};
use serde_json::json;
use std::sync::{Arc, atomic::{AtomicI64, Ordering}};

#[test]
fn permission_and_recording_queries_have_usable_indexes() {
    let fx = Fx::make("performance-indexes");
    let db = fx.store.raw_db();
    for (sql, index) in [
        ("SELECT COUNT(*) FROM events e JOIN sources s ON s.source_id=json_extract(e.payload_json,'$.native_legacy_source_id') WHERE e.source_key='media:SYNTHETIC' AND (s.status='revoked' OR json_extract(s.policy_json,'$.store_media')<>1)", "events_source_key"),
        ("SELECT id FROM media_parts WHERE recording_id='SYNTHETIC' ORDER BY json_extract(metadata,'$.sample_start')", "media_parts_recording"),
        ("SELECT job_id FROM jobs WHERE state IN ('queued','running') ORDER BY created_ms", "jobs_pending"),
    ] {
        let plan = db.query(&format!("EXPLAIN QUERY PLAN {sql}"), &[], |r| r.text(3)).unwrap().join("\n");
        assert!(plan.contains(index), "{plan}");
    }
}

#[test]
fn unchanged_scans_do_not_write_and_policy_changes_invalidate_without_sync_timestamp_churn() {
    let fx = Fx::make("performance-idle");
    let s = &fx.store;
    s.workflow_scan().unwrap();
    let before = fx.scalar("SELECT total_changes()");
    for _ in 0..30 { s.workflow_scan().unwrap(); s.prepare_media(None).unwrap(); }
    // The initial projection may register a source; warm it before measuring strictly idle work.
    let warm = fx.scalar("SELECT total_changes()");
    for _ in 0..30 { s.workflow_scan().unwrap(); s.prepare_media(None).unwrap(); }
    assert_eq!(warm, fx.scalar("SELECT total_changes()"));
    assert!(warm >= before);
    let rev = s.background_revision("workflows").unwrap();
    s.raw_db().execute("UPDATE sources SET last_sync_ms=42", &[]).unwrap();
    assert_eq!(s.background_revision("workflows").unwrap(), rev);
    let mut policy = s.sources().unwrap().into_iter().find(|p| p.source_id == fx.source).unwrap().policy;
    policy.store_text = false;
    s.update_policy(&fx.source, &policy).unwrap();
    assert!(s.background_revision("workflows").unwrap() > rev);
}

#[test]
fn due_work_excludes_terminal_jobs_future_retries_and_live_leases() {
    let mut fx = Fx::make("performance-due");
    let now = Arc::new(AtomicI64::new(1_800_000_000_000));
    let clock = now.clone();
    fx.store.clock = Clock::with(move || clock.load(Ordering::Relaxed), Zone::Local);
    let s = &fx.store;
    let enqueue = |key| s.job_command(&json!({"action":"enqueue","kind":"workflow.title","input_ref":key,"config_version":"SYNTHETIC/1"})).unwrap();
    let history = enqueue("old");
    s.job_command(&json!({"action":"cancel","id":history["id"]})).unwrap();
    let queued = enqueue("queued");
    let running = s.job_command(&json!({"action":"claim","owner":"SYNTHETIC","kind":"workflow.title","lease_ms":1000})).unwrap();
    assert_eq!(running["id"], queued["id"]);
    let later = enqueue("later");
    s.raw_db().script(&format!("UPDATE jobs SET next_run_ms={} WHERE job_id='{}'", now.load(Ordering::Relaxed) + 500, later["id"].as_str().unwrap())).unwrap();
    assert!(s.pending_workflow_jobs().unwrap()["jobs"].as_array().unwrap().is_empty());
    assert_eq!(s.next_workflow_deadline().unwrap(), Some(now.load(Ordering::Relaxed) + 500));
    now.fetch_add(500, Ordering::Relaxed);
    let jobs = s.pending_workflow_jobs().unwrap();
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(jobs["jobs"][0]["id"], later["id"]);
    now.fetch_add(500, Ordering::Relaxed);
    assert_eq!(s.pending_workflow_jobs().unwrap()["jobs"].as_array().unwrap().len(), 2);
}

#[test]
fn unchanged_wait_reason_does_not_dirty_the_queue() {
    let fx = Fx::make("performance-wait");
    let s = &fx.store;
    let j = s.job_command(&json!({"action":"enqueue","kind":"workflow.title","input_ref":"SYNTHETIC","config_version":"fixed/1:x"})).unwrap();
    s.workflow_wait(j["id"].as_str().unwrap(), "waiting_model").unwrap();
    let revision = s.background_revision("jobs").unwrap();
    for _ in 0..20 { s.workflow_wait(j["id"].as_str().unwrap(), "waiting_model").unwrap(); }
    assert_eq!(s.background_revision("jobs").unwrap(), revision);
    s.workflow_wait(j["id"].as_str().unwrap(), "waiting_authorization").unwrap();
    assert!(s.background_revision("jobs").unwrap() > revision);
}

#[test]
fn claiming_a_prepared_job_does_not_claim_an_earlier_blocked_job() {
    let fx = Fx::make("performance-specific-claim");
    let s = &fx.store;
    let first = s.job_command(&json!({"action":"enqueue","kind":"workflow.title","input_ref":"SYNTHETIC-blocked","config_version":"fixed/1:x"})).unwrap();
    let second = s.job_command(&json!({"action":"enqueue","kind":"workflow.title","input_ref":"SYNTHETIC-ready","config_version":"fixed/1:x"})).unwrap();
    s.workflow_wait(first["id"].as_str().unwrap(), "waiting_model").unwrap();
    let claimed = s.job_command(&json!({"action":"claim","id":second["id"],"kind":"workflow.title","owner":"SYNTHETIC","lease_ms":30000})).unwrap();
    assert_eq!(claimed["id"], second["id"]);
    assert_eq!(s.job_command(&json!({"action":"get","id":first["id"]})).unwrap()["state"], "queued");
    assert!(s.job_command(&json!({"action":"claim","id":first["id"],"kind":"speech.recognize","owner":"SYNTHETIC"})).unwrap().is_null());
}

#[test]
fn unrelated_connector_health_and_model_changes_do_not_invalidate_media_projection() {
    let fx=Fx::make("performance-media-domain");let s=&fx.store;
    let agent=s.register_source("agent.transcript","SYNTHETIC connector",&SourcePolicy::default()).unwrap();
    let media=s.register_source("mac.microphone","资料库",&SourcePolicy::new(true,true)).unwrap();
    let before=s.background_revision("media_projection").unwrap();
    for status in ["error","active","error"] {
        s.set_source_status(&agent,status,None).unwrap();
        s.raw_db().script("INSERT OR REPLACE INTO store_meta VALUES('models.config','SYNTHETIC changed')").unwrap();
        s.prepare_media(None).unwrap();
    }
    assert_eq!(s.background_revision("media_projection").unwrap(),before);
    s.set_source_status(&media,"revoked",None).unwrap();
    assert!(s.background_revision("media_projection").unwrap()>before);
    let plan=s.raw_db().query("EXPLAIN QUERY PLAN SELECT event_id FROM events WHERE kind='speech.segment' AND json_extract(payload_json,'$.native_projection_owner')='SYNTHETIC'",&[],|r|r.text(3)).unwrap().join("\n");
    assert!(plan.contains("events_projection_owner"),"{plan}");
}
