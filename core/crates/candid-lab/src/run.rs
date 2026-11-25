//! `run`: clone a pack, configure it from the experiment file, drive the production background worker until it
//! settles, then dump the result.

use crate::proxy::Proxy;
use candid_service::router::SharedStore;
use candid_store::testing::rusqlite::types::Value as P;
use candid_store::{Clock, TimelineStore, Zone};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub fn parse_time(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).unwrap_or_else(|_| panic!("bad time {s}")).timestamp_millis()
}

pub fn lab_dir() -> std::path::PathBuf {
    std::env::var_os("CANDID_LAB").map(Into::into).unwrap_or_else(|| Path::new(&std::env::var("HOME").unwrap()).join("CandidLab"))
}

fn sh(cmd: &str, args: &[&str]) {
    let ok = std::process::Command::new(cmd).args(args).status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "{cmd} {args:?} failed");
}

/// Features the experiment does not name are switched off, so nothing else calls a model.
const RESETS: &[(&str, &[&str])] = &[
    ("curated", &[
        "DELETE FROM items", "DELETE FROM messages", "DELETE FROM conversations", "DELETE FROM excerpts",
        "DELETE FROM store_meta WHERE key LIKE 'curated.%' OR key LIKE 'items.%'",
    ]),
    ("excerpt", &[
        "DELETE FROM store_meta WHERE key LIKE 'excerpt.%' OR key LIKE 'screen.excerpt/%'",
        "DELETE FROM jobs WHERE kind='screen.excerpt'",
    ]),
    ("items", &[
        "DELETE FROM items", "DELETE FROM memories WHERE kind='fact'",
        "DELETE FROM store_meta WHERE key LIKE 'items.%' OR key LIKE 'memo.%'",
        "DELETE FROM jobs WHERE kind='items.extract'",
    ]),
    ("people", &[
        "DELETE FROM store_meta WHERE key LIKE 'people.%'",
        "DELETE FROM jobs WHERE kind='people.judge'",
    ]),
    ("screen", &[
        "DELETE FROM episodes WHERE episode_id GLOB 'screen:*'",
        "DELETE FROM store_meta WHERE key LIKE 'screen.%'",
        "DELETE FROM jobs WHERE kind='screen.summarize'",
    ]),
];

fn signature(s: &TimelineStore) -> String {
    s.raw_db().first(
        "SELECT (SELECT COUNT(*) FROM ai_calls)||'/'||(SELECT COUNT(*) FROM jobs WHERE state IN ('queued','running'))||'/'||(SELECT COUNT(*) FROM jobs WHERE state='done')||'/'||(SELECT COUNT(*) FROM messages)||'/'||(SELECT COUNT(*) FROM items)||'/'||(SELECT COUNT(*) FROM episodes)",
        &[], |r| r.get::<_, String>(0).unwrap(),
    ).unwrap().unwrap_or_default()
}

