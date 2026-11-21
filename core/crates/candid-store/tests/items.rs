//! Items: what extraction operations do, merging, states, proposals, triggers. Synthetic data only; no model.
mod common;
use candid_store::fixtures::{temp_root, FIXED_NOW_MS};
use candid_store::testing::RowExt;
use candid_store::*;
use common::Fx;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// 2027-01-15 14:00 +08:00, a Friday.
const T0: i64 = 1_799_992_800_000;

fn shot(fx: &Fx, key: &str, at: i64, app: &str, title: &str) -> String {
    let mut s = IngestEvent::new(key, "screen.shot", Some(at));
    s.app_name = Some(app.into());
    s.title = Some(title.into());
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s])).unwrap();
    fx.texts(&format!("SELECT event_id FROM events WHERE source_key='{key}'"))[0].clone()
}

fn chat(fx: &Fx, key: &str, at: i64, title: &str, messages: &[(&str, &str, &str)]) {
    let id = shot(fx, key, at, "微信", title);
    let out = json!({"kind": "chat", "title": title, "excerpt": "", "messages": messages.iter().map(|(f, s, t)| json!({"from": f, "sender": s, "text": t})).collect::<Vec<_>>()});
    fx.store.excerpt_write(&id, &out, "test-vlm").unwrap();
}

fn mid(fx: &Fx, text: &str) -> String {
    fx.texts(&format!("SELECT message_id FROM messages WHERE text='{text}'")).pop().unwrap()
}

fn conv(fx: &Fx, title: &str) -> String {
    fx.texts(&format!("SELECT conversation_id FROM conversations WHERE title='{title}'")).pop().unwrap()
}

/// Applies `ops`; `basis` maps the model's message numbers to message texts.
fn apply(fx: &Fx, conversation: &str, basis: &[(i64, &str)], ops: Value) -> Value {
    let refs: BTreeMap<i64, String> = basis.iter().map(|(n, t)| (*n, mid(fx, t))).collect();
    fx.store.items_apply_for_test(conversation, &refs, &json!({"ops": ops}), "test-llm").unwrap()
}

fn call(fx: &Fx, tool: &str, body: Value) -> Value {
    fx.store.call_tool(tool, body.to_string().as_bytes(), &fx.client).map_err(|_| tool.to_string()).unwrap()
}

fn items(fx: &Fx) -> Vec<Value> {
    call(fx, "items", json!({"state": "all"}))["items"].as_array().unwrap().clone()
}

fn agenda(fx: &Fx, v: Value) -> Value {
    let app = fx.store.internal_client("Candid App", &ClientScope::unbounded()).unwrap();
    fx.store.agenda_command(&v, &app).map_err(|e| e.message()).unwrap()
}

fn start_of(fx: &Fx, id: &str) -> Option<i64> {
    fx.store.raw_db().first("SELECT start_ms FROM items WHERE item_id=?", &[candid_store::testing::rusqlite::types::Value::Text(id.into())], |r| r.opt_int(0)).unwrap().flatten()
}

fn seed(fx: &Fx) -> String {
    chat(fx, "a", T0, "张三", &[("other", "", "周六下午三点见面聊报价吗？"), ("me", "", "好，周六三点")]);
    fx.store.people_sync().unwrap();
    conv(fx, "张三")
}

fn create_meeting(fx: &Fx, c: &str) -> String {
    let r = apply(fx, c, &[(1, "周六下午三点见面聊报价吗？"), (2, "好，周六三点")], json!([
        {"op": "create", "kind": "event", "owner": "self", "title": "和张三见面聊报价", "when_text": "周六下午三点", "start": "2027-01-16T15:00:00+08:00", "end": "2027-01-16T16:00:00+08:00", "basis": [1, 2]}
    ]));
    assert_eq!(r["applied"]["created"], 1, "{r}");
    items(fx)[0]["item_id"].as_str().unwrap().to_string()
}

