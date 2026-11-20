//! The AI-readable layer: mail and transcripts become conversations with messages; deletion and scope carry over.
//! Synthetic data only.
mod common;
use candid_store::*;
use common::Fx;
use serde_json::{json, Value};

const T0: i64 = 1_799_992_800_000;

fn call(fx: &Fx, client: &str, tool: &str, body: Value) -> Value {
    fx.store.call_tool(tool, body.to_string().as_bytes(), client).unwrap_or_else(|e| match e {
        ToolCallFailure::Store(e) => panic!("{tool}: {} {}", e.code(), e.message()),
        ToolCallFailure::Decode(d) => panic!("{tool}: decode {d}"),
    })
}

fn mail(key: &str, at: i64, actor: &str, from: &str, body: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "mail.message", Some(at));
    e.actor = actor.into();
    e.app_name = Some("邮件".into());
    e.title = Some("Q4 预算".into());
    e.thread_key = Some("t-budget".into());
    e.payload = json!({"from": {"name": from}, "direction": if actor == "self" { "out" } else { "in" }});
    e.text = vec![IngestText::new("subject", "Q4 预算"), IngestText::new("body", body), IngestText::new("quoted", "> 很早以前的引用内容")];
    e
}

fn segment(key: &str, at: i64, hidden: bool, speaker: &str, body: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "speech.segment", Some(at));
    e.parent_key = Some("rec1".into());
    e.payload = if hidden { json!({"recording_event_id": "file-1", "speaker_label": speaker, "native_hidden": 1}) } else { json!({"recording_event_id": "file-1", "speaker_label": speaker}) };
    e.text = vec![IngestText::new("transcript", body)];
    e
}

fn seed(fx: &Fx) {
    let mut recording = IngestEvent::new("rec1", "audio.recording", Some(T0));
    recording.title = Some("周会".into());
    let mut batch = IngestBatch::events(
        &fx.source,
        vec![
            mail("m1", T0 + 1_000, "other", "张三", "周四下午可以开会吗？报价单附后。"),
            mail("m2", T0 + 60_000, "self", "dingyi", "周四三点可以，我把合同发你。"),
            recording,
            segment("s1", T0 + 120_000, false, "说话人 1", "我们先过一遍预算"),
            segment("s2", T0 + 130_000, true, "说话人 2", "这句已被重新转写隐藏"),
        ],
    );
    batch.threads = vec![IngestThread { source_key: "t-budget".into(), kind: "mail_thread".into(), title: Some("Q4 预算".into()), started_ms: None, ended_ms: None, participants: None, payload: None }];
    fx.store.ingest(&batch).unwrap();
}

#[test]
fn mail_and_transcripts_become_conversations() {
    let fx = Fx::make("curated-basic");
    seed(&fx);
    let list = call(&fx, &fx.client, "conversations", json!({}));
    let convs = list["conversations"].as_array().unwrap();
    assert_eq!(convs.len(), 2, "{list}");
    let mail = convs.iter().find(|c| c["kind"] == "mail").unwrap();
    assert_eq!(mail["title"], "Q4 预算");
    assert_eq!(mail["messages"], 2);
    let meeting = convs.iter().find(|c| c["kind"] == "meeting").unwrap();
    assert_eq!(meeting["title"], "周会");
    assert_eq!(meeting["messages"], 1, "hidden segments are not messages");

    let thread = call(&fx, &fx.client, "conversation", json!({"conversation_id": mail["conversation_id"]}));
    let msgs = thread["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["actor"], "other");
    assert_eq!(msgs[0]["sender"], "张三");
    assert_eq!(msgs[1]["actor"], "self");
    assert!(msgs.iter().all(|m| !m["text"].as_str().unwrap().contains("引用")), "quoted history is left out: {thread}");
    assert!(msgs[0]["ref"].is_string());

    let who = call(&fx, &fx.client, "conversations", json!({"who": "张三"}));
    assert_eq!(who["conversations"].as_array().unwrap().len(), 1);
}

#[test]
fn find_searches_clean_messages_only() {
    let fx = Fx::make("curated-find");
    seed(&fx);
    // Three-character terms use the index, shorter ones a scan.
    let long = call(&fx, &fx.client, "find", json!({"q": "合同发你"}));
    assert_eq!(long["results"].as_array().unwrap().len(), 1, "{long}");
    assert_eq!(long["results"][0]["conversation"]["kind"], "mail");
    let short = call(&fx, &fx.client, "find", json!({"q": "预算"}));
    assert_eq!(short["results"].as_array().unwrap().len(), 1, "only the transcript line says it in a message body: {short}");
    let hidden = call(&fx, &fx.client, "find", json!({"q": "重新转写"}));
    assert!(hidden["results"].as_array().unwrap().is_empty());
}

#[test]
fn deleting_the_record_removes_the_message_and_empty_conversation() {
    let fx = Fx::make("curated-delete");
    seed(&fx);
    call(&fx, &fx.client, "conversations", json!({}));
    let seg = fx.texts("SELECT event_id FROM events WHERE source_key='s1'");
    fx.store.delete_events(&seg, "user").unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM messages WHERE message_id LIKE 'ev:%' AND conversation_id LIKE 'rec:%'"), 0);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM conversations WHERE kind='meeting'"), 0);
    let found = call(&fx, &fx.client, "find", json!({"q": "预算"}));
    assert!(found["results"].as_array().unwrap().is_empty(), "{found}");
}

#[test]
fn scope_limits_what_a_client_sees() {
    let fx = Fx::make("curated-scope");
    seed(&fx);
    let narrow = ClientScope { kinds: vec!["speech.segment".into(), "audio.recording".into()], ..ClientScope::unbounded() };
    let client = fx.store.register_client("Narrow AI", "http", &narrow, None).unwrap().client_id;
    let list = call(&fx, &client, "conversations", json!({}));
    let kinds: Vec<&str> = list["conversations"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["meeting"], "{list}");
    let found = call(&fx, &client, "find", json!({"q": "合同发你"}));
    assert!(found["results"].as_array().unwrap().is_empty());

    let msg = fx.texts("SELECT message_id FROM messages WHERE actor='self'")[0].clone();
    assert!(fx.store.call_tool("evidence", json!({"id": msg}).to_string().as_bytes(), &client).is_err());
    let ev = call(&fx, &fx.client, "evidence", json!({"id": msg}));
    assert_eq!(ev["records"].as_array().unwrap().len(), 1, "{ev}");
}

#[test]
fn sync_is_incremental() {
    let fx = Fx::make("curated-sync");
    seed(&fx);
    assert_eq!(fx.store.curated_sync().unwrap()["inserted"], 3);
    assert_eq!(fx.store.curated_sync().unwrap()["changed"], false);
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![{
        let mut e = mail("m3", T0 + 200_000, "other", "张三", "收到，谢谢");
        e.thread_key = None;
        e
    }])).unwrap();
    assert_eq!(fx.store.curated_sync().unwrap()["inserted"], 1);
}
