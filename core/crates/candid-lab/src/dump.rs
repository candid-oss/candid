//! A stable, comparable picture of what the data layer holds for a window: the curated layer (conversations,
//! excerpts, activity blocks), the understanding layer (people, items, memos) and how the work went (jobs, calls).

use candid_store::testing::rusqlite::types::Value as P;
use candid_store::testing::RowExt;
use candid_store::TimelineStore;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

fn hm(store: &TimelineStore, ms: i64) -> String {
    let iso = store.clock.iso(ms);
    iso.get(5..19).unwrap_or(&iso).replace('T', " ")
}

pub fn dump(s: &TimelineStore, from: i64, until: i64) -> Value {
    let db = s.raw_db();
    let w = || vec![P::Integer(from), P::Integer(until)];
    let convs = db.query(
        "SELECT c.conversation_id,c.kind,COALESCE(c.app,''),COALESCE(c.title,''),c.producer FROM conversations c WHERE EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id=c.conversation_id AND m.sent_ms BETWEEN ? AND ?) ORDER BY c.kind,c.app,c.title",
        &w(), |r| (r.text(0), r.text(1), r.text(2), r.text(3), r.text(4)),
    ).unwrap();
    let mut conversations = vec![];
    for (id, kind, app, title, producer) in convs {
        let messages: Vec<Value> = db.query(
            "SELECT m.sent_ms,m.actor,m.sender,m.text,m.sender_entity_id,m.producer,(SELECT COUNT(*) FROM message_sources s WHERE s.message_id=m.message_id) FROM messages m WHERE m.conversation_id=? AND m.sent_ms BETWEEN ? AND ? ORDER BY m.sent_ms,m.seq",
            &[P::Text(id.clone()), P::Integer(from), P::Integer(until)],
            |r| json!({"t": hm(s, r.int(0)), "actor": r.text(1), "sender": r.opt_text(2), "text": r.text(3), "entity": r.opt_text(4), "producer": r.text(5), "sources": r.int(6)}),
        ).unwrap();
        conversations.push(json!({"id": id, "kind": kind, "app": app, "title": title, "producer": producer, "messages": messages}));
    }
    let excerpts: Vec<Value> = db.query(
        "SELECT COALESCE(app,''),COALESCE(title,''),COALESCE(url,''),start_ms,end_ms,text,producer FROM excerpts WHERE end_ms>=? AND start_ms<=? ORDER BY app,start_ms",
        &w(), |r| { let t = r.text(5); json!({"app": r.text(0), "title": r.text(1), "url": r.text(2), "from": hm(s, r.int(3)), "to": hm(s, r.int(4)), "chars": t.chars().count(), "head": t.chars().take(240).collect::<String>(), "producer": r.text(6)}) },
    ).unwrap();
    let episodes: Vec<Value> = db.query(
        "SELECT e.episode_id,e.start_ms,e.end_ms,COALESCE(e.title,''),COALESCE(e.summary,''),e.status,e.producer,(SELECT COUNT(*) FROM episode_evidence v WHERE v.episode_id=e.episode_id AND v.rev=e.rev) FROM episodes e WHERE e.episode_id GLOB 'screen:*' AND e.end_ms>=? AND e.start_ms<=? AND e.rev=(SELECT MAX(rev) FROM episodes x WHERE x.episode_id=e.episode_id) ORDER BY e.start_ms",
        &w(), |r| json!({"from": hm(s, r.int(1)), "to": hm(s, r.int(2)), "title": r.text(3), "summary": r.text(4), "status": r.text(5), "producer": r.text(6), "evidence": r.int(7)}),
    ).unwrap();
    let mut people: Vec<Value> = db.query(
        "SELECT e.entity_id,COALESCE(e.name,''),e.status,(SELECT json_group_array(a.alias_kind||':'||a.alias) FROM (SELECT * FROM entity_aliases a WHERE a.entity_id=e.entity_id ORDER BY a.alias_kind,a.alias) a),(SELECT COUNT(*) FROM messages m WHERE m.sender_entity_id=e.entity_id) FROM entities e WHERE e.merged_into IS NULL",
        &[], |r| json!({"name": r.text(1), "status": r.text(2), "aliases": serde_json::from_str::<Value>(&r.text(3)).unwrap_or(json!([])), "messages": r.int(4)}),
    ).unwrap();
    people.sort_by_key(|p| p["aliases"].to_string());
    let items: Vec<Value> = db.query(
        "SELECT kind,owner,title,COALESCE(when_text,''),start_ms,review,state,COALESCE(counterpart,''),COALESCE(conversation_id,''),producer,(SELECT COUNT(*) FROM item_changes c JOIN item_sources s USING(change_id) WHERE c.item_id=i.item_id) FROM items i ORDER BY start_ms,title",
        &[], |r| json!({"kind": r.text(0), "owner": r.text(1), "title": r.text(2), "when": r.text(3), "start": r.opt_int(4).map(|t| hm(s, t)), "review": r.text(5), "state": r.text(6), "counterpart": r.text(7), "conversation": r.text(8), "producer": r.text(9), "sources": r.int(10)}),
    ).unwrap();
    let memos: Vec<Value> = db.query("SELECT text,status,producer FROM memories WHERE kind='fact' ORDER BY created_ms", &[], |r| json!({"text": r.text(0), "status": r.text(1), "producer": r.text(2)})).unwrap();
    let mut jobs: BTreeMap<String, i64> = BTreeMap::new();
    for (k, st, e, n) in db.query("SELECT kind,state,COALESCE(error_code,''),COUNT(*) FROM jobs GROUP BY 1,2,3", &[], |r| (r.text(0), r.text(1), r.text(2), r.int(3))).unwrap() {
        jobs.insert(format!("{k} {st} {e}").trim().into(), n);
    }
    let calls: Vec<Value> = db.query(
        "SELECT feature,model_id,status,COALESCE(error_code,''),COUNT(*),CAST(AVG(latency_ms) AS INTEGER),SUM(input_tokens),SUM(output_tokens) FROM ai_calls WHERE client='worker' AND provider_id='lab' GROUP BY 1,2,3,4",
        &[], |r| json!({"feature": r.text(0), "model": r.text(1), "status": r.text(2), "error": r.text(3), "count": r.int(4), "avg_ms": r.opt_int(5), "in": r.opt_int(6), "out": r.opt_int(7)}),
    ).unwrap();
    json!({
        "window": {"from": hm(s, from), "until": hm(s, until)},
        "counts": {
            "conversations": conversations.len(),
            "messages": conversations.iter().map(|c| c["messages"].as_array().unwrap().len()).sum::<usize>(),
            "excerpts": excerpts.len(), "episodes": episodes.len(),
            "episodes_summarised": episodes.iter().filter(|e| e["producer"] != "screen-cleanup/1").count(),
            "people": people.len(), "items": items.len(), "memos": memos.len(),
        },
        "conversations": conversations, "excerpts": excerpts, "episodes": episodes,
        "people": people, "items": items, "memos": memos, "jobs": jobs, "calls": calls,
    })
}

