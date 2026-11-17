//! Tools for the attached agent (agent-platform-v1 §4.1): reading derived memories, and the writes it may make inside
//! the store. Every write needs its category in the client's `scope.writes`, checks each referenced record against
//! the client's scope, is audited like a query, is marked with who wrote it (`by`: client, run, model) and can be
//! undone with `undo_write`. Nothing here reaches outside the store; actions outside the App stay `propose`.
//!
//! | Tool | Scope flag | What it writes |
//! |---|---|---|
//! | `memories` | `derived` | nothing (lists memories whose every source the client can read) |
//! | `annotate` | `annotate` | a fragment title/notes revision (`corrections`, `relabel`), as the App's editor does |
//! | `mark_reference` | `reference` | a `memories` row, kind `reference`, status `candidate` (an AI draft until confirmed) |
//! | `commitment` | `commitment` | a new revision of a commitment candidate (`confirmed` or `rejected`) and a correction |
//! | `save_draft` | `draft` | a `digests` revision with status `draft` |
//! | `undo_write` | the category of the write | a new revision that restores what was there before |

use super::core::{QErr, QResult};
use crate::db::{int, opt_text, text, RowExt, P};
use crate::ids::uuid_v7;
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Names of the agent tools; the local channel serves them, the loopback HTTP and MCP catalog never list them.
pub const AGENT_TOOL_NAMES: [&str; 6] = ["memories", "annotate", "mark_reference", "commitment", "save_draft", "undo_write"];

