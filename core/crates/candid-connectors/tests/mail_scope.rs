//! Mail scope, level rules, live bodies and re-leveling (agent-platform-v1 §2.0.2), against a synthetic Mail.app tree.
mod common;
use common::*;
use candid_connectors::mail::config::{decide, Condition, Decision, Facts, Level, MailConfig, Role, Rule};
use candid_connectors::sync_all;
use serde_json::{json, Value};
use std::sync::Mutex;

const D1: &str = "Tue, 10 Nov 2026 09:00:00 +0000";
const D2: &str = "Tue, 10 Nov 2026 10:00:00 +0000";
const R: i64 = 1_794_301_200;

struct World {
    dir: std::path::PathBuf,
    tree: MailTree,
    store: Mutex<candid_store::TimelineStore>,
    inbox: i64,
    sent: i64,
    trash: i64,
}

impl World {
    fn new(label: &str) -> Self {
        let dir = temp(label);
        let tree = MailTree::new(&dir);
        let (inbox, sent, trash) = (tree.mailbox("INBOX"), tree.mailbox("Sent Messages"), tree.mailbox("Deleted Messages"));
        let store = store(&dir);
        World { dir, tree, store, inbox, sent, trash }
    }
    fn config(&self, extra: Value) -> Value {
        let mut c = self.tree.config();
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    }
    fn authorize(&self, extra: Value) -> String {
        let r = cmd(&self.store, &self.dir, json!({"action": "authorize", "kind": "mail.apple", "config": self.config(extra)}));
        r["source_id"].as_str().unwrap().to_string()
    }
    fn update(&self, sid: &str, extra: Value) -> Value {
        cmd(&self.store, &self.dir, json!({"action": "update", "source_id": sid, "config": self.config(extra)}))
    }
    fn sync(&self) -> candid_connectors::SourceOutcome {
        sync_all(&self.store, None, &self.dir).sources.into_iter().next().unwrap()
    }
    fn add_inbox(&self, mid: &str, from: &str, subject: &str, extra: &str, body: &str) -> i64 {
        let m = mail(Some(mid), from, "me@example.test", subject, D1, extra, body);
        self.tree.add(self.inbox, &m, Some(mid), subject, from, R, Some(false))
    }
    fn event_id(&self, sid: &str, key: &str) -> String {
        self.store.lock().unwrap().event_id(sid, key).unwrap().unwrap_or_else(|| panic!("no event {key}"))
    }
    /// An answered conversation plus a stranger, a mailing list, and a new topic from the person the user wrote to.
    fn populate(&self) {
        let a = mail(Some("a@example.test"), "Alice <alice@example.test>", "me@example.test", "预算草案", D1, "", "请在周五前确认预算。");
        let b = mail(Some("b@example.test"), "Me <me@example.test>", "alice@example.test", "Re: 预算草案", D2, "In-Reply-To: <a@example.test>\r\nReferences: <a@example.test>\r\n", "好的，周四给你。");
        let c = mail(Some("c@example.test"), "Alice <alice@example.test>", "me@example.test", "Re: 预算草案", D2, "In-Reply-To: <b@example.test>\r\n", "收到。");
        self.tree.add(self.inbox, &a, Some("a@example.test"), "预算草案", "alice@example.test", R, Some(false));
        self.tree.add(self.sent, &b, Some("b@example.test"), "Re: 预算草案", "me@example.test", R + 3600, Some(false));
        self.tree.add(self.inbox, &c, Some("c@example.test"), "Re: 预算草案", "alice@example.test", R + 7200, Some(false));
        self.add_inbox("d@example.test", "Alice <alice@example.test>", "另一件事", "", "新话题，来自写过信的人。");
        self.add_inbox("s@example.test", "bob@other.test", "你好", "", "看看这个。");
        self.add_inbox("n@example.test", "news@lists.example.test", "周刊", "List-Unsubscribe: <mailto:u@lists.example.test>\r\n", "周刊正文。");
    }
}

fn level(w: &World, key: &str) -> (String, String, bool) {
    let p = payload(&w.store, key);
    (p["level"].as_str().unwrap().into(), p["level_basis"].as_str().unwrap().into(), p["important_candidate"].as_bool().unwrap_or(false))
}

