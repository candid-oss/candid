//! Offline replay of screenshot excerpts, for tuning the prompt on a COPY of a real library (never the live one).
//!
//! ```text
//! cargo run -p candid-store --example excerpt_replay -- select <copy-root> <hours-back> <apps,comma|*> <max> > inputs.json
//! # run each input through a vision model, writing [{"shot": .., "output": "<raw model reply>", "model": ..}]
//! cargo run -p candid-store --example excerpt_replay -- apply <copy-root> outputs.json
//! ```
//!
//! `select` resets the excerpt cursor of the copy, runs the same keyframe selection as the background scan over
//! the last `<hours-back>` hours, and prints the prompt, text part and image path of each chosen screenshot.
//! `apply` merges the replies as the job would and prints the resulting conversations and excerpts.

use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::{json, Value};

fn open(root: &str) -> TimelineStore {
    TimelineStore::open(root, "candid/excerpt-replay", false, Clock::default()).expect("open copy")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("select") => select(&args[2], args[3].parse().unwrap(), &args[4], args[5].parse().unwrap()),
        Some("apply") => apply(&args[2], &args[3]),
        Some("ocr") => ocr(&args[2], &args[3]),
        Some("compose") => {
            let store = open(&args[2]);
            let r = store.compose_prepare(&json!({"instruction": args[5], "app": args[3], "title": args[4]})).unwrap();
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
        _ => eprintln!("usage: excerpt_replay select <root> <hours> <apps|*> <max> | apply <root> <outputs.json>"),
    }
}

fn select(root: &str, hours: i64, apps: &str, max: usize) {
    let store = open(root);
    let db = store.raw_db();
    let mut config = store.feature_settings().unwrap();
    config["items"]["screen-excerpt"]["enabled"] = json!(true);
    config["items"]["screen-excerpt"]["daily_calls"] = json!(10000);
    store.workflow_command(&json!({"action": "features", "config": config})).unwrap();
    let now = chrono_now();
    db.execute("DELETE FROM store_meta WHERE key LIKE 'excerpt.%'", &[]).unwrap();
    db.execute("DELETE FROM jobs WHERE kind='screen.excerpt'", &[]).unwrap();
    db.execute(&format!("INSERT INTO store_meta(key,value) VALUES('excerpt.automatic-since/1','{}')", now - hours * 3_600_000), &[]).unwrap();
    loop {
        let before = db.first("SELECT value FROM store_meta WHERE key='excerpt.cursor/1'", &[], |r| r.text(0)).unwrap();
        store.excerpt_scan().unwrap();
        let after = db.first("SELECT value FROM store_meta WHERE key='excerpt.cursor/1'", &[], |r| r.text(0)).unwrap();
        if before == after {
            break;
        }
    }
    let wanted: Vec<&str> = apps.split(',').collect();
    let shots = db
        .query("SELECT j.input_ref,e.app_name FROM jobs j JOIN events e ON e.event_id=j.input_ref WHERE j.kind='screen.excerpt' ORDER BY e.start_ms", &[], |r| (r.text(0), r.opt_text(1).unwrap_or_default()))
        .unwrap();
    let total = shots.len();
    let chosen: Vec<String> = shots.into_iter().filter(|(_, app)| wanted == ["*"] || wanted.contains(&app.as_str())).map(|(s, _)| s).collect();
    let picked: Vec<Value> = chosen.iter().rev().take(max).rev().map(|s| {
        let mut p = store.excerpt_parts(s).unwrap();
        p["shot"] = json!(s);
        p
    }).collect();
    eprintln!("keyframes in window: {total}; matching apps: {}; printed: {}", chosen.len(), picked.len());
    println!("{}", serde_json::to_string_pretty(&picked).unwrap());
}

