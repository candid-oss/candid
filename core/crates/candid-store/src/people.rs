//! People (docs/plans/data-layer-v1.md §1): who a message is from, without a model.
//!
//! **Alias keys are the stable identity.** A key is `kind:alias` (`handle:wechat/张三`, `email:<address or hash>`,
//! `name:张三`, `voice:<id>`, or `self`). Entity ids change when people are recomputed; keys do not, so your
//! corrections (`corrections(target_type='entity')`) are written against keys:
//!
//! | action | target_id | value_json |
//! |---|---|---|
//! | `merge` | key A | `{"with": key B}` |
//! | `split` ("not the same person") | key A | `{"from": key B}` |
//! | `rename` | any key of the person | `{"name": "…"}` |
//! | `reject` / `confirm` | any key of the person | `{}` |
//!
//! Recompute order: rule joins (seeded from voice-library people) → your corrections → cached model verdicts
//! (store_meta `people.llm/1:*`, see [`PeopleVerdict`]). Recomputing never calls a model. Entities this module creates
//! have ids starting `ppl-`; people created elsewhere (the voice library) are reused and never deleted.
//!
//! State kept in `store_meta`: `people.node/1:<key>` (key → entity id, `-` for a rejected person), `people.cursor/1`
//! (last message seq backfilled) and `people.state/1` (fingerprint of everything a full recompute depends on).

