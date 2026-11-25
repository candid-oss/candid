//! Items (docs/plans/data-layer-v1.md §2.3–2.5): what the extraction model's operations do to `items`, the rules
//! around them, and how items are read. No model is called here.
//!
//! * **Time** is resolved against the sending time of the message an operation rests on. A time more than a day
//!   before it is dropped (an event without a time is then dropped altogether; a reminder stays undated).
//! * **Merging** (rules): the same counterpart with start times within 30 minutes, or without a counterpart the same
//!   start and a title trigram similarity of at least 0.5, is one item — a `merge` change adds the new source. A
//!   `create` that matches an item the user ignored in the last 30 days is dropped.
//! * **States**: a candidate (or confirmed item not yet in the calendar) follows the model directly. A confirmed item
//!   with an `external_ref` only gets a *proposal* (`update_calendar_event`, `delete_calendar_event`,
//!   `complete_reminder`, …) that waits for the user; the item changes when the App reports it executed.
//!   Fields in `user_fields` are never overwritten by the model. Events past their end are `expired` by rule,
//!   and so are candidates nobody looked at for 7 days.
//! * **Memos** are `memories(kind=fact, status=candidate)` sourced from the events behind the message; the person
//!   they belong to is the alias key kept in `memo.person/1:<memory_id>` (entity ids change on recompute, keys do not).

use crate::db::{int, opt_int, opt_text, placeholders, text, RowExt, P};
use crate::{uuid_v4, Result, TimelineError, TimelineStore};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MEMO_PERSON: &str = "memo.person/1:";
const DAY: i64 = 86_400_000;
const MERGE_WINDOW_MS: i64 = 30 * 60_000;
const SAME_TITLE: f64 = 0.5;
const IGNORED_DAYS: i64 = 30;
const CANDIDATE_DAYS: i64 = 7;
const DEFAULT_EVENT_MS: i64 = 3_600_000;
const FIELDS: [&str; 5] = ["title", "when_text", "start", "end", "all_day"];

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

/// The other side of a conversation: one alias key (stable) and the person it belongs to now.
#[derive(Clone, Debug)]
pub(crate) struct Counterpart {
    pub key: String,
    pub name: String,
}

/// What an operation is applied against.
pub(crate) struct Apply<'a> {
    pub conversation: &'a str,
    pub counterpart: Option<&'a Counterpart>,
    /// Message number in the model input → (message id, sent_ms).
    pub refs: &'a BTreeMap<i64, (String, i64)>,
    pub model: &'a str,
    pub prompt_version: &'a str,
}

#[derive(Clone, Debug)]
pub(crate) struct ItemRow {
    pub item_id: String,
    pub kind: String,
    pub owner: String,
    pub title: String,
    pub when_text: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub all_day: bool,
    pub counterpart: Option<String>,
    pub conversation_id: Option<String>,
    pub review: String,
    pub state: String,
    pub user_fields: Vec<String>,
    pub external_ref: Option<String>,
    pub updated_ms: i64,
}

pub(crate) const ITEM_COLUMNS: &str = "item_id,kind,owner,title,when_text,start_ms,end_ms,all_day,counterpart,conversation_id,review,state,user_fields,external_ref,updated_ms";

pub(crate) fn item_from(r: &rusqlite::Row) -> ItemRow {
    ItemRow {
        item_id: r.text(0),
        kind: r.text(1),
        owner: r.text(2),
        title: r.text(3),
        when_text: r.opt_text(4),
        start_ms: r.opt_int(5),
        end_ms: r.opt_int(6),
        all_day: r.int(7) != 0,
        counterpart: r.opt_text(8),
        conversation_id: r.opt_text(9),
        review: r.text(10),
        state: r.text(11),
        user_fields: serde_json::from_str(&r.text(12)).unwrap_or_default(),
        external_ref: r.opt_text(13),
        updated_ms: r.int(14),
    }
}

fn clean(v: &Value, max: usize) -> Option<String> {
    let s: String = v.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then(|| s.chars().take(max).collect())
}

/// Dice similarity of character trigrams (bigrams for very short strings), 0–1.
pub(crate) fn trigram_similarity(a: &str, b: &str) -> f64 {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<Vec<_>>();
    let (a, b) = (norm(a), norm(b));
    if a == b {
        return 1.0;
    }
    let n = if a.len().min(b.len()) >= 3 { 3 } else { 2 };
    if a.len() < n || b.len() < n {
        return 0.0;
    }
    let grams = |v: &[char]| v.windows(n).map(|w| w.iter().collect::<String>()).collect::<Vec<_>>();
    let (ga, mut gb) = (grams(&a), grams(&b));
    let total = ga.len() + gb.len();
    let mut hits = 0;
    for g in ga {
        if let Some(i) = gb.iter().position(|x| *x == g) {
            gb.swap_remove(i);
            hits += 1;
        }
    }
    2.0 * hits as f64 / total as f64
}

fn alias_key(kind: &str, alias: &str) -> String {
    if kind == "other" { alias.to_string() } else { format!("{kind}:{alias}") }
}

impl TimelineStore {
    // MARK: counterpart