fn facts<'a>(from: &'a str, subject: &'a str, list: bool, replied: bool) -> Facts<'a> {
    Facts { from: Some(from), recipients: vec!["me@example.test"], subject, list, automated: false, mailbox: "INBOX", role: Role::Inbox, replied }
}

#[test]
fn default_level_is_metadata_and_user_rules_are_ordered_first_match_wins() {
    let mut c = MailConfig::default();
    let f = facts("bob@other.test", "你好", false, false);
    assert_eq!(decide(&c, &f), Decision { level: Level::Metadata, basis: "default".into(), candidate: false, ai_pending: false });
    let to_normal = Rule { when: Condition { domain: Some("other.test".into()), ..Default::default() }, level: Level::Normal };
    let to_important = Rule { when: Condition { from: Some("BOB@other.test".into()), ..Default::default() }, level: Level::Important };
    c.rules = vec![to_normal.clone(), to_important.clone()];
    assert_eq!((decide(&c, &f).level, decide(&c, &f).basis), (Level::Normal, "rule:0".into()));
    c.rules = vec![to_important, to_normal];
    assert_eq!((decide(&c, &f).level, decide(&c, &f).basis), (Level::Important, "rule:0".into()));
    // Every condition kind: subject keyword, bulk headers, mailbox, recipient.
    for when in [
        Condition { subject_contains: Some("你".into()), ..Default::default() },
        Condition { to: Some("ME@example.test".into()), ..Default::default() },
        Condition { mailbox: Some("role:inbox".into()), ..Default::default() },
    ] {
        c.rules = vec![Rule { when, level: Level::Normal }];
        assert_eq!(decide(&c, &f).level, Level::Normal);
    }
    c.rules = vec![Rule { when: Condition { list: Some(true), ..Default::default() }, level: Level::Important }];
    assert_eq!(decide(&c, &f).level, Level::Metadata);
    assert_eq!(decide(&c, &facts("x@lists.test", "周刊", true, false)).level, Level::Important);
    // A user rule beats both built-ins; the built-ins are ordered replied → bulk and can be switched off.
    c.rules = vec![Rule { when: Condition { from: Some("x@lists.test".into()), ..Default::default() }, level: Level::Ignore }];
    assert_eq!(decide(&c, &facts("x@lists.test", "周刊", true, true)).level, Level::Ignore);
    c.rules.clear();
    let both = facts("x@lists.test", "周刊", true, true);
    assert_eq!((decide(&c, &both).basis, decide(&c, &both).candidate), ("builtin:replied".into(), true));
    c.builtin_replied = false;
    assert_eq!(decide(&c, &both).basis, "builtin:list");
    c.builtin_rules = false;
    assert_eq!(decide(&c, &both).basis, "default");
}

#[test]
fn replied_threads_and_people_written_to_become_normal_candidates_everything_else_metadata() {
    let w = World::new("scope-default");
    w.populate();
    w.authorize(json!({}));
    let out = w.sync();
    assert_eq!(out.inserted, 6, "{out:?}");
    // The thread the user replied to, including the incoming messages on either side of the reply.
    for k in ["mid:a@example.test", "mid:b@example.test", "mid:c@example.test"] {
        assert_eq!(level(&w, k), ("normal".into(), "builtin:replied".into(), true), "{k}");
        assert!(!texts(&w.store, k, "body").is_empty(), "{k} keeps its body");
    }
    // A new thread from someone the user has written to.
    assert_eq!(level(&w, "mid:d@example.test"), ("normal".into(), "builtin:replied".into(), true));
    // Nothing levels a stranger: metadata only, no body in the store; a mailing list stays metadata via the bulk rule.
    assert_eq!(level(&w, "mid:s@example.test"), ("metadata".into(), "default".into(), false));
    assert_eq!(level(&w, "mid:n@example.test"), ("metadata".into(), "builtin:list".into(), false));
    assert!(texts(&w.store, "mid:s@example.test", "body").is_empty());
    assert_eq!(texts(&w.store, "mid:s@example.test", "subject"), vec!["你好"]);
    // People written to are kept as salted hashes only.
    let b = payload(&w.store, "mid:b@example.test");
    assert_eq!(b["to_hashes"].as_array().unwrap().len(), 1);
    assert!(!b.to_string().contains("alice@example.test"));
    // The candidate stays "normal" until the user or a rule says important.
    assert_eq!(out.stats.unwrap().levels.get("important_candidate"), Some(&4));
}