/// Human-readable view of a dump plus the proxy's call log.
pub fn report(d: &Value, calls_log: &Path) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# Run report {} → {}\n", d["window"]["from"], d["window"]["until"]);
    let _ = writeln!(o, "counts: {}\n", d["counts"]);
    let lines: Vec<Value> = std::fs::read_to_string(calls_log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let hits = lines.iter().filter(|l| l["hit"] == true).count();
    let slow: Vec<u64> = lines.iter().filter(|l| l["hit"] != true).filter_map(|l| l["ms"].as_u64()).collect();
    let _ = writeln!(o, "model calls: {} ({} cached, {} live, live avg {} ms, reasoning chars total {})\n", lines.len(), hits, slow.len(),
        if slow.is_empty() { 0 } else { slow.iter().sum::<u64>() / slow.len() as u64 },
        lines.iter().filter_map(|l| l["reasoning_chars"].as_u64()).sum::<u64>());
    let _ = writeln!(o, "## Jobs\n");
    for (k, v) in d["jobs"].as_object().unwrap() {
        let _ = writeln!(o, "- {k}: {v}");
    }
    let _ = writeln!(o, "\n## Conversations\n");
    for c in d["conversations"].as_array().unwrap() {
        let _ = writeln!(o, "### [{}] {} · {} ({} 条)", c["kind"].as_str().unwrap(), c["app"].as_str().unwrap(), c["title"].as_str().unwrap(), c["messages"].as_array().unwrap().len());
        for m in c["messages"].as_array().unwrap().iter().take(60) {
            let who = match m["actor"].as_str().unwrap() { "self" => "我".to_string(), _ => m["sender"].as_str().unwrap_or("?").to_string() };
            let _ = writeln!(o, "  {} {}：{}", m["t"].as_str().unwrap(), who, m["text"].as_str().unwrap().replace('\n', " ").chars().take(160).collect::<String>());
        }
        let _ = writeln!(o);
    }
    let _ = writeln!(o, "## Items\n");
    for i in d["items"].as_array().unwrap() {
        let _ = writeln!(o, "- [{} {} {}] {} · {} · {}", i["kind"].as_str().unwrap(), i["review"].as_str().unwrap(), i["state"].as_str().unwrap(), i["title"].as_str().unwrap(), i["when"].as_str().unwrap(), i["counterpart"].as_str().unwrap());
    }
    let _ = writeln!(o, "\n## Memos\n");
    for m in d["memos"].as_array().unwrap() {
        let _ = writeln!(o, "- {}", m["text"].as_str().unwrap());
    }
    let _ = writeln!(o, "\n## Activity blocks\n");
    for e in d["episodes"].as_array().unwrap() {
        if e["producer"] != "screen-cleanup/1" || e["status"] != "superseded" {
            let _ = writeln!(o, "- {}–{} [{} {}] {} — {}", e["from"].as_str().unwrap(), &e["to"].as_str().unwrap()[6..], e["status"].as_str().unwrap(), e["producer"].as_str().unwrap(), e["title"].as_str().unwrap(), e["summary"].as_str().unwrap());
        }
    }
    let _ = writeln!(o, "\n## People with chat aliases\n");
    for p in d["people"].as_array().unwrap() {
        if p["aliases"].as_array().unwrap().iter().any(|a| a.as_str().unwrap_or("").starts_with("handle:")) {
            let _ = writeln!(o, "- {} {} ({} msgs)", p["name"].as_str().unwrap(), p["aliases"], p["messages"]);
        }
    }
    o
}