use crate::db::{int, opt_text, placeholders, text, RowExt, P};
use crate::screen_excerpt::chat_title;
use crate::{uuid_v7, Result, TimelineError, TimelineStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

mod judge;
mod pinyin;
pub use judge::{PeopleCandidate, BATCH};

pub const SELF_ENTITY: &str = "ppl-self";
const PREFIX: &str = "ppl-";
const SELF_KEY: &str = "self";
const NODE: &str = "people.node/1:";
const CURSOR: &str = "people.cursor/1";
const STATE: &str = "people.state/1";
const LLM: &str = "people.llm/1:";
const LLM_REV: &str = "people.llm.rev/1";
const RULES: &str = "rules/1";
const SEP: char = '\u{1f}';

/// A cached model conclusion about two alias keys; written by the LLM-candidate step, applied on every recompute.
///
/// `same` merges only when `corroborated` is true (the caller found a rule-level supporting fact, plan §0); otherwise
/// it stays a candidate and nothing changes. `different` becomes a soft "cannot connect" edge. `unsure` is recorded
/// (with `at_ms`) so the caller can wait before asking again. Rule vetoes and your `split` corrections always win.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PeopleVerdict {
    pub a: String,
    pub b: String,
    /// `same`, `different` or `unsure`.
    pub verdict: String,
    #[serde(default)]
    pub corroborated: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub model_ref: Option<String>,
    #[serde(default)]
    pub prompt_version: Option<String>,
    /// Hash of both alias sets, prompt and model the answer was given for; a different key asks again.
    #[serde(default)]
    pub cache_key: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonCorrectionRequest {
    /// `merge`, `split`, `rename`, `reject` or `confirm`.
    pub action: String,
    /// An alias key; `merge`, `rename`, `reject` and `confirm` also accept a person id.
    pub key: String,
    /// The other alias key (or person id for `merge`) of `merge` and `split`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

/// Trim, full-width to half-width, drop emoji and joiners, collapse spaces, lower-case.
pub(crate) fn normalize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = match c {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            _ => c,
        };
        let n = c as u32;
        if (0x1f000..=0x1faff).contains(&n) || (0x2600..=0x27bf).contains(&n) || (0x2b00..=0x2bff).contains(&n) || matches!(n, 0xfe0f | 0x200d | 0x200b | 0x20e3) {
            continue;
        }
        out.extend(c.to_lowercase());
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn app_slug(app: Option<&str>) -> String {
    let a = normalize(app.unwrap_or(""));
    let known = [
        ("企业微信", "wecom"), ("wecom", "wecom"), ("微信", "wechat"), ("wechat", "wechat"), ("qq", "qq"), ("钉钉", "dingtalk"), ("dingtalk", "dingtalk"),
        ("飞书", "feishu"), ("lark", "feishu"), ("feishu", "feishu"), ("telegram", "telegram"), ("whatsapp", "whatsapp"), ("slack", "slack"),
        ("discord", "discord"), ("信息", "imessage"), ("messages", "imessage"), ("imessage", "imessage"),
    ];
    for (needle, slug) in known {
        if a == needle || a.starts_with(needle) {
            return slug.into();
        }
    }
    if a.is_empty() { "chat".into() } else { a.replace(' ', "-") }
}

fn generic_speaker(n: &str) -> bool {
    let rest = n.strip_prefix("说话人").or_else(|| n.strip_prefix("speaker")).map(str::trim);
    n == "unresolved" || n == "unknown" || rest.is_some_and(|r| r.is_empty() || r.chars().all(|c| c.is_ascii_digit()))
}

fn key_of(kind: &str, alias: &str) -> String {
    if kind == "other" { alias.to_string() } else { format!("{kind}:{alias}") }
}

#[derive(Default)]
struct Node {
    kind: &'static str,
    alias: String,
    names: HashMap<String, i64>,
    count: i64,
    group_convs: HashSet<String>,
}

impl Node {
    fn display(&self) -> String {
        let mut v: Vec<_> = self.names.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        match v.first() {
            Some((n, _)) => n.to_string(),
            None => match self.kind {
                "handle" => self.alias.split_once('/').map(|x| x.1).unwrap_or(&self.alias).to_string(),
                "email" => format!("邮箱 {}", self.alias.chars().take(8).collect::<String>()),
                _ => self.alias.clone(),
            },
        }
    }
}

struct Raw {
    seq: i64,
    conv: String,
    kind: String,
    app: Option<String>,
    title: Option<String>,
    actor: String,
    sender: Option<String>,
    addr: Option<String>,
    current: Option<String>,
}

/// (key, node kind, alias, display name)
type NodeRef = (String, &'static str, String, Option<String>);

fn classify(r: &Raw, groups: &HashSet<String>) -> Option<NodeRef> {
    if r.actor == "self" {
        return Some((SELF_KEY.into(), "self", SELF_KEY.into(), None));
    }
    let sender = r.sender.as_deref().map(str::trim).filter(|s| !s.is_empty());
    match r.kind.as_str() {
        "chat" => {
            let shown = match sender {
                Some(s) => s.to_string(),
                None if !groups.contains(&r.conv) => chat_title(r.title.as_deref()?),
                None => return None,
            };
            let n = normalize(&shown);
            (!n.is_empty()).then(|| (format!("handle:{}/{n}", app_slug(r.app.as_deref())), "handle", format!("{}/{n}", app_slug(r.app.as_deref())), Some(shown)))
        }
        "mail" => match r.addr.as_deref().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()) {
            Some(a) => Some((format!("email:{a}"), "email", a, sender.map(String::from))),
            None => {
                let n = normalize(sender?);
                (!n.is_empty() && !generic_speaker(&n)).then(|| (format!("name:{n}"), "name", n, sender.map(String::from)))
            }
        },
        _ => {
            let n = normalize(sender?);
            (!n.is_empty() && !generic_speaker(&n)).then(|| (format!("name:{n}"), "name", n, sender.map(String::from)))
        }
    }
}

struct Dsu {
    idx: HashMap<String, usize>,
    keys: Vec<String>,
    parent: Vec<usize>,
    forbidden: Vec<(usize, usize)>,
}

impl Dsu {
    fn new() -> Self {
        Dsu { idx: HashMap::new(), keys: vec![], parent: vec![], forbidden: vec![] }
    }
    fn id(&mut self, key: &str) -> usize {
        if let Some(i) = self.idx.get(key) {
            return *i;
        }
        let i = self.keys.len();
        self.idx.insert(key.into(), i);
        self.keys.push(key.into());
        self.parent.push(i);
        i
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }
    /// Joins two sets unless a forbidden pair would end up together. Returns whether they are now one set.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return true;
        }
        for (x, y) in self.forbidden.clone() {
            let (rx, ry) = (self.find(x), self.find(y));
            if (rx == ra && ry == rb) || (rx == rb && ry == ra) {
                return false;
            }
        }
        self.parent[ra] = rb;
        true
    }
}

struct Corrections {
    merges: Vec<(String, String)>,
    splits: Vec<(String, String)>,
    names: HashMap<String, (usize, String)>,
    status: HashMap<String, (usize, bool)>,
}

fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b { (a.into(), b.into()) } else { (b.into(), a.into()) }
}