#[test]
fn create_update_done_with_sources_and_input_view() {
    let fx = Fx::make("items-flow");
    let c = seed(&fx);
    let id = create_meeting(&fx, &c);
    let item = &items(&fx)[0];
    assert_eq!((item["kind"].as_str(), item["review"].as_str(), item["state"].as_str()), (Some("event"), Some("candidate"), Some("open")));
    assert_eq!(item["when_text"], "周六下午三点");
    assert_eq!(item["start"], "2027-01-16T15:00:00.000+08:00");
    assert_eq!(item["person"]["name"], "张三");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM item_sources"), 2, "both messages are the sources of the create");
    assert_eq!(item["sources"].as_array().unwrap().len(), 2);

    // The next extraction sees the open item (with its id), the weekday of every message and a continuous numbering.
    chat(&fx, "b", T0 + 3_600_000, "张三", &[("other", "", "周六临时有事，改到周日上午十点吧")]);
    let preview = fx.store.items_preview(&c, 1).unwrap();
    let content = preview["content"].as_str().unwrap();
    assert!(content.contains(&format!("item_id={id}")) && content.contains("和张三见面聊报价") && content.contains("2027-01-16 周六 15:00"), "{content}");
    assert!(content.contains("上文") && content.contains("[1] 2027-01-15 周五 14:00") && content.contains("[3] 2027-01-15 周五 15:00 张三（对方）"), "{content}");
    assert!(content.contains("当前时间：2027-01-15 周五 16:00（时区 +08:00）"), "{content}");
    let key = preview["cache_key"].clone();

    let r = apply(&fx, &c, &[(3, "周六临时有事，改到周日上午十点吧")], json!([
        {"op": "update", "item_id": id, "when_text": "周日上午十点", "start": "2027-01-17T10:00:00+08:00", "end": "2027-01-17T11:00:00+08:00", "basis": [3]}
    ]));
    assert_eq!(r["applied"]["updated"], 1, "{r}");
    let item = &items(&fx)[0];
    assert_eq!((item["start"].as_str(), item["when_text"].as_str()), (Some("2027-01-17T10:00:00.000+08:00"), Some("周日上午十点")));
    assert_eq!(fx.texts(&format!("SELECT op FROM item_changes WHERE item_id='{id}' ORDER BY created_ms,rowid")), ["create", "update"]);
    // The open-item snapshot is part of the cache key.
    assert_ne!(fx.store.items_preview(&c, 1).unwrap()["cache_key"], key);

    chat(&fx, "c", T0 + 7_200_000, "张三", &[("me", "", "报价聊完了，已经发给你")]);
    let r = apply(&fx, &c, &[(1, "报价聊完了，已经发给你")], json!([{"op": "done", "item_id": id, "basis": [1]}]));
    assert_eq!(r["applied"]["done"], 1, "{r}");
    assert_eq!(items(&fx)[0]["state"], "done");
    assert_eq!(call(&fx, "items", json!({}))["items"], json!([]), "open only by default");
    assert_eq!(call(&fx, "items", json!({"state": "done"}))["items"].as_array().unwrap().len(), 1);
    // A finished item cannot be changed any more.
    let r = apply(&fx, &c, &[(1, "报价聊完了，已经发给你")], json!([{"op": "cancel", "item_id": id, "basis": [1]}]));
    assert_eq!(r["dropped"][0]["reason"], "item_not_open", "{r}");
}