#[test]
fn people_written_to_in_an_earlier_pass_are_remembered_and_user_rules_still_win() {
    let w = World::new("scope-later");
    let sid = w.authorize(json!({"rules": [{"when": {"from": "alice@example.test"}, "level": "metadata"}]}));
    let b = mail(Some("b@example.test"), "Me <me@example.test>", "alice@example.test", "你好", D1, "", "你好 Alice。");
    w.tree.add(w.sent, &b, Some("b@example.test"), "你好", "me@example.test", R, Some(false));
    w.sync();
    w.add_inbox("late@example.test", "alice@example.test", "晚一点的来信", "", "迟到的回复。");
    w.add_inbox("other@example.test", "carol@example.test", "陌生人", "", "你好。");
    w.sync();
    // The user's rule about alice beats the built-in replied rule.
    assert_eq!(level(&w, "mid:late@example.test").1, "rule:0");
    w.update(&sid, json!({}));
    let out = cmd(&w.store, &w.dir, json!({"action": "relevel"}));
    assert_eq!(out["sources"][0]["stats"]["changed"], 1, "{out}");
    assert_eq!(level(&w, "mid:late@example.test"), ("normal".into(), "builtin:replied".into(), true));
    assert_eq!(level(&w, "mid:other@example.test").1, "default");
}

#[test]
fn preview_counts_levels_from_headers_and_import_waits_for_confirmation() {
    let w = World::new("scope-preview");
    w.populate();
    w.tree.add(w.trash, &mail(Some("t@example.test"), "x@example.test", "me@example.test", "废纸篓", D1, "", "x"), Some("t@example.test"), "废纸篓", "x@example.test", R, Some(false));
    let config = w.config(json!({"import_confirmed": false}));
    // Before anything is registered: counts only, nothing written.
    let dry = cmd(&w.store, &w.dir, json!({"action": "dry_run", "kind": "mail.apple", "config": config}));
    assert_eq!(dry["would_insert"], 6, "{dry}");
    assert_eq!(dry["stats"]["scanned"], 6);
    assert_eq!(dry["stats"]["levels"], json!({"metadata": 2, "normal": 4, "important_candidate": 4}));
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM sources"), 0);
    // Registered but unconfirmed: reads nothing.
    let sid = w.authorize(json!({"import_confirmed": false}));
    let out = w.sync();
    assert_eq!((out.state.as_str(), out.code.as_deref(), out.inserted), ("skipped", Some("awaiting_confirmation"), 0), "{out:?}");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 0);
    // A different scope can be previewed without saving it: only mail from the last day, then nothing.
    let narrower = w.config(json!({"import_confirmed": false, "backfill_days": 1}));
    let dry = cmd(&w.store, &w.dir, json!({"action": "dry_run", "source_id": sid, "config": narrower}));
    assert_eq!(dry["would_insert"], 0, "{dry}");
    let stored = cmd(&w.store, &w.dir, json!({"action": "list"}));
    assert_eq!(stored["sources"][0]["config"]["backfill_days"], 90);
    // Rule order changes the preview without touching the store.
    let rules = |first: &str, second: &str| json!([{"when": {"from": "bob@other.test"}, "level": first}, {"when": {"domain": "other.test"}, "level": second}]);
    let a = cmd(&w.store, &w.dir, json!({"action": "dry_run", "source_id": sid, "config": w.config(json!({"rules": rules("important", "ignore")}))}));
    let b = cmd(&w.store, &w.dir, json!({"action": "dry_run", "source_id": sid, "config": w.config(json!({"rules": rules("ignore", "important")}))}));
    assert_eq!((a["stats"]["levels"]["important"].as_i64(), a["stats"]["levels"]["ignore"].as_i64()), (Some(1), None), "{a}");
    assert_eq!((b["stats"]["levels"]["ignore"].as_i64(), b["stats"]["levels"]["important"].as_i64()), (Some(1), None), "{b}");
    // Confirming starts the import.
    w.update(&sid, json!({"import_confirmed": true}));
    let out = w.sync();
    assert_eq!(out.inserted, 6, "{out:?}");
    // Regression: previewing the scope of an account already imported counted 0; it counts what is in scope and
    // says how much of it is stored.
    let dry = cmd(&w.store, &w.dir, json!({"action": "dry_run", "source_id": sid, "config": w.config(json!({}))}));
    assert_eq!((dry["stats"]["scanned"].as_i64(), dry["already_stored"].as_i64()), (Some(6), Some(6)), "{dry}");
}

