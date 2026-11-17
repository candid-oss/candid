//! Mail.app adapter against a synthetic `~/Library/Mail` tree (Envelope Index + .emlx), end to end through ingest.
mod common;
use common::*;
use candid_connectors::sync_all;
use candid_store::{TestHooks, TimelineError};
use serde_json::{json, Value};
use std::fs;
use std::sync::Arc;

const D1: &str = "Tue, 10 Nov 2026 09:00:00 +0000";
const D2: &str = "Tue, 10 Nov 2026 10:00:00 +0000";
const D3: &str = "Tue, 10 Nov 2026 11:00:00 +0000";
const R: i64 = 1_794_301_200; // seconds, 2026-11-10T09:00Z

struct World {
    dir: std::path::PathBuf,
    tree: MailTree,
    store: std::sync::Mutex<candid_store::TimelineStore>,
    inbox: i64,
    sent: i64,
    archive: i64,
    trash: i64,
    junk: i64,
}

impl World {
    fn new(label: &str) -> Self {
        let dir = temp(label);
        let tree = MailTree::new(&dir);
        let (inbox, sent, archive, trash, junk) = (tree.mailbox("INBOX"), tree.mailbox("Sent Messages"), tree.mailbox("Archive"), tree.mailbox("Deleted Messages"), tree.mailbox("Junk"));
        let store = store(&dir);
        World { dir, tree, store, inbox, sent, archive, trash, junk }
    }
    fn authorize(&self, extra: Value) -> String {
        let mut config = self.tree.config();
        for (k, v) in extra.as_object().unwrap() {
            config[k] = v.clone();
        }
        let r = cmd(&self.store, &self.dir, json!({"action": "authorize", "kind": "mail.apple", "config": config}));
        assert_eq!(r["probe"]["ok"], true, "{r}");
        r["source_id"].as_str().unwrap().to_string()
    }
    fn sync(&self) -> candid_connectors::SourceOutcome {
        let s = sync_all(&self.store, None, &self.dir);
        assert_eq!(s.sources.len(), 1);
        s.sources.into_iter().next().unwrap()
    }
    fn thread(&self) {
        let a = mail(Some("a@example.test"), "Alice <alice@example.test>", "me@example.test", "预算草案", D1, "", "请在周五前确认预算。\n\n-- \nAlice\nSynthetic Org");
        let b = mail(Some("b@example.test"), "Me <me@example.test>", "alice@example.test", "Re: 预算草案", D2, "In-Reply-To: <a@example.test>\r\nReferences: <a@example.test>\r\n", "好的，我周四给你。\n\n在 2026年11月10日 09:00，Alice 写道：\n> 请在周五前确认预算。");
        // Only In-Reply-To pointing at the reply: still the same thread.
        let c = mail(Some("c@example.test"), "Alice <alice@example.test>", "me@example.test", "Re: Re: 预算草案", D3, "In-Reply-To: <b@example.test>\r\n", "收到，谢谢。");
        self.tree.add(self.inbox, &a, Some("a@example.test"), "预算草案", "alice@example.test", R, Some(false));
        self.tree.add(self.sent, &b, Some("b@example.test"), "Re: 预算草案", "me@example.test", R + 3600, Some(false));
        self.tree.add(self.inbox, &c, Some("c@example.test"), "Re: Re: 预算草案", "alice@example.test", R + 7200, Some(false));
    }
}