#[test]
fn single_operations_are_dropped_by_their_checks() {
    let fx = Fx::make("items-checks");
    let c = seed(&fx);
    let m = [(1, "周六下午三点见面聊报价吗？"), (2, "好，周六三点")];
    let r = apply(&fx, &c, &m, json!([
        {"op": "create", "kind": "event", "owner": "self", "title": "引用越界", "start": "2027-01-16T15:00:00+08:00", "basis": [1, 9]},
        {"op": "create", "kind": "event", "owner": "self", "title": "没有依据", "start": "2027-01-16T15:00:00+08:00"},
        {"op": "create", "kind": "event", "owner": "self", "title": "时间早于消息两天", "start": "2027-01-13T10:00:00+08:00", "basis": [1]},
        {"op": "create", "kind": "reminder", "owner": "self", "title": "把报价单发给张三", "when_text": "上周三", "start": "2027-01-13T10:00:00+08:00", "basis": [1]},
        {"op": "update", "item_id": "it-nope", "title": "x", "basis": [1]},
        {"op": "fly", "basis": [1]},
        {"op": "memo", "text": "张三喜欢喝美式", "basis": [2]}
    ]));
    let reasons: Vec<&str> = r["dropped"].as_array().unwrap().iter().map(|d| d["reason"].as_str().unwrap()).collect();
    assert_eq!(reasons, ["basis_out_of_range", "no_basis", "event_without_time", "unknown_item", "unknown_op"], "{r}");
    assert_eq!(r["applied"]["created"], 1);
    assert_eq!(r["applied"]["memo"], 1);
    let all = items(&fx);
    assert_eq!(all.len(), 1);
    assert_eq!((all[0]["title"].as_str(), all[0]["start"].is_null()), (Some("把报价单发给张三"), true), "a reminder with a stale time stays undated");
}

#[test]
fn times_resolve_against_the_basis_message() {
    let fx = Fx::make("items-time");
    // A message from three days ago: "明天下午三点" meant Wednesday, which is before today and still valid.
    let old = T0 - 3 * 86_400_000;
    chat(&fx, "old", old, "李四", &[("other", "", "明天下午三点电话说一下方案")]);
    chat(&fx, "date", T0, "王五", &[("other", "", "下周一我们交付")]);
    let c = conv(&fx, "李四");
    apply(&fx, &c, &[(1, "明天下午三点电话说一下方案")], json!([
        {"op": "create", "kind": "event", "owner": "self", "title": "和李四电话说方案", "start": "2027-01-13T15:00", "basis": [1]}
    ]));
    let c = conv(&fx, "王五");
    apply(&fx, &c, &[(1, "下周一我们交付")], json!([
        {"op": "create", "kind": "commitment", "owner": "other", "title": "王五交付", "start": "2027-01-18", "basis": [1]}
    ]));
    let all = items(&fx);
    let by = |t: &str| all.iter().find(|i| i["title"] == t).unwrap().clone();
    assert_eq!(by("和李四电话说方案")["start"], "2027-01-13T15:00:00.000+08:00", "no offset in the reply means the clock's zone");
    let d = by("王五交付");
    assert_eq!((d["all_day"].clone(), d["start"].as_str()), (json!(true), Some("2027-01-18T00:00:00.000+08:00")));
    assert_eq!(d["owner"], "other");
}