#[test]
fn metadata_only_mail_is_read_live_never_stored_and_reports_a_deleted_source() {
    let w = World::new("scope-live");
    let row = w.add_inbox("s@example.test", "bob@other.test", "你好", "", "看看这个。\n\n--\nBob");
    let sid = w.authorize(json!({}));
    w.sync();
    let id = w.event_id(&sid, "mid:s@example.test");
    assert!(texts(&w.store, "mid:s@example.test", "body").is_empty());
    let before = count(&w.store, "SELECT COUNT(*) FROM text_chunks");
    let r = cmd(&w.store, &w.dir, json!({"action": "mail_body", "event_id": id}));
    assert_eq!((r["body"]["state"].as_str(), r["body"]["body"].as_str()), (Some("ok"), Some("看看这个。")), "{r}");
    assert_eq!(r["message"]["level"], "metadata");
    // Reading stores nothing and indexes nothing.
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM text_chunks"), before);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM text_fts WHERE text_fts MATCH '看看这个'"), 0);
    // Moved to the trash: not readable, same as deleted.
    w.tree.move_row(row, w.trash);
    let r = cmd(&w.store, &w.dir, json!({"action": "mail_body", "event_id": id}));
    assert_eq!(r["body"]["state"], "source_deleted", "{r}");
    // Raising a message whose source is gone cannot store a body.
    let e = cmd_err(&w.store, &w.dir, json!({"action": "set_level", "event_id": id, "level": "normal"}));
    assert_eq!(e, "source_deleted");
    assert!(texts(&w.store, "mid:s@example.test", "body").is_empty());
    // Expunged for good.
    let row2 = w.add_inbox("t@example.test", "bob@other.test", "再见", "", "第二封。");
    w.sync();
    w.tree.expunge(row2);
    let id2 = w.event_id(&sid, "mid:t@example.test");
    assert_eq!(cmd(&w.store, &w.dir, json!({"action": "mail_body", "event_id": id2}))["body"]["state"], "source_deleted");
}

