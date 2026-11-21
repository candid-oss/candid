//! People judged by a model (data-layer plan §1.3–1.4): candidate pairs, batching, corroboration, automatic merge,
//! candidates for you, undo. Synthetic data only; the model's replies are written by hand.
mod common;
use candid_store::*;
use common::Fx;
use serde_json::{json, Value};

const T0: i64 = 1_799_992_800_000;

fn shot(fx: &Fx, key: &str, at: i64, app: &str) -> String {
    let mut s = IngestEvent::new(key, "screen.shot", Some(at));
    s.app_name = Some(app.into());
    s.title = Some(app.into());
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s])).unwrap();
    fx.texts(&format!("SELECT event_id FROM events WHERE source_key='{key}'"))[0].clone()
}

fn chat(fx: &Fx, key: &str, at: i64, app: &str, title: &str, messages: &[(&str, &str, &str)]) {
    let id = shot(fx, key, at, app);
    let out = json!({"kind": "chat", "title": title, "excerpt": "", "messages": messages.iter().map(|(f, s, t)| json!({"from": f, "sender": s, "text": t})).collect::<Vec<_>>()});
    fx.store.excerpt_write(&id, &out, "test-vlm").unwrap();
}

fn mail(fx: &Fx, key: &str, at: i64, address: &str, name: &str) {
    let mut e = IngestEvent::new(key, "mail.message", Some(at));
    e.actor = "other".into();
    e.app_name = Some("邮件".into());
    e.title = Some(key.into());
    e.payload = json!({"from": {"name": name, "address": address}, "direction": "in"});
    e.text = vec![IngestText::new("subject", key), IngestText::new("body", &format!("邮件 {key}：下周评审"))];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![e])).unwrap();
}

fn sender_entity(fx: &Fx, text: &str) -> Option<String> {
    fx.texts(&format!("SELECT COALESCE(sender_entity_id,'') FROM messages WHERE text='{text}'")).pop().filter(|s| !s.is_empty())
}

fn model_role(fx: &Fx, role: &str) {
    let model = json!({"version":1,"providers":[{"provider_id":"s","name":"SYNTHETIC","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","destination":"on_device","enabled":true}],"models":[{"provider_id":"s","model_id":"synthetic"}],"roles":[{"role":role,"model":{"provider_id":"s","model_id":"synthetic"}}]});
    fx.store.model_command(&json!({"action":"save","config":model})).unwrap();
}

fn enable(fx: &Fx) {
    let mut config = fx.store.feature_settings().unwrap();
    config["items"]["people-judge"]["enabled"] = json!(true);
    fx.store.workflow_command(&json!({"action":"features","config":config})).unwrap();
}

fn seed(fx: &Fx) {
    // 张三 on WeChat and on QQ: same name in two apps, nothing else to go on.
    chat(fx, "a", T0, "微信", "张三", &[("other", "", "wx张三：周四评审会改到三点"), ("me", "", "wx我：好的")]);
    chat(fx, "b", T0 + 10_000, "QQ", "张三", &[("other", "", "qq张三：报价单我晚点发")]);
    // "San Zhang" on WeChat and a mail from San Zhang <zhangsan@corp.com>: the address spells the name.
    chat(fx, "c", T0 + 20_000, "微信", "San Zhang", &[("other", "", "wx_san：see you at the review")]);
    mail(fx, "m1", T0 + 30_000, "zhangsan@corp.com", "San Zhang");
    // Two people in the same group are never candidates.
    chat(fx, "d", T0 + 40_000, "微信", "项目群（4）", &[("other", "李雷", "群里李雷"), ("other", "李雷雷", "群里李雷雷")]);
    fx.store.curated_sync().unwrap();
    fx.store.people_sync().unwrap();
}

fn pairs(fx: &Fx) -> Vec<(String, String, &'static str)> {
    fx.store.people_candidates().unwrap().into_iter().map(|c| (c.a, c.b, c.reason)).collect()
}

