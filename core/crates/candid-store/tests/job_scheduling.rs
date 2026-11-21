//! Background job order, daily limits and stale-job cleanup. Synthetic jobs only; no model.
mod common;
use candid_store::*;
use common::Fx;
use serde_json::{json, Value};
use std::sync::{atomic::{AtomicI64, Ordering}, Arc};

const START: i64 = 1_800_000_000_000;

fn clocked(label: &str) -> (Fx, Arc<AtomicI64>) {
    let mut fx = Fx::make(label);
    let now = Arc::new(AtomicI64::new(START));
    let clock = now.clone();
    fx.store.clock = Clock::with(move || clock.load(Ordering::Relaxed), Zone::Local);
    (fx, now)
}

fn enqueue(s: &TimelineStore, kind: &str, key: &str) -> Value {
    s.job_command(&json!({"action":"enqueue","kind":kind,"input_ref":key,"config_version":"SYNTHETIC/1"})).unwrap()
}

#[test]
fn background_queue_runs_by_priority_then_by_age() {
    let (fx, now) = clocked("sched-priority");
    let s = &fx.store;
    let mut ids = Vec::new();
    // Oldest first, but lower priority kinds must still come later.
    for (kind, key) in [("workflow.title", "t1"), ("screen.summarize", "s1"), ("screen.excerpt", "e1"), ("speech.recognize", "r1"), ("workflow.summary", "t2"), ("screen.excerpt", "e2")] {
        ids.push(enqueue(s, kind, key)["id"].as_str().unwrap().to_string());
        now.fetch_add(10, Ordering::Relaxed);
    }
    let order: Vec<String> = s.pending_job_queue().unwrap().into_iter().map(|(id, ..)| id).collect();
    // P1 excerpts (e1, e2), then P2 in age order (t1, r1, t2), then P3 (s1).
    let expect: Vec<&String> = [2, 5, 0, 3, 4, 1].iter().map(|i| &ids[*i]).collect();
    assert_eq!(order.iter().collect::<Vec<_>>(), expect);
    assert_eq!(job_priority::job_priority("screen.excerpt"), 1);
    assert_eq!(job_priority::job_priority("workflow.daily"), 2);
    assert_eq!(job_priority::job_priority("screen.summarize"), 3);
    // The compatibility listing returns the same order.
    let listed: Vec<String> = s.pending_workflow_jobs().unwrap()["jobs"].as_array().unwrap().iter().map(|j| j["id"].as_str().unwrap().into()).collect();
    assert_eq!(listed, order);
}

#[test]
fn unknown_kinds_are_not_background_work() {
    let (fx, _) = clocked("sched-unknown");
    enqueue(&fx.store, "something.else", "x");
    assert!(fx.store.pending_job_queue().unwrap().is_empty());
}

#[test]
fn excerpts_older_than_a_day_are_abandoned_with_a_reason() {
    let (fx, now) = clocked("sched-expire");
    let s = &fx.store;
    let old = enqueue(s, "screen.excerpt", "old");
    let title = enqueue(s, "workflow.title", "title");
    now.fetch_add(23 * 3_600_000, Ordering::Relaxed);
    let fresh = enqueue(s, "screen.excerpt", "fresh");
    assert_eq!(s.expire_stale_jobs().unwrap(), 0);
    now.fetch_add(2 * 3_600_000, Ordering::Relaxed);
    assert_eq!(s.expire_stale_jobs().unwrap(), 1);
    let get = |j: &Value| s.job_command(&json!({"action":"get","id":j["id"]})).unwrap();
    assert_eq!(get(&old)["state"], "cancelled");
    assert_eq!(get(&old)["error_code"], "expired");
    assert_eq!(get(&fresh)["state"], "queued");
    assert_eq!(get(&title)["state"], "queued", "only kinds with an age limit expire");
}

#[test]
fn daily_limit_counts_every_call_and_resets_the_next_day() {
    let (fx, now) = clocked("sched-budget");
    let s = &fx.store;
    let mut config = s.feature_settings().unwrap();
    config["items"]["screen-excerpt"]["daily_calls"] = json!(2);
    s.save_feature_settings(&config).unwrap();
    assert_eq!(s.feature_budget("screen-excerpt").unwrap(), (0, 2));
    // A local call counts the same as a remote one, and so does one that failed.
    for status in ["ok", "error"] {
        let call = s.workflow_call_start(&json!({"step":"screen-excerpt","provider":{"provider_id":"local","destination":"on_device"},"assignment":{"model":{"model_id":"m"}}})).unwrap();
        s.workflow_call_finish(&call, status, None, &Value::Null).unwrap();
    }
    assert_eq!(s.feature_budget("screen-excerpt").unwrap(), (2, 2));
    // Other features keep their own counters.
    assert_eq!(s.feature_budget("transcribe").unwrap().0, 0);
    now.fetch_add(86_400_000, Ordering::Relaxed);
    assert_eq!(s.feature_budget("screen-excerpt").unwrap(), (0, 2));
}
