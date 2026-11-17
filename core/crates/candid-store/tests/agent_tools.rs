//! The attached agent's in-store writes (agent-platform-v1 §4.1) on the synthetic world: scope flags, audit, undo.
mod common;
use common::open;
use candid_store::fixtures::{fixture_path, load_world, temp_root, WorldIds};
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::{json, Value};

struct W {
    store: TimelineStore,
    root: std::path::PathBuf,
    ids: WorldIds,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn world() -> W {
    let root = temp_root("agent-tools");
    let store = open(&root);
    let ids = load_world(&store, &fixture_path("world.json")).unwrap();
    W { store, root, ids }
}

impl W {
    fn e(&self, key: &str) -> String {
        self.ids.events[key].clone()
    }
    fn agent(&self, writes: &[&str]) -> String {
        let scope = ClientScope { writes: writes.iter().map(|s| s.to_string()).collect(), propose_kinds: vec!["create_reminder".into()], ..ClientScope::unbounded() };
        self.store.attach_agent_client("agent-test", "Agent", &scope).unwrap();
        "agent-test".into()
    }
    fn call(&self, tool: &str, body: Value, client: &str) -> std::result::Result<Value, String> {
        self.store.call_tool(tool, body.to_string().as_bytes(), client).map_err(|e| match e {
            ToolCallFailure::Store(e) => { if e.code() == "database" { eprintln!("DB: {}", e.message()); } e.code() }
            ToolCallFailure::Decode(d) => format!("decode: {d}"),
        })
    }
    fn n(&self, sql: &str) -> i64 {
        self.store.raw_db().scalar(sql, &[]).unwrap()
    }
    fn denied(&self, client: &str) -> Vec<String> {
        self.store.raw_db().query("SELECT tool||':'||denied FROM access_log WHERE client_id=? AND denied IS NOT NULL ORDER BY at_ms,log_id", &[testing::rusqlite::types::Value::Text(client.into())], |r| r.text(0)).unwrap()
    }
}

#[test]
fn writes_need_their_scope_flag_and_are_audited() {
    let w = world();
    let agent = w.agent(&[]);
    for (tool, body) in [
        ("annotate", json!({"event_ids": [w.e("t1")], "title": "x"})),
        ("mark_reference", json!({"event_ids": [w.e("t1")], "label": "x"})),
        ("save_draft", json!({"level": "day", "start": "2027-01-15", "end": "2027-01-16", "text": "x", "citations": [w.e("t1")]})),
    ] {
        assert_eq!(w.call(tool, body, &agent).unwrap_err(), "write_not_permitted", "{tool}");
    }
    assert_eq!(w.denied(&agent), vec!["annotate:write_not_permitted", "mark_reference:write_not_permitted", "save_draft:write_not_permitted"]);
    // An ordinary external client (registered before `writes` existed) is refused the same way.
    let full = &w.ids.clients["full"];
    assert_eq!(w.call("annotate", json!({"event_ids": [w.e("t1")], "title": "x"}), full).unwrap_err(), "write_not_permitted");
    assert_eq!(w.n("SELECT COUNT(*) FROM corrections"), 0);
}

#[test]
fn annotate_is_a_correction_that_can_be_undone() {
    let w = world();
    let agent = w.agent(&["annotate"]);
    let app = w.store.internal_client("Candid App", &ClientScope::unbounded()).unwrap();
    // The user's own title first, through the App's editor.
    w.store.annotation_command(&json!({"action": "annotation_save", "event_ids": [w.e("t1")], "request_id": "user-1", "title": "用户标题", "notes": "用户备注"}), &app).unwrap();
    let tag = json!({"run_id": "run-1", "model_ref": "mock/m"});
    // A `_run` sent by the model is replaced by the service; here the store simply records what it is given.
    let r = w.call("annotate", json!({"event_ids": [w.e("t1")], "title": "AI 起的标题", "_run": tag}), &agent).unwrap();
    assert_eq!(r["title"], "AI 起的标题");
    assert_eq!(r["notes"], "用户备注", "renaming keeps the notes");
    let shown = w.store.annotation_command(&json!({"action": "annotation_get", "event_ids": [w.e("t1")]}), &app).unwrap();
    assert_eq!(shown["annotation"]["title"], "AI 起的标题");
    assert_eq!(shown["annotation"]["by"]["client"], "agent-test");
    assert_eq!(shown["annotation"]["by"]["run_id"], "run-1");
    let undo = w.call("undo_write", json!({"write_id": r["write_id"]}), &agent).unwrap();
    assert_eq!(undo["title"], "用户标题");
    let shown = w.store.annotation_command(&json!({"action": "annotation_get", "event_ids": [w.e("t1")]}), &app).unwrap();
    assert_eq!(shown["annotation"]["title"], "用户标题");
    assert_eq!(shown["annotation"]["notes"], "用户备注");
    // Undoing twice is refused: something newer (the undo) now stands.
    assert!(w.call("undo_write", json!({"write_id": r["write_id"]}), &agent).is_err());
    // The agent may not undo the user's own edit.
    assert_eq!(w.call("undo_write", json!({"write_id": "annotation:user-1"}), &agent).unwrap_err(), "not_own_write");
    assert_eq!(w.n("SELECT COUNT(*) FROM corrections WHERE target_type='event_label'"), 3);
}

#[test]
fn out_of_scope_targets_look_missing_and_are_logged() {
    let w = world();
    let agent = w.agent(&["annotate", "reference", "draft"]);
    // t4 belongs to a denied app (keychain): neither annotation nor citation may reach it.
    assert_eq!(w.call("annotate", json!({"event_ids": [w.e("t4")], "title": "x"}), &agent).unwrap_err(), "not_found");
    assert_eq!(w.call("mark_reference", json!({"event_ids": [w.e("t4")], "label": "x"}), &agent).unwrap_err(), "not_found");
    assert_eq!(w.call("save_draft", json!({"level": "day", "start": "2027-01-15", "end": "2027-01-16", "text": "x", "citations": [w.e("t1"), w.e("t4")]}), &agent).unwrap_err(), "not_found");
    assert_eq!(w.denied(&agent), vec!["annotate:out_of_scope", "mark_reference:out_of_scope", "save_draft:out_of_scope"]);
    // Proposals stay limited to the kinds the run allows.
    assert_eq!(w.call("propose", json!({"kind": "send_email", "payload": {"to": "x"}}), &agent).unwrap_err(), "propose_kind_not_permitted");
    let p = w.call("propose", json!({"kind": "create_reminder", "payload": {"title": "交预算"}, "evidence": [w.e("t1")]}), &agent).unwrap();
    assert_eq!(p["status"], "pending");
}

#[test]
fn reference_and_commitment_are_revisions_that_undo() {
    let w = world();
    let agent = w.agent(&["reference", "commitment"]);
    let r = w.call("mark_reference", json!({"event_ids": [w.e("t1")], "label": "预算复核要点", "tags": ["报销流程"]}), &agent).unwrap();
    assert_eq!(r["status"], "candidate");
    let id = r["memory_id"].as_str().unwrap().to_string();
    let listed = w.call("memories", json!({"kind": "reference"}), &agent).unwrap();
    assert_eq!(listed["memories"][0]["text"], "预算复核要点");
    w.call("undo_write", json!({"write_id": r["write_id"]}), &agent).unwrap();
    assert_eq!(w.call("memories", json!({"kind": "reference"}), &agent).unwrap()["memories"], json!([]));
    assert_eq!(w.n(&format!("SELECT COUNT(*) FROM memories WHERE memory_id='{id}' AND rev=2 AND status='rejected'")), 1);

    // A commitment candidate produced by extraction.
    let db = w.store.raw_db();
    db.execute("INSERT INTO memories(memory_id,rev,kind,text,status,producer,created_ms) VALUES('c1',1,'commitment','周五前交预算','candidate','extract/1',1)", &[]).unwrap();
    db.execute("INSERT INTO memory_sources(memory_id,rev,ref_type,ref_id) VALUES('c1',1,'event',?)", &[testing::rusqlite::types::Value::Text(w.e("t1"))]).unwrap();
    db.execute("INSERT INTO memories(memory_id,rev,kind,text,status,producer,created_ms) VALUES('c2',1,'commitment','钥匙串里的事','candidate','extract/1',1)", &[]).unwrap();
    db.execute("INSERT INTO memory_sources(memory_id,rev,ref_type,ref_id) VALUES('c2',1,'event',?)", &[testing::rusqlite::types::Value::Text(w.e("t4"))]).unwrap();
    let listed = w.call("memories", json!({"kind": "commitment", "status": "candidate"}), &agent).unwrap();
    assert_eq!(listed["memories"].as_array().unwrap().len(), 1, "a memory resting on an out-of-scope event is hidden");
    assert_eq!(w.call("commitment", json!({"memory_id": "c2", "decision": "confirm"}), &agent).unwrap_err(), "not_found");
    let c = w.call("commitment", json!({"memory_id": "c1", "decision": "confirm"}), &agent).unwrap();
    assert_eq!(c["status"], "confirmed");
    assert_eq!(w.n("SELECT COUNT(*) FROM corrections WHERE target_type='memory' AND target_id='c1' AND action='confirm'"), 1);
    assert_eq!(w.n("SELECT COUNT(*) FROM memory_sources WHERE memory_id='c1' AND rev=2"), 1);
    let u = w.call("undo_write", json!({"write_id": c["write_id"]}), &agent).unwrap();
    assert_eq!(u["status"], "candidate");
    assert_eq!(w.call("memories", json!({"kind": "commitment", "status": "candidate"}), &agent).unwrap()["memories"][0]["rev"], 3);
}

#[test]
fn drafts_are_marked_and_undo_to_rejected() {
    let w = world();
    let agent = w.agent(&["draft"]);
    let d = w.call("save_draft", json!({"level": "day", "start": "2027-01-15", "end": "2027-01-16", "text": "今天主要在写预算草稿。", "citations": [w.e("t1")], "_run": {"run_id": "r9"}}), &agent).unwrap();
    assert_eq!(d["status"], "draft");
    assert_eq!(w.n("SELECT COUNT(*) FROM digests WHERE status='draft' AND producer='pi' AND run_id='r9'"), 1);
    let app = w.store.internal_client("Candid App", &ClientScope::unbounded()).unwrap();
    // The App (user) may undo the agent's draft.
    let u = w.call("undo_write", json!({"write_id": d["write_id"]}), &app).unwrap();
    assert_eq!(u["status"], "rejected");
    assert_eq!(w.n("SELECT COUNT(*) FROM corrections WHERE target_type='digest' AND action='reject'"), 1);
}

#[test]
fn the_agent_client_rotates_its_token_and_respects_revocation() {
    let w = world();
    let scope = ClientScope::unbounded();
    let first = w.store.attach_agent_client("agent-x", "Agent", &scope).unwrap();
    let second = w.store.attach_agent_client("agent-x", "Agent", &scope).unwrap();
    assert_eq!(w.store.client_id_for_token(&first).unwrap(), None);
    assert_eq!(w.store.client_id_for_token(&second).unwrap().as_deref(), Some("agent-x"));
    w.store.detach_agent_client("agent-x").unwrap();
    assert_eq!(w.store.client_id_for_token(&second).unwrap(), None);
    w.store.revoke_client("agent-x").unwrap();
    assert_eq!(w.store.attach_agent_client("agent-x", "Agent", &scope).unwrap_err().code(), "client_revoked");
    // Model calls are logged as the agent's.
    let call = w.store.agent_call_start("agent_chat", "run-1", "mock", "m", "on_device", 10).unwrap();
    w.store.agent_call_finish(&call, "ok", None, &json!({"prompt_tokens": 7, "completion_tokens": 3}), Some(20)).unwrap();
    assert_eq!(w.n("SELECT COUNT(*) FROM ai_calls WHERE client='pi' AND status='ok' AND input_tokens=7 AND output_tokens=3 AND run_id='run-1'"), 1);
}
