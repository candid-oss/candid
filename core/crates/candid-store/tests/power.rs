//! The power gate (power v1 §2.1, §3.1, §9.1): pace, report, staleness, thermal and Low Power Mode, the manual
//! bypass, the app's own work, 「现在处理」 and the energy ledger. Synthetic data, fixed clock.
mod common;
use candid_store::fixtures::{temp_root, test_clock};
use candid_store::job_priority::Cost;
use candid_store::*;
use common::Fx;
use serde_json::json;

fn unplugged() -> (TimelineStore, std::path::PathBuf) {
    let root = temp_root("power");
    // Opened directly, not through the test support (which declares external power).
    (TimelineStore::open(&root, "candid/test", true, test_clock()).unwrap(), root)
}

fn code(r: Result<()>) -> Option<String> {
    r.err().map(|e| e.code())
}

#[test]
fn unknown_power_counts_as_battery_and_only_heavy_work_waits_in_saver() {
    let (s, root) = unplugged();
    let status = s.power_status().unwrap();
    assert_eq!((status["pace"].as_str(), status["power_state"].as_str(), status["stale"].as_bool()), (Some("saver"), Some("unknown"), Some(true)));
    assert_eq!(code(s.power_allows(Cost::Heavy, false)).as_deref(), Some("waiting_power"));
    assert_eq!(code(s.power_allows(Cost::ByModel, false)).as_deref(), Some("waiting_power"), "unresolved counts as heavy");
    assert_eq!(code(s.power_allows(Cost::Light, false)), None);
    assert_eq!(code(s.power_allows(Cost::Heavy, true)), None, "a manual run passes");
    let r = s.power_report(&json!({"external_power": true, "battery_percent": 50, "low_power_mode": false, "thermal": "nominal", "idle_seconds": 3})).unwrap();
    assert_eq!((r["power_state"].as_str(), r["heavy_allowed"].as_bool(), r["changed"].as_bool()), (Some("ac"), Some(true), Some(true)));
    assert_eq!(s.power_report(&json!({"external_power": true, "idle_seconds": 60})).unwrap()["changed"], json!(false), "same decision");
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reports_go_stale_after_ten_minutes_and_declared_states_do_not() {
    let root = temp_root("power-stale");
    let now = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(fixtures::FIXED_NOW_MS));
    let clock = { let now = now.clone(); Clock::with(move || now.load(std::sync::atomic::Ordering::SeqCst), Zone::named("UTC").unwrap()) };
    let s = TimelineStore::open(&root, "candid/test", true, clock).unwrap();
    s.power_report(&json!({"external_power": true})).unwrap();
    now.fetch_add(10 * 60_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(s.power_status().unwrap()["power_state"], "ac");
    now.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    assert_eq!((s.power_status().unwrap()["power_state"].as_str(), s.power_status().unwrap()["reason"].as_str()), (Some("unknown"), Some("waiting_power")));
    s.power_declare(Some(true)).unwrap();
    now.fetch_add(86_400_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(s.power_status().unwrap()["power_state"], "ac");
    // A core started without --power forgets an earlier declaration.
    s.power_declare(None).unwrap();
    assert_eq!(s.power_status().unwrap()["power_state"], "unknown");
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn thermal_low_power_and_manual_pace_follow_the_table() {
    let (s, root) = unplugged();
    s.power_report(&json!({"external_power": true, "thermal": "serious"})).unwrap();
    s.power_set_pace("anytime").unwrap();
    assert_eq!(code(s.power_allows(Cost::Heavy, false)).as_deref(), Some("waiting_thermal"), "heat holds heavy work in every pace");
    assert_eq!(code(s.power_allows(Cost::Light, false)), None);
    s.power_report(&json!({"external_power": false, "thermal": "fair", "low_power_mode": true})).unwrap();
    assert_eq!(code(s.power_allows(Cost::Heavy, false)).as_deref(), Some("waiting_power"), "随时 with Low Power Mode acts as 省电");
    s.power_report(&json!({"external_power": false, "thermal": "fair", "low_power_mode": false})).unwrap();
    assert_eq!(code(s.power_allows(Cost::Heavy, false)), None, "随时 runs on battery");
    s.power_set_pace("manual").unwrap();
    for cost in [Cost::Heavy, Cost::Light, Cost::ByModel] {
        assert_eq!(code(s.power_allows(cost, false)).as_deref(), Some("waiting_manual"));
        assert_eq!(code(s.power_allows(cost, true)), None);
    }
    assert!(s.power_set_pace("nightly").is_err());
    assert!(s.power_report(&json!({"thermal": "hot"})).is_err());
    assert!(s.power_report(&json!({"battery_percent": 101})).is_err());
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_app_asks_by_registered_kind_and_its_runs_are_accounted() {
    let (s, root) = unplugged();
    let allows = |v: serde_json::Value| s.power_command(&v).unwrap();
    assert_eq!(allows(json!({"action": "allows", "kind": "app.vision", "local": true}))["reason"], "waiting_power");
    assert_eq!(allows(json!({"action": "allows", "kind": "app.vision", "local": false}))["allowed"], true);
    assert_eq!(allows(json!({"action": "allows", "kind": "app.vision", "local": true, "manual": true}))["allowed"], true);
    assert_eq!(allows(json!({"action": "allows", "kind": "app.ocr"}))["allowed"], true, "system OCR is light");
    assert!(s.power_command(&json!({"action": "allows", "kind": "app.unregistered"})).is_err());
    s.app_work_account("app.ocr", "shot-1", 120, 300).unwrap();
    s.power_command(&json!({"action": "account", "kind": "app.ocr", "subject": "shot-1", "cpu_ms": 30, "wall_ms": 100})).unwrap();
    let client = s.register_client("SYNTHETIC agent", "mcp", &ClientScope::unbounded(), None).unwrap().client_id;
    let summary = s.energy_summary(&json!({}), &client).unwrap();
    let kinds = &summary["days"][0]["kinds"];
    assert_eq!((kinds[0]["kind"].as_str(), kinds[0]["cpu_seconds"].as_f64(), kinds[0]["wall_seconds"].as_f64(), kinds[0]["battery_cpu_seconds"].as_f64()), (Some("app.ocr"), Some(0.2), Some(0.4), Some(0.2)));
    assert!(s.app_work_account("screen.excerpt", "x", 1, 1).is_err(), "only app kinds");
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn run_now_marks_queued_jobs_and_the_queue_skips_held_work() {
    let fx = Fx::make("power-queue");
    let s = &fx.store;
    s.power_declare(None).unwrap();
    let job = |kind: &str, input: &str| s.job_command(&json!({"action": "enqueue", "kind": kind, "input_ref": input, "config_version": "v"})).unwrap()["id"].as_str().unwrap().to_string();
    let heavy = job("speech.recording", "a");
    let remote = job("workflow.title", "b");
    // Heavy kinds are held without being prepared; jobs of model-dependent kinds still come back to be resolved.
    let queue: Vec<String> = s.pending_job_queue().unwrap().into_iter().map(|j| j.0).collect();
    assert_eq!(queue, vec![remote.clone()]);
    assert_eq!(s.job_command(&json!({"action": "get", "id": heavy})).unwrap()["error_code"], "waiting_power");
    assert_eq!(s.job_command(&json!({"action": "run_now", "id": heavy})).unwrap()["marked"], 1);
    let queue: Vec<String> = s.pending_job_queue().unwrap().into_iter().map(|j| j.0).collect();
    assert!(queue.contains(&heavy) && queue.contains(&remote), "{queue:?}");
    // 手动: only manual jobs come back; the others wait with the reason.
    s.power_set_pace("manual").unwrap();
    let queue: Vec<String> = s.pending_job_queue().unwrap().into_iter().map(|j| j.0).collect();
    assert_eq!(queue, vec![heavy.clone()]);
    assert_eq!(s.job_command(&json!({"action": "get", "id": remote})).unwrap()["error_code"], "waiting_manual");
    // Plugged in, 省电: the reasons are cleared and everything is due again.
    s.power_set_pace("saver").unwrap();
    s.power_declare(Some(true)).unwrap();
    assert_eq!(s.pending_job_queue().unwrap().len(), 2);
    assert!(s.job_command(&json!({"action": "get", "id": remote})).unwrap()["error_code"].is_null());
    assert!(s.job_command(&json!({"action": "run_now"})).is_err());
}