#[test]
fn sync_builds_threads_cleans_text_and_stores_no_plain_addresses() {
    let w = World::new("apple-basic");
    w.thread();
    w.authorize(json!({}));
    let out = w.sync();
    assert_eq!((out.state.as_str(), out.inserted), ("synced", 3), "{out:?}");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM threads WHERE kind='mail_thread'"), 1);
    assert_eq!(count(&w.store, "SELECT COUNT(DISTINCT thread_id) FROM events WHERE kind='mail.message'"), 1);
    let title: String = w.store.lock().unwrap().raw_db().first("SELECT title FROM threads", &[], |r| candid_store::testing::RowExt::text(r, 0)).unwrap().unwrap();
    assert_eq!(title, "预算草案");
    // body = new text only; quoted history and signature are kept apart from extraction input.
    assert_eq!(texts(&w.store, "mid:a@example.test", "body"), vec!["请在周五前确认预算。"]);
    assert!(texts(&w.store, "mid:a@example.test", "quoted")[0].contains("Synthetic Org"));
    assert_eq!(texts(&w.store, "mid:b@example.test", "body"), vec!["好的，我周四给你。"]);
    assert!(texts(&w.store, "mid:b@example.test", "quoted")[0].contains("> 请在周五前确认预算。"));
    assert_eq!(texts(&w.store, "mid:a@example.test", "subject"), vec!["预算草案"]);
    let b = payload(&w.store, "mid:b@example.test");
    assert_eq!((b["direction"].as_str(), b["mailboxes"].clone()), (Some("out"), json!(["Sent Messages"])));
    let a = payload(&w.store, "mid:a@example.test");
    assert_eq!((a["direction"].as_str(), a["level"].as_str(), a["body_status"].as_str()), (Some("in"), Some("normal"), Some("complete")));
    assert_eq!(a["from"]["name"], "Alice");
    assert_eq!(a["from"]["address_hash"].as_str().unwrap().len(), 64);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events WHERE actor='self' AND kind='mail.message'"), 1);
    // No plain address anywhere in the database (payloads, titles, text, threads).
    let db = fs::read(w.dir.join("db/timeline.sqlite")).unwrap();
    let wal = fs::read(w.dir.join("db/timeline.sqlite-wal")).unwrap_or_default();
    for blob in [db, wal] {
        assert!(!String::from_utf8_lossy(&blob).contains("alice@example.test"));
    }
    // Repeated sync: idempotent, no conflicts, no revisions.
    let again = w.sync();
    assert_eq!((again.inserted, again.revised, again.removed), (0, 0, 0));
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM ingest_conflicts"), 0);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 3);
}

#[test]
fn same_message_in_two_mailboxes_partial_missing_id_and_not_downloaded() {
    let w = World::new("apple-variants");
    // Sent to myself: INBOX and Sent hold the same Message-ID.
    let selfmail = mail(Some("self@example.test"), "me@example.test", "me@example.test", "备忘", D1, "", "记得续费。");
    w.tree.add(w.inbox, &selfmail, Some("self@example.test"), "备忘", "me@example.test", R, Some(false));
    w.tree.add(w.sent, &selfmail, Some("self@example.test"), "备忘", "me@example.test", R, Some(false));
    // Partial: attachments not downloaded by Mail.
    let partial = mail(Some("partial@example.test"), "bob@example.test", "me@example.test", "大附件", D2, "", "正文已下载，附件没有。");
    w.tree.add(w.inbox, &partial, Some("partial@example.test"), "大附件", "bob@example.test", R + 60, Some(true));
    // No Message-ID header at all.
    let noid = mail(None, "carol@example.test", "me@example.test", "无标识", D2, "", "没有 Message-ID。");
    let noid_row = w.tree.add(w.inbox, &noid, None, "无标识", "carol@example.test", R + 120, Some(false));
    // Index row only: Mail never downloaded the body; it is not fetched for us.
    w.tree.add(w.inbox, b"", Some("remote@example.test"), "只有索引", "dave@example.test", R + 180, None);
    w.authorize(json!({}));
    let out = w.sync();
    let stats = out.stats.clone().unwrap();
    assert_eq!(out.inserted, 4, "{out:?}");
    assert_eq!((stats.same_message_other_mailbox, stats.partial, stats.not_downloaded), (1, 1, 1));
    assert_eq!(payload(&w.store, "mid:self@example.test")["mailboxes"], json!(["INBOX", "Sent Messages"]));
    assert_eq!(payload(&w.store, "mid:partial@example.test")["body_status"], "partial");
    let fallback = format!("local:{ACCOUNT}:INBOX:{noid_row}");
    // Unleveled by any rule: metadata only (the body is read live), keyed by the adapter's fallback key.
    assert_eq!(payload(&w.store, &fallback)["level_basis"], "default");
    assert!(texts(&w.store, &fallback, "body").is_empty());
    assert_eq!(texts(&w.store, &fallback, "subject"), vec!["无标识"]);
    let remote = payload(&w.store, "mid:remote@example.test");
    assert_eq!((remote["body_status"].as_str(), remote["level"].as_str()), (Some("not_downloaded"), Some("metadata")));
    assert_eq!(texts(&w.store, "mid:remote@example.test", "subject"), vec!["只有索引"]);
    assert!(texts(&w.store, "mid:remote@example.test", "body").is_empty());
}

