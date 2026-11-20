//! Screenshots into conversations: keyframe selection and merging model output. Synthetic data only; no model.
mod common;
use candid_store::*;
use common::Fx;
use serde_json::{json, Value};

fn enable(store: &TimelineStore) {
    let mut config = store.feature_settings().unwrap();
    config["items"]["screen-excerpt"]["enabled"] = json!(true);
    store.workflow_command(&json!({"action":"features","config":config})).unwrap();
}

/// A screenshot and its OCR, `at` milliseconds after the store's clock.
fn shot(fx: &Fx, key: &str, at: i64, ocr: &str) -> String {
    let mut s = IngestEvent::new(key, "screen.shot", Some(at));
    s.app_name = Some("微信".into());
    s.title = Some("微信".into());
    let mut o = IngestEvent::new(&format!("{key}-ocr"), "image.text", Some(at));
    o.parent_key = Some(key.into());
    o.app_name = Some("微信".into());
    o.payload = json!({"image_event_id": "pending", "method": "ocr"});
    o.text = vec![IngestText::new("ocr", ocr)];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s, o])).unwrap();
    fx.texts(&format!("SELECT event_id FROM events WHERE source_key='{key}'"))[0].clone()
}

fn chat(title: &str, messages: &[(&str, &str, &str)]) -> Value {
    json!({"kind": "chat", "title": title, "excerpt": "", "messages": messages.iter().map(|(from, sender, text)| json!({"from": from, "sender": sender, "text": text})).collect::<Vec<_>>()})
}

fn call(fx: &Fx, tool: &str, body: Value) -> Value {
    fx.store.call_tool(tool, body.to_string().as_bytes(), &fx.client).map_err(|_| tool.to_string()).unwrap()
}