/// Who is writing: filled in by the service from the frozen run, never by the model (the service drops `_run` from
/// what the agent sends before setting its own).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunTag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_ref: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoriesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, rename = "_run", skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTag>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotateRequest {
    pub event_ids: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default, rename = "_run", skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTag>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkReferenceRequest {
    pub event_ids: Vec<String>,
    pub label: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "_run", skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTag>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitmentRequest {
    pub memory_id: String,
    /// `confirm` or `ignore`.
    pub decision: String,
    #[serde(default, rename = "_run", skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTag>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveDraftRequest {
    /// `hour`, `day` or `week`.
    pub level: String,
    pub start: String,
    pub end: String,
    pub text: String,
    pub citations: Vec<String>,
    #[serde(default, rename = "_run", skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTag>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoRequest {
    pub write_id: String,
    #[serde(default, rename = "_run", skip_serializing_if = "Option::is_none")]
    pub run: Option<RunTag>,
}

fn denied(code: &str) -> QErr {
    QErr::Err(TimelineError::Denied(code.into()))
}

fn invalid(message: &str) -> QErr {
    QErr::Err(TimelineError::InvalidInput(message.into()))
}

fn by(ctx_id: &str, run: &Option<RunTag>) -> Value {
    let run = run.clone().unwrap_or_default();
    json!({"client": ctx_id, "run_id": run.run_id, "model_ref": run.model_ref})
}

pub(super) struct MemoryRow {
    pub(super) kind: String,
    pub(super) text: String,
    pub(super) valid_from: Option<i64>,
    pub(super) valid_to: Option<i64>,
    pub(super) status: String,
    pub(super) producer: String,
    pub(super) model_ref: Option<String>,
    pub(super) prompt_version: Option<String>,
    pub(super) confidence: Option<f64>,
}

impl TimelineStore {
    fn require_write(&self, ctx: &super::core::ClientContext, flag: &str) -> QResult<()> {
        if !ctx.scope.writes.iter().any(|w| w == flag) {
            return Err(denied("write_not_permitted"));
        }
        if !ctx.scope.text || !ctx.scope.derived {
            return Err(denied("write_not_permitted"));
        }
        Ok(())
    }

    fn is_internal(&self, client: &str) -> Result<bool> {
        Ok(self.db.scalar("SELECT COUNT(*) FROM clients WHERE client_id=? AND transport='internal'", &[text(client)])? == 1)
    }

    /// Every event and episode a memory revision rests on is visible to `ctx` (an episode through all its evidence).
    fn memory_visible(&self, id: &str, rev: i64, ctx: &super::core::ClientContext) -> Result<bool> {
        let refs = self.db.query("SELECT ref_type,ref_id,ref_rev FROM memory_sources WHERE memory_id=? AND rev=?", &[text(id), int(rev)], |r| (r.text(0), r.text(1), r.opt_int(2)))?;
        if refs.is_empty() {
            return Ok(false);
        }
        let mut events = vec![];
        for (kind, ref_id, ref_rev) in refs {
            if kind == "event" {
                events.push(ref_id);
            } else {
                let rev = match ref_rev {
                    Some(r) => r,
                    None => self.db.scalar("SELECT COALESCE(MAX(rev),0) FROM episodes WHERE episode_id=?", &[text(&ref_id)])?,
                };
                let evidence = self.db.query("SELECT event_id FROM episode_evidence WHERE episode_id=? AND rev=?", &[text(&ref_id), int(rev)], |r| r.text(0))?;
                if evidence.is_empty() {
                    return Ok(false);
                }
                events.extend(evidence);
            }
        }
        events.sort();
        events.dedup();
        Ok(self.summaries(&events, ctx)?.len() == events.len())
    }

    pub(super) fn memory_row(&self, id: &str, rev: i64) -> Result<Option<MemoryRow>> {
        self.db.first(
            "SELECT kind,text,valid_from_ms,valid_to_ms,status,producer,model_ref,prompt_version,confidence FROM memories WHERE memory_id=? AND rev=?",
            &[text(id), int(rev)],
            |r| MemoryRow {
                kind: r.text(0),
                text: r.text(1),
                valid_from: r.opt_int(2),
                valid_to: r.opt_int(3),
                status: r.text(4),
                producer: r.text(5),
                model_ref: r.opt_text(6),
                prompt_version: r.opt_text(7),
                confidence: r.get::<_, Option<f64>>(8).ok().flatten(),
            },
        )
    }

    pub(super) fn memory_latest(&self, id: &str) -> Result<i64> {
        self.db.scalar("SELECT COALESCE(MAX(rev),0) FROM memories WHERE memory_id=?", &[text(id)])
    }

    /// Writes revision `rev` of a memory from `row`, with the sources of `sources_rev` (or `sources` when given).
    pub(super) fn memory_insert(&self, id: &str, rev: i64, row: &MemoryRow, sources_rev: Option<i64>, sources: &[(String, String, Option<String>)]) -> Result<()> {
        self.db.execute(
            "INSERT INTO memories(memory_id,rev,kind,text,valid_from_ms,valid_to_ms,status,producer,model_ref,prompt_version,confidence,created_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            &[
                text(id),
                int(rev),
                text(&row.kind),
                text(&row.text),
                crate::db::opt_int(row.valid_from),
                crate::db::opt_int(row.valid_to),
                text(&row.status),
                text(&row.producer),
                opt_text(row.model_ref.clone()),
                opt_text(row.prompt_version.clone()),
                row.confidence.map(P::Real).unwrap_or(P::Null),
                int(self.clock.now_ms()),
            ],
        )?;
        if let Some(from) = sources_rev {
            self.db.execute(
                "INSERT INTO memory_sources(memory_id,rev,ref_type,ref_id,ref_rev,selector_json) SELECT memory_id,?,ref_type,ref_id,ref_rev,selector_json FROM memory_sources WHERE memory_id=? AND rev=?",
                &[int(rev), text(id), int(from)],
            )?;
        }
        for (kind, ref_id, selector) in sources {
            self.db.execute(
                "INSERT INTO memory_sources(memory_id,rev,ref_type,ref_id,ref_rev,selector_json) VALUES(?,?,?,?,NULL,?)",
                &[text(id), int(rev), text(kind), text(ref_id), opt_text(selector.clone())],
            )?;
        }
        self.record_change("memory", id, "insert")
    }

    pub(super) fn correction(&self, target_type: &str, target_id: &str, action: &str, value: &Value) -> Result<String> {
        let now = self.clock.now_ms();
        let id = format!("agent:{}", uuid_v7(Some(now)));
        self.db.execute("INSERT INTO corrections VALUES(?,?,?,?,?,?)", &[text(&id), text(target_type), text(target_id), text(action), text(value.to_string()), int(now)])?;
        Ok(id)
    }

    // MARK: memories (read)

    pub fn memories(&self, req: &MemoriesRequest, client_id: &str) -> Result<Value> {
        self.audited("memories", client_id, req, |v: &Value| v["memories"].as_array().map(|a| a.len() as i64), |ctx| {
            if !ctx.scope.derived || !ctx.scope.text {
                return Ok(json!({"memories": [], "note": "此访问方不能读取派生内容"}));
            }
            let limit = req.limit.unwrap_or(20).clamp(1, 50);
            let mut sql = "SELECT m.memory_id,m.rev,m.kind,m.text,m.status,m.producer,m.confidence,m.created_ms FROM memories m WHERE m.rev=(SELECT MAX(rev) FROM memories x WHERE x.memory_id=m.memory_id)".to_string();
            let mut params = vec![];
            if let Some(kind) = &req.kind {
                sql += " AND m.kind=?";
                params.push(text(kind));
            }
            match req.status.as_deref() {
                Some(status) => {
                    sql += " AND m.status=?";
                    params.push(text(status));
                }
                None => sql += " AND m.status NOT IN ('rejected','superseded','expired')",
            }
            sql += " ORDER BY m.created_ms DESC LIMIT 500";
            let rows = self.db.query(&sql, &params, |r| (r.text(0), r.int(1), r.text(2), r.text(3), r.text(4), r.text(5), r.get::<_, Option<f64>>(6).ok().flatten(), r.int(7)))?;
            let mut out = vec![];
            for (id, rev, kind, body, status, producer, confidence, created) in rows {
                if out.len() as i64 >= limit {
                    break;
                }
                if !self.memory_visible(&id, rev, ctx)? {
                    continue;
                }
                let sources = self.db.query("SELECT ref_type,ref_id FROM memory_sources WHERE memory_id=? AND rev=? ORDER BY ref_id", &[text(&id), int(rev)], |r| json!({"ref_type": r.text(0), "ref_id": r.text(1)}))?;
                out.push(json!({"memory_id": id, "rev": rev, "kind": kind, "text": body, "status": status, "producer": producer, "confidence": confidence, "created": self.clock.iso(created), "sources": sources}));
            }
            Ok(json!({"memories": out}))
        })
    }

    // MARK: annotate

    pub fn annotate(&self, req: &AnnotateRequest, client_id: &str) -> Result<Value> {
        self.audited("annotate", client_id, req, |_: &Value| Some(1), |ctx| {
            self.require_write(ctx, "annotate")?;
            if req.title.is_none() && req.notes.is_none() {
                return Err(invalid("需要 title 或 notes"));
            }
            let (ids, refs) = self.annotation_targets(req.event_ids.clone(), ctx).map_err(|e| match e {
                TimelineError::NotFound(w) => QErr::ScopeMiss(w),
                TimelineError::Denied(_) => QErr::ScopeMiss("片段".into()),
                other => QErr::Err(other),
            })?;
            let current = self.visible_annotation(&ids[0], ctx)?;
            // Unchanged fields keep their current value, so renaming does not erase notes.
            let keep = |field: &str, new: &Option<String>| match new {
                Some(v) => Value::String(v.clone()),
                None => current.as_ref().map(|c| c[field].clone()).unwrap_or(Value::Null),
            };
            let request = format!("agent-{}", uuid_v7(Some(self.clock.now_ms())));
            let saved = self.annotation_write(&ids, &refs, current.as_ref(), &keep("title", &req.title), &keep("notes", &req.notes), &request, Some(&by(&ctx.id, &req.run)))?;
            Ok(json!({"write_id": format!("annotation:{request}"), "event_ids": ids, "title": saved["annotation"]["title"], "notes": saved["annotation"]["notes"], "recorded_as": "correction"}))
        })
    }

    // MARK: mark_reference

    pub fn mark_reference(&self, req: &MarkReferenceRequest, client_id: &str) -> Result<Value> {
        self.audited("mark_reference", client_id, req, |_: &Value| Some(1), |ctx| {
            self.require_write(ctx, "reference")?;
            let label = req.label.trim();
            if label.is_empty() || label.chars().count() > 500 {
                return Err(invalid("资料名称为空或过长"));
            }
            if req.tags.len() > 10 || req.tags.iter().any(|t| t.trim().is_empty() || t.chars().count() > 40) {
                return Err(invalid("标签最多 10 个，每个不超过 40 字"));
            }
            let mut ids = req.event_ids.clone();
            ids.sort();
            ids.dedup();
            if ids.is_empty() || ids.len() > 20 {
                return Err(invalid("需要 1 到 20 条事件"));
            }
            if self.summaries(&ids, ctx)?.len() != ids.len() {
                return Err(QErr::ScopeMiss("事件".into()));
            }
            let now = self.clock.now_ms();
            let id = uuid_v7(Some(now));
            let run = req.run.clone().unwrap_or_default();
            let selector = json!({"tags": req.tags, "by": by(&ctx.id, &req.run)}).to_string();
            let row = MemoryRow { kind: "reference".into(), text: label.into(), valid_from: None, valid_to: None, status: "candidate".into(), producer: "pi".into(), model_ref: run.model_ref, prompt_version: None, confidence: None };
            let sources: Vec<_> = ids.iter().map(|e| ("event".to_string(), e.clone(), Some(selector.clone()))).collect();
            self.db.transaction(|| self.memory_insert(&id, 1, &row, None, &sources))?;
            Ok(json!({"write_id": format!("memory:{id}:1"), "memory_id": id, "status": "candidate", "recorded_as": "ai_draft", "note": "已存为资料候选（AI 草稿），用户在「待你处理」确认后才算"}))
        })
    }

    // MARK: commitment

    pub fn commitment(&self, req: &CommitmentRequest, client_id: &str) -> Result<Value> {
        self.audited("commitment", client_id, req, |_: &Value| Some(1), |ctx| {
            self.require_write(ctx, "commitment")?;
            let status = match req.decision.as_str() {
                "confirm" => "confirmed",
                "ignore" => "rejected",
                _ => return Err(invalid("decision 只能是 confirm 或 ignore")),
            };
            let rev = self.memory_latest(&req.memory_id)?;
            let Some(mut row) = self.memory_row(&req.memory_id, rev)? else {
                return Err(QErr::ScopeMiss("承诺".into()));
            };
            if !self.memory_visible(&req.memory_id, rev, ctx)? {
                return Err(QErr::ScopeMiss("承诺".into()));
            }
            if row.kind != "commitment" || row.status != "candidate" {
                return Err(invalid("只能处理状态为候选的承诺"));
            }
            row.status = status.into();
            let value = json!({"decision": req.decision, "from_rev": rev, "by": by(&ctx.id, &req.run)});
            let correction = self.db.transaction(|| {
                self.memory_insert(&req.memory_id, rev + 1, &row, Some(rev), &[])?;
                self.correction("memory", &req.memory_id, if status == "confirmed" { "confirm" } else { "reject" }, &value)
            })?;
            Ok(json!({"write_id": format!("memory:{}:{}", req.memory_id, rev + 1), "memory_id": req.memory_id, "status": status, "correction_id": correction, "recorded_as": "correction"}))
        })
    }

    // MARK: save_draft

    pub fn save_draft(&self, req: &SaveDraftRequest, client_id: &str) -> Result<Value> {
        self.audited("save_draft", client_id, req, |_: &Value| Some(1), |ctx| {
            self.require_write(ctx, "draft")?;
            if !["hour", "day", "week"].contains(&req.level.as_str()) {
                return Err(invalid("level 只能是 hour、day、week"));
            }
            let start = self.clock.parse(&req.start, false)?;
            let end = self.clock.parse(&req.end, true)?;
            if end <= start || end - start > 8 * 86_400_000 {
                return Err(invalid("时间范围无效（最长 8 天）"));
            }
            let body = req.text.trim();
            if body.is_empty() || body.chars().count() > 20_000 {
                return Err(invalid("草稿为空或超过 20000 字"));
            }
            let mut citations = req.citations.clone();
            citations.sort();
            citations.dedup();
            if citations.is_empty() || citations.len() > 100 {
                return Err(invalid("草稿需要 1 到 100 条引用"));
            }
            if self.summaries(&citations, ctx)?.len() != citations.len() {
                return Err(QErr::ScopeMiss("引用".into()));
            }
            let id = format!("{}-{start}", req.level);
            let rev = self.db.scalar("SELECT COALESCE(MAX(rev),0) FROM digests WHERE digest_id=?", &[text(&id)])? + 1;
            let run = req.run.clone().unwrap_or_default();
            let body_json = json!({"format": "candid-agent-draft/1", "text": body, "citations": citations, "by": by(&ctx.id, &req.run)});
            self.db.transaction(|| {
                self.db.execute(
                    "INSERT INTO digests(digest_id,rev,level,start_ms,end_ms,body_json,status,producer,model_ref,prompt_version,run_id,created_ms) VALUES(?,?,?,?,?,?,'draft','pi',?,NULL,?,?)",
                    &[text(&id), int(rev), text(&req.level), int(start), int(end), text(body_json.to_string()), opt_text(run.model_ref), opt_text(run.run_id), int(self.clock.now_ms())],
                )?;
                self.record_change("digest", &id, "insert")
            })?;
            Ok(json!({"write_id": format!("digest:{id}:{rev}"), "digest_id": id, "rev": rev, "status": "draft", "recorded_as": "ai_draft"}))
        })
    }

    // MARK: undo_write

    /// Undoes one write by appending what was there before. The App may undo any agent write; an agent only its own,
    /// and only while nothing newer has been written over it.
    pub fn undo_write(&self, req: &UndoRequest, client_id: &str) -> Result<Value> {
        let internal = self.is_internal(client_id)?;
        self.audited("undo_write", client_id, req, |_: &Value| Some(1), |ctx| {
            let parts: Vec<&str> = req.write_id.split(':').collect();
            let own = |b: &Value| internal || b["client"].as_str() == Some(ctx.id.as_str());
            let undo_by = json!({"client": ctx.id, "run_id": req.run.as_ref().and_then(|r| r.run_id.clone()), "undo_of": req.write_id});
            match parts.as_slice() {
                ["annotation", request] => {
                    if !internal {
                        self.require_write(ctx, "annotate")?;
                    }
                    let rows = self.db.query("SELECT target_id,value_json FROM corrections WHERE correction_id LIKE ? ESCAPE '\\' ORDER BY target_id", &[text(format!("annotation:{}:%", request.replace('%', "\\%").replace('_', "\\_")))], |r| (r.text(0), r.text(1)))?;
                    let Some((first, value)) = rows.first() else { return Err(QErr::ScopeMiss("写入".into())) };
                    let value: Value = serde_json::from_str(value).map_err(TimelineError::from)?;
                    if !own(&value["by"]) {
                        return Err(denied("not_own_write"));
                    }
                    let ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
                    let (ids, refs) = self.annotation_targets(ids, ctx).map_err(|_| QErr::ScopeMiss("片段".into()))?;
                    let current = self.visible_annotation(first, ctx)?;
                    if current.as_ref().map(|c| c["request_id"].clone()) != Some(value["request_id"].clone()) {
                        return Err(invalid("这条修改之后又有新的修改，不能直接撤销"));
                    }
                    let previous = match value["previous"].as_str() {
                        Some(p) => self
                            .db
                            .first("SELECT value_json FROM corrections WHERE correction_id=?", &[text(format!("annotation:{p}:{first}"))], |r| r.text(0))?
                            .and_then(|v| serde_json::from_str::<Value>(&v).ok()),
                        None => None,
                    };
                    let (title, notes) = previous.map(|p| (p["title"].clone(), p["notes"].clone())).unwrap_or((Value::Null, Value::Null));
                    let undo = format!("undo-{}", uuid_v7(Some(self.clock.now_ms())));
                    self.annotation_write(&ids, &refs, current.as_ref(), &title, &notes, &undo, Some(&undo_by))?;
                    Ok(json!({"undone": req.write_id, "write_id": format!("annotation:{undo}"), "title": title, "notes": notes}))
                }
                ["memory", id, rev] | ["digest", id, rev] => {
                    let rev: i64 = rev.parse().map_err(|_| invalid("write_id 无效"))?;
                    let memory = parts[0] == "memory";
                    if memory {
                        let latest = self.memory_latest(id)?;
                        let Some(row) = self.memory_row(id, rev)? else { return Err(QErr::ScopeMiss("写入".into())) };
                        if !self.memory_visible(id, rev, ctx)? {
                            return Err(QErr::ScopeMiss("写入".into()));
                        }
                        let author = if rev == 1 {
                            self.db.first("SELECT selector_json FROM memory_sources WHERE memory_id=? AND rev=1 LIMIT 1", &[text(*id)], |r| r.opt_text(0))?.flatten().and_then(|s| serde_json::from_str::<Value>(&s).ok()).map(|v| v["by"].clone()).unwrap_or(Value::Null)
                        } else {
                            self.db
                                .first("SELECT value_json FROM corrections WHERE target_type='memory' AND target_id=? AND json_extract(value_json,'$.from_rev')=? ORDER BY created_ms DESC LIMIT 1", &[text(*id), int(rev - 1)], |r| r.text(0))?
                                .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                                .map(|v| v["by"].clone())
                                .unwrap_or(Value::Null)
                        };
                        if !own(&author) {
                            return Err(denied("not_own_write"));
                        }
                        if !internal {
                            self.require_write(ctx, if row.kind == "commitment" { "commitment" } else { "reference" })?;
                        }
                        if latest != rev {
                            return Err(invalid("这条修改之后又有新的修改，不能直接撤销"));
                        }
                        let restored = match self.memory_row(id, rev - 1)? {
                            Some(prev) => prev,
                            None => MemoryRow { status: "rejected".into(), ..row },
                        };
                        let status = restored.status.clone();
                        self.db.transaction(|| {
                            self.memory_insert(id, rev + 1, &restored, Some(if rev > 1 { rev - 1 } else { rev }), &[])?;
                            self.correction("memory", id, if status == "rejected" { "reject" } else { "relabel" }, &json!({"from_rev": rev, "by": undo_by}))
                        })?;
                        Ok(json!({"undone": req.write_id, "write_id": format!("memory:{id}:{}", rev + 1), "status": status}))
                    } else {
                        let row = self.db.first("SELECT level,start_ms,end_ms,body_json,status FROM digests WHERE digest_id=? AND rev=?", &[text(*id), int(rev)], |r| (r.text(0), r.int(1), r.int(2), r.text(3), r.text(4)))?;
                        let Some((level, start, end, body, status)) = row else { return Err(QErr::ScopeMiss("写入".into())) };
                        let body: Value = serde_json::from_str(&body).map_err(TimelineError::from)?;
                        let citations: Vec<String> = serde_json::from_value(body["citations"].clone()).unwrap_or_default();
                        if citations.is_empty() || self.summaries(&citations, ctx)?.len() != citations.len() {
                            return Err(QErr::ScopeMiss("写入".into()));
                        }
                        if !own(&body["by"]) {
                            return Err(denied("not_own_write"));
                        }
                        if !internal {
                            self.require_write(ctx, "draft")?;
                        }
                        let latest = self.db.scalar("SELECT COALESCE(MAX(rev),0) FROM digests WHERE digest_id=?", &[text(*id)])?;
                        if latest != rev || status != "draft" {
                            return Err(invalid("这份草稿之后又有新的修改，不能直接撤销"));
                        }
                        let mut undone = body.clone();
                        undone["by"] = undo_by.clone();
                        self.db.transaction(|| {
                            self.db.execute(
                                "INSERT INTO digests(digest_id,rev,level,start_ms,end_ms,body_json,status,producer,model_ref,prompt_version,run_id,created_ms) VALUES(?,?,?,?,?,?,'rejected','pi',NULL,NULL,?,?)",
                                &[text(*id), int(rev + 1), text(&level), int(start), int(end), text(undone.to_string()), opt_text(req.run.as_ref().and_then(|r| r.run_id.clone())), int(self.clock.now_ms())],
                            )?;
                            self.correction("digest", id, "reject", &json!({"from_rev": rev, "by": undo_by}))?;
                            self.record_change("digest", id, "insert")
                        })?;
                        Ok(json!({"undone": req.write_id, "write_id": format!("digest:{id}:{}", rev + 1), "status": "rejected"}))
                    }
                }
                _ => Err(invalid("write_id 无效")),
            }
        })
    }
}