impl TimelineStore {
    fn people_corrections(&self) -> Result<Corrections> {
        let rows = self.db.query("SELECT target_id,action,value_json FROM corrections WHERE target_type='entity' ORDER BY created_ms,rowid", &[], |r| (r.text(0), r.text(1), r.text(2)))?;
        let mut state: BTreeMap<(String, String), (usize, bool)> = BTreeMap::new();
        let mut out = Corrections { merges: vec![], splits: vec![], names: HashMap::new(), status: HashMap::new() };
        for (order, (target, action, value)) in rows.into_iter().enumerate() {
            let v: Value = serde_json::from_str(&value).unwrap_or(Value::Null);
            match action.as_str() {
                "merge" | "split" => {
                    if let Some(other) = v["with"].as_str().or_else(|| v["from"].as_str()) {
                        state.insert(pair(&target, other), (order, action == "merge"));
                    }
                }
                "rename" => {
                    if let Some(n) = v["name"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
                        out.names.insert(target, (order, n.to_string()));
                    }
                }
                "reject" => {
                    out.status.insert(target, (order, false));
                }
                "confirm" => {
                    out.status.insert(target, (order, true));
                }
                _ => {}
            }
        }
        for ((a, b), (_, merge)) in state {
            if merge { out.merges.push((a, b)) } else { out.splits.push((a, b)) }
        }
        Ok(out)
    }

    fn people_verdicts(&self) -> Result<Vec<(PeopleVerdict, i64)>> {
        let rows = self.db.query("SELECT value FROM store_meta WHERE key GLOB 'people.llm/1:*' ORDER BY key", &[], |r| r.text(0))?;
        Ok(rows
            .into_iter()
            .filter_map(|v| {
                let j: Value = serde_json::from_str(&v).ok()?;
                let at = j["at_ms"].as_i64().unwrap_or(0);
                Some((serde_json::from_value(j).ok()?, at))
            })
            .collect())
    }

