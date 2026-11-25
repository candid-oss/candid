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

fn prompt(key: &str, at: i64, session: &str, text: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "ai.prompt", Some(at));
    e.payload = json!({"agent": "claude_code", "session_ref": session, "message_ref": key, "project": "/work/candid", "followups": 0, "truncated": false, "origin": "transcript"});
    e.text = vec![IngestText::new("prompt", text)];
    e
}

#[test]
fn ai_prompts_become_agent_conversations_without_answers() {
    let fx = Fx::make("curated-agent");
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![
        prompt("p1", T0, "agent:claude_code:s1", "帮我看看这个报错"),
        prompt("p2", T0 + 60_000, "agent:claude_code:s1", "再把测试补上"),
        prompt("p3", T0 + 90_000, "agent:claude_code:s2", "另一个会话的问题"),
    ])).unwrap();
    let list = call(&fx, &fx.client, "conversations", json!({"kind": "agent"}));
    let convs = list["conversations"].as_array().unwrap();
    assert_eq!(convs.len(), 2, "{list}");
    let c = convs.iter().find(|c| c["conversation_id"] == "agent:claude_code:s1").unwrap();
    assert_eq!((c["messages"].as_i64(), c["app"].as_str(), c["title"].as_str()), (Some(2), Some("claude_code"), Some("candid")));
    let thread = call(&fx, &fx.client, "conversation", json!({"conversation_id": "agent:claude_code:s1"}));
    assert!(thread["messages"].as_array().unwrap().iter().all(|m| m["actor"] == "self"));
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM messages WHERE conversation_id LIKE 'agent:%' AND producer<>'projection/1'"), 0);
    // Other kinds do not list them, and find can be narrowed to them.
    assert!(call(&fx, &fx.client, "conversations", json!({"kind": "mail"}))["conversations"].as_array().unwrap().is_empty());
    let found = call(&fx, &fx.client, "find", json!({"q": "补上", "conversation_kind": "agent"}));
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
    let none = call(&fx, &fx.client, "find", json!({"q": "补上", "conversation_kind": "mail"}));
    assert!(none["results"].as_array().unwrap().is_empty());
    // Deleting the record deletes the message and, with the last one, the conversation.
    let ids = fx.texts("SELECT event_id FROM events WHERE source_key='p3'");
    fx.store.delete_events(&ids, "user").unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM conversations WHERE conversation_id='agent:claude_code:s2'"), 0);
}

fn ax(key: &str, at: i64, app: &str, title: &str, url: Option<&str>, body: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "screen.text", Some(at));
    e.app_name = Some(app.into());
    e.title = Some(title.into());
    let object = match url {
        Some(u) => json!({"basis": "path", "key": u}),
        None => json!({"basis": "title", "key": title}),
    };
    e.payload = json!({"trigger": "timer", "text_status": "captured", "record_evidence": {"format": "candid-record-evidence/1", "object": object, "field": null, "input_value": null, "input_status": "disabled", "active": true, "reading": [], "confirmation": null}});
    e.text = vec![IngestText::new("body", body)];
    e
}

#[test]
fn ax_page_text_becomes_one_excerpt_per_visit() {
    let fx = Fx::make("curated-ax");
    let page = "https://example.com/article";
    let m = 60_000;
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![
        ax("a1", T0, "Google Chrome", "文章 - Google Chrome", Some(page), "导航\n预算会议纪要第一段内容，讨论了下季度各团队的投入和人力安排\n第二段讲报价单，供应商给了三档价格和各自的交付周期"),
        ax("a2", T0 + m, "Google Chrome", "文章 - 内存用量高 - 910 MB - Google Chrome", Some(page), "导航\n预算会议纪要第一段内容，讨论了下季度各团队的投入和人力安排\n第二段讲报价单，供应商给了三档价格和各自的交付周期\n第三段是新滚出来的内容，列出了需要在周五前确认的事项"),
        ax("a3", T0 + 2 * m, "Google Chrome", "文章 - Google Chrome", Some(page), "导航\n第三段是新滚出来的内容，列出了需要在周五前确认的事项"),
        // The same page after a long absence is a new visit.
        ax("a4", T0 + 60 * m, "Google Chrome", "文章 - Google Chrome", Some(page), "晚些时候重新打开读到的第四段内容，这一段比较长所以够得上摘录，讲的是会后各自要跟进的事情和负责人：采购那边下周一给最终报价，财务周三前核对预算余额，项目组周五开会定下最终方案并同步给所有相关的同事"),
        // Left out: private window, AI chat site, a chat client (screenshot path), a window with no URL goes by title.
        ax("x1", T0, "Google Chrome", "秘密页面 - Google Chrome（无痕）", Some("https://secret.example/a"), "不应该出现的无痕页面内容"),
        ax("x2", T0, "Google Chrome", "ChatGPT", Some("https://chatgpt.com/c/1"), "AI 的回答不应该进摘录里面来"),
        ax("x3", T0, "微信", "张三", None, "聊天气泡的内容走截图摘录而不是这里"),
        ax("n1", T0, "Notion", "项目计划", None, "Notion 页面的正文内容在这里面，同样需要足够长才会成为摘录：项目分三个阶段推进，每个阶段都有明确的验收标准，第一阶段完成原型和用户访谈，第二阶段打磨核心流程，第三阶段做灰度发布和数据复盘"),
    ])).unwrap();
    fx.store.curated_sync().unwrap();
    let rows = fx.texts("SELECT title||'|'||COALESCE(url,'')||'|'||start_ms||'|'||end_ms||'|'||replace(text,char(10),'/') FROM excerpts ORDER BY start_ms, key");
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows.contains(&format!("文章|{page}|{}|{}|预算会议纪要第一段内容，讨论了下季度各团队的投入和人力安排/第二段讲报价单，供应商给了三档价格和各自的交付周期/第三段是新滚出来的内容，列出了需要在周五前确认的事项", T0, T0 + 2 * m)), "{rows:?}");
    assert!(rows.iter().any(|r| r.starts_with("文章|") && r.ends_with("晚些时候重新打开读到的第四段内容，这一段比较长所以够得上摘录，讲的是会后各自要跟进的事情和负责人：采购那边下周一给最终报价，财务周三前核对预算余额，项目组周五开会定下最终方案并同步给所有相关的同事")));
    assert!(rows.iter().any(|r| r.starts_with("项目计划||")), "no URL: keyed by app and title");
    // Sources are the events that contributed lines; the repeated page body did not add a1's lines twice.
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM excerpt_sources s JOIN events e ON e.event_id=s.event_id WHERE e.source_key IN ('a1','a2')"), 2);
    // find reaches them; deleting the contributing events removes the excerpt.
    let found = call(&fx, &fx.client, "find", json!({"q": "报价单", "kind": "excerpt"}));
    assert_eq!(found["results"].as_array().unwrap().len(), 1, "{found}");
    let late = fx.texts("SELECT event_id FROM events WHERE source_key='a4'");
    fx.store.delete_events(&late, "user").unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM excerpts WHERE text LIKE '%第四段%'"), 0);
}