// MARK: The attached agent's registration and model calls

impl TimelineStore {
    /// Registers the attached agent's client under a fixed id, or updates its scope, and gives it a new token (the
    /// previous one stops working). A client the user revoked stays revoked: the agent cannot re-enable itself.
    pub fn attach_agent_client(&self, client_id: &str, name: &str, scope: &crate::ClientScope) -> Result<String> {
        let token = crate::ids::token_hex();
        let hash = crate::ids::sha256_hex(&token);
        let existing = self.db.first("SELECT revoked_ms FROM clients WHERE client_id=?", &[text(client_id)], |r| r.opt_int(0))?;
        match existing {
            Some(Some(_)) => return Err(TimelineError::Denied("client_revoked".into())),
            Some(None) => {
                self.db.execute("UPDATE clients SET token_hash=?, scope_json=?, name=? WHERE client_id=?", &[text(&hash), text(crate::json::canonical_of(scope)), text(name), text(client_id)])?;
            }
            None => {
                self.db.execute(
                    "INSERT INTO clients(client_id,name,transport,token_hash,scope_json,created_ms) VALUES(?,?,'http',?,?,?)",
                    &[text(client_id), text(name), text(&hash), text(crate::json::canonical_of(scope)), int(self.clock.now_ms())],
                )?;
            }
        }
        Ok(token)
    }