    /// The single other person of a conversation, when messages carry exactly one `sender_entity_id` besides yours.
    pub(crate) fn conversation_counterpart(&self, conversation: &str) -> Result<Option<Counterpart>> {
        let ids = self.db.query(
            "SELECT DISTINCT sender_entity_id FROM messages WHERE conversation_id=? AND actor='other' AND sender_entity_id IS NOT NULL",
            &[text(conversation)],
            |r| r.text(0),
        )?;
        let [entity] = ids.as_slice() else { return Ok(None) };
        let aliases = self.db.query(
            "SELECT alias_kind,alias FROM entity_aliases WHERE entity_id=? ORDER BY CASE alias_kind WHEN 'handle' THEN 0 WHEN 'email' THEN 1 WHEN 'name' THEN 2 WHEN 'voice' THEN 3 ELSE 4 END, alias",
            &[text(entity)],
            |r| (r.text(0), r.text(1)),
        )?;
        let (Some((kind, alias)), Some(name)) = (aliases.first(), self.db.first("SELECT name FROM entities WHERE entity_id=?", &[text(entity)], |r| r.text(0))?) else { return Ok(None) };
        Ok(Some(Counterpart { key: alias_key(kind, alias), name }))
    }

    /// Every alias key of the person `key` belongs to (people merged into them included).
    pub(crate) fn counterpart_keys(&self, key: &str) -> Result<Vec<String>> {
        let (kind, alias) = key.split_once(':').filter(|(k, _)| ["handle", "email", "name", "voice", "domain", "bundle"].contains(k)).unwrap_or(("other", key));
        let entities = self.db.query("SELECT DISTINCT entity_id FROM entity_aliases WHERE alias_kind=? AND alias=?", &[text(kind), text(alias)], |r| r.text(0))?;
        let mut keys = BTreeSet::from([key.to_string()]);
        if !entities.is_empty() {
            keys.extend(self.entity_keys(&entities)?);
        }
        Ok(keys.into_iter().collect())
    }

    /// All alias keys of the given people and everyone merged into them.
    pub(crate) fn entity_keys(&self, entities: &[String]) -> Result<Vec<String>> {
        let family = self.people_family(entities)?;
        if family.is_empty() {
            return Ok(vec![]);
        }
        let rows = self.db.query(
            &format!("SELECT alias_kind,alias FROM entity_aliases WHERE entity_id IN ({})", placeholders(family.len())),
            &family.iter().map(text).collect::<Vec<P>>(),
            |r| alias_key(&r.text(0), &r.text(1)),
        )?;
        Ok(rows)
    }

    fn person_name_of_key(&self, key: &str) -> Option<(String, String)> {
        let (kind, alias) = key.split_once(':').filter(|(k, _)| ["handle", "email", "name", "voice", "domain", "bundle"].contains(k)).unwrap_or(("other", key));
        self.db
            .first(
                "SELECT e.entity_id,e.name FROM entity_aliases a JOIN entities e ON e.entity_id=a.entity_id WHERE a.alias_kind=? AND a.alias=? AND e.status IN ('candidate','confirmed') ORDER BY e.status DESC LIMIT 1",
                &[text(kind), text(alias)],
                |r| (r.text(0), r.text(1)),
            )
            .ok()
            .flatten()
    }

    // MARK: rows

    pub(crate) fn item_row(&self, id: &str) -> Result<Option<ItemRow>> {
        self.db.first(&format!("SELECT {ITEM_COLUMNS} FROM items WHERE item_id=?"), &[text(id)], item_from)
    }

    fn add_change(&self, item: &str, op: &str, fields: &Value, by: &str, basis: &[(String, i64)]) -> Result<String> {
        let id = format!("ic-{}", &uuid_v4().replace('-', "")[..16]);
        self.db.execute(
            "INSERT INTO item_changes(change_id,item_id,op,fields_json,\"by\",created_ms) VALUES(?,?,?,?,?,?)",
            &[text(&id), text(item), text(op), text(fields.to_string()), text(by), int(self.clock.now_ms())],
        )?;
        for (message, _) in basis {
            self.db.execute("INSERT OR IGNORE INTO item_sources(change_id,message_id) VALUES(?,?)", &[text(&id), text(message)])?;
        }
        Ok(id)
    }

    fn source_events(&self, basis: &[(String, i64)]) -> Result<Vec<String>> {
        let mut out = BTreeSet::new();
        for (m, _) in basis {
            out.extend(self.db.query("SELECT event_id FROM message_sources WHERE message_id=?", &[text(m)], |r| r.text(0))?);
        }
        Ok(out.into_iter().collect())
    }

    // MARK: time

    /// A model time (`2026-11-24T15:00:00+08:00`, the same without offset or seconds, or a bare date) as
    /// (ms, date-only). `None` when it is unreadable or more than a day before `basis_ms`.
    fn resolve_time(&self, s: &str, basis_ms: i64) -> Option<(i64, bool)> {
        let s = s.trim();
        if !s.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let (ms, date_only) = if s.len() == 10 {
            (self.clock.parse(s, false).ok()?, true)
        } else {
            let mut t = s.replace(' ', "T");
            let has_offset = t.ends_with('Z') || t.rfind(['+', '-']).is_some_and(|i| i > 10);
            let time_part = t.split_once('T').map(|x| x.1.to_string()).unwrap_or_default();
            if time_part.matches(':').count() == 1 && !has_offset {
                t += ":00";
            } else if time_part.matches(':').count() == 1 {
                let i = t.rfind(['+', '-', 'Z']).filter(|i| *i > 10)?;
                t.insert_str(i, ":00");
            }
            if !has_offset {
                let iso = self.clock.iso(basis_ms);
                t += &iso[iso.len().saturating_sub(if iso.ends_with('Z') { 1 } else { 6 })..];
            }
            (self.clock.parse(&t, false).ok()?, false)
        };
        (ms >= basis_ms - DAY && ms <= basis_ms + 5 * 365 * DAY).then_some((ms, date_only))
    }

