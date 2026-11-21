//! Offline replay of people judgement on a COPY of a real library (never the live one).
//!
//! ```text
//! cargo run -p candid-store --example people_replay -- candidates <copy-root> [examples]
//! cargo run -p candid-store --example people_replay -- inputs <copy-root> <batches> > inputs.json
//! # run each input through a model, writing [{"pairs": [...], "output": "<raw model reply>", "model": ..}]
//! cargo run -p candid-store --example people_replay -- apply <copy-root> outputs.json
//! ```
//!
//! `candidates` brings people up to date and counts the candidate pairs by rule; `inputs` prints the prompt and the
//! model input of the first batches; `apply` stores the replies as the job would and lists what merged, what is
//! left for you, and what was answered.

use candid_store::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn open(root: &str) -> TimelineStore {
    TimelineStore::open(root, "candid/people-replay", false, Clock::default()).expect("open copy")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let store = open(args.get(2).expect("root"));
    match args.get(1).map(String::as_str) {
        Some("candidates") => {
            store.curated_sync().unwrap();
            println!("people_sync: {}", store.people_sync().unwrap());
            let found = store.people_candidates().unwrap();
            let mut by: BTreeMap<&str, usize> = BTreeMap::new();
            for c in &found {
                *by.entry(c.reason).or_default() += 1;
            }
            println!("candidate pairs: {} by rule {by:?}", found.len());
            let n: usize = args.get(3).and_then(|n| n.parse().ok()).unwrap_or(0);
            for c in found.iter().take(n) {
                println!("  [{}] {}  <->  {}", c.reason, c.a, c.b);
            }
        }
        Some("inputs") => {
            store.curated_sync().unwrap();
            store.people_sync().unwrap();
            let found = store.people_candidates().unwrap();
            let batches: usize = args[3].parse().unwrap();
            let mut out = vec![];
            for chunk in found.chunks(PEOPLE_JUDGE_BATCH).take(batches) {
                let pairs: Vec<Value> = chunk.iter().map(|c| json!({"a": c.a, "b": c.b, "reason": c.reason, "key": c.key})).collect();
                let (content, kept) = store.judge_input_for_replay(&pairs).unwrap().expect("pairs");
                out.push(json!({"prompt": store.judge_prompt_for_replay(), "content": content, "pairs": kept}));
            }
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        }
        Some("apply") => {
            let items: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&args[3]).unwrap()).unwrap();
            for item in &items {
                let pairs = item["pairs"].as_array().cloned().unwrap_or_default();
                match store.people_judge_apply(&pairs, item["output"].as_str().unwrap_or(""), item["model"].as_str().unwrap_or("replay")) {
                    Ok(r) => eprintln!("{r}"),
                    Err(e) => eprintln!("ERROR {}", e.message()),
                }
            }
            store.people_sync().unwrap();
            let list = store.people_candidate_list().unwrap();
            println!("candidates for you: {}", list["candidates"].as_array().unwrap().len());
            for c in list["candidates"].as_array().unwrap() {
                println!("  {} <-> {}  | {}", c["a"], c["b"], c["reason"]);
            }
            println!("merged automatically: {}", list["auto_merged"].as_array().unwrap().len());
            for c in list["auto_merged"].as_array().unwrap() {
                println!("  {} = {}  | {}", c["a"], c["b"], c["reason"]);
            }
        }
        _ => eprintln!("usage: people_replay candidates|inputs|apply <root> …"),
    }
}