#[test]
fn single_mail_or_whole_thread_can_be_raised_and_lowered_by_hand() {
    let w = World::new("scope-manual");
    let a = mail(Some("a@example.test"), "bob@other.test", "me@example.test", "项目", D1, "", "第一封。");
    let b = mail(Some("b@example.test"), "bob@other.test", "me@example.test", "Re: 项目", D2, "In-Reply-To: <a@example.test>\r\n", "第二封。");
    let c = mail(Some("c@example.test"), "bob@other.test", "me@example.test", "无关", D2, "", "第三封。");
    for (m, mid, subj, t) in [(&a, "a@example.test", "项目", R), (&b, "b@example.test", "Re: 项目", R + 60), (&c, "c@example.test", "无关", R + 120)] {
        w.tree.add(w.inbox, m, Some(mid), subj, "bob@other.test", t, Some(false));
    }
    let sid = w.authorize(json!({}));
    w.sync();
    assert_eq!(level(&w, "mid:a@example.test").0, "metadata");
    // One message: normal, body stored and searchable, manual basis, the old payload kept as a revision.
    let ida = w.event_id(&sid, "mid:a@example.test");
    let r = cmd(&w.store, &w.dir, json!({"action": "set_level", "event_id": ida, "level": "normal"}));
    assert_eq!(r["changed"], 1, "{r}");
    assert_eq!(level(&w, "mid:a@example.test"), ("normal".into(), "manual".into(), false));
    assert_eq!(texts(&w.store, "mid:a@example.test", "body"), vec!["第一封。"]);
    assert_eq!(texts(&w.store, "mid:a@example.test", "subject"), vec!["项目"]);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM text_fts WHERE text_fts MATCH '第一封'"), 1);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM agent_event_revisions"), 1);
    assert_eq!(level(&w, "mid:b@example.test").0, "metadata");
    // Whole thread: important; the unrelated mail stays metadata.
    let r = cmd(&w.store, &w.dir, json!({"action": "set_level", "event_id": ida, "level": "important", "scope": "thread"}));
    assert_eq!(r["changed"], 2, "{r}");
    assert_eq!(level(&w, "mid:a@example.test").0, "important");
    assert_eq!(texts(&w.store, "mid:b@example.test", "body"), vec!["第二封。"]);
    assert_eq!(level(&w, "mid:c@example.test").0, "metadata");
    // Lowering drops the body and its search entry but keeps the subject.
    let r = cmd(&w.store, &w.dir, json!({"action": "set_level", "event_id": ida, "level": "metadata"}));
    assert_eq!(r["changed"], 1, "{r}");
    assert!(texts(&w.store, "mid:a@example.test", "body").is_empty());
    assert_eq!(texts(&w.store, "mid:a@example.test", "subject"), vec!["项目"]);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM text_fts WHERE text_fts MATCH '第一封'"), 0);
    assert_eq!(cmd_err(&w.store, &w.dir, json!({"action": "set_level", "event_id": ida, "level": "ignore"})), "invalid_level");
    assert_eq!(cmd_err(&w.store, &w.dir, json!({"action": "set_level", "event_id": "nope", "level": "normal"})), "unknown_event");
}

#[test]
fn releveling_applies_new_rules_to_stored_mail_keeps_manual_levels_and_is_repeatable() {
    let w = World::new("scope-relevel");
    w.add_inbox("s@example.test", "bob@other.test", "你好", "", "看看这个。");
    w.add_inbox("m@example.test", "carol@other.test", "手动", "", "手动升级的。");
    w.add_inbox("n@example.test", "dave@other.test", "忽略我", "", "规则说忽略。");
    let sid = w.authorize(json!({}));
    w.sync();
    assert_eq!(level(&w, "mid:s@example.test").0, "metadata");
    let manual = w.event_id(&sid, "mid:m@example.test");
    cmd(&w.store, &w.dir, json!({"action": "set_level", "event_id": manual, "level": "important"}));
    // New rules: bob → normal (body is added from the source), dave → ignore (kept as metadata, never deleted), carol → metadata (manual wins).
    w.update(&sid, json!({"rules": [
        {"when": {"from": "bob@other.test"}, "level": "normal"},
        {"when": {"from": "dave@other.test"}, "level": "ignore"},
        {"when": {"from": "carol@other.test"}, "level": "metadata"}]}));
    let r = cmd(&w.store, &w.dir, json!({"action": "relevel"}));
    let s = &r["sources"][0]["stats"];
    assert_eq!((s["examined"].as_i64(), s["changed"].as_i64(), s["manual"].as_i64(), s["ignored_kept"].as_i64()), (Some(3), Some(2), Some(1), Some(1)), "{r}");
    assert_eq!(level(&w, "mid:n@example.test"), ("metadata".into(), "rule:1".into(), false));
    assert_eq!(level(&w, "mid:s@example.test"), ("normal".into(), "rule:0".into(), false));
    assert_eq!(texts(&w.store, "mid:s@example.test", "body"), vec!["看看这个。"]);
    assert_eq!(level(&w, "mid:m@example.test"), ("important".into(), "manual".into(), false));
    assert_eq!(level(&w, "mid:n@example.test").0, "metadata");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events WHERE kind='mail.message'"), 3);
    // Running it again changes nothing.
    let again = cmd(&w.store, &w.dir, json!({"action": "relevel", "source_id": sid}));
    assert_eq!(again["sources"][0]["stats"]["changed"], 0, "{again}");
    // Dropping the rule lowers it again and removes the stored body and its search entry.
    w.update(&sid, json!({}));
    let r = cmd(&w.store, &w.dir, json!({"action": "relevel"}));
    assert_eq!(r["sources"][0]["stats"]["changed"], 2, "{r}");
    assert_eq!(level(&w, "mid:s@example.test"), ("metadata".into(), "default".into(), false));
    assert!(texts(&w.store, "mid:s@example.test", "body").is_empty());
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM text_fts WHERE text_fts MATCH '看看这个'"), 0);
}

