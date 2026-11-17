//! The AI page's 「今天」 column (agent-platform-v1 §4.2, §4.3) on the synthetic world: proposal approval with the
//! system's receipt, rejection and failure; commitment candidates confirmed into proposals or ignored; the latest
//! day review. Synthetic data only.
mod common;
use common::open;
use candid_store::fixtures::{fixture_path, load_world, temp_root, WorldIds};
use candid_store::testing::rusqlite::types::Value as Sql;
use candid_store::*;
use serde_json::{json, Value};

struct W {
    store: TimelineStore,
    root: std::path::PathBuf,
    ids: WorldIds,
    app: String,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn world() -> W {
    let root = temp_root("agenda");
    let store = open(&root);
    let ids = load_world(&store, &fixture_path("world.json")).unwrap();
    let app = store.internal_client("Candid App", &ClientScope::unbounded()).unwrap();
    W { store, root, ids, app }
}

impl W {
    fn agenda(&self, v: Value) -> std::result::Result<Value, String> {
        self.store.agenda_command(&v, &self.app).map_err(|e| e.message())
    }
    fn propose(&self, kind: &str, payload: Value) -> String {
        let scope = ClientScope { propose_kinds: vec![], ..ClientScope::unbounded() };
        self.store.attach_agent_client("agent-test", "Agent", &scope).unwrap();
        let body = json!({"kind": kind, "payload": payload, "evidence": [self.ids.events["t1"]]});
        let v = self.store.call_tool("propose", body.to_string().as_bytes(), "agent-test").map_err(|_| ()).unwrap();
        serde_json::to_value(v).unwrap()["proposal_id"].as_str().unwrap().to_string()
    }
    fn status(&self, id: &str) -> String {
        self.agenda(json!({"action": "today"})).unwrap()["proposals"].as_array().unwrap().iter()
            .find(|p| p["proposal_id"] == id).map(|p| p["status"].as_str().unwrap().to_string()).unwrap_or_default()
    }
}

#[test]
fn approval_records_the_receipt_and_rejection_and_failure_are_kept() {
    let w = world();
    let reminder = w.propose("create_reminder", json!({"title": "【合成】交差旅报销", "due": "2027-01-16T09:00:00+08:00"}));
    let event = w.propose("create_calendar_event", json!({"title": "【合成】预算复核", "start": "2027-01-16T14:00:00+08:00", "end": "2027-01-16T14:30:00+08:00"}));
    let other = w.propose("send_email", json!({"to": "someone"}));
    let delete = w.propose("delete_reminder", json!({"item_id": "EK-9", "title": "【合成】旧提醒"}));
    let today = w.agenda(json!({"action": "today"})).unwrap();
    assert_eq!(today["proposals"].as_array().unwrap().len(), 4);
    // Edits of system items are carried out by the App too.
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": delete, "decision": "approve"})).unwrap()["status"], "approved");
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": delete, "decision": "executed", "receipt": {"item_id": "EK-9", "container": "已删除"}})).unwrap()["status"], "executed");
    let p = today["proposals"].as_array().unwrap().iter().find(|p| p["proposal_id"] == reminder.as_str()).unwrap().clone();
    assert_eq!((p["kind"].as_str(), p["status"].as_str(), p["client_name"].as_str()), (Some("create_reminder"), Some("pending"), Some("Agent")));
    assert_eq!(p["payload"]["title"], "【合成】交差旅报销");

    // Executed only after approval, and only with a receipt.
    assert!(w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "executed", "receipt": {"item_id": "x"}})).is_err());
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "approve"})).unwrap()["status"], "approved");
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "approve"})).unwrap()["status"], "approved", "repeating is harmless");
    assert!(w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "executed", "receipt": {}})).is_err());
    let done = w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "executed", "receipt": {"item_id": "EK-1", "calendar": "提醒事项"}})).unwrap();
    assert_eq!((done["status"].as_str(), done["result"]["receipt"]["item_id"].as_str()), (Some("executed"), Some("EK-1")));
    assert!(done["executed"].is_string());
    // A retry of the receipt after a lost answer returns the same proposal; nothing else moves it.
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "executed", "receipt": {"item_id": "EK-2"}})).unwrap()["result"]["receipt"]["item_id"], "EK-1");
    assert!(w.agenda(json!({"action": "proposal", "proposal_id": reminder, "decision": "reject"})).is_err());

    // Failure keeps the reason; it can be approved again (retry) or rejected.
    w.agenda(json!({"action": "proposal", "proposal_id": event, "decision": "approve"})).unwrap();
    let failed = w.agenda(json!({"action": "proposal", "proposal_id": event, "decision": "failed", "error": "没有日历写入权限"})).unwrap();
    assert_eq!((failed["status"].as_str(), failed["result"]["error"].as_str()), (Some("failed"), Some("没有日历写入权限")));
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": event, "decision": "approve"})).unwrap()["result"], Value::Null);
    assert_eq!(w.agenda(json!({"action": "proposal", "proposal_id": event, "decision": "reject"})).unwrap()["status"], "rejected");

    // A kind the App cannot carry out is never approved here; it can still be rejected.
    assert!(w.agenda(json!({"action": "proposal", "proposal_id": other, "decision": "approve"})).is_err());
    assert_eq!(w.status(&other), "pending");
    assert!(w.agenda(json!({"action": "proposal", "proposal_id": "nope", "decision": "approve"})).is_err());
    assert!(w.agenda(json!({"action": "proposal", "proposal_id": other, "decision": "maybe"})).is_err());

    // Decided ones stay listed for a day (the chat cards show their result), then drop out.
    assert_eq!(w.status(&reminder), "executed");
    let later = w.agenda(json!({"action": "today", "since_ms": i64::MAX})).unwrap();
    let ids: Vec<&str> = later["proposals"].as_array().unwrap().iter().map(|p| p["proposal_id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![other.as_str()]);
}

#[test]
fn commitments_become_reminder_proposals_or_are_ignored() {
    let w = world();
    let db = w.store.raw_db();
    for (id, body) in [("c1", "【合成】下周一跟进设备采购回复"), ("c2", "【合成】周五前给报价单")] {
        db.execute("INSERT INTO memories(memory_id,rev,kind,text,valid_to_ms,status,producer,confidence,created_ms) VALUES(?,1,'commitment',?,1800000000000,'candidate','workflow.extract',0.7,1)", &[Sql::Text(id.into()), Sql::Text(body.into())]).unwrap();
        db.execute("INSERT INTO memory_sources(memory_id,rev,ref_type,ref_id) VALUES(?,1,'event',?)", &[Sql::Text(id.into()), Sql::Text(w.ids.events["t1"].clone())]).unwrap();
    }
    let today = w.agenda(json!({"action": "today"})).unwrap();
    let listed = today["commitments"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["source"]["event_id"], w.ids.events["t1"].as_str());
    assert!(listed[0]["due"].is_string());

    let confirmed = w.agenda(json!({"action": "commitment", "memory_id": "c1", "decision": "confirm"})).unwrap();
    assert_eq!(confirmed["status"], "confirmed");
    let proposal = confirmed["proposal"].clone();
    assert_eq!((proposal["kind"].as_str(), proposal["status"].as_str()), (Some("create_reminder"), Some("pending")));
    assert_eq!(proposal["payload"]["memory_id"], "c1");
    assert_eq!(proposal["payload"]["title"], "【合成】下周一跟进设备采购回复");
    assert_eq!(proposal["evidence"], json!([w.ids.events["t1"]]));
    assert_eq!(proposal["client_id"], w.app.as_str());
    // A retry returns the same proposal; no second one is filed.
    let again = w.agenda(json!({"action": "commitment", "memory_id": "c1", "decision": "confirm"})).unwrap();
    assert_eq!(again["proposal"]["proposal_id"], proposal["proposal_id"]);
    assert!(w.agenda(json!({"action": "commitment", "memory_id": "c1", "decision": "ignore"})).is_err(), "decided once");

    let ignored = w.agenda(json!({"action": "commitment", "memory_id": "c2", "decision": "ignore"})).unwrap();
    assert_eq!((ignored["status"].as_str(), ignored["proposal"].is_null()), (Some("rejected"), true));
    assert_eq!(w.agenda(json!({"action": "today"})).unwrap()["commitments"], json!([]));
    assert_eq!(db.scalar("SELECT COUNT(*) FROM corrections WHERE target_type='memory'", &[]).unwrap(), 2, "both decisions are corrections");
    assert_eq!(db.scalar("SELECT COUNT(*) FROM proposals", &[]).unwrap(), 1);
    assert!(w.agenda(json!({"action": "commitment", "memory_id": "c1", "decision": "later"})).is_err());
}

#[test]
fn the_latest_day_review_is_shown() {
    let w = world();
    assert_eq!(w.agenda(json!({"action": "today"})).unwrap()["review"], Value::Null);
    let db = w.store.raw_db();
    db.execute("INSERT INTO digests VALUES('day:2027-01-14',1,'day',1000,2000,'{\"text\":\"旧回顾\"}','draft','workflow.daily',NULL,NULL,NULL,1)", &[]).unwrap();
    db.execute("INSERT INTO digests VALUES('day:2027-01-15',1,'day',3000,4000,'{\"text\":\"【合成】上午写连接器，下午预算周会。\"}','draft','workflow.daily','mock/m',NULL,NULL,2)", &[]).unwrap();
    db.execute("INSERT INTO digests VALUES('day:2027-01-16',1,'day',5000,6000,'{\"text\":\"被拒绝的\"}','rejected','workflow.daily',NULL,NULL,NULL,3)", &[]).unwrap();
    let review = w.agenda(json!({"action": "today"})).unwrap()["review"].clone();
    assert_eq!((review["digest_id"].as_str(), review["text"].as_str()), (Some("day:2027-01-15"), Some("【合成】上午写连接器，下午预算周会。")));
    assert_eq!(review["producer"], "workflow.daily");
}
