//! Mail connector (agent-platform-v1 §2.0.1): one shared pipeline — decode, dedupe by Message-ID, thread, clean,
//! level — over swappable read-only storage adapters (Mail.app data, IMAP via email-lib).
pub mod apple;
pub mod config;
pub mod imap;
pub mod mime;

use config::{decide, role_of, Decision, Facts, Level, MailConfig, Role};
use candid_store::{agents::redact, sha256_hex, IngestBatch, IngestCursor, IngestEvent, IngestText, IngestThread, PayloadRevision};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const CURSOR_STREAM: &str = "mail";

/// Metadata the adapter's own index holds, used when the body was never downloaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexMeta {
    pub subject: Option<String>,
    pub from_address: Option<String>,
    pub from_name: Option<String>,
    pub date_ms: Option<i64>,
}

/// One new message as an adapter found it. `key` is `mid:<Message-ID>` or the adapter's fallback key.
#[derive(Clone, Debug, Default)]
pub struct RawMail {
    pub key: String,
    pub account: Option<String>,
    pub mailbox: String,
    /// RFC 822 bytes; `None` when the client never downloaded the body (it is not fetched on our behalf).
    pub bytes: Option<Vec<u8>>,
    pub partial: bool,
    pub received_ms: Option<i64>,
    pub index: IndexMeta,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub key: String,
    pub mailbox: String,
}

/// What one adapter pass returns. `present` lists every current location (outside trash/junk/drafts) of the
/// messages this source already stored; `None` means presence could not be established this pass, so nothing is
/// treated as deleted.
#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub new: Vec<RawMail>,
    pub present: Option<Vec<Location>>,
    pub cursor: String,
    pub more: bool,
    pub issues: Vec<String>,
    pub format: String,
}

/// What the source already holds: `source_key → (payload, thread key)`.
pub type Known = BTreeMap<String, (Value, Option<String>)>;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub scanned: usize,
    pub new_events: usize,
    pub same_message_other_mailbox: usize,
    pub levels: BTreeMap<String, usize>,
    pub threads_new: usize,
    pub decode_failures: usize,
    pub partial: usize,
    pub not_downloaded: usize,
    pub removed: usize,
    pub moved: usize,
    pub ai_pending: usize,
    pub more: bool,
    pub issues: Vec<String>,
}

pub fn mid_key(id: &str) -> String {
    format!("mid:{id}")
}

pub fn address_hash(salt: &str, address: &str) -> String {
    sha256_hex(format!("{salt}\u{1f}{}", address.trim().to_lowercase()))
}

struct Group {
    first: RawMail,
    mailboxes: BTreeSet<String>,
}

fn redacted(s: &str) -> String {
    redact(s)
}

/// One scanned message, decoded and placed in its thread, before its level is decided.
struct Item {
    key: String,
    account: Option<String>,
    mailbox: String,
    parsed: Option<mime::ParsedMail>,
    p: mime::ParsedMail,
    body_status: &'static str,
    mailboxes: BTreeSet<String>,
    role: Role,
    outgoing: bool,
    thread_key: String,
    start: Option<i64>,
}

/// What the stored mail says about the user's own correspondence: threads with a message the user sent, and the
/// salted hashes of everyone the user has written to.
#[derive(Default)]
struct Correspondence {
    threads: BTreeSet<String>,
    people: BTreeSet<String>,
}

impl Correspondence {
    fn from_known(known: &Known) -> Self {
        let mut c = Correspondence::default();
        for (payload, thread) in known.values() {
            if payload["direction"] == "out" {
                c.threads.extend(thread.clone());
                c.people.extend(payload["to_hashes"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)));
            }
        }
        c
    }
    fn replied(&self, salt: &str, thread: &str, from: Option<&str>) -> bool {
        self.threads.contains(thread) || from.is_some_and(|a| self.people.contains(&address_hash(salt, a)))
    }
}

/// Builds one ingest batch from a scan. Pure: no store access, so dry runs use the same code path.
pub fn build(source_id: &str, adapter: &str, config: &MailConfig, salt: &str, scan: &Scan, known: &Known) -> (IngestBatch, Stats) {
    build_with(source_id, adapter, config, salt, scan, known, false)
}