#[test]
fn moves_are_revisions_and_deletions_are_tombstones() {
    let w = World::new("apple-move-delete");
    let a = mail(Some("move@example.test"), "erin@example.test", "me@example.test", "要归档", D1, "", "归档我。");
    let b = mail(Some("gone@example.test"), "erin@example.test", "me@example.test", "要删除", D1, "", "删除我。");
    let c = mail(Some("trash@example.test"), "erin@example.test", "me@example.test", "进废纸篓", D1, "", "丢进废纸篓。");
    let ra = w.tree.add(w.inbox, &a, Some("move@example.test"), "要归档", "erin@example.test", R, Some(false));
    let rb = w.tree.add(w.inbox, &b, Some("gone@example.test"), "要删除", "erin@example.test", R, Some(false));
    let rc = w.tree.add(w.inbox, &c, Some("trash@example.test"), "进废纸篓", "erin@example.test", R, Some(false));
    w.authorize(json!({}));
    assert_eq!(w.sync().inserted, 3);
    let sid = source(&w);
    let event_before: String = w.store.lock().unwrap().event_id(&sid, "mid:move@example.test").unwrap().unwrap();
    w.tree.move_row(ra, w.archive);
    w.tree.expunge(rb);
    w.tree.move_row(rc, w.trash);
    let out = w.sync();
    assert_eq!((out.inserted, out.revised, out.removed), (0, 1, 2), "{out:?}");
    // Same event, new mailbox, previous payload kept as a revision snapshot.
    assert_eq!(w.store.lock().unwrap().event_id(&sid, "mid:move@example.test").unwrap().unwrap(), event_before);
    let p = payload(&w.store, "mid:move@example.test");
    assert_eq!((p["mailboxes"].clone(), p["revision"].clone()), (json!(["Archive"]), json!(1)));
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM agent_event_revisions"), 1);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM tombstones WHERE reason='source_deleted' AND entity='event'"), 2);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 1);
    // A resend of a deleted message (e.g. restored from trash) is refused by its tombstone.
    w.tree.move_row(w.tree.db.query_row("SELECT MAX(ROWID) FROM messages WHERE mailbox=?1", [w.trash], |r| r.get(0)).unwrap(), w.inbox);
    let again = w.sync();
    assert_eq!(again.inserted, 0);
    assert!(w.store.lock().unwrap().source_key_tombstoned(&sid, "mid:trash@example.test").unwrap());
    assert_eq!(w.sync().revised, 0);
}

fn source(w: &World) -> String {
    w.store.lock().unwrap().connector_sources(&["mail.apple"]).unwrap()[0].source_id.clone()
}