/// A model reply that calls every pair `verdict`.
fn reply(fx: &Fx, verdict: &str) -> (Vec<Value>, String) {
    let c = fx.store.people_candidates().unwrap();
    let pairs: Vec<Value> = c.iter().enumerate().map(|(i, c)| json!({"id": i + 1, "a": c.a, "b": c.b, "key": c.key})).collect();
    let results: Vec<Value> = (1..=c.len()).map(|i| json!({"id": i, "verdict": verdict, "reason": "测试理由"})).collect();
    (pairs, format!("```json\n{}\n```", json!({"results": results})))
}

#[test]
fn candidates_come_from_rules_and_skip_vetoed_pairs() {
    let fx = Fx::make("judge-candidates");
    seed(&fx);
    let found = pairs(&fx);
    assert!(found.iter().any(|(a, b, why)| a == "handle:qq/张三" && b == "handle:wechat/张三" && *why == "same_name"), "{found:?}");
    assert!(found.iter().any(|(a, b, why)| a == "email:zhangsan@corp.com" && b == "handle:wechat/san zhang" && *why == "mail_name"), "{found:?}");
    assert!(!found.iter().any(|(a, b, _)| a.contains("李雷") && b.contains("李雷")), "both spoke in one group: {found:?}");
    // Names one inside the other are asked about too.
    chat(&fx, "e", T0 + 50_000, "QQ", "张三丰", &[("other", "", "qq丰：练拳")]);
    fx.store.people_sync().unwrap();
    assert!(pairs(&fx).iter().any(|(a, b, why)| a == "handle:qq/张三丰" && b.ends_with("张三") && *why == "contains"));
}