/// `headers_only`: decode only the mail headers (preview counts); nothing of the body is read.
pub fn build_with(source_id: &str, adapter: &str, config: &MailConfig, salt: &str, scan: &Scan, known: &Known, headers_only: bool) -> (IngestBatch, Stats) {
    let mut stats = Stats { scanned: scan.new.len(), more: scan.more, issues: scan.issues.clone(), ..Default::default() };
    // 1. Same Message-ID in several mailboxes → one message.
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for raw in &scan.new {
        match groups.get_mut(&raw.key) {
            Some(g) => {
                stats.same_message_other_mailbox += 1;
                g.mailboxes.insert(raw.mailbox.clone());
                if (g.first.bytes.is_none() || g.first.partial) && raw.bytes.is_some() && !raw.partial {
                    let mailbox = g.first.mailbox.clone();
                    g.first = raw.clone();
                    g.first.mailbox = mailbox;
                }
            }
            None => {
                order.push(raw.key.clone());
                groups.insert(raw.key.clone(), Group { first: raw.clone(), mailboxes: BTreeSet::from([raw.mailbox.clone()]) });
            }
        }
    }
    let presence: Option<BTreeMap<String, BTreeSet<String>>> = scan.present.as_ref().map(|p| {
        let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for l in p {
            m.entry(l.key.clone()).or_default().insert(l.mailbox.clone());
        }
        m
    });
    let self_addresses: BTreeSet<String> = config.self_addresses.iter().map(|a| a.trim().to_lowercase()).collect();
    let mut threads: BTreeMap<String, IngestThread> = BTreeMap::new();
    let mut batch_threads: BTreeMap<String, String> = BTreeMap::new(); // key → thread key, this batch
    let known_threads: BTreeSet<String> = known.values().filter_map(|(_, t)| t.clone()).collect();
    // 2. Decode, find the thread. Level decisions follow once the thread facts of the whole batch are known.
    let mut items: Vec<Item> = Vec::new();
    for key in order {
        if known.contains_key(&key) {
            continue; // already stored; presence below records any new mailbox
        }
        let g = &groups[&key];
        let raw = &g.first;
        let parsed = raw.bytes.as_deref().and_then(|b| if headers_only { mime::parse_headers(b) } else { mime::parse(b) });
        let body_status = match (&raw.bytes, &parsed) {
            (None, _) => "not_downloaded",
            (Some(_), None) => "undecodable",
            (Some(_), Some(_)) if raw.partial => "partial",
            _ => "complete",
        };
        let p = parsed.clone().unwrap_or_else(|| mime::ParsedMail {
            subject: raw.index.subject.clone().unwrap_or_default(),
            from_address: raw.index.from_address.clone(),
            from_name: raw.index.from_name.clone(),
            date_ms: raw.index.date_ms,
            ..Default::default()
        });
        match body_status {
            "not_downloaded" => stats.not_downloaded += 1,
            "undecodable" => stats.decode_failures += 1,
            "partial" => stats.partial += 1,
            _ => {}
        }
        let mailboxes: BTreeSet<String> = presence.as_ref().and_then(|m| m.get(&key).cloned()).unwrap_or_else(|| g.mailboxes.clone());
        let role = mailboxes.iter().map(|m| role_of(m)).find(|r| *r != Role::Other).unwrap_or(Role::Other);
        let outgoing = role == Role::Sent || p.from_address.as_ref().is_some_and(|a| self_addresses.contains(a));
        // Thread: a known/batch parent's thread first, then the References root, In-Reply-To, the message itself.
        let parents: Vec<String> = p.in_reply_to.iter().chain(p.references.iter().rev()).map(|id| mid_key(id)).collect();
        let inherited = parents.iter().find_map(|k| known.get(k).and_then(|(_, t)| t.clone()).or_else(|| batch_threads.get(k).cloned()));
        let root = p.references.first().or(p.in_reply_to.first()).cloned().or(p.message_id.clone());
        let thread_key = inherited.unwrap_or_else(|| format!("thread:{}", root.unwrap_or_else(|| key.clone())));
        batch_threads.insert(key.clone(), thread_key.clone());
        let start = p.date_ms.or(raw.received_ms);
        items.push(Item { key, account: raw.account.clone(), mailbox: raw.mailbox.clone(), parsed, p, body_status, mailboxes, role, outgoing, thread_key, start });
    }
    let mut mine = Correspondence::from_known(known);
    for it in items.iter().filter(|i| i.outgoing) {
        mine.threads.insert(it.thread_key.clone());
        mine.people.extend(it.p.to.iter().chain(it.p.cc.iter()).map(|a| address_hash(salt, a)));
    }
    // 3. Level, text and payload.
    let mut events: Vec<IngestEvent> = Vec::new();
    for it in items {
        let p = &it.p;
        let recipients: Vec<&str> = p.to.iter().chain(p.cc.iter()).map(String::as_str).collect();
        let replied = mine.replied(salt, &it.thread_key, p.from_address.as_deref());
        let mut decision = decide(
            config,
            &Facts { from: p.from_address.as_deref(), recipients, subject: &p.subject, list: p.list, automated: p.automated, mailbox: &it.mailbox, role: it.role, replied },
        );
        if it.parsed.is_none() && decision.level > Level::Metadata {
            // Nothing decodable to store beyond the index metadata.
            decision = Decision { level: Level::Metadata, basis: decision.basis, candidate: false, ai_pending: false };
        }
        *stats.levels.entry(decision.level.as_str().into()).or_default() += 1;
        if decision.candidate {
            *stats.levels.entry("important_candidate".into()).or_default() += 1;
        }
        if decision.level == Level::Ignore {
            continue;
        }
        if decision.ai_pending {
            stats.ai_pending += 1;
        }
        if !known_threads.contains(&it.thread_key) {
            let title = mime::base_subject(&redacted(&p.subject));
            let t = threads.entry(it.thread_key.clone()).or_insert_with(|| IngestThread {
                source_key: it.thread_key.clone(),
                kind: "mail_thread".into(),
                title: Some(title).filter(|t| !t.is_empty()),
                started_ms: it.start,
                ..Default::default()
            });
            if let (Some(s), Some(old)) = (it.start, t.started_ms) {
                t.started_ms = Some(s.min(old));
            }
        }
        let mut from = Map::new();
        if let Some(n) = &p.from_name {
            from.insert("name".into(), json!(redacted(n)));
        }
        if let Some(a) = &p.from_address {
            from.insert("address_hash".into(), json!(address_hash(salt, a)));
        }
        let mut payload = json!({
            "adapter": adapter,
            "direction": if it.outgoing { "out" } else { "in" },
            "from": from,
            "to_count": p.to.len(),
            "cc_count": p.cc.len(),
            "has_attachments": !p.attachments.is_empty(),
            "attachments": p.attachments.iter().take(50).map(|a| json!({"name": a.name.as_deref().map(redacted), "mime": a.mime, "size": a.size})).collect::<Vec<_>>(),
            "mailboxes": it.mailboxes.iter().collect::<Vec<_>>(),
            "level": decision.level.as_str(),
            "level_basis": decision.basis,
            "body_status": it.body_status,
            "body_format": p.body_format,
            "list": p.list,
            "automated": p.automated,
        });
        if headers_only {
            payload["quoted_chars"] = json!(0);
        } else {
            let (_, quoted, signature) = mime::split_body(&p.body);
            payload["quoted_chars"] = json!(quoted.chars().count());
            payload["signature_chars"] = json!(signature.chars().count());
        }
        if it.outgoing {
            // The people the user wrote to, as salted hashes, so later mail from them can be recognised.
            let hashes: BTreeSet<String> = p.to.iter().chain(p.cc.iter()).take(50).map(|a| address_hash(salt, a)).collect();
            payload["to_hashes"] = json!(hashes);
        }
        if decision.candidate {
            payload["important_candidate"] = json!(true);
        }
        if let Some(a) = &it.account {
            payload["account"] = json!(a);
        }
        if decision.ai_pending {
            payload["ai_judge"] = json!("pending");
        }
        let mut e = IngestEvent::new(&it.key, "mail.message", it.start);
        e.time_basis = if it.start.is_some() { "source".into() } else { "imported".into() };
        e.title = Some(redacted(&p.subject)).filter(|s| !s.is_empty());
        e.thread_key = Some(it.thread_key);
        e.actor = if it.outgoing { "self".into() } else { "other".into() };
        e.payload = payload;
        e.text = text_for(decision.level, &it.p);
        events.push(e);
        stats.new_events += 1;
    }
    stats.threads_new = threads.len();
    // 4. Deletions and moves of what the source already stored.
    let mut removed = Vec::new();
    let mut revisions = Vec::new();
    if let Some(presence) = &presence {
        for (key, (payload, _)) in known {
            match presence.get(key) {
                None => removed.push(key.clone()),
                Some(boxes) => {
                    let current: BTreeSet<String> = payload["mailboxes"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                    if &current != boxes {
                        let mut next = payload.clone();
                        next.as_object_mut().map(|o| o.remove("revision"));
                        next["mailboxes"] = json!(boxes.iter().collect::<Vec<_>>());
                        revisions.push(PayloadRevision { source_key: key.clone(), payload: next });
                    }
                }
            }
        }
    }
    stats.removed = removed.len();
    stats.moved = revisions.len();
    let batch = IngestBatch {
        source_id: source_id.into(),
        cursor_after: Some(IngestCursor::new(CURSOR_STREAM, &scan.cursor)),
        threads: threads.into_values().collect(),
        events,
        removed_source_keys: removed,
        payload_revisions: revisions,
        ..Default::default()
    };
    (batch, stats)
}

/// The text a message keeps at a level: subject always; body and the quoted/signature remainder from normal up.
/// Quoted history and signature are stored for search, never extraction input (subject + body only).
pub fn text_for(level: Level, p: &mime::ParsedMail) -> Vec<IngestText> {
    let mut text = Vec::new();
    if !p.subject.is_empty() {
        text.push(IngestText::new("subject", &redacted(&p.subject)));
    }
    if level >= Level::Normal {
        let (body, quoted, signature) = mime::split_body(&p.body);
        if !body.is_empty() {
            text.push(IngestText::new("body", &redacted(&body)));
        }
        let excluded = [quoted.as_str(), signature.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n\n");
        if !excluded.is_empty() {
            text.push(IngestText::new("quoted", &redacted(&excluded)));
        }
    }
    text
}

/// A stored level change: the new payload, and the replacement text when the body has to be added or dropped.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelChange {
    pub source_key: String,
    pub payload: Value,
    pub text: Option<Vec<IngestText>>,
}

/// The payload with a new level; `revision` is left to the store.
pub fn leveled_payload(old: &Value, level: Level, basis: &str, candidate: bool, ai_pending: bool) -> Value {
    let mut next = old.clone();
    let o = next.as_object_mut().expect("mail payload is an object");
    o.remove("revision");
    o.insert("level".into(), json!(level.as_str()));
    o.insert("level_basis".into(), json!(basis));
    if candidate { o.insert("important_candidate".into(), json!(true)); } else { o.remove("important_candidate"); }
    if ai_pending { o.insert("ai_judge".into(), json!("pending")); } else { o.remove("ai_judge"); }
    next
}

pub fn level_of(payload: &Value) -> Level {
    match payload["level"].as_str() {
        Some("important") => Level::Important,
        Some("metadata") => Level::Metadata,
        Some("ignore") => Level::Ignore,
        _ => Level::Normal,
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct RelevelStats {
    pub examined: usize,
    pub changed: usize,
    pub unchanged: usize,
    /// Raised or lowered by hand: rules never override it.
    pub manual: usize,
    /// No longer in a readable mailbox; the normal sync removes it.
    pub source_missing: usize,
    /// Would need a body that cannot be read now.
    pub unreadable: usize,
    /// A rule says ignore, but stored mail is never deleted by re-leveling: kept as metadata only.
    pub ignored_kept: usize,
    pub levels: BTreeMap<String, usize>,
}

/// Levels the stored mail again under the current rules. `live` holds the current copy of each message (read from the
/// source, in memory only), because stored rows keep neither addresses nor body for metadata-only mail.
pub fn relevel(config: &MailConfig, salt: &str, known: &Known, live: &BTreeMap<String, Option<RawMail>>) -> (Vec<LevelChange>, RelevelStats) {
    let mut stats = RelevelStats::default();
    let mut changes = Vec::new();
    let mine = Correspondence::from_known(known);
    for (key, (payload, thread)) in known {
        stats.examined += 1;
        let old = level_of(payload);
        if payload["level_basis"] == "manual" {
            stats.manual += 1;
            *stats.levels.entry(old.as_str().into()).or_default() += 1;
            continue;
        }
        let Some(Some(raw)) = live.get(key) else {
            stats.source_missing += 1;
            continue;
        };
        let parsed = raw.bytes.as_deref().and_then(mime::parse);
        let mut p = parsed.clone().unwrap_or_else(|| mime::ParsedMail {
            subject: raw.index.subject.clone().unwrap_or_default(),
            from_address: raw.index.from_address.clone(),
            from_name: raw.index.from_name.clone(),
            date_ms: raw.index.date_ms,
            list: payload["list"].as_bool().unwrap_or(false),
            automated: payload["automated"].as_bool().unwrap_or(false),
            ..Default::default()
        });
        if parsed.is_none() {
            p.body_format = "none";
        }
        let mailboxes: Vec<String> = payload["mailboxes"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        let role = mailboxes.iter().map(|m| role_of(m)).find(|r| *r != Role::Other).unwrap_or(Role::Other);
        let mailbox = mailboxes.first().cloned().unwrap_or_else(|| raw.mailbox.clone());
        let recipients: Vec<&str> = p.to.iter().chain(p.cc.iter()).map(String::as_str).collect();
        let replied = mine.replied(salt, thread.as_deref().unwrap_or(""), p.from_address.as_deref());
        let mut d = decide(config, &Facts { from: p.from_address.as_deref(), recipients, subject: &p.subject, list: p.list, automated: p.automated, mailbox: &mailbox, role, replied });
        if d.level == Level::Ignore {
            stats.ignored_kept += 1;
            d.level = Level::Metadata;
        }
        // A body that cannot be decoded now can still be dropped, never added.
        if parsed.is_none() && d.level > Level::Metadata && old == Level::Metadata {
            stats.unreadable += 1;
            continue;
        }
        let candidate = payload["important_candidate"].as_bool().unwrap_or(false);
        *stats.levels.entry(d.level.as_str().into()).or_default() += 1;
        if d.level == old && d.candidate == candidate && d.basis == payload["level_basis"].as_str().unwrap_or("") {
            stats.unchanged += 1;
            continue;
        }
        let body_changes = (old >= Level::Normal) != (d.level >= Level::Normal);
        changes.push(LevelChange {
            source_key: key.clone(),
            payload: leveled_payload(payload, d.level, &d.basis, d.candidate, d.ai_pending),
            text: body_changes.then(|| text_for(d.level, &p)),
        });
        stats.changed += 1;
    }
    (changes, stats)
}

/// A message's body as the source holds it now. Nothing of this is stored or indexed.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct LiveBody {
    /// `ok`, `partial` (Mail.app has only part of the body), `not_downloaded`, `undecodable` or `source_deleted`.
    pub state: String,
    pub subject: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub date_ms: Option<i64>,
    pub body: String,
    pub quoted: String,
}

pub fn live_body(raw: Option<&RawMail>) -> LiveBody {
    let Some(raw) = raw else { return LiveBody { state: "source_deleted".into(), ..Default::default() } };
    let mut out = LiveBody {
        subject: raw.index.subject.clone().unwrap_or_default(),
        from_name: raw.index.from_name.clone(),
        from_address: raw.index.from_address.clone(),
        date_ms: raw.index.date_ms,
        ..Default::default()
    };
    match raw.bytes.as_deref() {
        None => out.state = "not_downloaded".into(),
        Some(b) => match mime::parse(b) {
            None => out.state = "undecodable".into(),
            Some(p) => {
                let (body, quoted, signature) = mime::split_body(&p.body);
                out.state = if raw.partial { "partial" } else { "ok" }.into();
                out.subject = p.subject;
                out.from_name = p.from_name.or(out.from_name);
                out.from_address = p.from_address.or(out.from_address);
                out.date_ms = p.date_ms.or(out.date_ms);
                out.body = body;
                out.quoted = [quoted, signature].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n\n");
            }
        },
    }
    out
}