    // MARK: apply

    /// Applies the operations of one model reply. Each is checked on its own: one that is malformed, cites a message
    /// that was not in the input, or breaks a rule is dropped with a reason and the rest go on.
    pub(crate) fn items_apply(&self, a: &Apply, out: &Value) -> Result<Value> {
        let ops = out["ops"].as_array().or_else(|| out.as_array()).cloned().unwrap_or_default();
        let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
        let mut dropped = vec![];
        for op in ops.iter().take(30) {
            match self.apply_one(a, op) {
                Ok(outcome) => *counts.entry(outcome).or_default() += 1,
                Err(TimelineError::InvalidInput(reason)) => dropped.push(json!({"op": op["op"], "reason": reason})),
                Err(e) => return Err(e),
            }
        }
        Ok(json!({"applied": counts, "dropped": dropped}))
    }

    fn basis(&self, a: &Apply, op: &Value) -> Result<Vec<(String, i64)>> {
        let list = op["basis"].as_array().filter(|l| !l.is_empty()).ok_or_else(|| bad("no_basis"))?;
        let mut out = vec![];
        for v in list {
            let n = v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().trim_matches(|c| c == '[' || c == ']').parse().ok())).ok_or_else(|| bad("basis_out_of_range"))?;
            let m = a.refs.get(&n).ok_or_else(|| bad("basis_out_of_range"))?;
            if !out.contains(m) {
                out.push(m.clone());
            }
        }
        Ok(out)
    }

    fn apply_one(&self, a: &Apply, op: &Value) -> Result<&'static str> {
        let kind = op["op"].as_str().unwrap_or("");
        if !["create", "update", "done", "cancel", "memo"].contains(&kind) {
            return Err(bad("unknown_op"));
        }
        let basis = self.basis(a, op)?;
        let basis_ms = basis.iter().map(|b| b.1).max().unwrap_or(0);
        match kind {
            "create" => self.op_create(a, op, &basis, basis_ms),
            "memo" => self.op_memo(a, op, &basis, basis_ms),
            _ => {
                let id = op["item_id"].as_str().ok_or_else(|| bad("no_item_id"))?;
                let row = self.item_row(id)?.ok_or_else(|| bad("unknown_item"))?;
                if row.review == "rejected" || row.state != "open" {
                    return Err(bad("item_not_open"));
                }
                if !self.item_in_scope(a, &row)? {
                    return Err(bad("item_out_of_scope"));
                }
                match kind {
                    "update" => self.op_update(a, op, &row, &basis, basis_ms),
                    _ => self.op_end(a, kind, &row, &basis),
                }
            }
        }
    }

    /// The model may only touch items it was shown: those of this conversation or of this person.
    fn item_in_scope(&self, a: &Apply, row: &ItemRow) -> Result<bool> {
        if row.conversation_id.as_deref() == Some(a.conversation) {
            return Ok(true);
        }
        match (a.counterpart, &row.counterpart) {
            (Some(c), Some(k)) => Ok(self.counterpart_keys(&c.key)?.contains(k)),
            _ => Ok(false),
        }
    }

    /// An existing item the new one is the same as: `Some((item_id, ignored))`. Active items win over ignored ones.
    fn find_same(&self, a: &Apply, title: &str, start: Option<i64>) -> Result<Option<(String, bool)>> {
        let now = self.clock.now_ms();
        let (clause, mut params) = match a.counterpart {
            Some(c) => {
                let keys = self.counterpart_keys(&c.key)?;
                (format!("counterpart IN ({})", placeholders(keys.len())), keys.iter().map(text).collect::<Vec<P>>())
            }
            None => ("counterpart IS NULL".to_string(), vec![]),
        };
        params.push(int(now));
        let rows = self.db.query(
            &format!("SELECT item_id,title,start_ms,review FROM items WHERE {clause} AND NOT (review='rejected' AND updated_ms<?-{}) ORDER BY updated_ms DESC LIMIT 300", IGNORED_DAYS * DAY),
            &params,
            |r| (r.text(0), r.text(1), r.opt_int(2), r.text(3)),
        )?;
        let (mut active, mut ignored) = (None, None);
        for (id, other, other_start, review) in rows {
            let similar = trigram_similarity(title, &other) >= SAME_TITLE;
            let same = match (a.counterpart.is_some(), start, other_start) {
                (true, Some(s), Some(t)) => (s - t).abs() <= MERGE_WINDOW_MS,
                (false, Some(s), Some(t)) => s == t && similar,
                (_, None, None) => similar,
                _ => false,
            };
            if same {
                if review == "rejected" {
                    ignored.get_or_insert(id);
                } else {
                    active.get_or_insert(id);
                }
            }
        }
        Ok(active.map(|id| (id, false)).or_else(|| ignored.map(|id| (id, true))))
    }

    fn op_create(&self, a: &Apply, op: &Value, basis: &[(String, i64)], basis_ms: i64) -> Result<&'static str> {
        let kind = op["kind"].as_str().filter(|k| ["event", "reminder", "commitment"].contains(k)).ok_or_else(|| bad("bad_kind"))?;
        let owner = match op["owner"].as_str() {
            Some("other") => "other",
            _ => "self",
        };
        let title = clean(&op["title"], 80).ok_or_else(|| bad("no_title"))?;
        let when_text = clean(&op["when_text"], 60);
        let start = op["start"].as_str().and_then(|s| self.resolve_time(s, basis_ms));
        let end = op["end"].as_str().and_then(|s| self.resolve_time(s, basis_ms)).map(|e| e.0).filter(|e| start.is_none_or(|s| *e >= s.0));
        if kind == "event" && start.is_none() {
            return Err(bad("event_without_time"));
        }
        let all_day = op["all_day"] == true || start.is_some_and(|s| s.1);
        let start_ms = start.map(|s| s.0);
        match self.find_same(a, &title, start_ms)? {
            Some((_, true)) => Err(bad("matches_ignored_item")),
            Some((id, false)) => {
                let row = self.item_row(&id)?.ok_or_else(|| bad("unknown_item"))?;
                // A merge only adds the source, and fills what the item lacks (never a field the user set).
                let mut fill = serde_json::Map::new();
                if row.end_ms.is_none() && end.is_some() && !row.user_fields.contains(&"end".into()) {
                    self.db.execute("UPDATE items SET end_ms=? WHERE item_id=?", &[opt_int(end), text(&id)])?;
                    fill.insert("end".into(), json!(end.map(|e| self.clock.iso(e))));
                }
                if row.when_text.is_none() && when_text.is_some() {
                    self.db.execute("UPDATE items SET when_text=? WHERE item_id=?", &[opt_text(when_text.clone()), text(&id)])?;
                    fill.insert("when_text".into(), json!(when_text));
                }
                self.db.execute("UPDATE items SET updated_ms=? WHERE item_id=?", &[int(self.clock.now_ms()), text(&id)])?;
                self.add_change(&id, "merge", &json!({"title": title, "fill": fill}), "model", basis)?;
                Ok("merged")
            }
            None => {
                let id = format!("it-{}", &uuid_v4().replace('-', "")[..10]);
                let now = self.clock.now_ms();
                self.db.execute(
                    "INSERT INTO items(item_id,kind,owner,title,when_text,start_ms,end_ms,all_day,counterpart,conversation_id,review,state,user_fields,external_ref,producer,model_ref,prompt_version,created_ms,updated_ms) \
                     VALUES(?,?,?,?,?,?,?,?,?,?,'candidate','open','[]',NULL,?,?,?,?,?)",
                    &[
                        text(&id), text(kind), text(owner), text(&title), opt_text(when_text.clone()), opt_int(start_ms), opt_int(end), int(all_day as i64),
                        opt_text(a.counterpart.map(|c| c.key.clone())), text(a.conversation),
                        text(format!("{}:{}", a.prompt_version, a.model)), text(a.model), text(a.prompt_version), int(now), int(now),
                    ],
                )?;
                let fields = json!({"kind": kind, "owner": owner, "title": title, "when_text": when_text, "start": start_ms.map(|t| self.clock.iso(t)), "end": end.map(|t| self.clock.iso(t)), "all_day": all_day});
                self.add_change(&id, "create", &fields, "model", basis)?;
                Ok("created")
            }
        }
    }

    /// Fields of an `update` the model may set: present, valid and not edited by the user. (name, new value as JSON)
    fn update_fields(&self, op: &Value, row: &ItemRow, basis_ms: i64) -> Vec<(&'static str, Value, Option<i64>)> {
        let mut out = vec![];
        for f in FIELDS {
            if row.user_fields.iter().any(|u| u == f) || op.get(f).is_none() || op[f].is_null() {
                continue;
            }
            match f {
                "title" => {
                    if let Some(t) = clean(&op[f], 80).filter(|t| *t != row.title) {
                        out.push((f, json!(t), None));
                    }
                }
                "when_text" => {
                    if let Some(t) = clean(&op[f], 60).filter(|t| Some(t) != row.when_text.as_ref()) {
                        out.push((f, json!(t), None));
                    }
                }
                "start" | "end" => {
                    if let Some((ms, date_only)) = op[f].as_str().and_then(|s| self.resolve_time(s, basis_ms)) {
                        let current = if f == "start" { row.start_ms } else { row.end_ms };
                        if current != Some(ms) {
                            out.push((f, json!(self.clock.iso(ms)), Some(ms)));
                            if date_only && f == "start" && !row.all_day {
                                out.push(("all_day", json!(true), None));
                            }
                        }
                    }
                }
                _ => {
                    if let Some(b) = op[f].as_bool().filter(|b| *b != row.all_day) {
                        out.push((f, json!(b), None));
                    }
                }
            }
        }
        out
    }

    fn set_fields(&self, id: &str, fields: &[(&'static str, Value, Option<i64>)]) -> Result<()> {
        for (f, v, ms) in fields {
            let (column, value) = match *f {
                "start" => ("start_ms", opt_int(*ms)),
                "end" => ("end_ms", opt_int(*ms)),
                "all_day" => ("all_day", int(v.as_bool().unwrap_or(false) as i64)),
                other => (other, text(v.as_str().unwrap_or(""))),
            };
            self.db.execute(&format!("UPDATE items SET {column}=? WHERE item_id=?"), &[value, text(id)])?;
        }
        self.db.execute("UPDATE items SET updated_ms=? WHERE item_id=?", &[int(self.clock.now_ms()), text(id)])?;
        Ok(())
    }

    /// Confirmed and already in the calendar or Reminders: only the user's approval may change it.
    fn needs_approval(row: &ItemRow) -> bool {
        row.review == "confirmed" && row.external_ref.is_some()
    }

    fn op_update(&self, a: &Apply, op: &Value, row: &ItemRow, basis: &[(String, i64)], basis_ms: i64) -> Result<&'static str> {
        let fields = self.update_fields(op, row, basis_ms);
        if fields.is_empty() {
            return Ok("unchanged");
        }
        if Self::needs_approval(row) {
            let mut payload = json!({"item_id": row.external_ref, "candid_item": row.item_id});
            let mut set = serde_json::Map::new();
            for (f, v, ms) in &fields {
                set.insert((*f).into(), v.clone());
                if let Some(ms) = ms {
                    set.insert(format!("{f}_ms"), json!(ms));
                }
                match (*f, row.kind.as_str()) {
                    ("title", _) => payload["title"] = v.clone(),
                    ("start", "event") => payload["start"] = v.clone(),
                    ("end", "event") => payload["end"] = v.clone(),
                    ("start", _) => payload["due"] = v.clone(),
                    _ => {}
                }
            }
            payload["item_set"] = json!(set);
            let kind = if row.kind == "event" { "update_calendar_event" } else { "update_reminder" };
            self.item_proposal(a, row, kind, payload, basis)?;
            return Ok("proposed");
        }
        self.set_fields(&row.item_id, &fields)?;
        let changed: serde_json::Map<String, Value> = fields.iter().map(|(f, v, _)| ((*f).to_string(), v.clone())).collect();
        self.add_change(&row.item_id, "update", &Value::Object(changed), "model", basis)?;
        Ok("updated")
    }

    fn op_end(&self, a: &Apply, op: &str, row: &ItemRow, basis: &[(String, i64)]) -> Result<&'static str> {
        if Self::needs_approval(row) {
            let (kind, outcome) = match (op, row.kind.as_str()) {
                ("done", "event") => return Err(bad("event_done_ignored")),
                ("done", _) => ("complete_reminder", "proposed"),
                (_, "event") => ("delete_calendar_event", "proposed"),
                _ => ("delete_reminder", "proposed"),
            };
            self.item_proposal(a, row, kind, json!({"item_id": row.external_ref, "candid_item": row.item_id, "title": row.title}), basis)?;
            return Ok(outcome);
        }
        let state = if op == "done" { "done" } else { "cancelled" };
        self.db.execute("UPDATE items SET state=?,updated_ms=? WHERE item_id=?", &[text(state), int(self.clock.now_ms()), text(&row.item_id)])?;
        self.add_change(&row.item_id, op, &json!({}), "model", basis)?;
        Ok(if op == "done" { "done" } else { "cancelled" })
    }

    /// Files (or refreshes) the pending proposal of this kind for an item.
    fn item_proposal(&self, a: &Apply, row: &ItemRow, kind: &str, payload: Value, basis: &[(String, i64)]) -> Result<()> {
        let client = self.internal_client("item-extract", &crate::ClientScope::unbounded())?;
        let evidence = crate::json::canonical_of(&self.source_events(basis)?);
        let mut payload = payload;
        payload["notes"] = json!(a.counterpart.map(|c| format!("来自与 {} 的对话", c.name)).unwrap_or_else(|| "来自对话".into()));
        let existing = self.db.first(
            "SELECT proposal_id FROM proposals WHERE kind=? AND status='pending' AND json_extract(payload_json,'$.candid_item')=? LIMIT 1",
            &[text(kind), text(&row.item_id)],
            |r| r.text(0),
        )?;
        match existing {
            Some(id) => {
                self.db.execute("UPDATE proposals SET payload_json=?,evidence_json=?,created_ms=? WHERE proposal_id=?", &[text(crate::json::canonical(&payload)), text(evidence), int(self.clock.now_ms()), text(&id)])?;
                self.record_change("proposal", &id, "update")
            }
            None => {
                let id = crate::uuid_v7(Some(self.clock.now_ms()));
                self.db.execute(
                    "INSERT INTO proposals(proposal_id,client_id,kind,payload_json,evidence_json,status,created_ms) VALUES(?,?,?,?,?,'pending',?)",
                    &[text(&id), text(client), text(kind), text(crate::json::canonical(&payload)), text(evidence), int(self.clock.now_ms())],
                )?;
                self.record_change("proposal", &id, "insert")
            }
        }
    }

    fn op_memo(&self, a: &Apply, op: &Value, basis: &[(String, i64)], basis_ms: i64) -> Result<&'static str> {
        let body = clean(&op["text"], 160).ok_or_else(|| bad("no_text"))?;
        let key = a.counterpart.map(|c| c.key.clone()).unwrap_or_default();
        let old = self.db.query(
            "SELECT m.text FROM memories m JOIN store_meta s ON s.key=?||m.memory_id WHERE m.kind='fact' AND m.status IN ('candidate','confirmed','edited') AND m.rev=(SELECT MAX(rev) FROM memories x WHERE x.memory_id=m.memory_id) AND json_extract(s.value,'$.key')=?",
            &[text(MEMO_PERSON), text(&key)],
            |r| r.text(0),
        )?;
        if old.iter().any(|t| trigram_similarity(t, &body) >= 0.8) {
            return Err(bad("duplicate_memo"));
        }
        let events = self.source_events(basis)?;
        if events.is_empty() {
            return Err(bad("no_source_events"));
        }
        let id = format!("mem-{}", &uuid_v4().replace('-', "")[..12]);
        self.db.execute(
            "INSERT INTO memories(memory_id,rev,kind,text,valid_from_ms,valid_to_ms,status,producer,model_ref,prompt_version,confidence,created_ms) VALUES(?,1,'fact',?,?,NULL,'candidate',?,?,?,NULL,?)",
            &[text(&id), text(&body), int(basis_ms), text(format!("{}:{}", a.prompt_version, a.model)), text(a.model), text(a.prompt_version), int(self.clock.now_ms())],
        )?;
        for e in events {
            self.db.execute("INSERT INTO memory_sources(memory_id,rev,ref_type,ref_id,ref_rev,selector_json) VALUES(?,1,'event',?,NULL,NULL)", &[text(&id), text(e)])?;
        }
        self.automation_put(&format!("{MEMO_PERSON}{id}"), &json!({"key": key, "conversation": a.conversation}))?;
        self.record_change("memory", &id, "insert")?;
        Ok("memo")
    }

    // MARK: rules

    /// When [`TimelineStore::items_maintain`] next has something to expire (an event ending, a candidate turning a
    /// week old), so the background worker can sleep until then.
    pub(crate) fn items_next_expiry(&self) -> Result<Option<i64>> {
        Ok(self.db.first(
            "SELECT MIN(t) FROM (SELECT COALESCE(end_ms,start_ms+?) AS t FROM items WHERE state='open' AND kind='event' AND start_ms IS NOT NULL \
             UNION ALL SELECT created_ms+? FROM items WHERE state='open' AND review='candidate')",
            &[int(DEFAULT_EVENT_MS), int(CANDIDATE_DAYS * DAY)],
            |r| r.opt_int(0),
        )?.flatten().map(|t| t + 1))
    }

    /// Events past their end become `expired`; candidates nobody handled for a week are put away (`expired`, and
    /// still listed with `state=expired`). Returns how many items changed.
    pub fn items_maintain(&self) -> Result<i64> {
        let now = self.clock.now_ms();
        let rows = self.db.query(
            "SELECT item_id,CASE WHEN kind='event' AND start_ms IS NOT NULL AND COALESCE(end_ms,start_ms+?)<? THEN 'ended' ELSE 'stale_candidate' END FROM items \
             WHERE state='open' AND ((kind='event' AND start_ms IS NOT NULL AND COALESCE(end_ms,start_ms+?)<?) OR (review='candidate' AND created_ms<?))",
            &[int(DEFAULT_EVENT_MS), int(now), int(DEFAULT_EVENT_MS), int(now), int(now - CANDIDATE_DAYS * DAY)],
            |r| (r.text(0), r.text(1)),
        )?;
        if rows.is_empty() {
            return Ok(0);
        }
        self.db.transaction(|| {
            for (id, why) in &rows {
                self.db.execute("UPDATE items SET state='expired',updated_ms=? WHERE item_id=?", &[int(now), text(id)])?;
                self.add_change(id, "expire", &json!({"reason": why}), "rule", &[])?;
            }
            Ok(())
        })?;
        Ok(rows.len() as i64)
    }

    /// The App reported an approved proposal as executed: remember where the item now lives, or apply what it did.
    pub(crate) fn items_proposal_executed(&self, kind: &str, payload: &Value, receipt: &Value) -> Result<()> {
        let Some(id) = payload["candid_item"].as_str() else { return Ok(()) };
        let Some(row) = self.item_row(id)? else { return Ok(()) };
        let now = self.clock.now_ms();
        match kind {
            "create_calendar_event" | "create_reminder" => {
                let reference = receipt["item_id"].as_str().map(String::from);
                self.db.execute("UPDATE items SET external_ref=?,review='confirmed',updated_ms=? WHERE item_id=?", &[opt_text(reference.clone()), int(now), text(id)])?;
                self.add_change(id, "user", &json!({"external_ref": reference, "review": "confirmed"}), "calendar", &[])?;
            }
            "update_calendar_event" | "update_reminder" => {
                let set = &payload["item_set"];
                let mut fields = vec![];
                for f in FIELDS {
                    if set[f].is_null() {
                        continue;
                    }
                    fields.push((f, set[f].clone(), set[format!("{f}_ms")].as_i64()));
                }
                self.set_fields(id, &fields)?;
                if let Some(reference) = receipt["item_id"].as_str() {
                    self.db.execute("UPDATE items SET external_ref=? WHERE item_id=?", &[text(reference), text(id)])?;
                }
                self.add_change(id, "update", set, "calendar", &[])?;
            }
            "complete_reminder" | "delete_reminder" | "delete_calendar_event" => {
                let (state, op) = if kind == "complete_reminder" { ("done", "done") } else { ("cancelled", "cancel") };
                self.db.execute("UPDATE items SET state=?,updated_ms=? WHERE item_id=?", &[text(state), int(now), text(id)])?;
                self.add_change(id, op, &json!({}), "calendar", &[])?;
            }
            _ => {}
        }
        let _ = row;
        Ok(())
    }

    // MARK: reading

    /// Candidates the user has not looked at yet (the 「待你处理」 list), soonest first.
    pub(crate) fn candidate_items(&self, limit: i64) -> Result<Vec<Value>> {
        let rows = self.db.query(
            &format!("SELECT {ITEM_COLUMNS} FROM items WHERE state='open' AND review='candidate' ORDER BY start_ms IS NULL, start_ms, created_ms DESC LIMIT ?"),
            &[int(limit)],
            item_from,
        )?;
        rows.iter().map(|r| self.item_json(r)).collect()
    }

    /// Memos waiting for the user's decision, newest first, with the person they are about.
    pub(crate) fn pending_memos(&self, limit: i64) -> Result<Vec<Value>> {
        let rows = self.db.query(
            "SELECT m.memory_id,m.text,m.valid_from_ms,m.created_ms,s.value FROM memories m JOIN store_meta s ON s.key=?||m.memory_id WHERE m.kind='fact' AND m.status='candidate' \
             AND m.rev=(SELECT MAX(rev) FROM memories x WHERE x.memory_id=m.memory_id) ORDER BY m.created_ms DESC LIMIT ?",
            &[text(MEMO_PERSON), int(limit)],
            |r| (r.text(0), r.text(1), r.opt_int(2), r.int(3), r.text(4)),
        )?;
        Ok(rows
            .into_iter()
            .map(|(id, body, at, created, meta)| {
                let meta: Value = serde_json::from_str(&meta).unwrap_or(Value::Null);
                let key = meta["key"].as_str().filter(|k| !k.is_empty());
                let person = key.map(|k| self.person_name_of_key(k).map(|(pid, name)| json!({"person_id": pid, "name": name, "key": k})).unwrap_or_else(|| json!({"key": k})));
                json!({"memory_id": id, "text": body, "person": person, "time": at.map(|t| self.clock.iso(t)), "created": self.clock.iso(created)})
            })
            .collect())
    }

    /// The user's decision on an item from the 「待你处理」 list. `confirm` files the calendar or Reminders proposal
    /// (once); `ignore` hides the item for good and makes similar ones be dropped; `done` closes it; `edit` sets
    /// fields by hand, which the model never overwrites again.
    pub(crate) fn item_decide(&self, v: &Value, app_client: &str) -> Result<Value> {
        let id = v["item_id"].as_str().ok_or_else(|| bad("缺少 item_id"))?;
        let row = self.item_row(id)?.ok_or_else(|| TimelineError::NotFound("条目".into()))?;
        let decision = v["decision"].as_str().unwrap_or("");
        let now = self.clock.now_ms();
        let proposal_kind = if row.kind == "event" { "create_calendar_event" } else { "create_reminder" };
        let existing = || -> Result<Option<String>> {
            self.db.first("SELECT proposal_id FROM proposals WHERE kind=? AND json_extract(payload_json,'$.candid_item')=? AND status IN ('pending','approved','executed') ORDER BY created_ms LIMIT 1", &[text(proposal_kind), text(id)], |r| r.text(0))
        };
        let mut proposal = None;
        self.db.transaction(|| {
            match decision {
                "confirm" => {
                    if row.state != "open" || row.review == "rejected" {
                        return Err(bad("条目已经处理过"));
                    }
                    if row.kind == "event" && row.start_ms.is_none() {
                        return Err(bad("日程缺少开始时间"));
                    }
                    if let Some(p) = existing()? {
                        proposal = Some(p);
                        return Ok(());
                    }
                    let mut payload = json!({"title": row.title, "candid_item": id});
                    let notes = row.counterpart.as_deref().and_then(|k| self.person_name_of_key(k)).map(|(_, n)| if row.owner == "other" { format!("等 {n} 完成") } else { format!("与 {n} 的约定") });
                    payload["notes"] = json!(notes.unwrap_or_else(|| "来自对话".into()));
                    if row.kind == "event" {
                        payload["start"] = json!(row.start_ms.map(|t| self.clock.iso(t)));
                        if let Some(e) = row.end_ms {
                            payload["end"] = json!(self.clock.iso(e));
                        }
                    } else if let Some(t) = row.start_ms {
                        payload["due"] = json!(self.clock.iso(t));
                    }
                    let evidence = self.db.query("SELECT DISTINCT ms.event_id FROM item_changes c JOIN item_sources s ON s.change_id=c.change_id JOIN message_sources ms ON ms.message_id=s.message_id WHERE c.item_id=? ORDER BY ms.event_id", &[text(id)], |r| r.text(0))?;
                    let pid = crate::uuid_v7(Some(now));
                    self.db.execute(
                        "INSERT INTO proposals(proposal_id,client_id,kind,payload_json,evidence_json,status,created_ms) VALUES(?,?,?,?,?,'pending',?)",
                        &[text(&pid), text(app_client), text(proposal_kind), text(crate::json::canonical(&payload)), text(crate::json::canonical_of(&evidence)), int(now)],
                    )?;
                    self.record_change("proposal", &pid, "insert")?;
                    self.db.execute("UPDATE items SET review='confirmed',updated_ms=? WHERE item_id=?", &[int(now), text(id)])?;
                    self.add_change(id, "user", &json!({"review": "confirmed"}), "user", &[])?;
                    proposal = Some(pid);
                }
                "ignore" => {
                    if row.review != "rejected" {
                        self.db.execute("UPDATE items SET review='rejected',state='cancelled',updated_ms=? WHERE item_id=?", &[int(now), text(id)])?;
                        self.add_change(id, "user", &json!({"review": "rejected"}), "user", &[])?;
                    }
                }
                "done" => {
                    if row.state == "open" {
                        self.db.execute("UPDATE items SET state='done',updated_ms=? WHERE item_id=?", &[int(now), text(id)])?;
                        self.add_change(id, "done", &json!({}), "user", &[])?;
                    }
                }
                "edit" => {
                    let mut fields = vec![];
                    let mut touched: Vec<String> = row.user_fields.clone();
                    for f in FIELDS {
                        if v[f].is_null() {
                            continue;
                        }
                        let field = match f {
                            "start" | "end" => {
                                let ms = v[f].as_str().map(|s| self.clock.parse(s, false)).transpose()?;
                                (f, json!(ms.map(|m| self.clock.iso(m))), ms)
                            }
                            _ => (f, v[f].clone(), None),
                        };
                        if !touched.iter().any(|t| t == f) {
                            touched.push(f.to_string());
                        }
                        fields.push(field);
                    }
                    self.set_fields(id, &fields)?;
                    self.db.execute("UPDATE items SET user_fields=? WHERE item_id=?", &[text(json!(touched).to_string()), text(id)])?;
                    let changed: serde_json::Map<String, Value> = fields.iter().map(|(f, v, _)| ((*f).to_string(), v.clone())).collect();
                    self.add_change(id, "user", &Value::Object(changed), "user", &[])?;
                }
                _ => return Err(bad("decision 只能是 confirm、ignore、done、edit")),
            }
            Ok(())
        })?;
        let item = self.item_row(id)?.map(|r| self.item_json(&r)).transpose()?;
        Ok(json!({"item": item, "proposal_id": proposal}))
    }


    pub(crate) fn item_json(&self, row: &ItemRow) -> Result<Value> {
        let person = row.counterpart.as_deref().and_then(|k| self.person_name_of_key(k).map(|(id, name)| json!({"person_id": id, "name": name, "key": k})).or_else(|| Some(json!({"key": k}))));
        let conversation = match &row.conversation_id {
            Some(c) => self.db.first("SELECT conversation_id,kind,app,title FROM conversations WHERE conversation_id=?", &[text(c)], |r| json!({"conversation_id": r.text(0), "kind": r.text(1), "app": r.opt_text(2), "title": r.opt_text(3)}))?,
            None => None,
        };
        let sources = self.db.query(
            "SELECT DISTINCT s.message_id FROM item_changes c JOIN item_sources s ON s.change_id=c.change_id WHERE c.item_id=? ORDER BY c.created_ms LIMIT 10",
            &[text(&row.item_id)],
            |r| json!({"message_id": r.text(0), "ref": Value::Null}),
        )?;
        let sources: Vec<Value> = sources
            .into_iter()
            .map(|mut v| {
                let cite = self.db.first("SELECT MIN(event_id) FROM message_sources WHERE message_id=?", &[text(v["message_id"].as_str().unwrap_or(""))], |r| r.opt_text(0)).ok().flatten().flatten();
                v["ref"] = json!(cite);
                v
            })
            .collect();
        let (producer, created) = self.db.first("SELECT producer,created_ms FROM items WHERE item_id=?", &[text(&row.item_id)], |r| (r.text(0), r.int(1)))?.unwrap_or_default();
        Ok(json!({
            "item_id": row.item_id, "kind": row.kind, "owner": row.owner, "title": row.title, "when_text": row.when_text,
            "start": row.start_ms.map(|t| self.clock.iso(t)), "end": row.end_ms.map(|t| self.clock.iso(t)), "all_day": row.all_day,
            "person": person, "conversation": conversation, "review": row.review, "state": row.state,
            "in_calendar": row.external_ref.is_some(), "external_ref": row.external_ref, "user_fields": row.user_fields,
            "sources": sources, "producer": producer, "created": self.clock.iso(created), "updated": self.clock.iso(row.updated_ms),
        }))
    }

    /// Open items (not ignored, not finished) of the people with these alias keys.
    pub(crate) fn open_items_of_keys(&self, keys: &[String], limit: i64) -> Result<Vec<Value>> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let mut params: Vec<P> = keys.iter().map(text).collect();
        params.push(int(limit));
        let rows = self.db.query(
            &format!("SELECT {ITEM_COLUMNS} FROM items WHERE state='open' AND review<>'rejected' AND counterpart IN ({}) ORDER BY start_ms IS NULL, start_ms, created_ms DESC LIMIT ?", placeholders(keys.len())),
            &params,
            item_from,
        )?;
        rows.iter().map(|r| self.item_json(r)).collect()
    }

    /// Memos (fact memories made from conversations) of the people with these alias keys.
    pub(crate) fn memos_of_keys(&self, keys: &[String], limit: i64) -> Result<Vec<Value>> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let mut params: Vec<P> = vec![text(MEMO_PERSON)];
        params.extend(keys.iter().map(text));
        params.push(int(limit));
        self.db.query(
            &format!(
                "SELECT m.memory_id,m.text,m.status,m.valid_from_ms,m.created_ms FROM memories m JOIN store_meta s ON s.key=?||m.memory_id WHERE m.kind='fact' AND m.status IN ('candidate','confirmed','edited') \
                 AND m.rev=(SELECT MAX(rev) FROM memories x WHERE x.memory_id=m.memory_id) AND json_extract(s.value,'$.key') IN ({}) ORDER BY m.created_ms DESC LIMIT ?",
                placeholders(keys.len())
            ),
            &params,
            |r| json!({"memory_id": r.text(0), "text": r.text(1), "status": r.text(2), "time": r.opt_int(3).map(|t| self.clock.iso(t)), "created": self.clock.iso(r.int(4))}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigram_similarity_tells_same_from_different() {
        assert!(trigram_similarity("周四下午和张三开会", "周四和张三开会") >= 0.5);
        assert!(trigram_similarity("周四下午和张三开会", "给李四寄快递") < 0.5);
        assert_eq!(trigram_similarity("开会", "开会"), 1.0);
    }
}
