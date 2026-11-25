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
fn chat_job_batches_changed_frames_and_uses_saved_ocr_coordinates() {
    let fx = Fx::make("chat-batch");
    fx.store.model_command(&json!({"action":"save","config":{"version":1,"providers":[{"provider_id":"local","name":"Synthetic","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","enabled":true,"destination":"on_device"}],"models":[{"provider_id":"local","model_id":"test"}],"roles":[{"role":"vision","model":{"provider_id":"local","model_id":"test"}}]}})).unwrap();
    enable(&fx.store);
    fx.store.excerpt_scan().unwrap();
    let now = fx.store.clock.now_ms();
    let mut ids = vec![];
    for (i, body, side, draft) in [
        (0, "第一条已经发送", "侧栏预览甲", "随便写的草稿"),
        (1, "第一条已经发送", "侧栏预览乙", "变化的草稿"),
        (2, "第二条已经发送", "侧栏预览乙", "准备回复的草稿"),
        (3, "第三条已经发送", "侧栏预览乙", "再写一行草稿"),
    ] {
        let at = now + i * 5000;
        let key = format!("shot-{i}");
        let (path, sha) = common::write_staging(&fx.store, &[i as u8; 100], "jpg");
        let mut screenshot = common::shot(&key, at, &path, &sha);
        screenshot.app_name = Some("微信".into());
        screenshot.title = Some("微信".into());
        let lines = [("测试群", 315, 45), (body, 380, 410), (side, 100, 120), (draft, 315, 824), ("发送", 925, 940)];
        let blob = json!({"ocrLines":lines.map(|(t, x, y)| json!({"text":t,"x":x,"y":y,"width":50,"height":20}))});
        let (path, sha) = common::write_staging(&fx.store, blob.to_string().as_bytes(), "json");
        let mut ocr = IngestEvent::new(&format!("{key}-ocr"), "image.text", Some(at));
        ocr.parent_key = Some(key.clone());
        ocr.payload = json!({"method":"ocr","image_event_id":"pending","result_ref":sha});
        ocr.text = vec![IngestText::new("ocr", &lines.map(|(t, ..)| t).join("\n"))];
        let mut derived = IngestBlob::new(&path, &sha);
        derived.role = "derived".into();
        derived.mime_type = Some("application/json".into());
        ocr.blobs.push(derived);
        fx.store.ingest(&IngestBatch::events(&fx.source, vec![screenshot, ocr])).unwrap();
        ids.push(fx.store.event_id(&fx.source, &key).unwrap().unwrap());
        assert_eq!(fx.store.excerpt_scan().unwrap()["queued"], u64::from(i == 0));
        if i < 3 {
            let ready = fx.store.pending_job_queue().unwrap();
            assert!(ready.is_empty(), "frame {i}: {ready:?} {}", fx.store.excerpt_parts(&ids[i as usize]).unwrap()["context"]);
        }
    }
    let queue = fx.store.pending_job_queue().unwrap();
    assert_eq!(queue.len(), 1);
    let job = fx.store.job_command(&json!({"action":"claim","id":queue[0].0,"owner":"screen-batch-test","lease_ms":60000})).unwrap();
    let request = fx.store.workflow_prepare(&job).unwrap();
    assert_eq!(request["frames"], json!([ids[0], ids[2], ids[3]]));
    assert_eq!(request["messages"][1]["content"].as_array().unwrap().iter().filter(|p| p["type"] == "image_url").count(), 3);
    let content = request["content"].as_str().unwrap();
    assert!(content.contains("第二条已经发送") && !content.contains("侧栏") && !content.contains("草稿"), "{content}");
    let result = |n| json!({"kind":"chat","title":"测试群","excerpt":"","messages":[
        {"frame":1,"from":"other","sender":"发言人","text":"第一条已经发送"},
        {"frame":2,"from":"other","sender":"发言人","text":"第二条已经发送"},
        {"frame":2,"from":"other","sender":"发言人","text":"第二条已经发送"},
        {"frame":2,"from":"me","sender":"","text":"准备回复的草稿"},
        {"frame":n,"from":"other","sender":"发言人","text":"第三条已经发送"}
    ]});
    // A message pointing at a frame that is not in the batch is dropped; the rest of the reply is kept.
    let mut partial = result(4);
    partial["messages"].as_array_mut().unwrap().push(json!({"frame":3,"from":"other","sender":"发言人","text":"第三条已经发送"}));
    assert_eq!(fx.store.workflow_complete(&request, &json!(partial.to_string())).unwrap()["appended"], 3);
    let list = call(&fx, "conversations", json!({"kind":"chat"}));
    let messages = call(&fx, "conversation", json!({"conversation_id": list["conversations"][0]["conversation_id"]}));
    assert_eq!(messages["messages"][1]["ref"], ids[2]);
    assert_eq!(messages["messages"][2]["ref"], ids[3]);
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
