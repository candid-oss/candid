//! People from messages without a model: alias rules, vetoes, corrections that survive recompute. Synthetic data only.
mod common;
use candid_store::*;
use common::Fx;
use serde_json::{json, Value};

const T0: i64 = 1_799_992_800_000;

fn shot(fx: &Fx, key: &str, at: i64, app: &str) -> String {
    let mut s = IngestEvent::new(key, "screen.shot", Some(at));
    s.app_name = Some(app.into());
    s.title = Some(app.into());
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s])).unwrap();
    fx.texts(&format!("SELECT event_id FROM events WHERE source_key='{key}'"))[0].clone()
}

/// A chat frame in `app` titled `title`; each message is (from, sender, text).
fn chat(fx: &Fx, key: &str, at: i64, app: &str, title: &str, messages: &[(&str, &str, &str)]) {
    let id = shot(fx, key, at, app);
    let out = json!({"kind": "chat", "title": title, "excerpt": "", "messages": messages.iter().map(|(f, s, t)| json!({"from": f, "sender": s, "text": t})).collect::<Vec<_>>()});
    fx.store.excerpt_write(&id, &out, "test-vlm").unwrap();
}

fn mail(fx: &Fx, key: &str, at: i64, hash: &str, name: &str) {
    let mut e = IngestEvent::new(key, "mail.message", Some(at));
    e.actor = "other".into();
    e.app_name = Some("邮件".into());
    e.title = Some(key.into());
    e.payload = json!({"from": {"name": name, "address_hash": hash}, "direction": "in"});
    e.text = vec![IngestText::new("subject", key), IngestText::new("body", &format!("来自 {name} 的邮件 {key}"))];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![e])).unwrap();
}

fn person(fx: &Fx, who: &str) -> Value {
    fx.store.call_tool("person", json!({"who": who}).to_string().as_bytes(), &fx.client).map_err(|_| "person").unwrap()
}

fn sender_entity(fx: &Fx, text: &str) -> Option<String> {
    fx.texts(&format!("SELECT COALESCE(sender_entity_id,'') FROM messages WHERE text='{text}'")).pop().filter(|s| !s.is_empty())
}

fn count_people(fx: &Fx) -> i64 {
    fx.scalar("SELECT COUNT(*) FROM entities WHERE kind='person' AND status<>'merged' AND status<>'rejected'")
}

fn seed(fx: &Fx) {
    chat(fx, "a", T0, "微信", "张三", &[("other", "", "wx私聊张三"), ("me", "", "wx我回张三")]);
    // The same name speaking in a group, same app: the same person.
    chat(fx, "b", T0 + 10_000, "微信", "项目群（4）", &[("other", "张三", "wx群里张三"), ("other", "小张", "wx群里小张")]);
    // Another app, same name: not merged on its own.
    chat(fx, "c", T0 + 20_000, "QQ", "张三", &[("other", "", "qq私聊张三")]);
}

#[test]
fn same_app_name_merges_and_other_app_does_not() {
    let fx = Fx::make("people-rules");
    seed(&fx);
    fx.store.people_sync().unwrap();
    let wx = sender_entity(&fx, "wx私聊张三").unwrap();
    assert_eq!(sender_entity(&fx, "wx群里张三").as_ref(), Some(&wx), "private chat name and group sender are one person");
    let qq = sender_entity(&fx, "qq私聊张三").unwrap();
    assert_ne!(qq, wx, "same name in another app stays separate");
    assert_ne!(sender_entity(&fx, "wx群里小张").unwrap(), wx);
    assert_eq!(sender_entity(&fx, "wx我回张三").as_deref(), Some(SELF_ENTITY));
    assert_eq!(count_people(&fx), 4, "me, wechat 张三, wechat 小张, qq 张三");
    // Incremental: a new message from a known alias only gets backfilled.
    chat(&fx, "d", T0 + 30_000, "微信", "张三", &[("other", "", "wx私聊张三"), ("other", "", "wx新消息张三")]);
    let r = fx.store.people_sync().unwrap();
    assert!(r.get("people").is_none(), "no recompute for a known alias: {r}");
    assert_eq!(sender_entity(&fx, "wx新消息张三").unwrap(), wx);
    assert_eq!(fx.store.people_sync().unwrap()["changed"], false);
    // The card spans every alias.
    let card = person(&fx, "wechat/张三");
    assert_eq!(card["person_id"], wx.as_str());
    assert!(card["aliases"].as_array().unwrap().iter().any(|a| a["key"] == "handle:wechat/张三"));
    assert_eq!(card["open_items"], json!([]));
    let texts: Vec<&str> = card["messages"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"wx群里张三") && texts.contains(&"wx我回张三"), "{texts:?}");
    assert!(!texts.contains(&"qq私聊张三"));
    // A name shared by two people lists candidates.
    assert_eq!(person(&fx, "张三")["candidates"].as_array().unwrap().len(), 2);
}

