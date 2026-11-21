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
//!
//! Item extraction (docs/plans/data-layer-v1.md §2.2) is replayed the same way, on the conversations already in the copy:
//!
//! ```text
//! cargo run -p candid-store --example excerpt_replay -- extract select <copy-root> <days-back> <chat,mail,meeting|*> <max> <last-n> > inputs.json
//! # answer each input with the `summary` model (system = prompt, user = content), writing
//! #   [{"conversation_id": .., "last": <last-n>, "output": "<raw model reply>", "model": ..}]
//! cargo run -p candid-store --example excerpt_replay -- extract apply <copy-root> outputs.json
//! ```
//!
//! `select` switches the feature on in the copy, recomputes people and prints what the model would read for the last
//! `<last-n>` messages of the most recent conversations (prompt, input text, cache key). `apply` runs each reply
//! through the same checks as the job and prints the resulting items and memos.

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
        Some("extract") if args.get(2).map(String::as_str) == Some("select") => extract_select(&args[3], args[4].parse().unwrap(), &args[5], args[6].parse().unwrap(), args[7].parse().unwrap()),
        Some("extract") if args.get(2).map(String::as_str) == Some("apply") => extract_apply(&args[3], &args[4]),
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

fn extract_select(root: &str, days: i64, kinds: &str, max: usize, last: i64) {
    let store = open(root);
    let db = store.raw_db();
    let mut config = store.feature_settings().unwrap();
    config["items"]["item-extract"]["enabled"] = json!(true);
    config["items"]["item-extract"]["daily_calls"] = json!(10000);
    store.workflow_command(&json!({"action": "features", "config": config})).unwrap();
    // The projection moves one batch per call; run it to the end on the copy.
    while store.curated_sync().unwrap()["changed"] == true {}
    store.people_sync().unwrap();
    let wanted: Vec<&str> = kinds.split(',').collect();
    let since = chrono_now() - days * 86_400_000;
    let rows = db
        .query(
            "SELECT c.conversation_id,c.kind,c.app,c.title,COUNT(m.seq),MAX(m.sent_ms) FROM conversations c JOIN messages m ON m.conversation_id=c.conversation_id WHERE m.sent_ms>=? GROUP BY c.conversation_id HAVING COUNT(m.seq)>=2 ORDER BY MAX(m.sent_ms) DESC",
            &[candid_store::testing::rusqlite::types::Value::Integer(since)],
            |r| (r.text(0), r.text(1), r.opt_text(2), r.opt_text(3), r.int(4)),
        )
        .unwrap();
    let mut picked = vec![];
    for (id, kind, app, title, count) in rows {
        if (wanted != ["*"] && !wanted.contains(&kind.as_str())) || picked.len() >= max {
            continue;
        }
        let Ok(mut p) = store.items_preview(&id, last) else { continue };
        p["kind"] = json!(kind);
        p["app"] = json!(app);
        p["title"] = json!(title);
        p["messages"] = json!(count);
        p["last"] = json!(last);
        picked.push(p);
    }
    eprintln!("conversations printed: {}", picked.len());
    println!("{}", serde_json::to_string_pretty(&picked).unwrap());
}

fn extract_apply(root: &str, outputs: &str) {
    let store = open(root);
    let items: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(outputs).unwrap()).unwrap();
    for item in &items {
        let id = item["conversation_id"].as_str().unwrap();
        match store.items_apply_raw(id, item["last"].as_i64().unwrap_or(10), item["output"].as_str().unwrap_or(""), item["model"].as_str().unwrap_or("replay")) {
            Ok(r) => println!("{id}: {r}"),
            Err(e) => println!("{id}: ERROR {}", e.message()),
        }
    }
    let client = store.internal_client("excerpt-replay", &ClientScope::unbounded()).unwrap();
    let all = store.call_tool("items", json!({"state": "all", "limit": 100}).to_string().as_bytes(), &client).map_err(|_| "items").unwrap();
    for i in all["items"].as_array().unwrap() {
        println!(
            "\n[{}] {} · {} · {}\n  时间原话：{}  开始：{}  对方：{}",
            i["kind"].as_str().unwrap_or(""),
            i["title"].as_str().unwrap_or(""),
            i["review"].as_str().unwrap_or(""),
            i["state"].as_str().unwrap_or(""),
            i["when_text"].as_str().unwrap_or("-"),
            i["start"].as_str().unwrap_or("-"),
            i["person"]["name"].as_str().unwrap_or("-")
        );
    }
    let memos = store.raw_db().query("SELECT text FROM memories WHERE kind='fact' AND status='candidate' AND producer LIKE 'item-extract/%'", &[], |r| r.text(0)).unwrap();
    for m in memos {
        println!("\n[备忘] {m}");
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
