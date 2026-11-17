//! Workflow outputs in the derived layer (timeline-v1 §4.8.1): a recording's title and summary are one episode,
//! its decisions one memory, the day review one digest. Each write is a new revision; the previous one is
//! superseded. Tags and screenshot text have no table yet and stay as `draft/1` values in `store_meta`.
//!
//! A draft's provenance (source revision, job, model, prompt version) is kept beside it — on the episode's
//! primary evidence, the memory's source and inside the digest body — so `drafts` returns the same `draft/1`
//! objects as before and stale drafts are still dropped by source revision.
use crate::{
    db::{int, opt_text, text, RowExt},
    Result, TimelineStore,
};
use serde_json::{json, Value};

const EPISODE_STEPS: [&str; 2] = ["title", "summary"];
const MIGRATED: &str = "workflows.drafts.migrated/1";

fn meta(draft: &Value) -> Value {
    let mut m = draft.clone();
    if let Some(o) = m.as_object_mut() {
        o.remove("text");
        o.remove("format");
    }
    m
}

fn with_text(meta: &Value, text: &str) -> Value {
    let mut d = meta.clone();
    d["format"] = json!("draft/1");
    d["text"] = json!(text);
    d
}

impl TimelineStore {
    /// The event a recording projects to; derived rows only point at records that exist.
    fn recording_event(&self, id: &str) -> Result<Option<(String, Option<i64>, Option<i64>)>> {
        self.db.first(
            "SELECT event_id,start_ms,end_ms FROM events WHERE source_key=? ORDER BY event_id LIMIT 1",
            &[text(format!("media:{id}"))],
            |r| (r.text(0), r.opt_int(1), r.opt_int(2)),
        )
    }

    /// Every current draft for a workflow input, keyed by step; a table row wins over an old `store_meta` value.
    /// The caller filters by source revision.
    pub(crate) fn workflow_drafts(&self, id: &str) -> Result<Value> {
        let mut out = self
            .db
            .first("SELECT value FROM store_meta WHERE key=?", &[text(format!("workflows.drafts/1:{id}"))], |r| r.text(0))?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        if let Some(day) = id.strip_prefix("workflow-day:") {
            if let Some(body) = self.db.first(
                "SELECT body_json FROM digests WHERE digest_id=? AND status<>'superseded' ORDER BY rev DESC LIMIT 1",
                &[text(format!("day:{day}"))],
                |r| r.text(0),
            )? {
                out["daily"] = serde_json::from_str(&body)?;
            }
            return Ok(out);
        }
        let episode = format!("rec:{id}");
        if let Some((title, summary, selector)) = self.db.first(
            "SELECT p.title,p.summary,e.selector_json FROM episodes p JOIN episode_evidence e USING(episode_id,rev)
             WHERE p.episode_id=? AND p.status<>'superseded' AND e.role='primary' ORDER BY p.rev DESC LIMIT 1",
            &[text(&episode)],
            |r| (r.opt_text(0), r.opt_text(1), r.opt_text(2)),
        )? {
            let selector: Value = selector.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            for (step, value) in [("title", title), ("summary", summary)] {
                if let (Some(v), true) = (value, selector[step].is_object()) {
                    out[step] = with_text(&selector[step], &v);
                }
            }
        }
        if let Some((body, selector)) = self.db.first(
            "SELECT m.text,s.selector_json FROM memories m JOIN memory_sources s USING(memory_id,rev)
             WHERE m.memory_id=? AND m.status<>'superseded' ORDER BY m.rev DESC LIMIT 1",
            &[text(format!("decisions:{id}"))],
            |r| (r.text(0), r.opt_text(1)),
        )? {
            let selector: Value = selector.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            out["decisions"] = with_text(&selector, &body);
        }
        Ok(out)
    }

