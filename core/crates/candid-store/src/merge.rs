//! Typing continuations of `screen.text` (provisional rule; thresholds in `store_meta.text_merge`).
//!
//! A batch that asks for it (`IngestBatch.merge_text`) folds a new text sample into the latest `screen.text` event of
//! the same source, app, window, focused element (role and label) and capture run/activity when it follows within
//! `gap_ms`:
//! - same lines: no new event; the earlier one's end time is extended;
//! - lines overlap at least `min_line_overlap` (shared distinct lines / the larger count): the earlier event takes the
//!   new full text (its paragraphs are replaced), its end time is extended and `payload.merge.revisions` counts it;
//! - a near-empty sample (fewer than `min_chars`, or under `min_ratio` of the earlier text) or one whose
//!   `text_status` is not a normal read: no event, and the chain is not broken.
//!
//! Every absorbed sample's `source_key` is kept in `payload.merge.keys` (the newest 200), so a resend is a duplicate,
//! a screenshot whose parent was absorbed still finds it, and deleting the event tombstones those keys as well.

use crate::chunk::TextChunker;
use crate::db::{int, opt_text, text, RowExt};
use crate::ingest::PreparedEvent;
use crate::json;
use crate::store::TimelineStore;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

/// `store_meta` key holding a [`TextMergeConfig`] as JSON (absent: the defaults).
pub const META_TEXT_MERGE: &str = "text_merge";
const KEPT_KEYS: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextMergeConfig {
    pub gap_ms: i64,
    pub min_line_overlap: f64,
    pub min_chars: usize,
    pub min_ratio: f64,
    /// `text_status` values that mean a normal read.
    pub read_statuses: Vec<String>,
}

impl Default for TextMergeConfig {
    fn default() -> Self {
        TextMergeConfig { gap_ms: 120_000, min_line_overlap: 0.6, min_chars: 2, min_ratio: 0.1, read_statuses: vec!["captured".into(), "partial".into(), "ok".into()] }
    }
}

/// What the sample does to the chain it continues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Repeat,
    Revise,
    Absorb,
}

struct Candidate {
    event_id: String,
    /// Index into the batch's pending inserts, when the candidate is not stored yet.
    planned: Option<usize>,
    body: Vec<String>,
    selection: Vec<String>,
    payload: Value,
}