#[test]
fn corroborated_same_merges_and_uncorroborated_stays_a_candidate() {
    let fx = Fx::make("judge-merge");
    seed(&fx);
    let (batch, raw) = reply(&fx, "same");
    let r = fx.store.people_judge_apply(&batch, &raw, "test-model").unwrap();
    assert_eq!(r["same"], 2, "{r}");
    assert_eq!(r["auto_merge"], 1, "only the pair whose address matches the name is backed up: {r}");
    fx.store.people_sync().unwrap();
    // Pinyin-corroborated pair merged; the same-name pair is only a candidate.
    assert_eq!(sender_entity(&fx, "wx_san：see you at the review"), fx.texts("SELECT sender_entity_id FROM messages WHERE conversation_id IN (SELECT conversation_id FROM conversations WHERE kind='mail')").pop());
    assert_ne!(sender_entity(&fx, "wx张三：周四评审会改到三点"), sender_entity(&fx, "qq张三：报价单我晚点发"));
    let list = fx.store.people_candidate_list().unwrap();
    assert_eq!(list["candidates"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["candidates"][0]["a"], "handle:qq/张三");
    assert_eq!(list["auto_merged"].as_array().unwrap().len(), 1, "{list}");
    assert!(list["auto_merged"][0]["reason"].as_str().unwrap().contains("佐证"), "{list}");
    // Answered pairs are not asked again, until an alias is added or 30 days pass.
    assert!(pairs(&fx).is_empty(), "{:?}", pairs(&fx));
    // The card of the merged person says so.
    let card = fx.store.call_tool("person", json!({"who": "san zhang"}).to_string().as_bytes(), &fx.client).map_err(|_| "person").unwrap();
    assert_eq!(card["auto_merged"].as_array().unwrap().len(), 1, "{card}");
    assert!(card["aliases"].as_array().unwrap().iter().any(|a| a["source"] == "model"));

    // Accepting the candidate writes a merge correction.
    let c = &list["candidates"][0];
    fx.store.person_correct(&PersonCorrectionRequest { action: "merge".into(), key: c["a"].as_str().unwrap().into(), other: Some(c["b"].as_str().unwrap().into()), name: None }).unwrap();
    assert_eq!(sender_entity(&fx, "wx张三：周四评审会改到三点"), sender_entity(&fx, "qq张三：报价单我晚点发"));
    assert!(fx.store.people_candidate_list().unwrap()["candidates"].as_array().unwrap().is_empty());
}

#[test]
fn undoing_an_automatic_merge_is_not_the_same_person_and_sticks() {
    let fx = Fx::make("judge-undo");
    seed(&fx);
    let (batch, raw) = reply(&fx, "same");
    fx.store.people_judge_apply(&batch, &raw, "test-model").unwrap();
    fx.store.people_sync().unwrap();
    let list = fx.store.people_candidate_list().unwrap();
    let m = &list["auto_merged"][0];
    let (a, b) = (m["a"].as_str().unwrap().to_string(), m["b"].as_str().unwrap().to_string());
    assert!(sender_entity(&fx, "wx_san：see you at the review").is_some());
    fx.store.person_correct(&PersonCorrectionRequest { action: "split".into(), key: a.clone(), other: Some(b.clone()), name: None }).unwrap();
    let mail_sender = fx.texts("SELECT sender_entity_id FROM messages WHERE conversation_id IN (SELECT conversation_id FROM conversations WHERE kind='mail')").pop();
    assert_ne!(sender_entity(&fx, "wx_san：see you at the review"), mail_sender, "split after undo");
    // Recomputing from scratch, new messages and new verdicts never bring it back.
    fx.store.people_recompute().unwrap();
    let (batch, raw) = reply(&fx, "same");
    assert!(!batch.iter().any(|p| p["a"] == a.as_str() && p["b"] == b.as_str()), "the corrected pair is not asked again");
    let _ = raw;
    fx.store.people_set_verdict(&PeopleVerdict { a: a.clone(), b: b.clone(), verdict: "same".into(), corroborated: true, ..Default::default() }).unwrap();
    fx.store.people_recompute().unwrap();
    assert_ne!(sender_entity(&fx, "wx_san：see you at the review"), fx.texts("SELECT sender_entity_id FROM messages WHERE conversation_id IN (SELECT conversation_id FROM conversations WHERE kind='mail')").pop());
    let list = fx.store.people_candidate_list().unwrap();
    assert!(list["auto_merged"].as_array().unwrap().is_empty() && list["candidates"].as_array().unwrap().iter().all(|c| c["a"] != a.as_str()), "{list}");
}

#[test]
fn different_and_unsure_answers_are_remembered_and_missing_pairs_are_unsure() {
    let fx = Fx::make("judge-cache");
    seed(&fx);
    let c = fx.store.people_candidates().unwrap();
    assert_eq!(c.len(), 2);
    let batch: Vec<Value> = c.iter().enumerate().map(|(i, c)| json!({"id": i + 1, "a": c.a, "b": c.b, "key": c.key})).collect();
    // The reply names only pair 1 (and a made-up verdict for it is treated as unsure, as is the missing pair 2).
    let raw = json!({"results": [{"id": 1, "verdict": "different", "reason": "话题不同"}]}).to_string();
    let r = fx.store.people_judge_apply(&batch, &raw, "test-model").unwrap();
    assert_eq!((r["different"].as_i64(), r["unsure"].as_i64()), (Some(1), Some(1)), "{r}");
    fx.store.people_sync().unwrap();
    assert!(pairs(&fx).is_empty());
    assert!(fx.store.people_judge_apply(&batch, "不是 JSON", "m").is_err());
    // `different` forbids a later (corroborated) same verdict from the cache alone.
    fx.store.people_set_verdict(&PeopleVerdict { a: c[0].a.clone(), b: c[0].b.clone(), verdict: "same".into(), corroborated: true, ..Default::default() }).unwrap();
    // A new alias for one side changes the question: asked again.
    chat(&fx, "f", T0 + 60_000, "飞书", "张三", &[("other", "", "fs张三：新同事")]);
    fx.store.people_sync().unwrap();
    assert!(pairs(&fx).len() >= 2, "{:?}", pairs(&fx));
}

#[test]
fn scan_queues_batches_of_ten_when_the_feature_is_on_and_prepare_builds_the_input() {
    let fx = Fx::make("judge-job");
    for i in 0..12 {
        let name = format!("同名{i:02}");
        chat(&fx, &format!("w{i}"), T0 + i * 1000, "微信", &name, &[("other", "", &format!("wx{name}：周会讨论预算"))]);
        chat(&fx, &format!("q{i}"), T0 + i * 1000 + 500, "QQ", &name, &[("other", "", &format!("qq{name}：预算我看过了"))]);
    }
    fx.store.curated_sync().unwrap();
    fx.store.people_sync().unwrap();
    assert_eq!(fx.store.people_candidates().unwrap().len(), 12);
    // Off by default: nothing is queued.
    assert_eq!(fx.store.people_judge_scan().unwrap()["disabled"], true);
    enable(&fx);
    let r = fx.store.people_judge_scan().unwrap();
    assert_eq!(r["queued"], 2, "{r}");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='people.judge' AND state='queued' AND feature='people-judge'"), 2);
    assert_eq!(fx.store.people_judge_scan().unwrap()["changed"], false, "nothing new, nothing queued");
    assert_eq!(job_priority::job_priority("people.judge"), 3);

    let job = fx.store.job_command(&json!({"action":"claim","kind":"people.judge","owner":"synthetic","lease_ms":60000})).unwrap();
    // No model yet: waits.
    assert!(fx.store.workflow_prepare(&job).unwrap_err().message().contains("waiting_model"));
    model_role(&fx, "summary");
    let req = fx.store.workflow_prepare(&job).unwrap();
    let pairs = req["pairs"].as_array().unwrap();
    assert!(pairs.len() == 10 || pairs.len() == 2, "{}", pairs.len());
    let content = req["content"].as_str().unwrap();
    assert!(content.contains("第 1 对") && content.contains("handle:qq/") && content.contains("最近说过") && content.contains("规则线索"), "{content}");
    assert_eq!(req["json_repair"], true);
    assert_eq!(req["step"], "people-judge");
    assert_eq!(fx.store.feature_budget("people-judge").unwrap().1, 50);

    // The reply goes through the normal completion path and caches the verdicts.
    let results: Vec<Value> = (1..=pairs.len()).map(|i| json!({"id": i, "verdict": "unsure", "reason": "信息不足"})).collect();
    let mut r = req.clone();
    r["job"] = job.clone();
    fx.store.workflow_complete(&r, &json!(json!({"results": results}).to_string())).unwrap();
    assert_eq!(fx.store.job_command(&json!({"action":"get","id":job["id"]})).unwrap()["state"], "done");
    assert_eq!(fx.store.people_candidates().unwrap().len(), 12 - pairs.len());
}

#[test]
fn judge_batches_drop_pairs_that_stopped_being_candidates_and_the_feature_is_listed_off() {
    let fx = Fx::make("judge-stale");
    seed(&fx);
    let settings = fx.store.feature_settings().unwrap();
    assert_eq!(settings["items"]["people-judge"]["enabled"], false);
    assert_eq!(settings["items"]["people-judge"]["daily_calls"], 50);
    assert_eq!(settings["items"]["people-judge"]["allow_remote"], false);
    enable(&fx);
    fx.store.people_judge_scan().unwrap();
    // You split one pair before the job runs: it is dropped from the model input.
    let c = fx.store.people_candidates().unwrap();
    fx.store.person_correct(&PersonCorrectionRequest { action: "split".into(), key: c[0].a.clone(), other: Some(c[0].b.clone()), name: None }).unwrap();
    model_role(&fx, "summary");
    let job = fx.store.job_command(&json!({"action":"claim","kind":"people.judge","owner":"synthetic","lease_ms":60000})).unwrap();
    let req = fx.store.workflow_prepare(&job).unwrap();
    assert_eq!(req["pairs"].as_array().unwrap().len(), 1, "{}", req["content"]);
}

#[test]
fn the_same_words_in_another_order_or_with_notes_are_candidates() {
    let fx = Fx::make("judge-words");
    chat(&fx, "a", T0, "微信", "Kangran Zhao", &[("other", "", "wx：see you")]);
    chat(&fx, "b", T0 + 1000, "飞书", "Zhao Kangran (250010172)", &[("other", "", "fs：hello")]);
    fx.store.curated_sync().unwrap();
    fx.store.people_sync().unwrap();
    let found = pairs(&fx);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].2, "same_name");
}