#[test]
fn rules_scope_backfill_and_batches() {
    let w = World::new("apple-rules");
    let news = mail(Some("news@example.test"), "news@lists.example.test", "me@example.test", "周刊", D1, "List-Unsubscribe: <mailto:u@lists.example.test>\r\n", "很长的周刊正文。");
    let boss = mail(Some("boss@example.test"), "boss@corp.example.test", "me@example.test", "季度目标", D1, "", "请准备季度目标。");
    let spam = mail(Some("ad@example.test"), "ads@promo.example.test", "me@example.test", "促销", D1, "", "促销内容。");
    let junk = mail(Some("junk@example.test"), "x@example.test", "me@example.test", "垃圾", D1, "", "垃圾邮件。");
    let old = mail(Some("old@example.test"), "f@example.test", "me@example.test", "旧邮件", "Mon, 01 Jun 2026 09:00:00 +0000", "", "很久以前。");
    let noreply = mail(Some("bot@example.test"), "noreply@service.example.test", "me@example.test", "通知", D1, "", "自动通知。");
    w.tree.add(w.inbox, &news, Some("news@example.test"), "周刊", "news@lists.example.test", R, Some(false));
    w.tree.add(w.inbox, &boss, Some("boss@example.test"), "季度目标", "boss@corp.example.test", R, Some(false));
    w.tree.add(w.inbox, &spam, Some("ad@example.test"), "促销", "ads@promo.example.test", R, Some(false));
    w.tree.add(w.junk, &junk, Some("junk@example.test"), "垃圾", "x@example.test", R, Some(false));
    w.tree.add(w.inbox, &old, Some("old@example.test"), "旧邮件", "f@example.test", 1_780_304_400, Some(false));
    w.tree.add(w.inbox, &noreply, Some("bot@example.test"), "通知", "noreply@service.example.test", R, Some(false));
    let rules = json!([{"when": {"domain": "promo.example.test"}, "level": "ignore"}, {"when": {"from": "BOSS@corp.example.test"}, "level": "important"}]);
    // Dry run before authorising: counts only, nothing registered or written.
    let dry = cmd(&w.store, &w.dir, json!({"action": "dry_run", "kind": "mail.apple", "config": {"version": 1, "mail_root": w.tree.root.to_string_lossy(), "rules": rules}}));
    assert_eq!(dry["would_insert"], 3, "{dry}");
    assert_eq!(dry["stats"]["levels"], json!({"ignore": 1, "important": 1, "metadata": 2}));
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM sources"), 0);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM store_meta WHERE key LIKE 'connectors.%'"), 0);
    w.authorize(json!({"rules": rules, "batch_limit": 2}));
    let first = w.sync();
    assert_eq!((first.inserted, first.stats.as_ref().unwrap().more), (2, true), "{first:?}");
    let second = w.sync();
    assert_eq!(second.inserted, 1, "{second:?}");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 3);
    assert_eq!(payload(&w.store, "mid:boss@example.test")["level"], "important");
    assert_eq!(payload(&w.store, "mid:news@example.test")["level_basis"], "builtin:list");
    assert_eq!(payload(&w.store, "mid:bot@example.test")["level_basis"], "builtin:noreply");
    assert!(texts(&w.store, "mid:news@example.test", "body").is_empty(), "metadata level keeps no body");
    assert_eq!(texts(&w.store, "mid:news@example.test", "subject"), vec!["周刊"]);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events WHERE source_key IN ('mid:ad@example.test','mid:junk@example.test','mid:old@example.test')"), 0);
    // Widening the scope rescans rows the cursor passed; stored messages are not duplicated.
    let sid = source(&w);
    let mut cfg = w.tree.config();
    cfg["rules"] = rules.clone();
    cfg["backfill_days"] = json!(365);
    let r = cmd(&w.store, &w.dir, json!({"action": "update", "source_id": sid, "config": cfg}));
    assert_eq!(r["rescan"], true);
    assert_eq!(w.sync().inserted, 1);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM ingest_conflicts"), 0);
}

#[test]
fn ai_judge_is_only_a_flag() {
    let w = World::new("apple-ai");
    w.thread();
    w.authorize(json!({"ai_judge": {"enabled": true}}));
    let out = w.sync();
    assert_eq!(out.stats.unwrap().ai_pending, 3);
    assert_eq!(payload(&w.store, "mid:a@example.test")["ai_judge"], "pending");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM ai_calls"), 0);
}

#[test]
fn unknown_layouts_and_schemas_are_refused_without_writing() {
    for (label, layout, broken) in [("v11", "V11", false), ("schema", "V10", true)] {
        let dir = temp(&format!("apple-refuse-{label}"));
        let tree = MailTree::with_layout(&dir, layout);
        if broken {
            tree.db.execute_batch("ALTER TABLE messages RENAME COLUMN date_received TO received_at").unwrap();
        }
        let st = store(&dir);
        let r = cmd(&st, &dir, json!({"action": "authorize", "kind": "mail.apple", "config": tree.config()}));
        let code = if broken { "unknown_mail_schema" } else { "unknown_mail_version" };
        assert_eq!(r["probe"]["code"], code);
        let out = sync_all(&st, None, &dir).sources.remove(0);
        assert_eq!((out.state.as_str(), out.code.as_deref()), ("error", Some(code)));
        assert_eq!(count(&st, "SELECT COUNT(*) FROM events"), 0);
        let list = cmd(&st, &dir, json!({"action": "list"}));
        assert_eq!(list["sources"][0]["needs"], "unsupported_format");
    }
}