fn lines(raw: &str) -> Vec<String> {
    TextChunker::normalise(raw).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

fn field_lines(p: &PreparedEvent, field: &str) -> Vec<String> {
    p.event.text.iter().filter(|t| t.field == field).flat_map(|t| lines(&t.content)).collect()
}

fn chars(lines: &[String]) -> usize {
    lines.iter().map(|l| l.chars().count()).sum()
}

fn overlap(a: &[String], b: &[String]) -> f64 {
    let (sa, sb): (HashSet<&String>, HashSet<&String>) = (a.iter().collect(), b.iter().collect());
    let larger = sa.len().max(sb.len());
    if larger == 0 {
        return 1.0;
    }
    sa.intersection(&sb).count() as f64 / larger as f64
}

/// The fields two samples must share to belong to one chain.
fn focus(app: &Option<String>, title: &Option<String>, payload: &Value) -> [Option<String>; 6] {
    let s = |k: &str| json::str_at(payload, k).map(String::from);
    let legacy = |k: &str| payload.get("legacy").and_then(|l| l.get(k)).and_then(|v| v.as_str()).map(String::from);
    [app.clone(), title.clone(), s("focused_role"), s("focused_label"), legacy("run_id"), legacy("activity_id")]
}

/// The earlier event's payload after absorbing one more sample.
fn absorbed_payload(base: &Value, incoming: &Value, key: &str, step: Step) -> Value {
    let mut out: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    if step == Step::Revise {
        if let Some(fields) = incoming.as_object() {
            for (k, v) in fields {
                if !matches!(k.as_str(), "legacy" | "merge" | "trigger") {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let mut merge = out.get("merge").and_then(|m| m.as_object()).cloned().unwrap_or_default();
    let count = |k: &str| merge.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    let (revisions, samples) = (count("revisions") + (step == Step::Revise) as i64, count("samples") + 1);
    let mut keys: Vec<Value> = merge.get("keys").and_then(|k| k.as_array()).cloned().unwrap_or_default();
    keys.push(json!(key));
    if keys.len() > KEPT_KEYS {
        keys.drain(..keys.len() - KEPT_KEYS);
    }
    merge.insert("revisions".into(), json!(revisions));
    merge.insert("samples".into(), json!(samples));
    merge.insert("keys".into(), Value::Array(keys));
    out.insert("merge".into(), Value::Object(merge));
    Value::Object(out)
}

impl TimelineStore {
    pub fn text_merge_config(&self) -> TextMergeConfig {
        self.meta_value(META_TEXT_MERGE).ok().flatten().and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default()
    }

    /// The `screen.text` event that absorbed sample `key` (searched around `near_ms` when given).
    pub(crate) fn merged_event(&self, sid: &str, key: &str, near_ms: Option<i64>) -> Result<Option<String>> {
        let sql = "SELECT e.event_id FROM events e, json_each(e.payload_json,'$.merge.keys') k
            WHERE e.source_id=? AND e.kind='screen.text' AND k.value=?";
        match near_ms {
            Some(t) => self.db.first(
                &format!("{sql} AND e.start_ms<=? AND COALESCE(e.end_ms,e.start_ms)>=? LIMIT 1"),
                &[text(sid), text(key), int(t + 60_000), int(t - 3_600_000)],
                |r| r.text(0),
            ),
            None => self.db.first(&format!("{sql} LIMIT 1"), &[text(sid), text(key)], |r| r.text(0)),
        }
    }

    /// An event or the event that absorbed it.
    pub(crate) fn resolve_key(&self, sid: &str, key: &str, near_ms: Option<i64>) -> Result<Option<String>> {
        match self.event_id(sid, key)? {
            Some(id) => Ok(Some(id)),
            None => self.merged_event(sid, key, near_ms),
        }
    }

    pub(crate) fn has_merge_record(&self, event_id: &str) -> Result<bool> {
        Ok(self.db.scalar("SELECT COUNT(*) FROM events WHERE event_id=? AND json_extract(payload_json,'$.merge') IS NOT NULL", &[text(event_id)])? > 0)
    }

    fn stored_candidate(&self, sid: &str, p: &PreparedEvent, cfg: &TextMergeConfig) -> Result<Option<Candidate>> {
        let e = &p.event;
        let Some(start) = e.start_ms else { return Ok(None) };
        let want = focus(&e.app_bundle_id, &e.title, &e.payload);
        let rows = self.db.query(
            "SELECT event_id, payload_json FROM events WHERE source_id=? AND kind='screen.text' AND sensitivity<>'protected'
              AND app_bundle_id IS ? AND title IS ? AND start_ms<=? AND COALESCE(end_ms,start_ms)>=?
            ORDER BY COALESCE(end_ms,start_ms) DESC, ingest_seq DESC LIMIT 8",
            &[text(sid), opt_text(e.app_bundle_id.clone()), opt_text(e.title.clone()), int(start), int(start - cfg.gap_ms)],
            |r| (r.text(0), json::parse(&r.text(1)).unwrap_or(Value::Null)),
        )?;
        for (id, payload) in rows {
            if payload.get("record_evidence").is_some() || focus(&e.app_bundle_id, &e.title, &payload) != want {
                continue;
            }
            let mut fields: HashMap<String, Vec<String>> = HashMap::new();
            for (field, t) in self.db.query(
                "SELECT t.field, c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id WHERE t.event_id=? ORDER BY t.field, t.ord",
                &[text(&id)],
                |r| (r.text(0), r.text(1)),
            )? {
                fields.entry(field).or_default().extend(lines(&t));
            }
            return Ok(Some(Candidate {
                event_id: id,
                planned: None,
                body: fields.remove("body").unwrap_or_default(),
                selection: fields.remove("selection").unwrap_or_default(),
                payload,
            }));
        }
        Ok(None)
    }

    /// Folds `p` into the chain it continues; returns the event that now stands for it, or `None` to insert it.
    pub(crate) fn merge_text(
        &self,
        sid: &str,
        p: &PreparedEvent,
        plans: &mut [(PreparedEvent, String)],
        cfg: &TextMergeConfig,
        now: i64,
        cache: &mut HashMap<String, i64>,
    ) -> Result<Option<String>> {
        let e = &p.event;
        if e.kind != "screen.text" || e.sensitivity == "protected" || e.payload.get("record_evidence").is_some() {
            return Ok(None);
        }
        let Some(start) = e.start_ms else { return Ok(None) };
        let want = focus(&e.app_bundle_id, &e.title, &e.payload);
        // The newest candidate wins: a pending insert of this batch, else a stored event.
        let planned = plans.iter().enumerate().rev().find(|(_, (q, _))| {
            let end = q.event.end_ms.or(q.event.start_ms).unwrap_or(i64::MIN);
            q.event.kind == "screen.text"
                && q.event.payload.get("record_evidence").is_none()
                && q.event.sensitivity != "protected"
                && focus(&q.event.app_bundle_id, &q.event.title, &q.event.payload) == want
                && q.event.start_ms.is_some_and(|s| s <= start)
                && start - end <= cfg.gap_ms
        });
        let candidate = match planned {
            Some((i, (q, id))) => Some(Candidate {
                event_id: id.clone(),
                planned: Some(i),
                body: field_lines(q, "body"),
                selection: field_lines(q, "selection"),
                payload: q.event.payload.clone(),
            }),
            None => self.stored_candidate(sid, p, cfg)?,
        };
        let Some(c) = candidate else { return Ok(None) };
        let (body, selection) = (field_lines(p, "body"), field_lines(p, "selection"));
        let status = json::str_at(&e.payload, "text_status").unwrap_or("");
        let normal = cfg.read_statuses.iter().any(|s| s == status);
        let before = chars(&c.body);
        let step = if body == c.body && selection == c.selection {
            Step::Repeat
        } else if before > 0 && (!normal || chars(&body) < cfg.min_chars.max((cfg.min_ratio * before as f64) as usize)) {
            Step::Absorb
        } else if before > 0 && overlap(&c.body, &body) >= cfg.min_line_overlap {
            Step::Revise
        } else {
            return Ok(None);
        };
        let payload = absorbed_payload(&c.payload, &e.payload, &e.source_key, step);
        if let Some(i) = c.planned {
            let q = &mut plans[i].0;
            q.event.end_ms = Some(q.event.end_ms.unwrap_or(start).max(start));
            q.event.payload = payload;
            if step == Step::Revise {
                q.event.text = e.text.clone();
                q.texts = p.texts.clone();
            }
            return Ok(Some(c.event_id));
        }
        if step == Step::Revise {
            let old: Vec<i64> = self.db.query("SELECT chunk_id FROM event_text WHERE event_id=?", &[text(&c.event_id)], |r| r.int(0))?;
            self.db.execute("DELETE FROM event_text WHERE event_id=?", &[text(&c.event_id)])?;
            self.insert_text(p, &c.event_id, sid, now, cache)?;
            for id in old {
                if self.db.scalar("SELECT COUNT(*) FROM event_text WHERE chunk_id=?", &[int(id)])? == 0 {
                    if let Some(t) = self.db.first("SELECT text FROM text_chunks WHERE chunk_id=?", &[int(id)], |r| r.text(0))? {
                        self.db.execute("INSERT INTO text_fts(text_fts,rowid,text) VALUES('delete',?,?)", &[int(id), text(&t)])?;
                        self.db.execute("DELETE FROM text_chunks WHERE chunk_id=?", &[int(id)])?;
                        cache.retain(|_, v| *v != id);
                    }
                }
            }
        }
        self.db.execute(
            "UPDATE events SET end_ms=MAX(COALESCE(end_ms,start_ms),?), payload_json=? WHERE event_id=?",
            &[int(start), text(json::canonical(&payload)), text(&c.event_id)],
        )?;
        self.record_change("event", &c.event_id, "update")?;
        Ok(Some(c.event_id))
    }
}