#[test]
fn same_person_within_30_minutes_is_one_item_and_ignored_ones_stay_ignored() {
    let fx = Fx::make("items-merge");
    let c = seed(&fx);
    let id = create_meeting(&fx, &c);
    // The same person writes from a group (another conversation): 20 minutes later is the same appointment.
    chat(&fx, "g", T0 + 600_000, "项目群（4）", &[("other", "张三", "周六三点半那个报价会别忘了")]);
    fx.store.people_sync().unwrap();
    let g = conv(&fx, "项目群（4）");
    let r = apply(&fx, &g, &[(1, "周六三点半那个报价会别忘了")], json!([
        {"op": "create", "kind": "event", "owner": "self", "title": "周六报价会", "start": "2027-01-16T15:20:00+08:00", "basis": [1]},
        {"op": "create", "kind": "event", "owner": "self", "title": "周六晚饭", "start": "2027-01-16T19:00:00+08:00", "basis": [1]}
    ]));
    assert_eq!((r["applied"]["merged"].clone(), r["applied"]["created"].clone()), (json!(1), json!(1)), "{r}");
    assert_eq!(items(&fx).len(), 2);
    assert_eq!(fx.texts(&format!("SELECT op FROM item_changes WHERE item_id='{id}' ORDER BY created_ms,rowid")), ["create", "merge"]);
    assert_eq!(fx.scalar(&format!("SELECT COUNT(*) FROM item_sources s JOIN item_changes c ON c.change_id=s.change_id WHERE c.item_id='{id}'")), 3);

    // Ignoring the dinner makes the model's next suggestion for it vanish.
    let dinner = items(&fx).into_iter().find(|i| i["title"] == "周六晚饭").unwrap();
    agenda(&fx, json!({"action": "item", "item_id": dinner["item_id"], "decision": "ignore"}));
    let r = apply(&fx, &g, &[(1, "周六三点半那个报价会别忘了")], json!([
        {"op": "create", "kind": "event", "owner": "self", "title": "晚饭", "start": "2027-01-16T19:10:00+08:00", "basis": [1]}
    ]));
    assert_eq!(r["dropped"][0]["reason"], "matches_ignored_item", "{r}");
    assert_eq!(call(&fx, "items", json!({}))["items"].as_array().unwrap().len(), 1, "ignored items are never listed");
    assert!(fx.store.items_preview(&g, 1).unwrap()["content"].as_str().unwrap().contains("最近 30 天用户忽略的条目"));
}

#[test]
fn without_a_person_the_time_and_title_must_both_match() {
    let fx = Fx::make("items-nobody");
    // Two senders in each conversation: no single counterpart.
    chat(&fx, "x", T0, "A群", &[("other", "甲", "周四上午十点和客户开会"), ("other", "乙", "好的")]);
    chat(&fx, "y", T0 + 1000, "B群", &[("other", "丙", "周四十点客户会议讨论方案"), ("other", "丁", "收到")]);
    let (a, b) = (conv(&fx, "A群"), conv(&fx, "B群"));
    apply(&fx, &a, &[(1, "周四上午十点和客户开会")], json!([{"op": "create", "kind": "event", "owner": "self", "title": "周四和客户开会", "start": "2027-01-21T10:00:00+08:00", "basis": [1]}]));
    let r = apply(&fx, &b, &[(1, "周四十点客户会议讨论方案")], json!([
        {"op": "create", "kind": "event", "owner": "self", "title": "周四和客户开会讨论方案", "start": "2027-01-21T10:00:00+08:00", "basis": [1]},
        {"op": "create", "kind": "event", "owner": "self", "title": "给小王寄快递", "start": "2027-01-21T10:00:00+08:00", "basis": [1]},
        {"op": "create", "kind": "event", "owner": "self", "title": "周四和客户开会", "start": "2027-01-21T10:25:00+08:00", "basis": [1]}
    ]));
    assert_eq!((r["applied"]["merged"].clone(), r["applied"]["created"].clone()), (json!(1), json!(2)), "same time + similar title merges; a different title or a different time does not: {r}");
}