pub fn run(pack: &Path, exp_path: &Path, out: &Path) {
    let exp: Value = serde_json::from_str(&std::fs::read_to_string(exp_path).expect("experiment file")).expect("experiment json");
    if out.exists() {
        panic!("{} exists; pick a new run name", out.display());
    }
    std::fs::create_dir_all(out).unwrap();
    std::fs::copy(exp_path, out.join("experiment.json")).unwrap();
    let root = out.join("root");
    let t0 = Instant::now();
    sh("cp", &["-cR", pack.to_str().unwrap(), root.to_str().unwrap()]);
    eprintln!("cloned pack in {:.1}s", t0.elapsed().as_secs_f64());

    let lab = lab_dir();
    let log = Arc::new(Mutex::new(std::fs::File::create(out.join("calls.jsonl")).unwrap()));
    let proxy = Proxy {
        upstream: exp["upstream"].as_str().unwrap_or("http://127.0.0.1:1234/v1").into(),
        token: std::fs::read_to_string(lab.join(exp["token_file"].as_str().unwrap_or("lmstudio.token"))).ok().map(|s| s.trim().to_string()),
        cache: lab.join("cache"),
        offline: exp["offline"] == true,
        request: exp.get("request").cloned().unwrap_or(json!({})),
        model_request: exp.get("model_request").cloned().unwrap_or(json!({})),
        log,
        misses: Some(out.join("misses")),
    };
    let port = proxy.start();

    let from = parse_time(exp["window"]["from"].as_str().expect("window.from"));
    let until = parse_time(exp["window"]["until"].as_str().expect("window.until"));
    let now = Arc::new(AtomicI64::new(until));
    let clock_now = now.clone();
    let store = TimelineStore::open(&root, "candid/lab", false, Clock::with(move || clock_now.load(Ordering::SeqCst), Zone::Local)).expect("open clone");

    // Prompt overrides: <root>/prompts/<name> is read by the steps that support local overrides.
    if let Some(prompts) = exp["prompts"].as_object() {
        std::fs::create_dir_all(root.join("prompts")).unwrap();
        for (name, file) in prompts {
            let src = exp_path.parent().unwrap().join(file.as_str().unwrap());
            std::fs::copy(&src, root.join("prompts").join(name)).unwrap_or_else(|_| panic!("prompt {}", src.display()));
        }
    }

    // Models: one loopback provider (no key, no approval), the experiment's roles; engine roles (OCR) kept.
    let old = store.model_command(&json!({"action": "get"})).unwrap();
    let mut roles: Vec<Value> = old["roles"].as_array().into_iter().flatten().filter(|r| r["engine"].is_string()).cloned().collect();
    let mut models: Vec<Value> = vec![];
    for (role, model) in exp["roles"].as_object().expect("roles") {
        let id = model.as_str().unwrap();
        roles.push(json!({"role": role, "model": {"provider_id": "lab", "model_id": id}, "fallback_json": [], "limits_json": {"timeout_ms": 120000, "max_output_tokens": 4096}}));
        if !models.iter().any(|m| m["model_id"] == id) {
            models.push(json!({"provider_id": "lab", "model_id": id, "modalities_json": ["text", "image"]}));
        }
    }
    // `remote`: the provider looks like a remote service to the store (not 127.0.0.1/localhost), so remote-only
    // behaviour runs: source authorization, images sent as WebP. `lab.localhost` still resolves to this machine.
    let remote = exp["remote"] == true;
    let host = if remote { "lab.localhost" } else { "127.0.0.1" };
    let config = json!({"version": 1, "roles": roles, "models": models, "providers": [{
        "provider_id": "lab", "name": "lab", "api": "openai-chat", "base_url": format!("http://{host}:{port}/v1"),
        "destination": "self_hosted", "enabled": true, "headers_json": {}}]});
    store.model_command(&json!({"action": "save", "config": config})).expect("save model config");

    if remote {
        let mut automation = serde_json::to_value(store.automation_config().unwrap()).unwrap();
        automation["remote_sources"] = json!(store.raw_db().query("SELECT source_id FROM sources", &[], |r| r.get::<_, String>(0).unwrap()).unwrap());
        store.automation_command(&json!({"action": "config", "config": automation})).expect("authorize sources for the remote provider");
    }
    let mut features = store.feature_settings().unwrap();
    let wanted = exp["features"].as_object().cloned().unwrap_or_default();
    for (name, f) in features["items"].as_object_mut().unwrap() {
        f["enabled"] = json!(wanted.get(name) == Some(&json!(true)));
        f["daily_calls"] = json!(10000);
        f["allow_remote"] = json!(remote && wanted.get(name) == Some(&json!(true)));
    }
    store.workflow_command(&json!({"action": "features", "config": features})).expect("features");
    let db = store.raw_db();
    db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('workflows.settings/1','{\"automatic\":false,\"titles\":false,\"summary\":false,\"version\":1}')", &[]).unwrap();

    for name in exp["reset"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let (_, sqls) = RESETS.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("unknown reset {name}"));
        for sql in *sqls {
            if let Err(e) = db.execute(sql, &[]) {
                eprintln!("reset {name}: {sql}: {}", e.message());
            }
        }
    }
    // Hidden switches for experiments, e.g. {"excerpt.chat-batch/1": false}.
    for (key, value) in exp["meta"].as_object().into_iter().flatten() {
        db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[P::Text(key.clone()), P::Text(value.to_string())]).unwrap();
    }
    db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('excerpt.automatic-since/1',?)", &[P::Text(from.to_string())]).unwrap();
    db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('screen.automatic-since/1',?)", &[P::Text(from.to_string())]).unwrap();

    // Projections that need no model: bring them up to date first, so the item floor can sit at the window.
    let t1 = Instant::now();
    for _ in 0..10_000 {
        if store.curated_sync().unwrap()["changed"] != true {
            break;
        }
    }
    let _ = store.people_sync();
    eprintln!("projections in {:.1}s", t1.elapsed().as_secs_f64());
    let floor = if exp["items_floor"] == "zero" { 0 } else {
        db.scalar("SELECT COALESCE(MAX(seq),0) FROM messages WHERE sent_ms<?", &[P::Integer(from)]).unwrap()
    };
    db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('items.floor/1',?)", &[P::Text(floor.to_string())]).unwrap();

    let shared: SharedStore = Arc::new(Mutex::new(store));
    let max_ticks = exp["max_ticks"].as_u64().unwrap_or(20_000);
    let max_idle = exp["max_idle"].as_u64().unwrap_or(4);
    let step_ms = exp["idle_advance_min"].as_i64().unwrap_or(11) * 60_000;
    let (mut idle, mut ticks) = (0, 0);
    let t2 = Instant::now();
    while ticks < max_ticks && idle <= max_idle {
        let before = signature(&shared.lock().unwrap());
        candid_service::workflows::tick(&shared, &|| {});
        ticks += 1;
        let s = shared.lock().unwrap();
        // The pack holds records past the window; screenshot work outside it is dropped, not run.
        s.raw_db().execute(
            "UPDATE jobs SET state='cancelled',error_code='lab_outside_window' WHERE kind='screen.excerpt' AND state='queued' AND input_ref IN (SELECT event_id FROM events WHERE kind='screen.shot' AND start_ms>?)",
            &[P::Integer(until)],
        ).unwrap();
        // Optional focus: only these apps' screenshots are read (e.g. chat clients), to keep comparisons short.
        if let Some(apps) = exp["excerpt_apps"].as_array() {
            let list: Vec<P> = apps.iter().filter_map(Value::as_str).map(|a| P::Text(a.into())).collect();
            s.raw_db().execute(
                &format!("UPDATE jobs SET state='cancelled',error_code='lab_app_filtered' WHERE kind='screen.excerpt' AND state='queued' AND input_ref IN (SELECT event_id FROM events WHERE kind='screen.shot' AND COALESCE(app_name,'') NOT IN ({}))", candid_store::testing::placeholders(list.len())),
                &list,
            ).unwrap();
        }
        let after = signature(&s);
        if after != before {
            idle = 0;
        } else {
            idle += 1;
            let t = now.load(Ordering::SeqCst);
            let next = s.raw_db().first("SELECT MIN(next_run_ms) FROM jobs WHERE state='queued' AND next_run_ms>?", &[P::Integer(t)], |r| r.get::<_, Option<i64>>(0).unwrap()).unwrap().flatten();
            now.store(next.map(|n| n + 1).filter(|n| *n < t + step_ms).unwrap_or(t + step_ms), Ordering::SeqCst);
        }
        if ticks % 25 == 0 {
            eprintln!("tick {ticks}: {after} (calls/active/done/messages/items/episodes) {:.0}s", t2.elapsed().as_secs_f64());
        }
    }
    eprintln!("settled after {ticks} ticks in {:.0}s", t2.elapsed().as_secs_f64());
    let s = shared.lock().unwrap();
    let dump = crate::dump::dump(&s, from, until);
    std::fs::write(out.join("dump.json"), serde_json::to_string_pretty(&dump).unwrap()).unwrap();
    std::fs::write(out.join("report.md"), crate::dump::report(&dump, &out.join("calls.jsonl"))).unwrap();
    eprintln!("wrote {}", out.display());
}
