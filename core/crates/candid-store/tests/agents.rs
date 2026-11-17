//! Entirely synthetic transcript fixtures. No default home directory is opened by these tests.
use candid_store::testing::RowExt;
use candid_store::{
    agents::{AgentConsent, AgentRequest},
    *,
};
use serde_json::{json, Value};
use std::{fs, io::Write, path::PathBuf};
const NOW: i64 = 1794398400000;
struct World {
    dir: PathBuf,
    store: TimelineStore,
    client: String,
}
impl World {
    fn new() -> Self {
        let dir = fixtures::temp_root("agents");
        let store = TimelineStore::open(
            &dir.join("db"),
            "synthetic",
            true,
            Clock::fixed(NOW, Zone::named("UTC").unwrap()),
        )
        .unwrap();
        let mut scope = ClientScope::unbounded();
        scope.agents = vec!["codex".into(), "claude_code".into(), "pi".into()];
        let client = store
            .register_client("synthetic", "http", &scope, None)
            .unwrap()
            .client_id;
        Self { dir, store, client }
    }
    fn consent(&self, a: &str) -> AgentConsent {
        let mut c = AgentConsent::new(a);
        c.read = true;
        c.search = true;
        c.activity = true;
        c.paths = vec![self.dir.join(a).to_string_lossy().into()];
        c
    }
    fn save(&self, c: &AgentConsent) {
        self.store.set_agent_consent(c, false).unwrap();
    }
    fn path(&self, a: &str) -> PathBuf {
        self.dir.join(a).join("synthetic.jsonl")
    }
    fn write(&self, a: &str, lines: &[Value]) {
        fs::create_dir_all(self.dir.join(a)).unwrap();
        fs::write(
            self.path(a),
            lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
        )
        .unwrap();
    }
    fn append(&self, a: &str, v: Value) {
        writeln!(
            fs::OpenOptions::new()
                .append(true)
                .open(self.path(a))
                .unwrap(),
            "{v}"
        )
        .unwrap();
    }
    fn sync(&self) -> Value {
        let v = self.store.sync_agents().unwrap();
        assert_eq!(v["issues"], json!([]), "{v}");
        v
    }
    fn count(&self, sql: &str) -> i64 {
        self.store.raw_db().scalar(sql, &[]).unwrap()
    }
    fn strings(&self, sql: &str) -> Vec<String> {
        self.store.raw_db().query(sql, &[], |r| r.text(0)).unwrap()
    }
    fn call(&self, a: &str, action: &str, reference: Option<&str>) -> Result<Value> {
        self.store.agent_call(
            &AgentRequest {
                agent: a.into(),
                action: action.into(),
                reference: reference.map(String::from),
                query: Some("SYNTHETIC".into()),
                ..Default::default()
            },
            &self.client,
        )
    }
}
impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn stamp(sec: i64) -> String {
    chrono::DateTime::from_timestamp_millis(NOW - 30_000 + sec * 1000)
        .unwrap()
        .to_rfc3339()
}
fn cx(sec: i64, t: &str, p: Value) -> Value {
    json!({"timestamp":stamp(sec),"type":t,"payload":p})
}
fn cm(sec: i64, id: &str, role: &str, text: &str) -> Value {
    cx(
        sec,
        "response_item",
        json!({"type":"message","id":id,"role":role,"source":"user","content":[{"type":if role=="user"{"input_text"}else{"output_text"},"text":text}]}),
    )
}
fn header(a: &str) -> Value {
    match a {
        "codex" => cx(
            0,
            "session_meta",
            json!({"id":"synthetic-session","cwd":"/SYNTHETIC/project"}),
        ),
        "pi" => {
            json!({"type":"session","version":3,"id":"synthetic-session","timestamp":stamp(0),"cwd":"/SYNTHETIC/project"})
        }
        _ => {
            json!({"type":"custom-title","sessionId":"synthetic-session","customTitle":"SYNTHETIC title"})
        }
    }
}
fn msg(a: &str, sec: i64, id: &str, role: &str, content: Value) -> Value {
    match a {
        "codex" => cm(sec, id, role, content.as_str().unwrap()),
        "claude_code" => {
            json!({"type":role,"uuid":id,"sessionId":"synthetic-session","timestamp":stamp(sec),"cwd":"/SYNTHETIC/project","message":{"role":role,"content":content}})
        }
        _ => {
            json!({"type":"message","id":id,"parentId":null,"timestamp":stamp(sec),"message":{"role":role,"content":content}})
        }
    }
}