fn apply(root: &str, outputs: &str) {
    let store = open(root);
    let items: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(outputs).unwrap()).unwrap();
    let mut failed = 0;
    for item in &items {
        let shot = item["shot"].as_str().unwrap();
        match store.excerpt_apply_raw(shot, item["output"].as_str().unwrap_or(""), item["model"].as_str().unwrap_or("replay")) {
            Ok(r) => eprintln!("{shot}: {r}"),
            Err(e) => {
                failed += 1;
                eprintln!("{shot}: ERROR {}", e.message())
            }
        }
    }
    eprintln!("applied {} ({failed} failed)", items.len());
    let client = store.internal_client("excerpt-replay", &ClientScope::unbounded()).unwrap();
    let call = |tool: &str, body: Value| store.call_tool(tool, body.to_string().as_bytes(), &client).map_err(|_| tool.to_string()).unwrap();
    let list = call("conversations", json!({"kind": "chat", "limit": 20}));
    for c in list["conversations"].as_array().unwrap() {
        println!("\n### {} · {} ({} 条)", c["app"].as_str().unwrap_or(""), c["title"].as_str().unwrap_or(""), c["messages"]);
        let msgs = call("conversation", json!({"conversation_id": c["conversation_id"], "limit": 40}));
        for m in msgs["messages"].as_array().unwrap() {
            let who = if m["actor"] == "self" { "我".to_string() } else { m["sender"].as_str().unwrap_or("对方").to_string() };
            println!("  {}  {}：{}", &m["time"].as_str().unwrap_or("")[11..16], who, m["text"].as_str().unwrap_or("").replace('\n', " "));
        }
    }
    let excerpts = store.raw_db().query("SELECT app,title,substr(text,1,160) FROM excerpts ORDER BY start_ms DESC LIMIT 10", &[], |r| (r.opt_text(0), r.opt_text(1), r.text(2))).unwrap();
    for (app, title, body) in excerpts {
        println!("\n=== 摘录 {} · {}\n  {}", app.unwrap_or_default(), title.unwrap_or_default(), body.replace('\n', " "));
    }
}

/// Adds OCR to screenshots of the copy, as the App would: `{"<image path>": "[left-right,top] text\n..."}`.
fn ocr(root: &str, file: &str) {
    let store = open(root);
    let db = store.raw_db();
    let map: serde_json::Map<String, Value> = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let mut added = 0;
    for (path, body) in map {
        let rel = path.split("/blobs/").nth(1).map(|r| format!("blobs/{r}")).unwrap();
        let Some((source, key, at, app)) = db
            .first("SELECT e.source_id,e.source_key,e.start_ms,e.app_name FROM events e JOIN event_blobs eb ON eb.event_id=e.event_id JOIN blobs b ON b.sha256=eb.sha256 WHERE b.rel_path=? AND e.kind='screen.shot'", &[candid_store::testing::rusqlite::types::Value::Text(rel).into()], |r| (r.text(0), r.text(1), r.opt_int(2), r.opt_text(3)))
            .unwrap()
        else {
            continue;
        };
        let (mut texts, mut boxes) = (vec![], vec![]);
        for line in body.as_str().unwrap_or("").lines() {
            let Some((head, text)) = line.split_once("] ") else { continue };
            let nums: Vec<i64> = head.trim_start_matches('[').split(|c| c == '-' || c == ',').filter_map(|n| n.parse().ok()).collect();
            if nums.len() != 3 { continue; }
            texts.push(text.to_string());
            boxes.push(json!([nums[0], nums[2], nums[1] - nums[0], 12]));
        }
        let mut e = IngestEvent::new(&format!("replay-ocr:{key}"), "image.text", at);
        e.parent_key = Some(key.clone());
        e.app_name = app;
        e.payload = json!({"image_event_id": "replay", "method": "ocr", "model_id": "Apple Vision (replay)", "boxes": boxes});
        e.text = vec![IngestText::new("ocr", &texts.join("\n"))];
        if store.ingest(&IngestBatch::events(&source, vec![e])).is_ok() {
            added += 1;
        }
    }
    eprintln!("added OCR to {added} screenshots");
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}
