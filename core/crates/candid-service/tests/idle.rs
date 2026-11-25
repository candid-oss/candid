//! Idle costs nothing (power v1 §3.5): the background thread sleeps until a write, an explicit wake or the next due
//! time, and a wake with nothing changed runs no SQL. The fuller check (an idle hour after the three-day fixture) is
//! the last step of `lifecycle.rs`. Synthetic data only; no network.
use candid_service::background::{self, Background, Sleep, Wake};
use candid_service::serve::{ServeOptions, Service};
use candid_store::*;
use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Shared = Arc<Mutex<TimelineStore>>;

/// A virtual clock that each sleep moves forward; records when the loop woke.
struct Virtual {
    now: Arc<AtomicI64>,
    end: i64,
    woke_at: Vec<i64>,
}

impl Sleep for Virtual {
    fn sleep(&mut self, timeout: Duration) -> bool {
        let next = (self.now.load(Ordering::SeqCst) + timeout.as_millis() as i64).min(self.end);
        self.now.store(next, Ordering::SeqCst);
        if next >= self.end || self.woke_at.len() >= 1000 {
            return false;
        }
        self.woke_at.push(next);
        true
    }
}

fn store(label: &str) -> (Shared, Arc<AtomicI64>) {
    let now = Arc::new(AtomicI64::new(fixtures::FIXED_NOW_MS));
    let clock = {
        let now = now.clone();
        Clock::with(move || now.load(Ordering::SeqCst), Zone::named("Asia/Shanghai").unwrap())
    };
    let s = TimelineStore::open(&fixtures::temp_root(label), "candid/test", true, clock).unwrap();
    // Features on, no model configured: work that arrives waits instead of calling anything.
    let mut features = s.feature_settings().unwrap();
    for f in ["screen-excerpt", "screen-episodes", "people-judge", "item-extract"] {
        features["items"][f]["enabled"] = json!(true);
        features["items"][f]["sources"] = json!(["*"]);
    }
    s.workflow_command(&json!({"action": "features", "config": features})).unwrap();
    s.register_source("mac.desktop", "SYNTHETIC Mac", &SourcePolicy::new(true, true)).unwrap();
    (Arc::new(Mutex::new(s)), now)
}

fn settle(store: &Shared, worker: &mut Background) {
    while worker.pass(store, 0, &|| {}) {}
}

#[test]
fn an_idle_hour_wakes_at_most_seven_times_and_runs_no_sql() {
    let (store, now) = store("idle-hour");
    let mut worker = Background::default();
    settle(&store, &mut worker);
    let (writes, statements) = {
        let s = store.lock().unwrap();
        (s.write_count(), s.statement_count())
    };
    let wake = Wake::default();
    let mut idle = Virtual { now: now.clone(), end: now.load(Ordering::SeqCst) + 3_600_000, woke_at: vec![] };
    background::run(&mut worker, &store, &wake, &mut idle, &|| {});
    assert!(idle.woke_at.len() <= 7, "woke {} times", idle.woke_at.len());
    let s = store.lock().unwrap();
    assert_eq!(s.write_count(), writes, "{:?}", s.write_counts());
    assert_eq!(s.statement_count(), statements, "an idle wake ran SQL");
}

#[test]
fn a_write_runs_only_the_steps_that_read_it() {
    let (store, _) = store("idle-write");
    let mut worker = Background::default();
    settle(&store, &mut worker);
    let statements = store.lock().unwrap().statement_count();
    // A setting nothing reads: the gate sees a write to store_meta, but no step depends on this key.
    store.lock().unwrap().raw_db().execute("INSERT INTO store_meta VALUES('synthetic.unrelated/1','x')", &[]).unwrap();
    let before = store.lock().unwrap().statement_count();
    settle(&store, &mut worker);
    let after = store.lock().unwrap().statement_count();
    assert!(after - before <= 2, "an unrelated key woke steps: {} statements", after - before);
    assert!(before > statements);
}

#[test]
fn a_delayed_job_wakes_the_loop_at_its_due_time_not_before() {
    let (store, now) = store("idle-due");
    let mut worker = Background::default();
    let due = now.load(Ordering::SeqCst) + 25 * 60_000;
    {
        let s = store.lock().unwrap();
        let job = s.job_command(&json!({"action":"enqueue","kind":"items.extract","feature":"item-extract","input_ref":"chat:synthetic","config_version":"synthetic/1"})).unwrap();
        s.raw_db().execute("UPDATE jobs SET next_run_ms=? WHERE job_id=?", &[testing::rusqlite::types::Value::Integer(due), testing::rusqlite::types::Value::Text(job["id"].as_str().unwrap().into())]).unwrap();
    }
    settle(&store, &mut worker);
    let wake = Wake::default();
    let mut idle = Virtual { now: now.clone(), end: now.load(Ordering::SeqCst) + 3_600_000, woke_at: vec![] };
    background::run(&mut worker, &store, &wake, &mut idle, &|| {});
    assert!(idle.woke_at.contains(&due), "the loop should wake exactly when the job is due: {:?}", idle.woke_at);
    assert!(idle.woke_at.len() <= 7, "{:?}", idle.woke_at);
    let state: String = store.lock().unwrap().raw_db().first("SELECT state||COALESCE(error_code,'') FROM jobs", &[], |r| testing::RowExt::text(r, 0)).unwrap().unwrap();
    assert_ne!(state, "queued", "the job was looked at when it came due");
}

#[test]
fn the_service_thread_sleeps_when_idle_and_wakes_on_a_write() {
    let dir = std::env::temp_dir().join(format!("candid-idle-{}", &uuid_v4()[..8]));
    let mut o = ServeOptions::new(dir.join("timeline"), dir.join("run"));
    o.created_by = "candid/test".into();
    let service = Service::start(&o, |_| {}).unwrap();
    let wake = service.core.feed.wake().clone();
    // Startup work settles quickly; then nothing should wake the thread (the old loop woke every 2 seconds).
    let settled = wait_until(Duration::from_secs(5), || {
        let n = wake.wake_count();
        std::thread::sleep(Duration::from_millis(300));
        wake.wake_count() == n
    });
    assert!(settled, "the background thread never went quiet");
    let quiet = wake.wake_count();
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(wake.wake_count(), quiet, "woke with nothing happening");
    // Any write to the library wakes it, whichever path made it.
    service.store.lock().unwrap().register_source("mac.desktop", "SYNTHETIC Mac", &SourcePolicy::new(true, true)).unwrap();
    assert!(wait_until(Duration::from_secs(5), || wake.wake_count() > quiet), "a write did not wake the thread");
    // So does an explicit wake for something outside the library.
    let after_write = wait_quiet(&wake);
    service.core.wake_background();
    assert!(wait_until(Duration::from_secs(5), || wake.wake_count() > after_write), "wake_background did not wake the thread");
    service.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

fn wait_until(limit: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_quiet(wake: &Wake) -> u64 {
    let mut n = wake.wake_count();
    wait_until(Duration::from_secs(5), || {
        std::thread::sleep(Duration::from_millis(200));
        let m = wake.wake_count();
        let quiet = m == n;
        n = m;
        quiet
    });
    n
}