#[test]
fn three_formats_live_read_search_usage_and_idempotent_sync() {
    let w = World::new();
    for a in ["codex", "claude_code", "pi"] {
        w.write(
            a,
            &[
                header(a),
                msg(a, 1, "u1", "user", json!("SYNTHETIC prompt")),
                msg(
                    a,
                    2,
                    "a1",
                    "assistant",
                    json!("SYNTHETIC REPLY-BODY-NEVER-PERSIST"),
                ),
            ],
        );
        w.save(&w.consent(a));
        assert_eq!(
            w.call(a, "sessions", None).unwrap()["sessions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let r = w
            .call(a, "read", Some(&format!("agent:{a}:synthetic-session")))
            .unwrap();
        assert!(r["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "SYNTHETIC REPLY-BODY-NEVER-PERSIST"));
        assert_eq!(r["stored"], false);
        assert_eq!(
            w.call(a, "search", None).unwrap()["hits"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    assert_eq!(w.sync()["inserted"], 6);
    assert_eq!(w.sync()["inserted"], 0);
    assert_eq!(w.sync()["revised"], 0);
    assert_eq!(
        w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"),
        3
    );
    assert_eq!(
        w.count("SELECT COUNT(*) FROM threads WHERE kind='agent_session'"),
        3
    );
    for sql in [
        "SELECT text FROM text_chunks",
        "SELECT cursor FROM source_cursors",
        "SELECT entry_json FROM agent_session_entries",
        "SELECT event_json FROM agent_event_revisions",
        "SELECT payload_json FROM events",
    ] {
        assert!(!w
            .strings(sql)
            .join(" ")
            .contains("REPLY-BODY-NEVER-PERSIST"));
    }
    assert!(w.store.integrity_check(false).unwrap().ok);
}
#[test]
fn only_human_speech_is_admitted_and_slash_expansion_keeps_name() {
    let w = World::new();
    for a in ["codex", "claude_code", "pi"] {
        let mut lines = vec![
            header(a),
            msg(a, 1, "u1", "user", json!("SYNTHETIC genuine")),
        ];
        for (i, t) in [
            "# AGENTS.md instructions for synthetic",
            "<environment_context>injected",
            "<system-reminder>injected",
            "This session is being continued from a previous conversation summary",
            "<teammate-message>transferred",
            "<skill_content>expanded",
            "<codex_delegation><source_thread_id>transfer",
        ]
        .iter()
        .enumerate()
        {
            lines.push(msg(a, 2 + i as i64, &format!("bad{i}"), "user", json!(t)));
        }
        let mut meta = msg(a, 12, "meta", "user", json!("SYNTHETIC meta"));
        meta["isMeta"] = json!(true);
        lines.push(meta);
        let mut transfer = msg(a, 13, "transfer", "user", json!("SYNTHETIC transfer"));
        transfer["sourceSessionId"] = json!("other");
        lines.push(transfer);
        let mut summary = msg(a, 14, "summary", "user", json!("SYNTHETIC summary"));
        summary["isCompactSummary"] = json!(true);
        lines.push(summary);
        if a != "codex" {
            lines.push(msg(a,15,"tool-result","user",json!([{"type":"tool_result","content":"SYNTHETIC tool"},{"type":"text","text":"SYNTHETIC runner annotation"}])));
        }
        lines.push(msg(a,16,"command","user",json!("<command-name>/review</command-name><command-message>expanded secret body</command-message>")));
        w.write(a, &lines);
        w.save(&w.consent(a));
    }
    w.sync();
    let saved = w.strings("SELECT text FROM text_chunks").join("\n");
    assert!(saved.contains("SYNTHETIC genuine") && saved.contains("/review"));
    for bad in [
        "injected",
        "expanded",
        "transferred",
        "meta",
        "summary",
        "runner",
        "transfer",
        "tool",
    ] {
        assert!(!saved.contains(bad), "{saved}");
    }
}
#[test]
fn followups_revise_stable_prompt_and_work_and_preserve_refs() {
    let w = World::new();
    w.write(
        "codex",
        &[header("codex"), cm(1, "u1", "user", "SYNTHETIC first")],
    );
    w.save(&w.consent("codex"));
    w.sync();
    let ids = w.strings("SELECT event_id FROM events ORDER BY kind");
    w.append("codex", cm(2, "u2", "user", "SYNTHETIC followup"));
    assert_eq!(w.sync()["revised"], 2);
    assert_eq!(w.strings("SELECT event_id FROM events ORDER BY kind"), ids);
    let p: Value = serde_json::from_str(
        &w.strings("SELECT payload_json FROM events WHERE kind='ai.prompt'")[0],
    )
    .unwrap();
    assert_eq!(p["followups"], 1);
    assert_eq!(p["messages"].as_array().unwrap().len(), 2);
    assert_eq!(p["messages"][1]["ref"], "agent:codex:synthetic-session#u2");
    w.append("codex",cx(3,"response_item",json!({"type":"function_call","id":"t1","name":"Edit","arguments":"{\"file_path\":\"/SYNTHETIC/project/a.rs\"}"})));
    w.append(
        "codex",
        cm(4, "a1", "assistant", "REPLY-BODY-NEVER-PERSIST"),
    );
    w.sync();
    let work: Value =
        serde_json::from_str(&w.strings("SELECT payload_json FROM events WHERE kind='ai.work'")[0])
            .unwrap();
    assert_eq!(work["tool_calls"]["Edit"], 1);
    assert_eq!(work["files_touched"], 0);
    assert_eq!(work["file_targets"], 1);
    assert_eq!(work["files_touched_complete"], false);
    assert_eq!(work["assistant_messages"], 1);
    assert_eq!(work["status"], "running");
    w.append("codex", cx(5, "event_msg", json!({"type":"task_complete"})));
    w.sync();
    assert!(w.strings("SELECT payload_json FROM events WHERE kind='ai.work'")[0].contains("done"));
    w.append("codex", cm(6, "u3", "user", "SYNTHETIC next task"));
    w.sync();
    assert_eq!(
        w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"),
        2
    );
    assert_eq!(w.count("SELECT COUNT(*) FROM ingest_conflicts"), 0);
}
#[test]
fn timeout_is_interrupted_never_success_and_later_activity_can_resume() {
    let w = World::new();
    w.write(
        "codex",
        &[
            header("codex"),
            cm(-300, "u1", "user", "SYNTHETIC old"),
            cm(-299, "a1", "assistant", "reply"),
        ],
    );
    w.save(&w.consent("codex"));
    w.sync();
    assert!(
        w.strings("SELECT payload_json FROM events WHERE kind='ai.work'")[0]
            .contains("interrupted")
    );
    w.append("codex", cm(20, "a2", "assistant", "continued"));
    w.sync();
    assert!(
        w.strings("SELECT payload_json FROM events WHERE kind='ai.work'")[0].contains("running")
    );
}
#[test]
fn incomplete_tail_rotation_truncation_restart_and_source_removal_preserve_messages() {
    let mut w = World::new();
    w.write(
        "codex",
        &[header("codex"), cm(1, "u1", "user", "SYNTHETIC saved")],
    );
    w.save(&w.consent("codex"));
    w.sync();
    let tail = cm(2, "u2", "user", "SYNTHETIC tail").to_string();
    let split = tail.len() / 2;
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(w.path("codex"))
        .unwrap();
    f.write_all(tail[..split].as_bytes()).unwrap();
    w.sync();
    assert_eq!(w.count("SELECT COUNT(*) FROM agent_event_revisions"), 2);
    f.write_all(tail[split..].as_bytes()).unwrap();
    f.write_all(b"\n").unwrap();
    drop(f);
    w.sync();
    let dir = w.dir.clone();
    let replacement =
        TimelineStore::open(&dir.join("other"), "synthetic", true, Clock::default()).unwrap();
    drop(std::mem::replace(&mut w.store, replacement));
    w.store = TimelineStore::open(
        &dir.join("db"),
        "synthetic",
        false,
        Clock::fixed(NOW, Zone::named("UTC").unwrap()),
    )
    .unwrap();
    assert_eq!(w.sync()["revised"], 0);
    fs::rename(w.path("codex"), w.dir.join("codex/renamed.jsonl")).unwrap();
    assert_eq!(w.sync()["inserted"], 0);
    assert_eq!(w.sync()["revised"], 0);
    fs::write(
        w.dir.join("codex/renamed.jsonl"),
        format!(
            "{}\n{}\n",
            header("codex"),
            cm(3, "u3", "user", "SYNTHETIC after truncate")
        ),
    )
    .unwrap();
    w.sync();
    let t = w.strings("SELECT text FROM text_chunks").join(" ");
    assert!(t.contains("saved") && t.contains("tail") && t.contains("after truncate"));
    fs::remove_file(w.dir.join("codex/renamed.jsonl")).unwrap();
    w.sync();
    assert_eq!(w.count("SELECT COUNT(*) FROM events"), 2);
    assert!(w
        .call("codex", "read", Some("agent:codex:synthetic-session"))
        .is_err());
}
#[test]
fn permissions_search_and_index_deletion_follow_same_policy() {
    let w = World::new();
    w.write(
        "codex",
        &[
            header("codex"),
            cm(1, "u1", "user", "SYNTHETIC confidential"),
        ],
    );
    let mut c = w.consent("codex");
    c.read = false;
    c.search = false;
    c.activity = false;
    w.save(&c);
    assert_eq!(w.sync()["inserted"], 0);
    assert!(w
        .call("codex", "read", Some("agent:codex:synthetic-session"))
        .is_err());
    c.read = true;
    c.search = true;
    c.activity = true;
    w.save(&c);
    w.sync();
    let req = SearchRequest {
        q: Some("confidential".into()),
        ..Default::default()
    };
    assert_eq!(w.store.search(&req, &w.client).unwrap().hits.len(), 1);
    let denied = w
        .store
        .register_client("no agents", "http", &ClientScope::unbounded(), None)
        .unwrap()
        .client_id;
    assert!(w.store.search(&req, &denied).unwrap().hits.is_empty());
    assert!(w
        .store
        .agent_call(
            &AgentRequest {
                agent: "codex".into(),
                action: "read".into(),
                reference: Some("agent:codex:synthetic-session".into()),
                ..Default::default()
            },
            &denied
        )
        .is_err());
    c.search = false;
    w.save(&c);
    assert!(w.store.search(&req, &w.client).unwrap().hits.is_empty());
    assert!(w.call("codex", "search", None).is_err());
    assert!(w
        .call("codex", "read", Some("agent:codex:synthetic-session"))
        .is_ok());
    c.search = true;
    c.activity = false;
    w.save(&c);
    w.append("codex", cm(2, "u2", "user", "SYNTHETIC not admitted"));
    assert_eq!(w.sync()["inserted"], 0);
    assert!(!w
        .strings("SELECT text FROM text_chunks")
        .join(" ")
        .contains("not admitted"));
    c.read = false;
    c.search = false;
    w.save(&c);
    assert!(w.store.search(&req, &w.client).unwrap().hits.is_empty());
    w.store.set_agent_consent(&c, true).unwrap();
    for table in [
        "events",
        "event_text",
        "text_chunks",
        "source_cursors",
        "agent_session_entries",
        "agent_file_counts",
        "agent_event_revisions",
        "threads",
    ] {
        assert_eq!(
            w.count(&format!("SELECT COUNT(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    c.read = true;
    c.search = true;
    c.activity = true;
    w.save(&c);
    w.sync();
    assert!(!w
        .strings("SELECT text FROM text_chunks")
        .join(" ")
        .contains("confidential"));
}
#[test]
fn limits_redaction_exclusions_and_live_reply_pagination() {
    let w = World::new();
    let fake = format!("sk-{}", "A".repeat(32));
    w.write(
        "pi",
        &[
            header("pi"),
            msg(
                "pi",
                1,
                "u1",
                "user",
                json!(format!("{fake} SYNTHETIC prompt that is long")),
            ),
            msg("pi", 2, "a1", "assistant", json!("SYNTHETIC reply")),
        ],
    );
    let mut c = w.consent("pi");
    c.prompt_max_chars = Some(20);
    w.save(&c);
    w.sync();
    let texts = w.strings("SELECT text FROM text_chunks").join(" ");
    assert!(!texts.contains(&fake));
    assert!(texts.contains("[REDACTED]"));
    let p: Value = serde_json::from_str(
        &w.strings("SELECT payload_json FROM events WHERE kind='ai.prompt'")[0],
    )
    .unwrap();
    assert_eq!(p["truncated"], true);
    assert!(!w
        .strings("SELECT cursor FROM source_cursors")
        .join(" ")
        .contains("that is long"));
    let mut req = AgentRequest {
        agent: "pi".into(),
        action: "read".into(),
        reference: Some("agent:pi:synthetic-session".into()),
        limit: Some(1),
        ..Default::default()
    };
    let page = w.store.agent_call(&req, &w.client).unwrap();
    assert!(page["messages"][0]["text"]
        .as_str()
        .unwrap()
        .contains("that is long"));
    req.cursor = page["nextCursor"].as_str().map(String::from);
    assert!(req.cursor.is_some());
    let page = w.store.agent_call(&req, &w.client).unwrap();
    assert_eq!(page["messages"][0]["role"], "assistant");
    c.exclude.projects = vec!["/SYNTHETIC".into()];
    w.save(&c);
    assert!(w
        .call("pi", "read", Some("agent:pi:synthetic-session"))
        .is_err());
    let result = w
        .store
        .search(
            &SearchRequest {
                q: Some("SYNTHETIC".into()),
                ..Default::default()
            },
            &w.client,
        )
        .unwrap();
    assert!(result.hits.is_empty());
}
#[test]
fn unknown_format_and_failed_ingest_do_not_advance_checkpoint() {
    let w = World::new();
    w.write(
        "pi",
        &[json!({"type":"session","version":99,"id":"future"})],
    );
    w.save(&w.consent("pi"));
    assert_eq!(
        w.store.sync_agents().unwrap()["issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(w.count("SELECT COUNT(*) FROM source_cursors"), 0);
    // Unchanged, the unreadable file is skipped on the next pass but still reported; once it changes it is read again.
    assert_eq!(w.store.sync_agents().unwrap()["issues"].as_array().unwrap().len(), 1);
    w.write(
        "pi",
        &[
            header("pi"),
            msg("pi", 1, "u1", "user", json!("SYNTHETIC retry")),
        ],
    );
    w.store.raw_db().script("CREATE TRIGGER synthetic_failure BEFORE INSERT ON agent_event_revisions BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    assert!(!w.store.sync_agents().unwrap()["issues"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(w.count("SELECT COUNT(*) FROM events"), 0);
    assert_eq!(w.count("SELECT COUNT(*) FROM source_cursors"), 0);
    w.store
        .raw_db()
        .execute("DROP TRIGGER synthetic_failure", &[])
        .unwrap();
    assert_eq!(w.sync()["inserted"], 2);
}

#[test]
fn usage_content_controls_and_tool_bodies_remain_live_only() {
    let w = World::new();
    let mut assistant = msg(
        "claude_code",
        3,
        "a1",
        "assistant",
        json!([{"type":"text","text":"SYNTHETIC answer"},{"type":"thinking","thinking":"REASONING-ONLY-IN-SOURCE"},{"type":"tool_use","name":"Write","id":"t1","input":{"file_path":"/SYNTHETIC/project/file.rs","content":"TOOL-ARGUMENT-BODY"}}]),
    );
    assistant["message"]["id"] = json!("api-message");
    assistant["message"]["usage"] = json!({"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3});
    let mut second = assistant.clone();
    second["uuid"] = json!("a2");
    second["timestamp"] = json!(stamp(4));
    second["message"]["content"] = json!([]);
    second["message"]["usage"]["output_tokens"] = json!(9);
    let result = msg(
        "claude_code",
        5,
        "result",
        "user",
        json!([{"type":"tool_result","tool_use_id":"t1","content":"OUTPUT-ONLY-IN-SOURCE"}]),
    );
    w.write(
        "claude_code",
        &[
            header("claude_code"),
            msg("claude_code", 1, "u1", "user", json!("SYNTHETIC user")),
            assistant,
            second,
            result,
        ],
    );
    let mut c = w.consent("claude_code");
    w.save(&c);
    w.sync();
    let usage = w.call("claude_code", "usage", None).unwrap();
    assert_eq!(usage["totals"]["input"], 11);
    assert_eq!(usage["totals"]["output"], 9);
    assert_eq!(usage["totals"]["cacheRead"], 5);
    assert_eq!(usage["totals"]["cacheWrite"], 3);
    let read = w
        .call(
            "claude_code",
            "read",
            Some("agent:claude_code:synthetic-session"),
        )
        .unwrap()
        .to_string();
    assert!(
        !read.contains("OUTPUT-ONLY")
            && !read.contains("REASONING-ONLY")
            && !read.contains("TOOL-ARGUMENT")
    );
    assert!(read.contains("Write"));
    c.content.tool_output = true;
    c.content.reasoning = true;
    c.content.assistant = false;
    c.content.tool_calls = "none".into();
    w.save(&c);
    let read = w
        .call(
            "claude_code",
            "read",
            Some("agent:claude_code:synthetic-session"),
        )
        .unwrap()
        .to_string();
    assert!(read.contains("OUTPUT-ONLY") && read.contains("REASONING-ONLY"));
    assert!(!read.contains("SYNTHETIC answer"));
    for sql in [
        "SELECT cursor FROM source_cursors",
        "SELECT entry_json FROM agent_session_entries",
        "SELECT event_json FROM agent_event_revisions",
        "SELECT payload_json FROM events",
        "SELECT text FROM text_chunks",
    ] {
        let persisted = w.strings(sql).join(" ");
        for forbidden in ["OUTPUT-ONLY", "REASONING-ONLY", "TOOL-ARGUMENT"] {
            assert!(!persisted.contains(forbidden));
        }
    }
}
#[test]
fn identical_text_at_distinct_occurrences_keeps_distinct_identity() {
    let w = World::new();
    let mut u = cm(1, "unused", "user", "SYNTHETIC repeated");
    u["payload"].as_object_mut().unwrap().remove("id");
    w.write(
        "codex",
        &[
            header("codex"),
            u.clone(),
            cm(2, "a1", "assistant", "working"),
            u,
        ],
    );
    w.save(&w.consent("codex"));
    w.sync();
    assert_eq!(
        w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"),
        2
    );
    assert_eq!(w.sync()["inserted"], 0);
}
#[test]
fn source_redaction_masks_all_previous_secret_families() {
    let sk = format!("sk-{}", "A".repeat(32));
    let aws = format!("AKIA{}", "A".repeat(16));
    let gh = format!("ghp_{}", "B".repeat(30));
    let bearer = format!("Bearer {}", "c".repeat(24));
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nSYNTHETIC\n-----END RSA PRIVATE KEY-----";
    for secret in [sk, aws, gh, bearer, pem.into()] {
        let redacted = agents::redact(&secret);
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("SYNTHETIC"));
    }
    assert_eq!(agents::redact("plain sk-short text"), "plain sk-short text");
}

#[test]
fn codex_admission_requires_native_user_signal_not_just_a_user_role() {
    let w = World::new();
    let mut unproven = cm(
        1,
        "injected",
        "user",
        "SYNTHETIC arbitrary runner injection",
    );
    unproven["payload"]
        .as_object_mut()
        .unwrap()
        .remove("source");
    let mut user = cm(2, "native-response", "user", "SYNTHETIC actual user");
    user["payload"].as_object_mut().unwrap().remove("source");
    w.write(
        "codex",
        &[
            header("codex"),
            unproven,
            user,
            cx(
                2,
                "event_msg",
                json!({"type":"user_message","message":"SYNTHETIC actual user"}),
            ),
            cm(3, "a1", "assistant", "SYNTHETIC reply"),
        ],
    );
    w.save(&w.consent("codex"));
    w.sync();
    assert_eq!(
        w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"),
        1
    );
    let persisted = w.strings("SELECT text FROM text_chunks").join(" ");
    assert_eq!(persisted, "SYNTHETIC actual user");
    assert!(!w
        .strings("SELECT cursor FROM source_cursors")
        .join(" ")
        .contains("arbitrary runner"));
    let read = w
        .call("codex", "read", Some("agent:codex:synthetic-session"))
        .unwrap();
    assert_eq!(read["messages"].as_array().unwrap().len(), 2);
}

#[test]fn existing_search_and_reference_routes_keep_replies_external_and_prompts_unique(){
    let w=World::new();w.write("codex",&[header("codex"),cm(1,"u1","user","SYNTHETIC pineapple prompt"),cm(2,"a1","assistant","SYNTHETIC pineapple reply")]);w.save(&w.consent("codex"));w.sync();
    let result=w.store.call_tool("search",br#"{"q":"pineapple"}"#,&w.client).unwrap();let hits=result["hits"].as_array().unwrap();assert_eq!(hits.len(),2);assert_eq!(hits.iter().filter(|h|h["layer"]=="external").count(),1);assert_eq!(hits.iter().filter(|h|h["kind"]=="ai.prompt").count(),1);
    for (tool,key) in [("get_event","event_id"),("thread","thread_id")]{let args=json!({key:"agent:codex:synthetic-session"});let r=w.store.call_tool(tool,&serde_json::to_vec(&args).unwrap(),&w.client).unwrap();assert_eq!(r["stored"],false);assert_eq!(r["messages"].as_array().unwrap().len(),2);}
}

#[test]fn changing_search_policy_does_not_erase_admitted_history_after_truncation(){
    let w=World::new();w.write("codex",&[header("codex"),cm(1,"u1","user","SYNTHETIC retained first"),cm(2,"u2","user","SYNTHETIC retained followup")]);let mut c=w.consent("codex");w.save(&c);w.sync();
    c.search=false;w.save(&c);w.write("codex",&[header("codex"),cm(3,"u3","user","SYNTHETIC rotated tail")]);w.sync();
    let texts=w.strings("SELECT text FROM text_chunks").join(" ");assert!(texts.contains("retained first")&&texts.contains("retained followup")&&texts.contains("rotated tail"));
}
#[test]fn failed_source_check_reports_error_without_advancing_last_sync(){
    let w=World::new();w.write("pi",&[json!({"type":"session","version":99,"id":"future"})]);w.save(&w.consent("pi"));w.store.sync_agents().unwrap();
    let source=w.store.sources().unwrap().into_iter().find(|s|s.kind=="agent.transcript"&&s.display_name=="pi").unwrap();assert_eq!(source.status,"error");assert_eq!(source.last_sync_ms,None);
    w.write("pi",&[header("pi"),msg("pi",1,"u1","user",json!("SYNTHETIC retry"))]);w.sync();let source=w.store.sources().unwrap().into_iter().find(|s|s.kind=="agent.transcript"&&s.display_name=="pi").unwrap();assert_eq!(source.status,"active");assert_eq!(source.last_sync_ms,Some(NOW));
}

#[test]fn live_search_keeps_case_diacritic_snippets_and_deadline_behavior(){
    let w=World::new();w.write("pi",&[header("pi"),msg("pi",1,"u1","user",json!(format!("{} Café Pineapple SYNTHETIC","z".repeat(3000))))]);w.save(&w.consent("pi"));
    let mut r=AgentRequest{agent:"pi".into(),action:"search".into(),query:Some("CAFE".into()),..Default::default()};
    let result=w.store.agent_call(&r,&w.client).unwrap();assert_eq!(result["hits"].as_array().unwrap().len(),1);assert!(result["hits"][0]["snippet"].as_str().unwrap().contains("Café"));
    r.deadline_ms=Some(0);let result=w.store.agent_call(&r,&w.client).unwrap();assert_eq!(result["limited"],true);assert_eq!(result["hits"],json!([]));
    assert!(agents::redact("-----BEGIN CERTIFICATE-----\nSYNTHETIC CERT\n-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\r\nPRIVATE BODY\r\n-----END PRIVATE KEY-----").contains("[REDACTED]"));
    assert!(!agents::redact("-----BEGIN PRIVATE KEY-----\r\nPRIVATE BODY\r\n-----END PRIVATE KEY-----").contains("PRIVATE BODY"));
}

#[test]fn excluded_project_stops_at_metadata_and_normalises_scope_for_retained_rows(){
    let w=World::new();w.write("pi",&[header("pi"),msg("pi",1,"u1","user",json!("SYNTHETIC retained"))]);let mut c=w.consent("pi");w.save(&c);w.sync();
    c.exclude.projects=vec!["/SYNTHETIC/../SYNTHETIC".into()];w.save(&c);
    fs::write(w.path("pi"),format!("{}\nNOT-JSON-BODY-MUST-NOT-BE-READ\n",header("pi"))).unwrap();w.sync();
    assert!(w.store.search(&SearchRequest{q:Some("retained".into()),..Default::default()},&w.client).unwrap().hits.is_empty());
    assert!(w.call("pi","read",Some("agent:pi:synthetic-session")).is_err());assert_eq!(w.count("SELECT COUNT(*) FROM events"),2);
}

#[test]fn changed_file_count_requires_successful_tool_result_and_deduplicates_targets(){
    let w=World::new();let assistant=msg("claude_code",2,"a1","assistant",json!([{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/SYNTHETIC/project/a.rs"}},{"type":"tool_use","id":"t2","name":"Write","input":{"file_path":"/SYNTHETIC/project/b.rs"}}]));
    w.write("claude_code",&[header("claude_code"),msg("claude_code",1,"u1","user",json!("SYNTHETIC change files")),assistant]);w.save(&w.consent("claude_code"));w.sync();
    w.append("claude_code",msg("claude_code",3,"r1","user",json!([{"type":"tool_result","tool_use_id":"t1","content":"PRIVATE RESULT SUCCESS","is_error":false},{"type":"tool_result","tool_use_id":"t2","content":"PRIVATE RESULT FAILURE","is_error":true}])));w.sync();
    let work:Value=serde_json::from_str(&w.strings("SELECT payload_json FROM events WHERE kind='ai.work'")[0]).unwrap();assert_eq!(work["files_touched"],1);assert_eq!(work["file_targets"],2);assert_eq!(work["files_touched_complete"],true);
    assert!(!w.strings("SELECT cursor FROM source_cursors").join(" ").contains("PRIVATE RESULT"));
}

#[test]fn merged_prompt_limit_also_bounds_checkpoint_text_when_source_rotates(){
    let w=World::new();w.write("pi",&[header("pi"),msg("pi",1,"u1","user",json!("FIRST-1234"))]);let mut c=w.consent("pi");c.prompt_max_chars=Some(10);w.save(&c);w.sync();
    w.write("pi",&[header("pi"),msg("pi",2,"u2","user",json!("FOLLOWUP-PRIVATE-TAIL"))]);w.sync();
    for sql in ["SELECT cursor FROM source_cursors","SELECT event_json FROM agent_event_revisions","SELECT text FROM text_chunks"]{assert!(!w.strings(sql).join(" ").contains("FOLLOWUP"));}
    let p:Value=serde_json::from_str(&w.strings("SELECT payload_json FROM events WHERE kind='ai.prompt'")[0]).unwrap();assert_eq!(p["truncated"],true);assert_eq!(p["followups"],1);
}

#[test]fn derived_summaries_inherit_ai_permissions_and_leave_default_results_on_revision(){
    let w=World::new();w.write("codex",&[header("codex"),cm(1,"u1","user","SYNTHETIC input")]);let mut c=w.consent("codex");w.save(&c);w.sync();
    let source=w.store.agent_source_id("codex").unwrap().unwrap();let event=w.store.event_id(&source,"agent:codex:synthetic-session#u1").unwrap().unwrap();
    w.store.raw_db().conn.execute("INSERT INTO episodes VALUES('synthetic-episode',1,?,?,'SYNTHETIC title','SYNTHETIC summary','activity','draft','synthetic/1',NULL,NULL,NULL,?)",(NOW-30_000,NOW,NOW)).unwrap();
    w.store.raw_db().conn.execute("INSERT INTO episode_evidence VALUES('synthetic-episode',1,?,'primary','{}')",[&event]).unwrap();
    let req=EpisodeRequest{episode_id:"synthetic-episode".into(),..Default::default()};assert!(w.store.episode(&req,&w.client).is_ok());
    let denied=w.store.register_client("SYNTHETIC no agents","http",&ClientScope::unbounded(),None).unwrap().client_id;assert!(w.store.episode(&req,&denied).is_err());
    c.read=false;c.search=false;c.activity=false;w.save(&c);assert!(w.store.episode(&req,&w.client).is_err());
    c.read=true;c.search=true;c.activity=true;w.save(&c);w.append("codex",cm(2,"u2","user","SYNTHETIC followup"));w.sync();
    assert_eq!(w.strings("SELECT status FROM episodes"),vec!["superseded"]);assert!(w.store.episodes(&EpisodesRequest::default(),&w.client).unwrap().episodes.is_empty());
}


/// Modification time intentionally disagrees with source timestamps. No real sessions are opened.
#[test]
fn rotated_fragments_preserve_chronology_live_reads_and_atomic_checkpoints() {
    for reverse in [false, true] {
        let w = World::new();
        let old = w.path("codex");
        w.write("codex", &[header("codex"), cm(1,"u1","user","SYNTHETIC EARLY"), cm(2,"a1","assistant","SYNTHETIC early reply")]);
        let new = w.dir.join("codex/newer.jsonl");
        let lines = [header("codex"),cm(3,"u2","user","SYNTHETIC LATER"),cm(4,"a2","assistant","SYNTHETIC later reply")];
        fs::write(&new,lines.iter().map(|v|format!("{v}\n")).collect::<String>()).unwrap();
        let base = std::time::SystemTime::UNIX_EPOCH;
        for (path,seconds) in [(&old,if reverse {2} else {1}),(&new,if reverse {1} else {2})] {
            fs::File::open(path).unwrap().set_times(fs::FileTimes::new().set_modified(base+std::time::Duration::from_secs(seconds))).unwrap();
        }
        w.save(&w.consent("codex"));
        assert_eq!(w.sync()["inserted"],4);
        assert_eq!(w.count("SELECT end_ms-start_ms FROM events WHERE source_key LIKE '%#u2:work'"),1000);
        assert_eq!(w.count("SELECT end_ms-start_ms FROM events WHERE source_key LIKE '%#u1:work'"),2000);
        assert_eq!(w.sync()["revised"],0);
        assert_eq!(w.count("SELECT COUNT(*) FROM source_cursors WHERE stream LIKE 'file:%'"),2);
        let live=w.call("codex","read",Some("agent:codex:synthetic-session#u1")).unwrap();
        let messages=live["messages"].as_array().unwrap();
        assert_eq!(messages.len(),4);
        assert_eq!(messages[0]["text"],"SYNTHETIC EARLY");
        assert_eq!(messages[2]["text"],"SYNTHETIC LATER");
        assert!(w.store.integrity_check(false).unwrap().ok);
    }
}

#[test]
fn followups_in_separate_files_are_grouped_before_any_event_is_written() {
    let w=World::new();
    w.write("codex",&[header("codex"),cm(1,"u1","user","SYNTHETIC FIRST")]);
    let lines=[header("codex"),cm(2,"u2","user","SYNTHETIC FOLLOWUP"),cm(3,"a1","assistant","reply")];
    fs::write(w.dir.join("codex/followup.jsonl"),lines.iter().map(|v|format!("{v}\n")).collect::<String>()).unwrap();
    w.save(&w.consent("codex"));
    assert_eq!(w.sync()["inserted"],2);
    assert_eq!(w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"),1);
    assert_eq!(w.count("SELECT COUNT(*) FROM events WHERE source_key LIKE '%#u2%'"),0);
    let payloads=w.strings("SELECT payload_json FROM events WHERE kind='ai.prompt'");
    let payload:Value=serde_json::from_str(&payloads[0]).unwrap();
    assert_eq!(payload["followups"],1);
    assert_eq!(payload["messages"][0]["ref"],"agent:codex:synthetic-session#u1");
    assert_eq!(w.sync()["revised"],0);
}

#[test]
fn historical_fragment_backfill_repairs_order_without_changing_existing_event_ids() {
    let w=World::new();
    w.write("codex",&[header("codex"),cm(3,"u2","user","SYNTHETIC LATER"),cm(4,"a2","assistant","later reply")]);
    w.save(&w.consent("codex"));w.sync();
    let ids=w.strings("SELECT event_id FROM events WHERE source_key LIKE '%#u2%' ORDER BY kind");
    let lines=[header("codex"),cm(1,"u1","user","SYNTHETIC EARLY"),cm(2,"a1","assistant","early reply")];
    fs::write(w.dir.join("codex/historical.jsonl"),lines.iter().map(|v|format!("{v}\n")).collect::<String>()).unwrap();
    assert_eq!(w.sync()["inserted"],2);
    assert_eq!(w.count("SELECT end_ms-start_ms FROM events WHERE source_key LIKE '%#u2:work'"),1000);
    assert_eq!(ids,w.strings("SELECT event_id FROM events WHERE source_key LIKE '%#u2%' ORDER BY kind"));
    assert_eq!(w.sync()["revised"],0);
}

#[test]
fn message_time_candidates_include_late_followups_and_preserve_exact_source() {
    let w=World::new(); w.save(&w.consent("codex"));
    w.write("codex", &[header("codex"),cm(1,"first","user","SYNTHETIC first")]);w.sync();
    w.append("codex",cm(25,"late","user","SYNTHETIC  exact\n\n尾部 👩🏽‍💻"));w.sync();
    let v=w.store.call_tool("source_query",&serde_json::to_vec(&json!({"action":"candidates","start_ms":NOW-6000,"end_ms":NOW,"sessions":["agent:codex:synthetic-session"]})).unwrap(),&w.client).unwrap();
    assert_eq!(v["incomplete"],false,"{v}");assert_eq!(v["messages"].as_array().unwrap().len(),1);
    assert_eq!(v["messages"][0]["text"],"SYNTHETIC  exact\n\n尾部 👩🏽‍💻");
    assert!(v["messages"][0]["source_hash"].as_str().unwrap().len()>10);
    let mut c=w.consent("codex");c.activity=false;w.save(&c);
    let hidden=w.store.call_tool("source_query",&serde_json::to_vec(&json!({"start_ms":NOW-6000,"end_ms":NOW})).unwrap(),&w.client).unwrap();
    assert_eq!(hidden["messages"],json!([]));
}

#[test]
fn linked_note_inherits_target_deletion_and_unknown_links_stay_hidden() {
    let w=World::new();w.save(&w.consent("codex"));
    w.write("codex", &[header("codex"),cm(1,"first","user","SYNTHETIC target")]);w.sync();
    let target=w.strings("SELECT event_id FROM events WHERE kind='ai.prompt'")[0].clone();
    let links=w.store.call_tool("source_query",&serde_json::to_vec(&json!({"action":"reference","reference":format!("candid://event/{target}")})).unwrap(),&w.client).unwrap();
    let source=w.store.register_source("manual","synthetic notes",&SourcePolicy::default()).unwrap();
    let mut event=IngestEvent::new("synthetic-note","note",Some(NOW));event.text=vec![IngestText::new("body","SYNTHETIC linked note")];event.payload=json!({"source_links":links});
    let mut unknown=event.clone();unknown.source_key="unknown-links".into();unknown.payload["source_links"]["format"]=json!("candid-source-links/99");
    assert!(w.store.ingest(&IngestBatch::events(&source,vec![unknown])).is_err());
    w.store.ingest(&IngestBatch::events(&source,vec![event])).unwrap();
    let args=serde_json::to_vec(&json!({"q":"linked note"})).unwrap();
    assert_eq!(w.store.call_tool("search",&args,&w.client).unwrap()["hits"].as_array().unwrap().len(),1);
    w.store.delete_events(&[target],"user").unwrap();
    assert_eq!(w.store.call_tool("search",&args,&w.client).unwrap()["hits"],json!([]));
}

#[test]
fn external_proof_revision_and_note_cannot_outlive_activity_or_target() {
    let w=World::new();w.save(&w.consent("codex"));w.write("codex",&[header("codex"),cm(1,"first","user","SYNTHETIC exact tail")]);w.sync();
    let body:Value=serde_json::from_str(&w.strings("SELECT payload_json FROM events WHERE kind='ai.prompt'")[0]).unwrap();
    let m=&body["messages"][0];
    let screen=w.store.register_source("mac.desktop","synthetic screen",&SourcePolicy::default()).unwrap();
    let mut raw=IngestEvent::new("raw","screen.text",Some(NOW-29000));raw.app_bundle_id=Some("synthetic.chat".into());raw.text=vec![IngestText::new("body","SYNTHETIC independent screen")];
    let mut proof=IngestEvent::new("proof","screen.text",Some(NOW-28000));proof.app_bundle_id=raw.app_bundle_id.clone();proof.text=vec![IngestText::new("message","SYNTHETIC exact tail")];
    proof.payload=json!({"record_evidence":{"format":"candid-record-evidence/2","object":{"key":"agent:codex:synthetic-session","basis":"sidebar"},"field":"window:composer","input_value":"SYNTHETIC exact tail","input_status":"captured","active":true,"reading":[],"confirmation":{"observation":"raw","message_id":m["ref"],"source_hash":m["source_hash"],"text":"SYNTHETIC exact tail","basis":"source_send"}}});
    w.store.ingest(&IngestBatch::events(&screen,vec![raw,proof])).unwrap();
    let proof=w.strings("SELECT event_id FROM events WHERE source_key='proof'")[0].clone();
    let revision=w.store.stage_record_revision(&screen,records::Rules::default()).unwrap();
    assert!(w.store.record_revision(&revision,&w.client).is_ok());
    let projected=w.store.call_tool("timeline",br#"{"start":"2026-11-11T00:00:00Z","end":"2026-11-12T00:00:00Z","kinds":["ai.prompt"],"level":"event"}"#,&w.client).unwrap();
    assert_eq!(projected["entries"].as_array().unwrap().len(),1,"{projected}");
    let get=serde_json::to_vec(&json!({"event_id":proof,"include":["text"]})).unwrap();
    assert!(w.store.call_tool("get_event",&get,&w.client).is_ok());
    let links=w.store.call_tool("source_query",&serde_json::to_vec(&json!({"action":"reference","reference":format!("candid://event/{proof}")})).unwrap(),&w.client).unwrap();
    assert_eq!(links["refs"].as_array().unwrap().len(),2);
    let notes=w.store.register_source("manual","synthetic notes",&SourcePolicy::default()).unwrap();
    let mut note=IngestEvent::new("n","note",Some(NOW));note.payload=json!({"source_links":links});note.text=vec![IngestText::new("body","SYNTHETIC attached proof")];
    w.store.ingest(&IngestBatch::events(&notes,vec![note])).unwrap();
    let mut narrowed=w.consent("codex");narrowed.paths=vec![w.dir.join("different-authorized-root").to_string_lossy().into_owned()];w.save(&narrowed);
    assert!(w.store.call_tool("get_event",&get,&w.client).is_err());
    assert!(w.store.record_revision(&revision,&w.client).is_err());
    assert_eq!(w.store.call_tool("search",br#"{"q":"attached proof"}"#,&w.client).unwrap()["hits"],json!([]));
    let mut c=w.consent("codex");c.activity=false;w.save(&c);
    assert!(w.store.call_tool("get_event",&get,&w.client).is_err());
    assert!(w.store.record_revision(&revision,&w.client).is_err());
    assert_eq!(w.store.call_tool("search",br#"{"q":"attached proof"}"#,&w.client).unwrap()["hits"],json!([]));
    assert_eq!(w.store.call_tool("search",br#"{"q":"independent screen"}"#,&w.client).unwrap()["hits"].as_array().unwrap().len(),1);
}

#[test]
fn unsynced_duplicate_and_oversize_source_never_confirm_unique_send() {
    let w=World::new();w.save(&w.consent("codex"));w.write("codex",&[header("codex"),cm(25,"first","user","SYNTHETIC same")]);w.sync();
    w.append("codex",cm(26,"second","user","SYNTHETIC same"));
    let args=serde_json::to_vec(&json!({"start_ms":NOW-6000,"end_ms":NOW})).unwrap();
    let v=w.store.call_tool("source_query",&args,&w.client).unwrap();assert_eq!(v["incomplete"],true,"{v}");
    w.sync();let v=w.store.call_tool("source_query",&args,&w.client).unwrap();assert_eq!(v["messages"].as_array().unwrap().len(),2);
    assert_eq!(w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"),2);
    fs::OpenOptions::new().write(true).open(w.path("codex")).unwrap().set_len(9*1024*1024).unwrap();
    let v=w.store.call_tool("source_query",&args,&w.client).unwrap();assert_eq!(v["incomplete"],true);
}

#[test]
fn candidate_cursor_reaches_revised_old_instruction_and_rejects_changed_snapshot() {
    let w=World::new();w.save(&w.consent("codex"));
    let mut lines=vec![header("codex")];
    for i in 0..130 {lines.push(cm(1,&format!("u{i}"),"user",&format!("SYNTHETIC instruction {i}")));lines.push(cm(2,&format!("a{i}"),"assistant","SYNTHETIC reply"));}
    w.write("codex",&lines);w.sync();
    lines.insert(2,cm(25,"late","user","SYNTHETIC late old followup"));w.write("codex",&lines);w.sync();
    let mut args=json!({"start_ms":NOW-6000,"end_ms":NOW});
    let first=w.store.call_tool("source_query",&serde_json::to_vec(&args).unwrap(),&w.client).unwrap();
    assert_eq!(first["incomplete"],false,"{first}");assert_eq!(first["messages"][0]["text"],"SYNTHETIC late old followup");assert_eq!(first["next_cursor"],Value::Null);
    for i in 0..130 {w.append("codex",cm(26,&format!("recent-u{i}"),"user",&format!("SYNTHETIC recent {i}")));w.append("codex",cm(27,&format!("recent-a{i}"),"assistant","SYNTHETIC reply"));}w.sync();
    args["sessions"]=json!(["agent:codex:unselected-session"]);
    let page=w.store.call_tool("source_query",&serde_json::to_vec(&args).unwrap(),&w.client).unwrap();
    assert_eq!(page["incomplete"],false);assert!(page["next_cursor"].is_string());
    args["cursor"]=page["next_cursor"].clone();
    let second=w.store.call_tool("source_query",&serde_json::to_vec(&args).unwrap(),&w.client).unwrap();
    assert_eq!(second["incomplete"],false);assert_eq!(second["next_cursor"],Value::Null);
    w.append("codex",cm(26,"new","user","SYNTHETIC new revision"));w.sync();
    assert!(w.store.call_tool("source_query",&serde_json::to_vec(&args).unwrap(),&w.client).is_err());
}

#[test]
fn old_file_checkpoints_replay_to_recover_source_provenance_without_duplicate_activity() {
    let w=World::new();w.save(&w.consent("codex"));w.write("codex",&[header("codex"),cm(1,"first","user","SYNTHETIC old checkpoint")]);w.sync();
    w.store.raw_db().execute("UPDATE source_cursors SET cursor=json_set(json_remove(cursor,'$.checkpoint_format'),'$.entries',json((SELECT json_group_array(json_remove(value,'$.source_file','$.source_line')) FROM json_each(cursor,'$.entries'))))",&[]).unwrap();
    // An old parser checkpoint is encountered on process restart, before the per-store file cache warms.
    w.store.raw_db().execute("PRAGMA wal_checkpoint(TRUNCATE)",&[]).unwrap();
    let reopened=w.dir.join("reopened");fs::create_dir(&reopened).unwrap();
    for name in ["timeline.sqlite","store.json"] {fs::copy(w.dir.join("db").join(name),reopened.join(name)).unwrap();}
    let store=TimelineStore::open(&reopened,"SYNTHETIC",false,Clock::fixed(NOW,Zone::named("UTC").unwrap())).unwrap();
    assert_eq!(store.sync_agents().unwrap()["issues"],json!([]));
    let db=store.raw_db();
    assert_eq!(db.scalar("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'",&[]).unwrap(),1);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM agent_session_entries WHERE COALESCE(json_extract(entry_json,'$.source_file'),'')=''",&[]).unwrap(),0);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM source_cursors WHERE stream LIKE 'file:%' AND json_extract(cursor,'$.checkpoint_format')='agent-source-checkpoint/4'",&[]).unwrap(),1);
}

#[test]
fn conflicting_message_identity_across_files_remains_unconfirmed() {
    let w=World::new();w.save(&w.consent("codex"));w.write("codex",&[header("codex"),cm(25,"u1","user","SYNTHETIC first")]);w.sync();
    fs::write(w.dir.join("codex/conflicting.jsonl"),format!("{}\n{}\n",header("codex"),cm(25,"u1","user","SYNTHETIC conflicting"))).unwrap();
    assert!(!w.store.sync_agents().unwrap()["issues"].as_array().unwrap().is_empty());
    assert!(w.call("codex","read",Some("agent:codex:synthetic-session")).is_err());
    let v=w.store.call_tool("source_query",&serde_json::to_vec(&json!({"start_ms":NOW-6000,"end_ms":NOW})).unwrap(),&w.client).unwrap();
    assert_eq!(v["incomplete"],true);
}

/// Newer Codex clients record a send as a completed `UserMessage` item with the client's id. Only threads the
/// person started count; a thread another agent created carries that agent's instructions, not the person's.
#[test]
fn codex_item_completed_user_message_counts_only_in_the_persons_threads() {
    let send = |sec: i64, id: &str, text: &str, client: bool| {
        let mut item = json!({"type":"UserMessage","id":id,"content":[{"type":"text","text":text,"text_elements":[]}]});
        if client { item["client_id"] = json!(format!("client-{id}")); }
        cx(sec, "event_msg", json!({"type":"item_completed","thread_id":"t","turn_id":"turn","item":item}))
    };
    let meta = |source: &str| cx(0, "session_meta", json!({"id":"synthetic-session","cwd":"/SYNTHETIC/project","thread_source":source}));
    // As in real files, the response copy of the prompt carries no `source`; only the send event admits it.
    let mut copy = cm(1, "r1", "user", "SYNTHETIC new format prompt");
    copy["payload"].as_object_mut().unwrap().remove("source");
    let w = World::new();
    w.write("codex", &[
        meta("user"),
        copy,
        send(1, "u1", "SYNTHETIC new format prompt", true),
        send(2, "u2", "SYNTHETIC replayed without client", false),
        cm(3, "a1", "assistant", "SYNTHETIC reply"),
    ]);
    w.save(&w.consent("codex"));
    w.sync();
    assert_eq!(w.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"), 1);
    assert_eq!(w.strings("SELECT text FROM text_chunks").join(" "), "SYNTHETIC new format prompt");

    let other = World::new();
    other.write("codex", &[meta("agent_created_thread"), send(1, "u1", "SYNTHETIC delegated task", true), cm(2, "a1", "assistant", "SYNTHETIC reply")]);
    other.save(&other.consent("codex"));
    other.sync();
    assert_eq!(other.count("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'"), 0);
}

/// Claude Code keeps subagent sessions in a folder below the project; that folder is skipped, and the person's
/// own sessions still sync instead of the whole source failing.
#[test]
fn claude_code_deeper_subagent_folders_do_not_stop_the_source() {
    let w = World::new();
    w.write("claude_code", &[header("claude_code"), msg("claude_code", 1, "u1", "user", json!("SYNTHETIC claude prompt"))]);
    let deep = w.dir.join("claude_code").join("project").join("session").join("subagents");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("agent.jsonl"), format!("{}\n", msg("claude_code", 2, "s1", "user", json!("SYNTHETIC subagent task")))).unwrap();
    w.save(&w.consent("claude_code"));
    w.sync();
    assert_eq!(w.strings("SELECT text FROM text_chunks").join(" "), "SYNTHETIC claude prompt");
}

/// Newer Claude Code tags every entry with an origin object: only `kind: human` is the person's own prompt.
#[test]
fn claude_code_origin_kind_human_is_a_prompt_and_other_kinds_are_not() {
    let w = World::new();
    let with_origin = |sec: i64, id: &str, text: &str, kind: &str| {
        let mut m = msg("claude_code", sec, id, "user", json!(text));
        m["origin"] = json!({"kind": kind});
        m["promptSource"] = json!(if kind == "human" { "sdk" } else { "system" });
        m
    };
    w.write("claude_code", &[
        header("claude_code"),
        with_origin(1, "u1", "SYNTHETIC typed prompt", "human"),
        with_origin(2, "n1", "SYNTHETIC task notification", "task-notification"),
        with_origin(3, "p1", "SYNTHETIC peer message", "peer"),
    ]);
    w.save(&w.consent("claude_code"));
    w.sync();
    assert_eq!(w.strings("SELECT text FROM text_chunks").join(" "), "SYNTHETIC typed prompt");
}

#[test]
fn reply_reads_forward_from_the_prompt_line_in_a_long_session() {
    let w = World::new();
    w.save(&w.consent("codex"));
    // About 10 MB of unrelated lines before the prompt: past the whole-file read's 8 MB budget.
    let mut lines = vec![header("codex")];
    for i in 0..100 { lines.push(cx(0, "turn_context", json!({"pad": format!("SYNTHETIC {i} {}", "x".repeat(100_000))}))); }
    lines.extend([cm(1,"u1","user","SYNTHETIC early"), cm(2,"a1","assistant","SYNTHETIC early reply"),
                  cm(3,"u2","user","SYNTHETIC question"), cm(4,"a2","assistant","SYNTHETIC answer"), cm(5,"u3","user","SYNTHETIC next")]);
    w.write("codex", &lines);
    w.sync();
    assert!(fs::metadata(w.path("codex")).unwrap().len() > 8 * 1024 * 1024);
    let mut req = AgentRequest { agent: "codex".into(), action: "read".into(), reference: Some("agent:codex:synthetic-session#u2".into()), limit: Some(2), ..Default::default() };
    let started = std::time::Instant::now();
    let page = w.store.agent_call(&req, &w.client).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let texts: Vec<_> = page["messages"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(texts, ["SYNTHETIC question", "SYNTHETIC answer"]);
    assert_eq!(page["messages"][0]["ref"], "agent:codex:synthetic-session#u2");
    // The next page resumes at the following line.
    req.cursor = page["nextCursor"].as_str().map(String::from);
    let page = w.store.agent_call(&req, &w.client).unwrap();
    assert_eq!(page["messages"][0]["text"], "SYNTHETIC next");
    assert!(page["nextCursor"].is_null());
    // A file rewritten under the same name is never read at the old line as if nothing changed.
    let mut changed = lines.clone();
    changed[103] = cm(3,"u2","user","SYNTHETIC rewritten");
    w.write("codex", &changed);
    req.cursor = None;
    assert!(w.store.agent_call(&req, &w.client).map(|v| v["messages"][0]["text"] != "SYNTHETIC question").unwrap_or(true));
}