#[test]
fn group_veto_blocks_model_verdicts_and_corrections_survive_recompute() {
    let fx = Fx::make("people-corrections");
    seed(&fx);
    fx.store.people_sync().unwrap();
    let verdict = |a: &str, b: &str, v: &str| fx.store.people_set_verdict(&PeopleVerdict { a: a.into(), b: b.into(), verdict: v.into(), corroborated: true, ..Default::default() }).unwrap();
    // Both spoke in the same group: a "same" verdict is ignored.
    verdict("handle:wechat/张三", "handle:wechat/小张", "same");
    fx.store.people_sync().unwrap();
    assert_ne!(sender_entity(&fx, "wx群里张三"), sender_entity(&fx, "wx群里小张"));
    // An uncorroborated verdict is only a candidate.
    fx.store.people_set_verdict(&PeopleVerdict { a: "handle:qq/张三".into(), b: "handle:wechat/张三".into(), verdict: "same".into(), corroborated: false, ..Default::default() }).unwrap();
    fx.store.people_sync().unwrap();
    assert_ne!(sender_entity(&fx, "qq私聊张三"), sender_entity(&fx, "wx私聊张三"));
    // Corroborated: merged by the model, from cache alone.
    verdict("handle:qq/张三", "handle:wechat/张三", "same");
    fx.store.people_sync().unwrap();
    assert_eq!(sender_entity(&fx, "qq私聊张三"), sender_entity(&fx, "wx私聊张三"));
    // You say they are not the same person: that wins, and survives any recompute.
    fx.store.person_correct(&PersonCorrectionRequest { action: "split".into(), key: "handle:qq/张三".into(), other: Some("handle:wechat/张三".into()), name: None }).unwrap();
    fx.store.people_recompute().unwrap();
    assert_ne!(sender_entity(&fx, "qq私聊张三"), sender_entity(&fx, "wx私聊张三"));
    // Merge by key, rename, and ids that change on recompute do not matter.
    fx.store.person_correct(&PersonCorrectionRequest { action: "merge".into(), key: "handle:wechat/小张".into(), other: Some("handle:wechat/张三".into()), name: None }).unwrap();
    let merged = sender_entity(&fx, "wx群里小张");
    assert_eq!(merged, sender_entity(&fx, "wx群里张三"), "your merge overrides the rule veto");
    let id = merged.unwrap();
    fx.store.person_correct(&PersonCorrectionRequest { action: "rename".into(), key: id.clone(), other: None, name: Some("张三（同事）".into()) }).unwrap();
    fx.store.people_recompute().unwrap();
    assert_eq!(sender_entity(&fx, "wx群里小张"), sender_entity(&fx, "wx私聊张三"));
    let name = fx.texts(&format!("SELECT name FROM entities WHERE entity_id='{}'", sender_entity(&fx, "wx私聊张三").unwrap()));
    assert_eq!(name, ["张三（同事）"]);
    // Filters follow the person across aliases.
    let list = fx.store.call_tool("conversations", json!({"person": "张三（同事）"}).to_string().as_bytes(), &fx.client).map_err(|_| ()).unwrap();
    assert_eq!(list["conversations"].as_array().unwrap().len(), 2, "{list}");
    let found = fx.store.call_tool("find", json!({"q": "群里", "person": "张三（同事）"}).to_string().as_bytes(), &fx.client).map_err(|_| ()).unwrap();
    assert_eq!(found["results"].as_array().unwrap().len(), 2, "{found}");
}