#[test]
fn confirmed_item_in_the_calendar_only_gets_proposals() {
    let fx = Fx::make("items-proposals");
    let c = seed(&fx);
    let id = create_meeting(&fx, &c);
    chat(&fx, "b", T0 + 3_600_000, "张三", &[("other", "", "周六临时有事，改到周日上午十点吧"), ("other", "", "算了，周日也不行，先取消吧")]);
    // Confirm: a create proposal; the user approves, the App writes it and reports the receipt.
    let out = agenda(&fx, json!({"action": "item", "item_id": id, "decision": "confirm"}));
    let p = out["proposal"].clone();
    assert_eq!((p["kind"].as_str(), p["status"].as_str(), p["payload"]["start"].as_str()), (Some("create_calendar_event"), Some("pending"), Some("2027-01-16T15:00:00.000+08:00")), "{p}");
    assert_eq!(agenda(&fx, json!({"action": "item", "item_id": id, "decision": "confirm"}))["proposal"]["proposal_id"], p["proposal_id"], "confirming twice files one proposal");
    let pid = p["proposal_id"].as_str().unwrap();
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "approve"}));
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "executed", "receipt": {"item_id": "EK1@1799999999", "container": "工作"}}));
    assert_eq!(items(&fx)[0]["external_ref"], "EK1@1799999999");

    // The model now hears a change: the item stays as it is, a proposal waits for approval.
    let r = apply(&fx, &c, &[(1, "周六临时有事，改到周日上午十点吧")], json!([
        {"op": "update", "item_id": id, "start": "2027-01-17T10:00:00+08:00", "end": "2027-01-17T11:00:00+08:00", "title": "和张三聊报价（改期）", "basis": [1]}
    ]));
    assert_eq!(r["applied"]["proposed"], 1, "{r}");
    assert_eq!(items(&fx)[0]["start"], "2027-01-16T15:00:00.000+08:00", "unchanged until approved");
    let pending = agenda(&fx, json!({"action": "today"}))["proposals"].as_array().unwrap().iter().find(|p| p["status"] == "pending").unwrap().clone();
    assert_eq!(pending["kind"], "update_calendar_event");
    assert_eq!((pending["payload"]["item_id"].as_str(), pending["payload"]["start"].as_str(), pending["payload"]["title"].as_str()), (Some("EK1@1799999999"), Some("2027-01-17T10:00:00.000+08:00"), Some("和张三聊报价（改期）")));
    assert!(!pending["evidence"].as_array().unwrap().is_empty(), "the proposal rests on the message's events");
    let pid = pending["proposal_id"].as_str().unwrap();
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "approve"}));
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "executed", "receipt": {"item_id": "EK1@1800003600", "container": "工作"}}));
    let item = &items(&fx)[0];
    assert_eq!((item["start"].as_str(), item["title"].as_str(), item["external_ref"].as_str()), (Some("2027-01-17T10:00:00.000+08:00"), Some("和张三聊报价（改期）"), Some("EK1@1800003600")));
    assert_eq!(fx.texts(&format!("SELECT \"by\" FROM item_changes WHERE item_id='{id}' ORDER BY created_ms,rowid")).last().unwrap(), "calendar");

    // A cancellation is a delete proposal; the item closes when it has been carried out.
    let r = apply(&fx, &c, &[(2, "算了，周日也不行，先取消吧")], json!([{"op": "cancel", "item_id": id, "basis": [2]}]));
    assert_eq!(r["applied"]["proposed"], 1, "{r}");
    assert_eq!(items(&fx)[0]["state"], "open");
    let del = agenda(&fx, json!({"action": "today"}))["proposals"].as_array().unwrap().iter().find(|p| p["status"] == "pending").unwrap().clone();
    assert_eq!((del["kind"].as_str(), del["payload"]["item_id"].as_str()), (Some("delete_calendar_event"), Some("EK1@1800003600")));
    let pid = del["proposal_id"].as_str().unwrap();
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "approve"}));
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "executed", "receipt": {"item_id": "EK1@1800003600", "container": "工作"}}));
    assert_eq!(items(&fx)[0]["state"], "cancelled");

    // A reminder in Reminders: done means complete_reminder.
    chat(&fx, "r", T0 + 9_000_000, "张三", &[("other", "", "你把报价单发我一下"), ("me", "", "已发")]);
    apply(&fx, &c, &[(1, "你把报价单发我一下")], json!([{"op": "create", "kind": "reminder", "owner": "self", "title": "发报价单给张三", "basis": [1]}]));
    let rid = items(&fx).into_iter().find(|i| i["kind"] == "reminder").unwrap()["item_id"].as_str().unwrap().to_string();
    let out = agenda(&fx, json!({"action": "item", "item_id": rid, "decision": "confirm"}));
    assert_eq!((out["proposal"]["kind"].as_str(), out["proposal"]["payload"]["due"].is_null()), (Some("create_reminder"), true));
    let pid = out["proposal"]["proposal_id"].as_str().unwrap();
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "approve"}));
    agenda(&fx, json!({"action": "proposal", "proposal_id": pid, "decision": "executed", "receipt": {"item_id": "RM1", "container": "提醒"}}));
    let r = apply(&fx, &c, &[(2, "已发")], json!([{"op": "done", "item_id": rid, "basis": [2]}]));
    assert_eq!(r["applied"]["proposed"], 1, "{r}");
    let p = agenda(&fx, json!({"action": "today"}))["proposals"].as_array().unwrap().iter().find(|p| p["status"] == "pending").unwrap().clone();
    assert_eq!((p["kind"].as_str(), p["payload"]["item_id"].as_str()), (Some("complete_reminder"), Some("RM1")));
}