    /// Ends the agent's access until the next start: its token no longer resolves.
    pub fn detach_agent_client(&self, client_id: &str) -> Result<()> {
        self.db.execute("UPDATE clients SET token_hash=NULL WHERE client_id=?", &[text(client_id)])
    }

    /// Opens an `ai_calls` row for one model call of the attached agent (`client = pi`). Until finished it reads as an
    /// interrupted call of unknown cost, as the App's own calls do.
    pub fn agent_call_start(&self, feature: &str, run_id: &str, provider_id: &str, model_id: &str, destination: &str, request_bytes: i64) -> Result<String> {
        let id = crate::ids::uuid_v4();
        self.db.execute(
            "INSERT INTO ai_calls(call_id,started_ms,feature,client,run_id,provider_id,model_id,destination,status,error_code,cost_source,request_bytes) VALUES(?,?,?,'pi',?,?,?,?,'error','interrupted_unknown_charge','unknown',?)",
            &[text(&id), int(self.clock.now_ms()), text(feature), text(run_id), text(provider_id), text(model_id), text(destination), int(request_bytes)],
        )?;
        Ok(id)
    }

    pub fn agent_call_finish(&self, id: &str, status: &str, error: Option<&str>, usage: &Value, response_bytes: Option<i64>) -> Result<()> {
        let now = self.clock.now_ms();
        let cost = usage["cost"].as_f64().filter(|c| c.is_finite() && *c >= 0.0);
        self.db.execute(
            "UPDATE ai_calls SET ended_ms=?,latency_ms=?-started_ms,status=?,error_code=?,input_tokens=?,output_tokens=?,cache_read_tokens=?,cost=?,cost_source=?,response_bytes=? WHERE call_id=?",
            &[
                int(now),
                int(now),
                text(status),
                opt_text(error),
                crate::db::opt_int(usage["prompt_tokens"].as_i64()),
                crate::db::opt_int(usage["completion_tokens"].as_i64()),
                crate::db::opt_int(usage["prompt_tokens_details"]["cached_tokens"].as_i64()),
                cost.map(P::Real).unwrap_or(P::Null),
                text(if cost.is_some() { "provider" } else { "unknown" }),
                crate::db::opt_int(response_bytes),
                text(id),
            ],
        )
    }
}

impl TimelineStore {
    /// Records a call refused before it reached a tool (for example a tool this client may never use).
    pub fn audit_refusal(&self, client_id: &str, tool: &str, code: &str) -> Result<()> {
        let now = self.clock.now_ms();
        self.db.execute(
            "INSERT INTO access_log(log_id,at_ms,client_id,tool,params_json,duration_ms,denied) VALUES(?,?,?,?,'{}',0,?)",
            &[text(uuid_v7(Some(now))), int(now), text(client_id), text(tool), text(code)],
        )
    }
}