    /// Stores a model conclusion and makes the next `people_sync` recompute. Cheap; no model is called.
    pub fn people_set_verdict(&self, v: &PeopleVerdict) -> Result<()> {
        if !["same", "different", "unsure"].contains(&v.verdict.as_str()) {
            return Err(bad("verdict 只能是 same、different 或 unsure"));
        }
        let (a, b) = pair(&v.a, &v.b);
        let mut j = serde_json::to_value(v).unwrap_or(Value::Null);
        j["a"] = json!(a);
        j["b"] = json!(b);
        j["at_ms"] = json!(self.clock.now_ms());
        self.db.transaction(|| {
            self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(format!("{LLM}{a}{SEP}{b}")), text(j.to_string())])?;
            let rev = self.meta_value(LLM_REV)?.and_then(|x| x.parse::<i64>().ok()).unwrap_or(0) + 1;
            self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(LLM_REV), text(rev.to_string())])
        })
    }

    fn people_fingerprint(&self) -> Result<String> {
        let c = self.db.first("SELECT COUNT(*),COALESCE(MAX(rowid),0) FROM corrections WHERE target_type='entity'", &[], |r| (r.int(0), r.int(1)))?.unwrap_or_default();
        let e = self
            .db
            .first("SELECT COUNT(*),COALESCE(MAX(updated_ms),0) FROM entities WHERE kind='person' AND substr(entity_id,1,4)<>'ppl-'", &[], |r| (r.int(0), r.int(1)))?
            .unwrap_or_default();
        let a = self.db.scalar(
            "SELECT COUNT(*) FROM entity_aliases a JOIN entities e USING(entity_id) WHERE e.kind='person' AND substr(e.entity_id,1,4)<>'ppl-' AND a.source='user'",
            &[],
        )?;
        let l = self.meta_value(LLM_REV)?.unwrap_or_default();
        Ok(format!("{RULES}|{}.{}|{}.{}|{a}|{l}", c.0, c.1, e.0, e.1))
    }

    fn people_raws(&self, after: i64) -> Result<Vec<Raw>> {
        self.db.query(
            "SELECT m.seq,m.conversation_id,c.kind,c.app,c.title,m.actor,m.sender,m.sender_entity_id,
               CASE WHEN c.kind='mail' AND m.actor<>'self' THEN (SELECT COALESCE(json_extract(e.payload_json,'$.from.address'),json_extract(e.payload_json,'$.from.address_hash'))
                 FROM message_sources s JOIN events e ON e.event_id=s.event_id WHERE s.message_id=m.message_id AND e.kind='mail.message' LIMIT 1) END
             FROM messages m JOIN conversations c ON c.conversation_id=m.conversation_id WHERE m.seq>? ORDER BY m.seq",
            &[int(after)],
            |r| Raw { seq: r.int(0), conv: r.text(1), kind: r.text(2), app: r.opt_text(3), title: r.opt_text(4), actor: r.text(5), sender: r.opt_text(6), current: r.opt_text(7), addr: r.opt_text(8) },
        )
    }

    /// Chat conversations that are groups: someone other than the title (or two people) spoke. `scope` limits the
    /// check to those conversations when only part of the messages is being read.
    fn people_groups(&self, raws: &[Raw], full: bool) -> Result<HashSet<String>> {
        let chats: BTreeMap<&str, &Raw> = raws.iter().filter(|r| r.kind == "chat").map(|r| (r.conv.as_str(), r)).collect();
        let mut senders: HashMap<String, HashSet<String>> = HashMap::new();
        if full {
            for r in raws.iter().filter(|r| r.kind == "chat" && r.actor != "self") {
                if let Some(s) = r.sender.as_deref().map(normalize).filter(|s| !s.is_empty()) {
                    senders.entry(r.conv.clone()).or_default().insert(s);
                }
            }
        } else if !chats.is_empty() {
            let ids: Vec<&str> = chats.keys().copied().collect();
            let p: Vec<P> = ids.iter().map(|i| text(*i)).collect();
            for (c, s) in self.db.query(
                &format!("SELECT DISTINCT conversation_id,sender FROM messages WHERE actor<>'self' AND sender IS NOT NULL AND conversation_id IN ({})", placeholders(ids.len())),
                &p,
                |r| (r.text(0), r.text(1)),
            )? {
                let n = normalize(&s);
                if !n.is_empty() {
                    senders.entry(c).or_default().insert(n);
                }
            }
        }
        Ok(chats
            .into_iter()
            .filter(|(c, r)| {
                let title = normalize(&chat_title(r.title.as_deref().unwrap_or("")));
                senders.get(*c).is_some_and(|s| s.len() >= 2 || s.iter().any(|n| *n != title))
            })
            .map(|(c, _)| c.to_string())
            .collect())
    }

    /// Brings people up to date. Cheap when nothing changed; new messages from known aliases only backfill
    /// `sender_entity_id`; a new alias, a changed correction or verdict, or a changed voice-library person recomputes.
    pub fn people_sync(&self) -> Result<Value> {
        let cursor: i64 = self.meta_value(CURSOR)?.and_then(|v| v.parse().ok()).unwrap_or(-1);
        let max_seq = self.db.scalar("SELECT COALESCE(MAX(seq),0) FROM messages", &[])?;
        if cursor >= 0 && self.meta_value(STATE)?.as_deref() == Some(self.people_fingerprint()?.as_str()) {
            if max_seq <= cursor {
                return Ok(json!({"changed": false}));
            }
            let raws = self.people_raws(cursor)?;
            let groups = self.people_groups(&raws, false)?;
            let mut targets: Vec<(i64, Option<String>, Option<String>)> = vec![];
            let mut known = true;
            let mut cache: HashMap<String, Option<String>> = HashMap::new();
            for r in &raws {
                let Some((key, ..)) = classify(r, &groups) else {
                    continue;
                };
                if !cache.contains_key(&key) {
                    let v = self.meta_value(&format!("{NODE}{key}"))?;
                    cache.insert(key.clone(), v);
                }
                match &cache[&key] {
                    None => {
                        known = false;
                        break;
                    }
                    Some(v) => targets.push((r.seq, r.current.clone(), (v != "-").then(|| v.clone()))),
                }
            }
            if known {
                let updated = self.db.transaction(|| {
                    let n = self.people_backfill(targets)?;
                    self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(CURSOR), text(max_seq.to_string())])?;
                    Ok(n)
                })?;
                return Ok(json!({"changed": updated > 0, "backfilled": updated}));
            }
        }
        self.people_recompute()
    }

    fn people_backfill(&self, targets: Vec<(i64, Option<String>, Option<String>)>) -> Result<usize> {
        let mut by: BTreeMap<Option<String>, Vec<i64>> = BTreeMap::new();
        for (seq, current, want) in targets {
            if current != want {
                by.entry(want).or_default().push(seq);
            }
        }
        let mut n = 0;
        for (entity, seqs) in by {
            for chunk in seqs.chunks(500) {
                let mut p: Vec<P> = vec![opt_text(entity.as_deref())];
                p.extend(chunk.iter().map(|s| int(*s)));
                self.db.execute(&format!("UPDATE messages SET sender_entity_id=? WHERE seq IN ({})", placeholders(chunk.len())), &p)?;
                n += chunk.len();
            }
        }
        Ok(n)
    }

    /// Rebuilds every person from the messages, your corrections and the cached verdicts. Never calls a model.
    pub fn people_recompute(&self) -> Result<Value> {
        let now = self.clock.now_ms();
        let max_seq = self.db.scalar("SELECT COALESCE(MAX(seq),0) FROM messages", &[])?;
        let raws = self.people_raws(0)?;
        let groups = self.people_groups(&raws, true)?;

        // Nodes from messages.
        let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
        let mut mail_names: HashMap<String, HashSet<String>> = HashMap::new();
        let mut assigned: Vec<(i64, Option<String>, Option<String>)> = vec![]; // seq, current, node key
        nodes.insert(SELF_KEY.into(), Node { kind: "self", alias: SELF_KEY.into(), ..Default::default() });
        for r in &raws {
            let Some((key, kind, alias, display)) = classify(r, &groups) else {
                assigned.push((r.seq, r.current.clone(), None));
                continue;
            };
            let n = nodes.entry(key.clone()).or_insert_with(|| Node { kind, alias, ..Default::default() });
            n.count += 1;
            if let Some(d) = display {
                if kind == "email" {
                    mail_names.entry(key.clone()).or_default().insert(normalize(&d));
                }
                *n.names.entry(d).or_default() += 1;
            }
            if kind == "handle" && groups.contains(&r.conv) {
                n.group_convs.insert(r.conv.clone());
            }
            assigned.push((r.seq, r.current.clone(), Some(key)));
        }

        // Existing people.
        struct Existing {
            name: String,
            status: String,
            merged_into: Option<String>,
            created: i64,
        }
        let mut existing: HashMap<String, Existing> = HashMap::new();
        for (id, name, status, into, created) in self.db.query("SELECT entity_id,name,status,merged_into,created_ms FROM entities WHERE kind='person'", &[], |r| (r.text(0), r.text(1), r.text(2), r.opt_text(3), r.int(4)))? {
            existing.insert(id, Existing { name, status, merged_into: into, created });
        }
        let mut user_aliases: HashMap<String, Vec<String>> = HashMap::new(); // external entity -> seed keys
        for (id, kind, alias) in self.db.query("SELECT entity_id,alias_kind,alias FROM entity_aliases WHERE source='user' AND alias_kind IN ('voice','name')", &[], |r| (r.text(0), r.text(1), r.text(2)))? {
            if existing.contains_key(&id) && !id.starts_with(PREFIX) {
                user_aliases.entry(id).or_default().push(key_of(&kind, &alias));
            }
        }
        let resolve = |mut id: String| -> Option<String> {
            for _ in 0..8 {
                let e = existing.get(&id)?;
                match (e.status.as_str(), &e.merged_into) {
                    ("merged", Some(n)) => id = n.clone(),
                    ("rejected", _) => return None,
                    _ => return Some(id),
                }
            }
            None
        };
        // Seed groups: each voice-library person's own aliases, merged people following their target.
        let mut seeds: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, keys) in &user_aliases {
            if let Some(target) = resolve(id.clone()) {
                seeds.entry(target).or_default().extend(keys.iter().cloned());
            }
        }
        for keys in seeds.values() {
            for k in keys {
                nodes.entry(k.clone()).or_insert_with(|| {
                    let (kind, alias) = k.split_once(':').unwrap_or(("name", k));
                    Node { kind: if kind == "voice" { "voice" } else { "name" }, alias: alias.into(), ..Default::default() }
                });
            }
        }

        let corrections = self.people_corrections()?;
        let mut dsu = Dsu::new();
        for k in nodes.keys() {
            dsu.id(k);
        }
        // User "not the same person" first, so every later join is checked against it.
        for (a, b) in &corrections.splits {
            if nodes.contains_key(a) && nodes.contains_key(b) {
                let (x, y) = (dsu.id(a), dsu.id(b));
                dsu.forbidden.push((x, y));
            }
        }
        for keys in seeds.values() {
            for w in keys.windows(2) {
                let (x, y) = (dsu.id(&w[0]), dsu.id(&w[1]));
                dsu.union(x, y);
            }
        }
        let mut touched: HashSet<String> = HashSet::new();
        for (a, b) in &corrections.merges {
            if nodes.contains_key(a) && nodes.contains_key(b) {
                let (x, y) = (dsu.id(a), dsu.id(b));
                if dsu.union(x, y) {
                    touched.insert(a.clone());
                    touched.insert(b.clone());
                }
            }
        }
        // Cached model verdicts: `different` forbids; `same` joins only when corroborated and not vetoed.
        let verdicts = self.people_verdicts()?;
        for (v, _) in &verdicts {
            if v.verdict == "different" && nodes.contains_key(&v.a) && nodes.contains_key(&v.b) {
                let (x, y) = (dsu.id(&v.a), dsu.id(&v.b));
                dsu.forbidden.push((x, y));
            }
        }
        let vetoed = |a: &str, b: &str| -> bool {
            let (Some(na), Some(nb)) = (nodes.get(a), nodes.get(b)) else { return false };
            if na.kind == "handle" && nb.kind == "handle" && na.group_convs.intersection(&nb.group_convs).next().is_some() {
                return true;
            }
            if na.kind == "email" && nb.kind == "email" {
                let (x, y) = (mail_names.get(a), mail_names.get(b));
                return matches!((x, y), (Some(x), Some(y)) if !x.is_empty() && !y.is_empty() && x.is_disjoint(y));
            }
            false
        };
        let mut model_keys: HashSet<String> = HashSet::new();
        for (v, _) in &verdicts {
            if v.verdict == "same" && v.corroborated && nodes.contains_key(&v.a) && nodes.contains_key(&v.b) && !vetoed(&v.a, &v.b) {
                let (x, y) = (dsu.id(&v.a), dsu.id(&v.b));
                if dsu.union(x, y) {
                    model_keys.insert(v.a.clone());
                    model_keys.insert(v.b.clone());
                }
            }
        }

        // Components, biggest first so stable ids go to the people with the most history.
        let mut comps: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for k in nodes.keys() {
            let i = dsu.id(k);
            let r = dsu.find(i);
            comps.entry(r).or_default().push(k.clone());
        }
        let mut comps: Vec<Vec<String>> = comps.into_values().collect();
        comps.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));

        // Where each key was placed last time.
        let mut registry: HashMap<String, String> = HashMap::new();
        for (k, v) in self.db.query("SELECT key,value FROM store_meta WHERE key GLOB 'people.node/1:*'", &[], |r| (r.text(0), r.text(1)))? {
            registry.insert(k[NODE.len()..].to_string(), v);
        }
        let mut seed_of: HashMap<&str, &String> = HashMap::new();
        for (id, keys) in &seeds {
            for k in keys {
                seed_of.insert(k.as_str(), id);
            }
        }

        let mut claimed: HashSet<String> = HashSet::new();
        let mut desired_node: BTreeMap<String, String> = BTreeMap::new();
        struct Plan {
            id: String,
            name: String,
            status: Option<String>,
            external: bool,
            renamed: bool,
            keys: Vec<String>,
            absorb: Vec<String>,
        }
        let mut plans: Vec<Plan> = vec![];
        for (placed, comp) in comps.iter().enumerate() {
            let is_self = comp.iter().any(|k| k == SELF_KEY);
            let mut external: Vec<String> = comp.iter().filter_map(|k| seed_of.get(k.as_str()).map(|s| s.to_string())).collect::<BTreeSet<_>>().into_iter().collect();
            external.sort_by_key(|id| (existing.get(id).map(|e| e.created).unwrap_or(0), id.clone()));
            external.retain(|id| !claimed.contains(id));
            let id = if is_self {
                SELF_ENTITY.to_string()
            } else if let Some(first) = external.first() {
                first.clone()
            } else {
                let mut votes: HashMap<&String, usize> = HashMap::new();
                for k in comp {
                    if let Some(id) = registry.get(k).filter(|id| id.starts_with(PREFIX) && *id != SELF_ENTITY) {
                        *votes.entry(id).or_default() += 1;
                    }
                }
                let mut v: Vec<_> = votes.into_iter().filter(|(id, _)| !claimed.contains(*id)).collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(existing.get(a.0).map(|e| e.created).cmp(&existing.get(b.0).map(|e| e.created))).then(a.0.cmp(b.0)));
                v.first().map(|x| x.0.clone()).unwrap_or_else(|| format!("{PREFIX}{}", uuid_v7(Some(now + placed as i64))))
            };
            claimed.insert(id.clone());
            let absorb: Vec<String> = external.into_iter().filter(|e| *e != id && !claimed.contains(e)).collect();
            for a in &absorb {
                claimed.insert(a.clone());
            }
            let is_external = !id.starts_with(PREFIX);
            let rename = comp.iter().filter_map(|k| corrections.names.get(k)).max_by_key(|x| x.0).map(|x| x.1.clone());
            let status = comp.iter().filter_map(|k| corrections.status.get(k)).max_by_key(|x| x.0).map(|x| x.1);
            let name = rename.clone().or_else(|| {
                if is_self {
                    Some("我".into())
                } else if is_external {
                    existing.get(&id).map(|e| e.name.clone())
                } else {
                    None
                }
            });
            let name = name.unwrap_or_else(|| {
                let mut best: Vec<(&Node, &String)> = comp.iter().filter_map(|k| nodes.get(k).map(|n| (n, k))).filter(|(n, _)| !matches!(n.kind, "voice" | "self")).collect();
                best.sort_by(|a, b| b.0.count.cmp(&a.0.count).then(a.1.cmp(b.1)));
                best.first().map(|(n, _)| n.display()).or_else(|| comp.first().cloned()).unwrap_or_default()
            });
            let user_touched = comp.iter().any(|k| touched.contains(k));
            let status_s = match status {
                Some(false) => Some("rejected".to_string()),
                Some(true) => Some("confirmed".into()),
                None if is_self || is_external || rename.is_some() || user_touched => Some("confirmed".into()),
                None => Some("candidate".into()),
            };
            let rejected = status_s.as_deref() == Some("rejected");
            for k in comp {
                desired_node.insert(k.clone(), if rejected { "-".into() } else { id.clone() });
            }
            plans.push(Plan { id, name, status: status_s, external: is_external, renamed: rename.is_some(), keys: comp.clone(), absorb });
        }

        let (created, removed) = self.db.transaction(|| {
            let mut created = 0;
            let mut desired_ids: HashSet<String> = HashSet::new();
            for p in &plans {
                desired_ids.insert(p.id.clone());
                match existing.get(&p.id) {
                    None => {
                        self.db.execute("INSERT INTO entities VALUES(?,'person',?,?,NULL,?,?)", &[text(&p.id), text(&p.name), opt_text(p.status.as_deref()), int(now), int(now)])?;
                        created += 1;
                    }
                    Some(e) => {
                        let status = if p.external && p.status.as_deref() == Some("candidate") { e.status.as_str() } else { p.status.as_deref().unwrap_or(&e.status) };
                        let name = if p.external && !p.renamed { e.name.as_str() } else { p.name.as_str() };
                        let status = if p.external && e.status == "merged" { "confirmed" } else { status };
                        if e.name != name || e.status != status || e.merged_into.is_some() {
                            self.db.execute("UPDATE entities SET name=?,status=?,merged_into=NULL,updated_ms=? WHERE entity_id=?", &[text(name), text(status), int(now), text(&p.id)])?;
                        }
                    }
                }
                for dup in &p.absorb {
                    if existing.get(dup).is_some_and(|e| e.status != "merged" || e.merged_into.as_deref() != Some(p.id.as_str())) {
                        self.db.execute("UPDATE entities SET status='merged',merged_into=?,updated_ms=? WHERE entity_id=?", &[text(&p.id), int(now), text(dup)])?;
                    }
                }
                // Aliases: add what is missing, drop rule/model aliases that no longer belong.
                let mut want: BTreeSet<(String, String, &str)> = BTreeSet::new();
                for k in &p.keys {
                    let n = &nodes[k];
                    let src = if model_keys.contains(k) { "model" } else { "rule" };
                    match n.kind {
                        "self" => {
                            want.insert((SELF_KEY.into(), "other".into(), src));
                        }
                        kind => {
                            want.insert((n.alias.clone(), kind.into(), src));
                            if kind == "email" {
                                for d in mail_names.get(k).into_iter().flatten().filter(|d| !d.is_empty()) {
                                    want.insert((d.clone(), "name".into(), "rule"));
                                }
                            }
                        }
                    }
                }
                let have = self.db.query("SELECT alias,alias_kind,source FROM entity_aliases WHERE entity_id=?", &[text(&p.id)], |r| (r.text(0), r.text(1), r.text(2)))?;
                for (alias, kind, source) in have {
                    if source != "user" && !want.iter().any(|(a, k, _)| *a == alias && *k == kind) {
                        self.db.execute("DELETE FROM entity_aliases WHERE entity_id=? AND alias=? AND alias_kind=?", &[text(&p.id), text(&alias), text(&kind)])?;
                    }
                }
                for (alias, kind, source) in want {
                    self.db.execute("INSERT OR IGNORE INTO entity_aliases VALUES(?,?,?,?)", &[text(&p.id), text(&alias), text(&kind), text(source)])?;
                }
            }

            // Registry.
            for k in registry.keys().filter(|k| !desired_node.contains_key(*k)) {
                self.db.execute("DELETE FROM store_meta WHERE key=?", &[text(format!("{NODE}{k}"))])?;
            }
            for (k, v) in &desired_node {
                if registry.get(k) != Some(v) {
                    self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(format!("{NODE}{k}")), text(v)])?;
                }
            }
            // Messages.
            let targets: Vec<_> = assigned
                .into_iter()
                .map(|(seq, cur, key)| (seq, cur, key.and_then(|k| desired_node.get(&k).cloned()).filter(|v| v != "-")))
                .collect();
            self.people_backfill(targets)?;
            // People that no longer exist; messages pointing at them were re-pointed above.
            let mut removed = 0;
            for id in self.db.query("SELECT entity_id FROM entities WHERE substr(entity_id,1,4)='ppl-'", &[], |r| r.text(0))? {
                if !desired_ids.contains(&id) {
                    self.db.execute("DELETE FROM entities WHERE entity_id=?", &[text(&id)])?;
                    removed += 1;
                }
            }
            self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(CURSOR), text(max_seq.to_string())])?;
            Ok((created, removed))
        })?;
        // Our own writes moved the fingerprint; record the state they produced.
        self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(STATE), text(self.people_fingerprint()?)])?;
        let people = plans.iter().filter(|p| p.status.as_deref() != Some("rejected")).count();
        Ok(json!({"changed": true, "people": people, "aliases": nodes.len(), "created": created, "removed": removed}))
    }

    /// The alias key standing for a person: a key as given, or the best key of a person id.
    fn people_key(&self, key_or_id: &str) -> Result<String> {
        if self.meta_value(&format!("{NODE}{key_or_id}"))?.is_some() {
            return Ok(key_or_id.to_string());
        }
        let mut keys: Vec<String> = self
            .db
            .query("SELECT substr(key,?) FROM store_meta WHERE key GLOB 'people.node/1:*' AND value=?", &[int(NODE.len() as i64 + 1), text(key_or_id)], |r| r.text(0))?;
        let rank = |k: &str| match k.split(':').next().unwrap_or("") {
            "self" => 0,
            "email" => 1,
            "handle" => 2,
            "voice" => 3,
            _ => 4,
        };
        keys.sort_by_key(|k| (rank(k), k.clone()));
        keys.into_iter().next().ok_or_else(|| bad("不认识这个人或别名"))
    }

    /// Your correction: writes it against alias keys and recomputes (from cache; no model).
    pub fn person_correct(&self, req: &PersonCorrectionRequest) -> Result<Value> {
        let _ = self.people_sync();
        let key = self.people_key(req.key.trim())?;
        let value = match req.action.as_str() {
            "merge" => {
                let other = self.people_key(req.other.as_deref().unwrap_or("").trim())?;
                if other == key {
                    return Err(bad("两边是同一个别名"));
                }
                json!({"with": other})
            }
            "split" => {
                let other = req.other.as_deref().unwrap_or("").trim().to_string();
                if self.meta_value(&format!("{NODE}{key}"))?.is_none() || self.meta_value(&format!("{NODE}{other}"))?.is_none() {
                    return Err(bad("拆分需要两个别名键"));
                }
                json!({"from": other})
            }
            "rename" => {
                let name = req.name.as_deref().map(str::trim).filter(|n| (1..=80).contains(&n.chars().count())).ok_or_else(|| bad("名字需要 1–80 个字"))?;
                json!({"name": name})
            }
            "reject" | "confirm" => json!({}),
            _ => return Err(bad("action 只能是 merge、split、rename、reject 或 confirm")),
        };
        let now = self.clock.now_ms();
        self.db.execute("INSERT INTO corrections VALUES(?,'entity',?,?,?,?)", &[text(uuid_v7(Some(now))), text(&key), text(&req.action), text(value.to_string()), int(now)])?;
        let r = self.people_recompute()?;
        let person = self.meta_value(&format!("{NODE}{key}"))?.filter(|v| v != "-");
        Ok(json!({"ok": true, "key": key, "person_id": person, "people": r["people"]}))
    }
}