#[test]
fn chat_frames_merge_into_one_conversation_without_repeats() {
    let fx = Fx::make("excerpt-merge");
    let now = fx.scalar("SELECT 0") + 1_799_992_800_000;
    let a = shot(&fx, "a", now, "x");
    let b = shot(&fx, "b", now + 5_000, "y");
    let c = shot(&fx, "c", now + 9_000, "z");
    fx.store.excerpt_write(&a, &chat("张三", &[("other", "", "周四下午可以开会吗？"), ("me", "", "可以，三点")]), "test-vlm").unwrap();
    // The next frame scrolled: one old message, one new, with a small OCR-like difference.
    let r = fx.store.excerpt_write(&b, &chat("张三", &[("me", "", "可以, 三点"), ("other", "", "好的，我把报价单发你")]), "test-vlm").unwrap();
    assert_eq!(r["appended"], 1, "{r}");
    // Scrolled back to the start: nothing new.
    let r = fx.store.excerpt_write(&c, &chat("张三", &[("other", "", "周四下午可以开会吗？"), ("me", "", "可以，三点")]), "test-vlm").unwrap();
    assert_eq!(r["appended"], 0, "{r}");

    let list = call(&fx, "conversations", json!({"kind": "chat"}));
    assert_eq!(list["conversations"].as_array().unwrap().len(), 1, "{list}");
    let id = list["conversations"][0]["conversation_id"].clone();
    assert_eq!(list["conversations"][0]["title"], "张三");
    let msgs = call(&fx, "conversation", json!({"conversation_id": id}));
    let texts: Vec<(&str, &str)> = msgs["messages"].as_array().unwrap().iter().map(|m| (m["actor"].as_str().unwrap(), m["text"].as_str().unwrap())).collect();
    assert_eq!(texts, [("other", "周四下午可以开会吗？"), ("self", "可以，三点"), ("other", "好的，我把报价单发你")]);

    // Deleting the first screenshot removes only what it contributed.
    fx.store.delete_events(&[a], "user").unwrap();
    let msgs = call(&fx, "conversation", json!({"conversation_id": id}));
    assert_eq!(msgs["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn model_output_needs_the_derived_scope() {
    let fx = Fx::make("excerpt-scope");
    let a = shot(&fx, "a", 1_799_992_800_000, "x");
    fx.store.excerpt_write(&a, &chat("李四", &[("other", "", "明天见")]), "test-vlm").unwrap();
    let plain = ClientScope { derived: false, ..ClientScope::unbounded() };
    let client = fx.store.register_client("No derived", "http", &plain, None).unwrap().client_id;
    let list = fx.store.call_tool("conversations", b"{}", &client).map_err(|_| ()).unwrap();
    assert!(list["conversations"].as_array().unwrap().is_empty(), "{list}");
}

#[test]
fn reading_becomes_excerpts_and_near_repeats_extend_them() {
    let fx = Fx::make("excerpt-doc");
    let a = shot(&fx, "a", 1_799_992_800_000, "x");
    let b = shot(&fx, "b", 1_799_992_860_000, "y");
    let body = "分布式训练里，梯度压缩可以把通信量降低一个数量级，但会带来收敛变慢的问题。";
    let doc = |t: &str| json!({"kind": "web", "title": "梯度压缩综述", "messages": [], "excerpt": t});
    assert_eq!(fx.store.excerpt_write(&a, &doc(body), "test-vlm").unwrap()["written"], 1);
    assert_eq!(fx.store.excerpt_write(&b, &doc(&format!("{body}。")), "test-vlm").unwrap()["written"], 0);
    let found = call(&fx, "find", json!({"q": "梯度压缩"}));
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["results"][0]["type"], "excerpt");
}

#[test]
fn scan_queues_only_screenshots_with_new_text() {
    let fx = Fx::make("excerpt-scan");
    // Nothing happens while the feature is off.
    assert_eq!(fx.store.excerpt_scan().unwrap()["disabled"], true);
    enable(&fx.store);
    fx.store.excerpt_scan().unwrap(); // starts automatic work from now
    let now = fx.scalar("SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='excerpt.automatic-since/1'");
    let at = if now > 0 { now } else { 1_799_992_800_000 };
    shot(&fx, "a", at + 1, "通讯录\n张三\n周四下午可以开会吗？\n可以，三点");
    shot(&fx, "b", at + 2, "通讯录\n张三\n周四下午可以开会吗？\n可以，三点\n12:47");
    shot(&fx, "c", at + 3, "通讯录\n张三\n可以，三点\n好的，我把报价单发你\n合同明天给你");
    let r = fx.store.excerpt_scan().unwrap();
    assert_eq!(r["queued"], 2, "first frame and the one with two new lines: {r}");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='screen.excerpt'"), 2);
    assert_eq!(fx.store.excerpt_scan().unwrap()["queued"], 0);
}

#[test]
fn compose_uses_the_on_screen_conversation_and_own_wording() {
    let fx = Fx::make("excerpt-compose");
    let c = json!({"version":1,"providers":[{"provider_id":"local","name":"SYNTHETIC","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","enabled":true,"destination":"on_device"}],"models":[{"provider_id":"local","model_id":"SYNTHETIC"}],"roles":[{"role":"agent_chat","model":{"provider_id":"local","model_id":"SYNTHETIC"}}]});
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    let now = candid_store::fixtures::FIXED_NOW_MS - 60_000;
    let a = shot(&fx, "a", now, "x");
    fx.store.excerpt_write(&a, &chat("张三", &[("other", "", "周四下午可以开会吗？报价单也发我一下"), ("me", "", "我看下日程哈")]), "test-vlm").unwrap();
    let r = fx.store.compose_prepare(&json!({"instruction": "说周四三点可以，报价晚点发", "app": "微信", "title": "微信"})).unwrap();
    let user = r["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("张三：周四下午可以开会吗？"), "{user}");
    assert!(user.contains("我：我看下日程哈"));
    assert!(user.contains("用户以前的说法"));
    assert!(user.ends_with("说周四三点可以，报价晚点发"));
    assert_eq!(r["role"], "agent_chat");
    let labels: Vec<&str> = r["context"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["微信 · 张三（最近 2 条）", "你的 1 条历史发言"]);
    assert!(fx.store.compose_prepare(&json!({"instruction": " "})).is_err());
}