    /// Stores one step's draft. Runs inside the caller's transaction.
    pub(crate) fn workflow_draft_put(&self, id: &str, step: &str, draft: &Value) -> Result<()> {
        self.media_projection_stamp.set(None);
        self.workflow_scan_stamp.set(None);
        let now = self.clock.now_ms();
        let model = draft["producer"].as_str().map(str::to_string);
        let producer = format!("workflow.{step}");
        let prompt = draft["prompt_version"].as_i64().map(|v| v.to_string());
        let body = draft["text"].as_str().unwrap_or("");
        if step == "daily" {
            if let Some(day) = id.strip_prefix("workflow-day:") {
                let (start, end) = day_bounds(day);
                let rev = self.db.scalar("SELECT COALESCE(MAX(rev),0) FROM digests WHERE digest_id=?", &[text(format!("day:{day}"))])?;
                self.db.execute("UPDATE digests SET status='superseded' WHERE digest_id=? AND status='draft'", &[text(format!("day:{day}"))])?;
                return self.db.execute(
                    "INSERT INTO digests VALUES(?,?,'day',?,?,?,'draft',?,?,?,?,?)",
                    &[text(format!("day:{day}")), int(rev + 1), int(start), int(end), text(draft.to_string()), text(producer), opt_text(model), opt_text(prompt), opt_text(draft["job_id"].as_str()), int(now)],
                );
            }
        }
        let event = self.recording_event(id)?;
        let Some((event_id, start, end)) = event.filter(|_| EPISODE_STEPS.contains(&step) || step == "decisions") else {
            return self.workflow_draft_put_meta(id, step, draft);
        };
        if step == "decisions" {
            let memory = format!("decisions:{id}");
            let rev = self.db.scalar("SELECT COALESCE(MAX(rev),0) FROM memories WHERE memory_id=?", &[text(&memory)])?;
            self.db.execute("UPDATE memories SET status='superseded' WHERE memory_id=? AND status='candidate'", &[text(&memory)])?;
            self.db.execute(
                "INSERT INTO memories VALUES(?,?,'decision',?,?,NULL,'candidate',?,?,?,NULL,?)",
                &[text(&memory), int(rev + 1), text(body), crate::db::opt_int(start), text(producer), opt_text(model), opt_text(prompt), int(now)],
            )?;
            return self.db.execute(
                "INSERT INTO memory_sources VALUES(?,?,'event',?,NULL,?)",
                &[text(&memory), int(rev + 1), text(&event_id), text(meta(draft).to_string())],
            );
        }
        let episode = format!("rec:{id}");
        let previous = self.db.first(
            "SELECT p.rev,p.title,p.summary,e.selector_json FROM episodes p LEFT JOIN episode_evidence e ON e.episode_id=p.episode_id AND e.rev=p.rev AND e.role='primary'
             WHERE p.episode_id=? ORDER BY p.rev DESC LIMIT 1",
            &[text(&episode)],
            |r| (r.int(0), r.opt_text(1), r.opt_text(2), r.opt_text(3)),
        )?;
        let (rev, mut title, mut summary, selector) = previous.unwrap_or((0, None, None, None));
        let mut selector: Value = selector.and_then(|s| serde_json::from_str(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
        if step == "title" { title = Some(body.into()); } else { summary = Some(body.into()); }
        selector[step] = meta(draft);
        let start = start.unwrap_or(now);
        self.db.execute("UPDATE episodes SET status='superseded' WHERE episode_id=? AND status='draft'", &[text(&episode)])?;
        self.db.execute(
            "INSERT INTO episodes VALUES(?,?,?,?,?,?,'conversation','draft',?,?,?,NULL,?)",
            &[text(&episode), int(rev + 1), int(start), int(end.unwrap_or(start).max(start)), opt_text(title), opt_text(summary), text("workflow"), opt_text(model), opt_text(prompt), int(now)],
        )?;
        self.db.execute(
            "INSERT INTO episode_evidence VALUES(?,?,?,'primary',?)",
            &[text(&episode), int(rev + 1), text(&event_id), text(selector.to_string())],
        )
    }

    fn workflow_draft_put_meta(&self, id: &str, step: &str, draft: &Value) -> Result<()> {
        let key = format!("workflows.drafts/1:{id}");
        let mut all = self
            .db
            .first("SELECT value FROM store_meta WHERE key=?", &[text(&key)], |r| r.text(0))?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        all[step] = draft.clone();
        self.db.execute("INSERT OR REPLACE INTO store_meta VALUES(?,?)", &[text(key), text(all.to_string())])
    }

    /// Removes a deleted recording's derived rows and the day reviews that used it.
    pub(crate) fn workflow_drafts_forget(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM episodes WHERE episode_id=?", &[text(format!("rec:{id}"))])?;
        self.db.execute("DELETE FROM memories WHERE memory_id=?", &[text(format!("decisions:{id}"))])?;
        self.db.execute(
            "DELETE FROM digests WHERE digest_id IN (SELECT 'day:'||substr(key,length('workflow.sources/1:workflow-day:')+1) FROM store_meta
             WHERE key LIKE 'workflow.sources/1:workflow-day:%' AND EXISTS(SELECT 1 FROM json_each(value) WHERE value=?))",
            &[text(id)],
        )
    }

    /// One-time move of `draft/1` values written before database v5 into the derived tables. The old keys
    /// stay readable for one version: a step already in a table wins over the key.
    pub fn migrate_workflow_drafts(&self) -> Result<usize> {
        if self.db.scalar("SELECT COUNT(*) FROM store_meta WHERE key=?", &[text(MIGRATED)])? == 1 {
            return Ok(0);
        }
        let rows = self.db.query("SELECT key,value FROM store_meta WHERE key LIKE 'workflows.drafts/1:%'", &[], |r| (r.text(0), r.text(1)))?;
        self.db.transaction(|| {
            let mut moved = 0;
            for (key, value) in &rows {
                let id = &key["workflows.drafts/1:".len()..];
                let Ok(Value::Object(drafts)) = serde_json::from_str::<Value>(value) else { continue };
                for step in ["title", "summary", "decisions", "daily"] {
                    let Some(draft) = drafts.get(step).filter(|d| d["text"].is_string()) else { continue };
                    let table = if step == "daily" { id.starts_with("workflow-day:") } else { self.recording_event(id)?.is_some() };
                    if table {
                        self.workflow_draft_put(id, step, draft)?;
                        moved += 1;
                    }
                }
            }
            self.db.execute("INSERT INTO store_meta VALUES(?,?)", &[text(MIGRATED), text(moved.to_string())])?;
            Ok(moved)
        })
    }
}

/// A day review covers that local day; the store has no time zone of its own, so the host's applies.
fn day_bounds(day: &str) -> (i64, i64) {
    use chrono::{Local, NaiveDate, TimeZone};
    let start = NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0);
    (start, start + 86_400_000)
}
