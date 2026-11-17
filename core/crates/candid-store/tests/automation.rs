mod common;
use common::Fx;
use candid_store::*;
use serde_json::{json, Value};
fn config(fx: &Fx) -> Value {
    serde_json::to_value(fx.store.automation_config().unwrap()).unwrap()
}
fn enqueue(fx: &Fx, key: &str) -> Value {
    fx.store
        .automation_command(&json!({"action":"run","skill":"daily-review","key":key}))
        .unwrap()
}

#[test]
fn notification_poll_returns_only_ready_unnotified_runs_and_redacts_revoked_sources() {
    let fx = Fx::make("automation-notification-poll");
    let event = common::text_event("SYNTHETIC-notification", fx.store.clock.now_ms(), "SYNTHETIC evidence");
    fx.store.ingest(&IngestBatch { source_id: fx.source.clone(), events: vec![event], ..Default::default() }).unwrap();
    let event_id = fx.store.event_id(&fx.source, "SYNTHETIC-notification").unwrap().unwrap();
    let mut c = config(&fx);
    c["skills"]["daily-review"]["notify"] = json!(true);
    fx.store.automation_command(&json!({"action":"config","config":c})).unwrap();
    let run = fx.store.automation_command(&json!({"action":"run","skill":"daily-review","key":"SYNTHETIC-notify","references":[{"event_id":event_id,"source_id":fx.source}]})).unwrap();
    fx.store.automation_claim().unwrap();
    let output = json!({"has_updates":true,"summary":"SYNTHETIC useful result"}).to_string();
    fx.store.automation_finish(run["id"].as_str().unwrap(), 1, "done", &json!(output), None).unwrap();
    let status = || fx.store.automation_command(&json!({"action":"status","notifications_only":true})).unwrap();
    assert_eq!(status()["runs"].as_array().unwrap().len(), 1);
    let mut policy = fx.store.sources().unwrap().into_iter().find(|s| s.source_id == fx.source).unwrap().policy;
    policy.agent_visible = false;
    fx.store.update_policy(&fx.source, &policy).unwrap();
    let hidden = status();
    assert_eq!(hidden["runs"][0]["unavailable"], true);
    assert!(hidden["runs"][0]["result"].is_null());
    assert!(hidden["runs"][0]["references"].as_array().unwrap().is_empty());
    fx.store.automation_command(&json!({"action":"notified","id":run["id"]})).unwrap();
    assert!(status()["runs"].as_array().unwrap().is_empty());
    assert_eq!(fx.store.automation_command(&json!({"action":"status"})).unwrap()["runs"].as_array().unwrap().len(), 1);
}
#[test]
fn settings_runs_restart_retry_and_immutable_revisions() {
    let mut fx = Fx::make("automation-durable");
    let id = enqueue(&fx, "manual-one")["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(enqueue(&fx, "manual-one")["id"], id);
    let run = fx.store.automation_claim().unwrap();
    assert_eq!(run["state"], "running");
    fx.store.automation_step(&id, "search", 0, &[]).unwrap();
    fx.store
        .automation_finish(&id, 1, "failed", &Value::Null, Some("offline"))
        .unwrap();
    fx.store.close();
    fx.store = common::open(&fx.root);
    let status = fx
        .store
        .automation_command(&json!({"action":"status"}))
        .unwrap();
    assert_eq!(status["runs"][0]["error"], "offline");
    assert_eq!(status["runs"][0]["steps"].as_array().unwrap().len(), 1);
    fx.store
        .automation_command(&json!({"action":"retry","id":id}))
        .unwrap();
    let second = fx.store.automation_claim().unwrap();
    assert_eq!(second["attempt"], 2);
    assert!(fx
        .store
        .automation_finish(&id, 1, "done", &json!("old"), None)
        .is_err());
    fx.store
        .automation_finish(&id, 2, "done", &json!("synthetic result"), None)
        .unwrap();
    if let Ok(dir)=std::env::var("CANDID_PROCESSING_FIXTURES") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("agent-run.v1.json"),serde_json::to_vec_pretty(&fx.store.automation_run(&id).unwrap()).unwrap()).unwrap();
    }
    assert_eq!(fx.texts("SELECT json_extract(value,'$.state') FROM store_meta WHERE key LIKE 'automation.revision/1:%:1'"),vec!["queued"]);
}
#[test]
fn disabled_budget_bad_versions_and_permission_changes_are_enforced() {
    let fx = Fx::make("automation-permissions");
    let mut c = config(&fx);
    c["skills"]["daily-review"]["enabled"] = json!(false);
    fx.store
        .automation_command(&json!({"action":"config","config":c}))
        .unwrap();
    assert!(fx
        .store
        .automation_command(&json!({"action":"run","skill":"daily-review"}))
        .unwrap_err()
        .message()
        .contains("feature_disabled"));
    c["skills"]["daily-review"]["enabled"] = json!(true);
    c["skills"]["daily-review"]["daily_runs"] = json!(0);
    fx.store
        .automation_command(&json!({"action":"config","config":c}))
        .unwrap();
    assert!(fx
        .store
        .automation_command(&json!({"action":"run","skill":"daily-review"}))
        .unwrap_err()
        .message()
        .contains("daily_run_budget"));
    c["skills"]["daily-review"]["daily_runs"] = json!(3);
    fx.store
        .automation_command(&json!({"action":"config","config":c}))
        .unwrap();
    let run = enqueue(&fx, "x");
    fx.store.automation_claim().unwrap();
    let mut policy = SourcePolicy::default();
    policy.agent_visible = false;
    fx.store.update_policy(&fx.source, &policy).unwrap();
    assert!(fx.store.automation_validate(&run).is_err());
    assert!(fx
        .store
        .automation_pi_config(run["id"].as_str().unwrap(), 1)
        .is_err());
    let status = fx
        .store
        .automation_command(&json!({"action":"status"}))
        .unwrap();
    assert_eq!(status["runs"][0]["state"], "cancelled");
    assert!(status["runs"][0]["references"]
        .as_array()
        .unwrap()
        .is_empty());
    c["version"] = json!(999);
    assert!(fx
        .store
        .automation_command(&json!({"action":"config","config":c}))
        .is_err());
}
#[test]
fn daily_hook_waits_for_measured_quality_then_queues_once() {
    let fx = Fx::make("automation-hooks");
    let mut c = config(&fx);
    c["skills"]["daily-review"]["automatic"] = json!(true);
    c["skills"]["daily-review"]["time"] = json!("00:00");
    fx.store
        .automation_command(&json!({"action":"config","config":c}))
        .unwrap();
    fx.store.automation_scan().unwrap();
    assert!(fx.store.automation_rows(100).unwrap().is_empty());
    let scope = fx
        .store
        .automation_command(&json!({"action":"evaluation_scope"}))
        .unwrap();
    let mut report = scope;
    report["format"] = json!("candid-context-evaluation/1");
    report["dataset"] = json!("SYNTHETIC tests only");
    report["queries"] = json!(30);
    report["relevant"] = json!(30);
    report["retrieved_relevant"] = json!(26);
    report["noise_checked"] = json!(30);
    report["noise_correct"] = json!(29);
    report["summary_checked"] = json!(10);
    report["summary_errors"] = json!(0);
    report["grounded"] = json!(true);
    assert_eq!(
        fx.store
            .automation_command(&json!({"action":"quality","report":report}))
            .unwrap()["passed"],
        true
    );
    fx.store.automation_scan().unwrap();
    fx.store.automation_scan().unwrap();
    assert_eq!(fx.store.automation_rows(100).unwrap().len(), 1);
    assert_eq!(
        fx.store.automation_rows(100).unwrap()[0]["policy"]["notify"],
        false
    );
}
#[test]
fn remote_scope_is_intersection_and_cannot_forge_source_references() {
    let fx = Fx::make("automation-remote");
    let mut c = config(&fx);
    c["skills"]["daily-review"]["allow_remote"] = json!(true);
    fx.store
        .automation_command(&json!({"action":"config","config":c}))
        .unwrap();
    let run = enqueue(&fx, "remote");
    fx.store.automation_claim().unwrap();
    let id = run["id"].as_str().unwrap();
    assert!(fx
        .store
        .automation_provider_allowed(id, &json!({"base_url":"https://model.example/v1"}))
        .is_err());
    assert!(fx
        .store
        .automation_provider_allowed(id, &json!({"base_url":"http://127.0.0.1:1234/v1"}))
        .is_ok());
    assert!(fx.store.automation_command(&json!({"action":"run","skill":"daily-review","references":[{"event_id":"not-an-event","source_id":fx.source}]})).is_err());
}