#[test]
fn user_edited_fields_are_never_overwritten() {
    let fx = Fx::make("items-user-fields");
    let c = seed(&fx);
    let id = create_meeting(&fx, &c);
    agenda(&fx, json!({"action": "item", "item_id": id, "decision": "edit", "title": "报价会（我改的）"}));
    let r = apply(&fx, &c, &[(1, "周六下午三点见面聊报价吗？")], json!([
        {"op": "update", "item_id": id, "title": "模型想改的标题", "when_text": "周六下午三点整", "basis": [1]}
    ]));
    assert_eq!(r["applied"]["updated"], 1, "{r}");
    let item = &items(&fx)[0];
    assert_eq!((item["title"].as_str(), item["when_text"].as_str()), (Some("报价会（我改的）"), Some("周六下午三点整")));
    let r = apply(&fx, &c, &[(1, "周六下午三点见面聊报价吗？")], json!([{"op": "update", "item_id": id, "title": "又想改", "basis": [1]}]));
    assert_eq!(r["applied"]["unchanged"], 1, "{r}");
}

#[test]
fn memos_hang_on_the_person_and_wait_for_the_user() {
    let fx = Fx::make("items-memo");
    let c = seed(&fx);
    let id = create_meeting(&fx, &c);
    let r = apply(&fx, &c, &[(1, "周六下午三点见面聊报价吗？")], json!([{"op": "memo", "text": "张三下午三点之后才有空", "basis": [1]}]));
    assert_eq!(r["applied"]["memo"], 1, "{r}");
    let again = apply(&fx, &c, &[(1, "周六下午三点见面聊报价吗？")], json!([{"op": "memo", "text": "张三下午三点之后才有空", "basis": [1]}]));
    assert_eq!(again["dropped"][0]["reason"], "duplicate_memo");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM memories WHERE kind='fact' AND status='candidate'"), 1);
    assert!(fx.scalar("SELECT COUNT(*) FROM memory_sources") > 0);

    let card = call(&fx, "person", json!({"who": "张三"}));
    assert_eq!(card["open_items"].as_array().unwrap().len(), 1);
    assert_eq!(card["open_items"][0]["item_id"], id.as_str());
    assert_eq!(card["memos"][0]["text"], "张三下午三点之后才有空");
    let today = agenda(&fx, json!({"action": "today"}));
    assert_eq!(today["items"].as_array().unwrap().len(), 1);
    assert_eq!(today["memos"][0]["person"]["name"], "张三");
    // Another person's card does not show them.
    chat(&fx, "o", T0 + 5000, "赵六", &[("other", "", "你好")]);
    fx.store.people_sync().unwrap();
    assert_eq!(call(&fx, "person", json!({"who": "赵六"}))["memos"], json!([]));

    agenda(&fx, json!({"action": "memo", "memory_id": today["memos"][0]["memory_id"], "decision": "confirm"}));
    assert_eq!(agenda(&fx, json!({"action": "today"}))["memos"], json!([]), "decided memos leave the list");
    assert_eq!(call(&fx, "person", json!({"who": "张三"}))["memos"].as_array().unwrap().len(), 1, "but stay on the card");
}