#[test]
fn relevel_skips_mail_whose_source_is_gone_and_recent_lists_metadata_only() {
    let w = World::new("scope-gone");
    let row = w.add_inbox("s@example.test", "bob@other.test", "你好", "", "看看这个。");
    w.add_inbox("t@example.test", "bob@other.test", "还在", "", "还在。");
    let sid = w.authorize(json!({}));
    w.sync();
    w.tree.expunge(row);
    w.update(&sid, json!({"rules": [{"when": {"from": "bob@other.test"}, "level": "normal"}]}));
    let r = cmd(&w.store, &w.dir, json!({"action": "relevel"}));
    let s = &r["sources"][0]["stats"];
    assert_eq!((s["source_missing"].as_i64(), s["changed"].as_i64()), (Some(1), Some(1)), "{r}");
    let list = cmd(&w.store, &w.dir, json!({"action": "mail_recent", "source_id": sid}));
    let rows = list["messages"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(!list.to_string().contains("还在。"), "the list carries no body text");
    assert!(rows.iter().any(|m| m["title"] == "你好" && m["level"] == "metadata"));
}

#[test]
fn scope_options_list_accounts_and_folders_with_counts_and_flag_the_never_read_ones() {
    let w = World::new("scope-options");
    w.add_inbox("a@example.test", "bob@other.test", "你好", "", "x");
    w.add_inbox("b@example.test", "bob@other.test", "再见", "", "y");
    w.tree.mailbox("Junk");
    let reply = mail(Some("c@example.test"), "Me <me@example.test>", "bob@other.test", "Re: 你好", D2, "", "z");
    w.tree.add(w.sent, &reply, Some("c@example.test"), "Re: 你好", "Me@Example.test", R + 60, Some(false));
    let r = cmd(&w.store, &w.dir, json!({"action": "mail_scope", "config": w.config(json!({}))}));
    let account = &r["accounts"][0];
    assert_eq!(account["address"], "me@example.test", "the account is named by the address it sends from");
    assert_eq!((account["id"].as_str(), account["scheme"].as_str()), (Some(ACCOUNT), Some("imap")), "{r}");
    let boxes = account["mailboxes"].as_array().unwrap();
    let find = |name: &str| boxes.iter().find(|m| m["name"] == name).unwrap_or_else(|| panic!("{name}"));
    assert_eq!((find("INBOX")["count"].as_i64(), find("INBOX")["role"].as_str(), find("INBOX")["never"].as_bool()), (Some(2), Some("inbox"), Some(false)));
    assert_eq!((find("Junk")["never"].as_bool(), find("Deleted Messages")["never"].as_bool(), find("Sent Messages")["role"].as_str()), (Some(true), Some(true), Some("sent")));
    assert_eq!(cmd_err(&w.store, &w.dir, json!({"action": "mail_scope", "config": {"version": 1, "mail_root": w.dir.join("none").to_string_lossy()}})), "mail_data_not_found");
}

/// What happens to mail no rule decides is the user's choice; so is whether AI clients may read this source.
#[test]
fn default_level_and_ai_access_are_user_settings() {
    let w = World::new("scope-defaults");
    w.add_inbox("d@example.test", "carol@other.test", "季度计划", "", "下周开会。");
    let sid = w.authorize(json!({"default_level": "normal"}));
    w.sync();
    let list = cmd(&w.store, &w.dir, json!({"action": "mail_recent", "source_id": sid}));
    assert_eq!(list["messages"][0]["level"], "normal", "{list}");
    let policy = |w: &World| w.store.lock().unwrap().connector_source(&sid).unwrap().unwrap().policy.agent_visible;
    assert!(!policy(&w), "AI may not read mail until the user allows it");
    cmd(&w.store, &w.dir, json!({"action": "update", "source_id": sid, "config": w.config(json!({"default_level": "normal"})), "agent_visible": true}));
    assert!(policy(&w));
}
