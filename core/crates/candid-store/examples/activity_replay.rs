//! Offline replay of activity-block summaries on a COPY of a real library (never the live one).
//!
//! ```text
//! cargo run -p candid-store --example activity_replay -- select <copy-root> <hours-back> <max>
//! ```
//! Builds the blocks of the last `<hours-back>` hours, then prints the model input (curated layer only) of up to
//! `<max>` blocks that have messages or excerpts, plus the rule text of blocks that have none. Run each input
//! through a model by hand and compare; nothing here calls one.

use candid_store::*;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("select") || args.len() < 5 {
        eprintln!("usage: activity_replay select <root> <hours> <max>");
        return;
    }
    let store = TimelineStore::open(&args[2], "candid/activity-replay", false, Clock::default()).expect("open copy");
    let hours: i64 = args[3].parse().unwrap();
    let max: usize = args[4].parse().unwrap();
    let mut config = store.feature_settings().unwrap();
    config["items"]["screen-episodes"]["enabled"] = json!(true);
    store.workflow_command(&json!({"action": "features", "config": config})).unwrap();
    let _ = store.curated_sync();
    store.screen_command(&json!({"action": "rebuild"})).unwrap();
    let _ = hours;
    let status = store.screen_command(&json!({"action": "status"})).unwrap();
    let (mut shown, mut bare) = (0, 0);
    for e in status["episodes"].as_array().unwrap() {
        let id = e["id"].as_str().unwrap();
        let p = store.activity_preview(id).unwrap();
        if p["has_curated"] == true && shown < max {
            shown += 1;
            println!("=== {id}\n{}\n--- rule: {} | {}\n", p["content"].as_str().unwrap(), p["rule_title"], p["rule_summary"]);
        } else if p["has_curated"] != true {
            bare += 1;
            if bare <= 2 {
                println!("=== (no curated content, no model call) {id}\nrule title: {}\nrule summary: {}\n", p["rule_title"], p["rule_summary"]);
            }
        }
    }
    println!("blocks: {} with curated content shown {shown}, without {bare}", status["episodes"].as_array().unwrap().len());
}