#[test]
fn deleting_the_source_removes_the_item() {
    let fx = Fx::make("items-delete");
    let c = seed(&fx);
    create_meeting(&fx, &c);
    assert_eq!(items(&fx).len(), 1);
    let shots = fx.texts("SELECT event_id FROM events WHERE kind='screen.shot'");
    fx.store.delete_events(&shots, "user").unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM items"), 0, "no source left, no item");
}

/// A store whose clock the test moves.
fn moving(label: &str) -> (Fx, Arc<AtomicI64>) {
    let now = Arc::new(AtomicI64::new(FIXED_NOW_MS));
    let n = now.clone();
    let root = temp_root(label);
    let store = TimelineStore::open(&root, "candid/test", true, Clock::with(move || n.load(Ordering::SeqCst), Zone::named("Asia/Shanghai").unwrap())).unwrap();
    let source = store.register_source("mac.desktop", "Synthetic Mac", &SourcePolicy::new(true, true)).unwrap();
    let client = store.register_client("Synthetic AI", "http", &ClientScope::unbounded(), None).unwrap().client_id;
    (Fx { store, root, source, client }, now)
}

#[test]
fn ended_events_and_old_candidates_expire_by_rule() {
    let (fx, now) = moving("items-expire");
    let c = seed(&fx);
    create_meeting(&fx, &c);
    apply(&fx, &c, &[(1, "周六下午三点见面聊报价吗？")], json!([{"op": "create", "kind": "reminder", "owner": "self", "title": "发报价单", "basis": [1]}]));
    assert_eq!(fx.store.items_maintain().unwrap(), 0);
    // Saturday 16:30: the meeting is over, the reminder is a day old.
    now.store(FIXED_NOW_MS + 24 * 3_600_000 + 1_800_000, Ordering::SeqCst);
    assert_eq!(fx.store.items_maintain().unwrap(), 1);
    let all = items(&fx);
    assert_eq!(all.iter().find(|i| i["kind"] == "event").unwrap()["state"], "expired");
    assert_eq!(all.iter().find(|i| i["kind"] == "reminder").unwrap()["state"], "open");
    assert_eq!(fx.texts("SELECT \"by\" FROM item_changes WHERE op='expire'"), ["rule"]);
    // A week without a decision puts the candidate away; one the user confirmed stays.
    let rid = all.iter().find(|i| i["kind"] == "reminder").unwrap()["item_id"].as_str().unwrap().to_string();
    now.store(FIXED_NOW_MS + 8 * 86_400_000, Ordering::SeqCst);
    assert_eq!(fx.store.items_maintain().unwrap(), 1);
    assert_eq!(items(&fx).iter().find(|i| i["item_id"] == rid.as_str()).unwrap()["state"], "expired");
    assert_eq!(start_of(&fx, &rid), None);
}

fn enable(store: &TimelineStore) {
    let mut config = store.feature_settings().unwrap();
    config["items"]["item-extract"]["enabled"] = json!(true);
    store.workflow_command(&json!({"action": "features", "config": config})).unwrap();
}

fn jobs(fx: &Fx) -> i64 {
    fx.scalar("SELECT COUNT(*) FROM jobs WHERE kind='items.extract'")
}