#[test]
fn mail_senders_and_voice_people_are_reused() {
    let fx = Fx::make("people-mail");
    mail(&fx, "m1", T0, "hash-a", "Alice Wong");
    mail(&fx, "m2", T0 + 1000, "hash-a", "alice wong");
    mail(&fx, "m3", T0 + 2000, "hash-b", "Bob Li");
    mail(&fx, "m4", T0 + 3000, "hash-c", "Alice Wong");
    // A voice-library person already exists.
    fx.store.raw_db().execute("INSERT INTO entities VALUES('voice-p','person','王五','confirmed',NULL,1,1)", &[]).unwrap();
    fx.store.raw_db().execute("INSERT INTO entity_aliases VALUES('voice-p','王五','name','user')", &[]).unwrap();
    fx.store.raw_db().execute("INSERT INTO entity_aliases VALUES('voice-p','v-1','voice','user')", &[]).unwrap();
    let mut rec = IngestEvent::new("rec1", "audio.recording", Some(T0));
    rec.title = Some("周会".into());
    let mut seg = IngestEvent::new("s1", "speech.segment", Some(T0 + 5000));
    seg.parent_key = Some("rec1".into());
    seg.payload = json!({"recording_event_id": "f1", "speaker_label": "王五"});
    seg.text = vec![IngestText::new("transcript", "我来说两句")];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![rec, seg])).unwrap();
    fx.store.curated_sync().unwrap();
    fx.store.people_sync().unwrap();
    let a1 = sender_entity(&fx, "来自 Alice Wong 的邮件 m1").unwrap();
    assert_eq!(sender_entity(&fx, "来自 alice wong 的邮件 m2").unwrap(), a1, "same address");
    assert_ne!(sender_entity(&fx, "来自 Alice Wong 的邮件 m4").unwrap(), a1, "same display name, other address");
    assert_ne!(sender_entity(&fx, "来自 Bob Li 的邮件 m3").unwrap(), a1);
    assert_eq!(sender_entity(&fx, "我来说两句").as_deref(), Some("voice-p"), "voice person reused");
    // Different addresses with different display names can never be joined by a model.
    fx.store.people_set_verdict(&PeopleVerdict { a: "email:hash-a".into(), b: "email:hash-b".into(), verdict: "same".into(), corroborated: true, ..Default::default() }).unwrap();
    fx.store.people_sync().unwrap();
    assert_ne!(sender_entity(&fx, "来自 Bob Li 的邮件 m3").unwrap(), a1);
    // Only the App may correct.
    let r = fx.store.call_tool("app_person_correct", json!({"action": "confirm", "key": "email:hash-a"}).to_string().as_bytes(), &fx.client);
    assert!(r.is_err());
    // Stale people disappear on recompute, voice people never do.
    fx.store.people_recompute().unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM entities WHERE entity_id='voice-p'"), 1);
}

/// Run by hand on a copy of a real data root: `PEOPLE_ROOT=<dir with timeline.sqlite> cargo test -p candid-store --test people real_copy -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_copy() {
    let root = std::path::PathBuf::from(std::env::var("PEOPLE_ROOT").expect("PEOPLE_ROOT"));
    let store = common::open(&root);
    let t = std::time::Instant::now();
    store.curated_sync().unwrap();
    println!("curated {:?}", t.elapsed());
    let t = std::time::Instant::now();
    println!("first {} in {:?}", store.people_sync().unwrap(), t.elapsed());
    let db = store.raw_db();
    for (label, sql) in [
        ("people (not merged/rejected)", "SELECT COUNT(*) FROM entities WHERE kind='person' AND status NOT IN ('merged','rejected')"),
        ("aliases", "SELECT COUNT(*) FROM entity_aliases"),
        ("messages", "SELECT COUNT(*) FROM messages"),
        ("messages with sender_entity_id", "SELECT COUNT(*) FROM messages WHERE sender_entity_id IS NOT NULL"),
    ] {
        println!("{label}: {}", db.scalar(sql, &[]).unwrap());
    }
    let t = std::time::Instant::now();
    println!("second {} in {:?}", store.people_sync().unwrap(), t.elapsed());
}