#[test]
fn missing_full_disk_access_is_reported_once_without_retry_noise() {
    let w = World::new("apple-fda");
    w.thread();
    w.authorize(json!({}));
    let mut perm = fs::metadata(&w.tree.root).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o000);
    fs::set_permissions(&w.tree.root, perm.clone()).unwrap();
    let first = sync_all(&w.store, None, &w.dir);
    let second = sync_all(&w.store, None, &w.dir);
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    fs::set_permissions(&w.tree.root, perm).unwrap();
    assert_eq!(first.sources[0].code.as_deref(), Some("full_disk_access_required"));
    assert_eq!((first.new_problems().len(), second.new_problems().len()), (1, 0));
    let list = cmd(&w.store, &w.dir, json!({"action": "list"}));
    assert_eq!((list["sources"][0]["status"].as_str(), list["sources"][0]["needs"].as_str()), (Some("error"), Some("full_disk_access")));
    // Access granted again: the next pass imports and clears the error.
    assert_eq!(w.sync().inserted, 3);
    let list = cmd(&w.store, &w.dir, json!({"action": "list"}));
    assert_eq!((list["sources"][0]["status"].as_str(), list["sources"][0]["needs"].clone()), (Some("active"), Value::Null));
}

#[test]
fn revoke_stops_reading_and_optionally_deletes() {
    let w = World::new("apple-revoke");
    w.thread();
    let sid = w.authorize(json!({}));
    assert_eq!(w.sync().inserted, 3);
    assert_eq!(cmd_err(&w.store, &w.dir, json!({"action": "authorize", "kind": "mail.apple", "config": w.tree.config()})), "already_authorized");
    cmd(&w.store, &w.dir, json!({"action": "revoke", "source_id": sid}));
    // Not read any more: even an unreadable tree would not matter.
    fs::rename(&w.tree.root, w.dir.join("moved-away")).unwrap();
    let out = w.sync();
    assert_eq!(out.state, "skipped");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 3, "kept by choice");
    assert_eq!(cmd_err(&w.store, &w.dir, json!({"action": "dry_run", "source_id": sid})), "source_revoked");
    let r = cmd(&w.store, &w.dir, json!({"action": "revoke", "source_id": sid, "delete_stored": true}));
    assert_eq!(r["deleted_events"], 3);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 0);
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM threads"), 0);
}

#[test]
fn interrupted_sync_commits_nothing_and_retries_cleanly() {
    let w = World::new("apple-crash");
    w.thread();
    let sid = w.authorize(json!({}));
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let f = fired.clone();
    w.store.lock().unwrap().set_hooks(TestHooks {
        ingest: Some(Box::new(move |stage: &str| {
            if stage == "beforeCommit" && !f.swap(true, std::sync::atomic::Ordering::SeqCst) {
                Err(TimelineError::SimulatedCrash)
            } else {
                Ok(())
            }
        })),
    });
    let out = w.sync();
    assert_eq!(out.state, "error");
    assert_eq!(count(&w.store, "SELECT COUNT(*) FROM events"), 0);
    assert_eq!(w.store.lock().unwrap().cursor(&sid, "mail").unwrap(), None);
    let retry = w.sync();
    assert_eq!((retry.state.as_str(), retry.inserted), ("synced", 3));
    assert_eq!(w.sync().inserted, 0);
}

#[test]
fn background_pass_backs_off_a_failing_source_but_explicit_sync_does_not() {
    let w = World::new("apple-backoff");
    w.thread();
    let sid = w.authorize(json!({}));
    let mut perm = fs::metadata(&w.tree.root).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o000);
    fs::set_permissions(&w.tree.root, perm.clone()).unwrap();
    let first = candid_connectors::sync_due(&w.store, &w.dir);
    let second = candid_connectors::sync_due(&w.store, &w.dir);
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    fs::set_permissions(&w.tree.root, perm).unwrap();
    assert_eq!(first.sources[0].state, "error");
    // Not read again right away: the background pass waits before retrying.
    assert_eq!((second.sources[0].state.as_str(), second.sources[0].code.as_deref()), ("skipped", Some("retry_later")));
    // Asking for it explicitly reads at once and, on success, clears the backoff.
    let explicit = sync_all(&w.store, Some(&sid), &w.dir);
    assert_eq!(explicit.sources[0].inserted, 3);
    assert_eq!(candid_connectors::sync_due(&w.store, &w.dir).sources[0].state, "synced");
}