#[test]
fn extraction_is_queued_when_a_chat_goes_quiet_and_not_before() {
    let (fx, now) = moving("items-scan");
    assert_eq!(fx.store.items_scan().unwrap()["disabled"], true);
    assert_eq!(item_extract_default(&fx), (false, 150), "off by default, 150 calls a day");
    enable(&fx.store);
    fx.store.items_scan().unwrap(); // baseline: only what arrives from now on
    chat(&fx, "a", FIXED_NOW_MS, "张三", &[("other", "", "周六下午三点见面聊报价吗？")]);
    fx.store.people_sync().unwrap();
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 0, "still talking");
    now.fetch_add(9 * 60_000, Ordering::SeqCst);
    fx.store.items_scan().unwrap();
    assert_eq!(jobs(&fx), 0, "9 minutes");
    now.fetch_add(2 * 60_000, Ordering::SeqCst);
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 1);
    assert_eq!(jobs(&fx), 1);
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 0, "once per batch of messages");
    assert_eq!(candid_store::job_priority::job_priority("items.extract"), 1);

    // A screenshot of that chat still waiting for its excerpt holds the next extraction back.
    chat(&fx, "b", now.load(Ordering::SeqCst), "张三", &[("other", "", "对了，把报价单也带上")]);
    let pending = shot(&fx, "pending", now.load(Ordering::SeqCst), "微信", "张三");
    fx.store.job_command(&json!({"action": "enqueue", "kind": "screen.excerpt", "feature": "screen-excerpt", "input_ref": pending, "config_version": "v1"})).unwrap();
    now.fetch_add(11 * 60_000, Ordering::SeqCst);
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 0);
    fx.store.job_command(&json!({"action": "cancel", "id": fx.texts("SELECT job_id FROM jobs WHERE kind='screen.excerpt'")[0]})).unwrap();
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 1);
    assert_eq!(jobs(&fx), 2);
}

fn item_extract_default(fx: &Fx) -> (bool, i64) {
    let s = fx.store.feature_settings().unwrap();
    let p = &s["items"]["item-extract"];
    (p["enabled"].as_bool().unwrap(), p["daily_calls"].as_i64().unwrap())
}

fn mail(fx: &Fx, key: &str, at: i64, level: &str) {
    let mut e = IngestEvent::new(key, "mail.message", Some(at));
    e.actor = "other".into();
    e.app_name = Some("邮件".into());
    e.title = Some(key.into());
    e.payload = json!({"from": {"name": "客户", "address_hash": "h1"}, "direction": "in", "level": level});
    e.text = vec![IngestText::new("subject", key), IngestText::new("body", &format!("下周二上午十点开评审会，邮件 {key}"))];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![e])).unwrap();
}

#[test]
fn important_mail_and_finished_recordings_are_queued() {
    let (fx, now) = moving("items-mail");
    enable(&fx.store);
    fx.store.items_scan().unwrap();
    mail(&fx, "normal-mail", FIXED_NOW_MS, "normal");
    fx.store.curated_sync().unwrap();
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 0, "ordinary mail is not read");
    mail(&fx, "important-mail", FIXED_NOW_MS + 1000, "important");
    now.fetch_add(1000, Ordering::SeqCst);
    fx.store.curated_sync().unwrap();
    assert_eq!(fx.store.items_scan().unwrap()["queued"], 1, "no waiting for important mail");
    let input = fx.store.items_preview(&fx.texts("SELECT input_ref FROM jobs WHERE kind='items.extract'")[0], 5).unwrap();
    assert!(input["content"].as_str().unwrap().contains("邮件"), "{input}");
}

#[test]
fn feature_catalog_entry_matches_the_plan() {
    let fx = Fx::make("items-catalog");
    let cat: Value = serde_json::from_str(include_str!("../../../workflows/catalog.json")).unwrap();
    let f = cat["features"].as_array().unwrap().iter().find(|f| f["id"] == "item-extract").unwrap();
    assert_eq!((f["role"].as_str(), f["job_kind"].as_str(), f["default_enabled"].as_bool(), f["default_daily_calls"].as_i64(), f["allow_remote"].as_bool()), (Some("summary"), Some("items.extract"), Some(false), Some(150), Some(false)));
    drop(fx);
}
