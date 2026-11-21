//! The App's own read tools over the Mac desktop source (ADR-024 §2: the same dispatcher, audit and client check as
//! the query tools). They return the records the desktop timeline draws — activities grouped by rule at read time,
//! a page of observations with their screenshots and gaps, recognition results — and absolute file paths for the
//! originals instead of bytes. Only the internal client (`transport = 'internal'`, the App on its local channel) may
//! call them; they are not in the tool catalog, so the loopback HTTP and MCP never list or serve them.
//!
//! Grouping (the ledger's activity projection, now computed per request):
//! - a screenshot or gap whose parent is a known observation of the same capture run joins the parent's activity;
//! - an observation with an explicit `activity_id` belongs to `(run_id, activity_id)`;
//! - an older observation without one (window, or context sampled by timer/content/selection/focus) continues the
//!   previous observation's activity when run, app and non-empty title match;
//! - everything else is an activity of its own.

use super::core::{QErr, QResult, ClientContext};
use crate::db::{int, placeholders, text, RowExt, P};
use crate::json;
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Event kinds the desktop timeline is drawn from (the ledger's record kinds).
const DESKTOP_KINDS: &[&str] = &["app.focus", "window.focus", "screen.text", "screen.shot", "marker", "system.state", "coverage.gap"];
/// Rows read before the requested window so an activity that began a little earlier keeps its start.
const LEAD_MS: i64 = 6 * 3_600_000;
const MAX_LIMIT: i64 = 1000;

fn record_kind(event_kind: &str) -> &'static str {
    match event_kind {
        "app.focus" => "application",
        "window.focus" => "window",
        "screen.text" => "context",
        "screen.shot" => "screenshot",
        "marker" => "marker",
        "system.state" => "system",
        _ => "gap",
    }
}

/// The text and state a desktop observation carried (the ledger's `DesktopContextSnapshot`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_evidence: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readable_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_protected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_battery: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_power: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_composing: Option<bool>,
}

/// One desktop record as the App draws it. `id` is the capture's own record ID (the event's `source_key`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppRecord {
    pub id: String,
    pub event_id: String,
    pub ts_ms: i64,
    pub kind: String,
    pub app_name: String,
    pub bundle_id: String,
    pub window_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_notes: Option<String>,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_height: Option<i64>,
    /// Absolute path of the original screenshot (internal client only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    /// Absolute path of the screenshot's AX evidence JSON, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax_evidence_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<AppContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
}

/// An activity: its anchor (first observation), latest observation, extent and, for a detail read, one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppActivity {
    pub anchor: AppRecord,
    pub latest: AppRecord,
    pub start_ms: i64,
    pub end_ms: i64,
    pub sample_count: i64,
    pub has_screenshot: bool,
    pub has_issue: bool,
    /// The observation page (summaries carry only the latest observation).
    pub observations: Vec<AppRecord>,
    pub observation_offset: i64,
    pub screenshots: Vec<AppRecord>,
    pub issues: Vec<AppRecord>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStats {
    pub record_count: i64,
    pub screenshot_count: i64,
    pub storage_bytes: i64,
    /// Screenshot originals stepped down to text only to stay within the storage limit, in total.
    #[serde(default)]
    pub freed_bytes: i64,
    #[serde(default)]
    pub freed_screenshots: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppActivitiesRequest {
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppActivitiesResponse {
    /// Newest end first.
    pub activities: Vec<AppActivity>,
    pub stats: AppStats,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppActivityRequest {
    /// The anchor's record ID.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppActivityResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<AppActivity>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppMediaRequest {
    pub sha256: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppMediaResponse {
    /// sha256 → absolute path, for originals that exist on disk.
    pub paths: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppRecognitionRequest {
    pub screenshot_id: String,
}

/// One recognition run on a screenshot (`image.text`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppRecognition {
    pub event_id: String,
    pub source_key: String,
    /// `ocr` or `vlm`.
    pub method: String,
    pub model: String,
    pub text: String,
    pub completed_ms: i64,
    /// The App's full result JSON (line boxes, layout), when the run kept one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppRecognitionResponse {
    pub screenshot_event_id: String,
    pub image_hash: String,
    /// The screenshot's original and AX evidence, for the recognizer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax_evidence_path: Option<String>,
    /// Newest run per method.
    pub results: Vec<AppRecognition>,
    /// Earlier runs that a newer one replaced but that still exist (to be superseded).
    pub older: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppPendingRequest {
    /// `ocr` or `vlm`.
    pub method: String,
    pub since_ms: i64,
    #[serde(default)]
    pub skip: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppPendingResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<AppRecord>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppOcrRequest {
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppOcrResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AppRecognition>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppCountRequest {
    pub before_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppCountResponse {
    pub records: i64,
}

/// Day boundaries chosen by the App (its local days, so daylight-saving changes stay correct): `[start_ms, end_ms)`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppDayCountsRequest {
    pub days: Vec<[i64; 2]>,
}

/// How much was recorded on one requested day. Counts only; nothing about content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppDayCount {
    pub start_ms: i64,
    /// Distinct minutes with a screen record (focus, window, text or screenshot).
    pub screen_minutes: i64,
    /// Recordings and images.
    pub media: i64,
    /// AI instructions and notes.
    pub ai: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppDayCountsResponse {
    /// Only days with something recorded, in request order.
    pub days: Vec<AppDayCount>,
}

/// Most days one call may ask about (a little over a year).
pub const MAX_COUNT_DAYS: usize = 400;
const SCREEN_KINDS: &[&str] = &["app.focus", "window.focus", "screen.text", "screen.shot"];
const MEDIA_KINDS: &[&str] = &["audio.recording", "image.capture"];
const AI_KINDS: &[&str] = &["ai.prompt", "note"];

/// One loaded desktop event before it becomes an [`AppRecord`].
#[derive(Clone)]
struct Row {
    event_id: String,
    key: String,
    kind: &'static str,
    ts: i64,
    /// End of a span, e.g. a text event that absorbed later samples while someone typed; else `ts`.
    end: i64,
    bundle: String,
    app: String,
    title: String,
    parent_event: Option<String>,
    protected: bool,
    payload: Value,
}

impl Row {
    fn legacy(&self, key: &str) -> Option<String> {
        self.payload.get("legacy").and_then(|l| l.get(key)).and_then(|v| v.as_str()).map(String::from)
    }
    fn run(&self) -> String {
        self.legacy("run_id").unwrap_or_default()
    }
    fn trigger(&self) -> Option<String> {
        let raw = match self.kind {
            "application" => json::str_at(&self.payload, "reason"),
            "context" | "screenshot" => json::str_at(&self.payload, "trigger"),
            _ => None,
        };
        raw.filter(|t| *t != "unknown").map(String::from)
    }
    fn is_attachment_kind(&self) -> bool {
        self.kind == "screenshot" || self.kind == "gap"
    }
    fn legacy_candidate(&self) -> bool {
        if self.legacy("activity_id").is_some() || self.run().is_empty() || self.bundle.is_empty() || self.title.trim().is_empty() {
            return false;
        }
        self.kind == "window" || (self.kind == "context" && matches!(self.trigger().as_deref(), Some("timer" | "content" | "selection" | "focus")))
    }
}

/// A screenshot's original `(sha256, path)` and its AX evidence path.
type ShotFiles = (Option<(String, String)>, Option<String>);

/// An activity being assembled: indices into the loaded rows.
#[derive(Default)]
struct Group {
    observations: Vec<usize>,
    attachments: Vec<usize>,
}

fn order(rows: &[Row], a: usize, b: usize) -> std::cmp::Ordering {
    rows[a].ts.cmp(&rows[b].ts).then_with(|| rows[a].key.cmp(&rows[b].key))
}

/// The latest end among a group's observations.
fn group_end(rows: &[Row], g: &Group) -> i64 {
    g.observations.iter().map(|&i| rows[i].end).max().unwrap_or(i64::MIN)
}

/// Assigns every row to an activity; returns the groups in first-observation order. Attachments whose parent is not
/// among `rows` stand alone, as the ledger kept them visible.
fn group_rows(rows: &[Row]) -> Vec<Group> {
    let by_event: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.event_id.as_str(), i)).collect();
    let mut group_of: Vec<Option<usize>> = vec![None; rows.len()];
    let mut groups: Vec<Group> = Vec::new();
    let mut explicit: HashMap<(String, String), usize> = HashMap::new();
    let mut previous: Option<usize> = None; // last root observation, for the legacy chain
    let parent_of = |i: usize| -> Option<usize> {
        let r = &rows[i];
        let p = by_event.get(r.parent_event.as_deref()?).copied()?;
        (p != i && !rows[p].is_attachment_kind() && rows[p].run() == r.run()).then_some(p)
    };
    // Roots first, in time order; then attachments join their parent's group.
    for i in 0..rows.len() {
        let r = &rows[i];
        if r.is_attachment_kind() && parent_of(i).is_some() {
            continue;
        }
        let activity = (!r.is_attachment_kind()).then(|| r.legacy("activity_id")).flatten();
        let g = if let Some(a) = activity {
            *explicit.entry((r.run(), a)).or_insert_with(|| {
                groups.push(Group::default());
                groups.len() - 1
            })
        } else if let Some(p) = previous.filter(|&p| {
            r.legacy_candidate() && rows[p].legacy_candidate() && rows[p].run() == r.run() && rows[p].bundle == r.bundle && rows[p].title == r.title
        }) {
            group_of[p].unwrap_or(0)
        } else {
            groups.push(Group::default());
            groups.len() - 1
        };
        group_of[i] = Some(g);
        groups[g].observations.push(i);
        previous = Some(i);
    }
    for i in 0..rows.len() {
        if group_of[i].is_some() {
            continue;
        }
        if let Some(g) = parent_of(i).and_then(|p| group_of[p]) {
            group_of[i] = Some(g);
            groups[g].attachments.push(i);
        }
    }
    for g in &mut groups {
        g.observations.sort_by(|&a, &b| order(rows, a, b));
        g.attachments.sort_by(|&a, &b| order(rows, a, b));
    }
    groups
}

impl TimelineStore {
    /// Refuses every client but the App's internal one.
    fn require_internal(&self, client_id: &str) -> QResult<()> {
        let transport = self.db.first("SELECT transport FROM clients WHERE client_id=?", &[text(client_id)], |r| r.text(0))?;
        if transport.as_deref() != Some("internal") {
            return Err(TimelineError::Denied("internal_only".into()).into());
        }
        Ok(())
    }

    /// Source IDs of the Mac desktop capture.
    pub(crate) fn desktop_sources(&self) -> Result<Vec<String>> {
        self.db.query("SELECT source_id FROM sources WHERE kind='mac.desktop' AND status<>'revoked' ORDER BY source_id", &[], |r| r.text(0))
    }

    fn load_rows(&self, sources: &[String], extra: &str, params: Vec<P>) -> Result<Vec<Row>> {
        if sources.is_empty() {
            return Ok(vec![]);
        }
        let mut p: Vec<P> = sources.iter().map(text).collect();
        p.extend(DESKTOP_KINDS.iter().map(|k| text(*k)));
        p.extend(params);
        self.db.query(
            &format!(
                "SELECT e.event_id,e.source_key,e.kind,e.start_ms,e.app_bundle_id,e.app_name,e.title,e.parent_event_id,e.sensitivity,CASE WHEN (SELECT json_extract(policy_json,'$.store_text') FROM sources WHERE source_id=e.source_id)=1 THEN e.payload_json ELSE json_remove(e.payload_json,'$.record_evidence') END,COALESCE(e.end_ms,e.start_ms)
                FROM events e WHERE e.source_id IN ({}) AND e.kind IN ({}) AND e.start_ms IS NOT NULL AND COALESCE(json_extract(e.payload_json,'$.record_evidence.confirmation.basis'),'')<>'source_send' {extra}
                ORDER BY e.start_ms, e.source_key",
                placeholders(sources.len()),
                placeholders(DESKTOP_KINDS.len())
            ),
            &p,
            |r| Row {
                event_id: r.text(0),
                key: r.text(1),
                kind: record_kind(&r.text(2)),
                ts: r.int(3),
                end: r.int(10),
                bundle: r.opt_text(4).unwrap_or_default(),
                app: r.opt_text(5).unwrap_or_default(),
                title: r.opt_text(6).unwrap_or_default(),
                parent_event: r.opt_text(7),
                protected: r.text(8) == "protected",
                payload: json::parse(&r.text(9)).unwrap_or(Value::Null),
            },
        )
    }

    /// Text per event: `body` and `selection`, paragraphs joined by a blank line.
    fn texts_for(&self, ids: &[&str]) -> Result<HashMap<String, (String, String)>> {
        let mut out: HashMap<String, (String, String)> = HashMap::new();
        for chunk in ids.chunks(400) {
            let rows = self.db.query(
                &format!(
                    "SELECT t.event_id,t.field,c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id
                    WHERE t.event_id IN ({}) AND EXISTS (SELECT 1 FROM events e JOIN sources s ON s.source_id=e.source_id WHERE e.event_id=t.event_id AND s.status<>'revoked' AND json_extract(s.policy_json,'$.store_text')=1) AND t.field IN ('body','selection') ORDER BY t.event_id,t.field,t.ord",
                    placeholders(chunk.len())
                ),
                &chunk.iter().map(|s| text(*s)).collect::<Vec<_>>(),
                |r| (r.text(0), r.text(1), r.text(2)),
            )?;
            for (id, field, t) in rows {
                let entry = out.entry(id).or_default();
                let target = if field == "body" { &mut entry.0 } else { &mut entry.1 };
                if !target.is_empty() {
                    target.push_str("\n\n");
                }
                target.push_str(&t);
            }
        }
        Ok(out)
    }

    /// Original (and AX evidence) paths per screenshot event.
    fn blobs_for(&self, ids: &[&str]) -> Result<HashMap<String, ShotFiles>> {
        let mut out: HashMap<String, ShotFiles> = HashMap::new();
        for chunk in ids.chunks(400) {
            let rows = self.db.query(
                &format!(
                    "SELECT eb.event_id,eb.role,b.sha256,b.rel_path,b.mime_type FROM event_blobs eb JOIN blobs b ON b.sha256=eb.sha256
                    WHERE eb.event_id IN ({})",
                    placeholders(chunk.len())
                ),
                &chunk.iter().map(|s| text(*s)).collect::<Vec<_>>(),
                |r| (r.text(0), r.text(1), r.text(2), r.text(3), r.text(4)),
            )?;
            for (id, role, sha, rel, mime) in rows {
                let entry = out.entry(id).or_default();
                let path = self.root.join(&rel).display().to_string();
                if role == "original" {
                    entry.0 = Some((sha, path));
                } else if role == "derived" && mime == "application/json" {
                    entry.1 = Some(path);
                }
            }
        }
        Ok(out)
    }

    fn context_of(row: &Row, texts: &HashMap<String, (String, String)>) -> Option<AppContext> {
        if row.kind != "context" {
            return None;
        }
        let p = &row.payload;
        let b = |k: &str| p.get(k).and_then(|v| v.as_bool());
        let s = |k: &str| json::str_at(p, k).map(String::from);
        let (body, selection) = texts.get(&row.event_id).cloned().unwrap_or_default();
        Some(AppContext {
            record_evidence: p.get("record_evidence").filter(|v|v.is_object()).cloned(),
            focused_role: s("focused_role"),
            focused_label: s("focused_label"),
            selected_text: (!selection.is_empty()).then_some(selection),
            readable_text: (!body.is_empty()).then_some(body),
            text_status: s("text_status"),
            is_protected: row.protected.then_some(true),
            session_active: b("session_active"),
            on_battery: b("on_battery"),
            low_power: b("low_power"),
            thermal_state: s("thermal_state"),
            idle_seconds: p.get("idle_seconds").and_then(|v| v.as_f64()),
            is_composing: b("is_composing"),
        })
    }

    /// Builds records for `indices`, with text, image paths and, for screenshots, the parent's context.
    fn records(&self, rows: &[Row], indices: &[usize], ctx: &ClientContext) -> Result<HashMap<usize, AppRecord>> {
        let by_event: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.event_id.as_str(), i)).collect();
        let mut wanted: Vec<usize> = indices.to_vec();
        for &i in indices {
            if rows[i].kind == "screenshot" {
                if let Some(p) = rows[i].parent_event.as_deref().and_then(|p| by_event.get(p)) {
                    wanted.push(*p);
                }
            }
        }
        wanted.sort_unstable();
        wanted.dedup();
        let ids: Vec<&str> = wanted.iter().map(|&i| rows[i].event_id.as_str()).collect();
        let texts = self.texts_for(&ids)?;
        let shots: Vec<&str> = wanted.iter().filter(|&&i| rows[i].kind == "screenshot").map(|&i| rows[i].event_id.as_str()).collect();
        let blobs = self.blobs_for(&shots)?;
        // Parents outside the loaded rows still give their record ID.
        let unresolved: Vec<&str> =
            wanted.iter().filter_map(|&i| rows[i].parent_event.as_deref()).filter(|p| !by_event.contains_key(p)).collect();
        let mut parent_keys: HashMap<String, String> = HashMap::new();
        for chunk in unresolved.chunks(400) {
            for (id, key) in self.db.query(
                &format!("SELECT event_id,source_key FROM events WHERE event_id IN ({})", placeholders(chunk.len())),
                &chunk.iter().map(|s| text(*s)).collect::<Vec<_>>(),
                |r| (r.text(0), r.text(1)),
            )? {
                parent_keys.insert(id, key);
            }
        }
        let mut out = HashMap::new();
        for &i in &wanted {
            let r = &rows[i];
            let num = |k: &str| r.payload.get(k).and_then(|v| v.as_i64());
            let mut record = AppRecord {
                id: r.key.clone(),
                event_id: r.event_id.clone(),
                ts_ms: r.ts,
                kind: r.kind.into(),
                app_name: r.app.clone(),
                bundle_id: r.bundle.clone(),
                window_title: r.title.clone(),
                detail: r.legacy("detail").unwrap_or_default(),
                parent_id: r.parent_event.as_deref().and_then(|p| by_event.get(p).map(|&p| rows[p].key.clone()).or_else(|| parent_keys.get(p).cloned())),
                run_id: r.run(),
                trigger: r.trigger(),
                context: Self::context_of(r, &texts),
                activity_id: r.legacy("activity_id"),
                ..Default::default()
            };
            if let Some(v)=self.visible_annotation(&r.event_id,ctx)? {
                record.manual_title=v["title"].as_str().map(String::from);
                record.manual_notes=v["notes"].as_str().map(String::from);
            }
            if r.kind == "marker" {
                // The ledger kept a marker's label as its title and its note as detail.
                if let Some(label) = json::str_at(&r.payload, "label") {
                    record.window_title = label.into();
                }
                if record.detail.is_empty() {
                    record.detail = json::str_at(&r.payload, "note").unwrap_or_default().into();
                }
            }
            if r.kind == "screenshot" {
                record.image_width = num("pixel_w");
                record.image_height = num("pixel_h");
                if let Some((original, evidence)) = blobs.get(&r.event_id) {
                    if let Some((sha, path)) = original {
                        record.image_hash = Some(sha.clone());
                        record.image_path = Some(path.clone());
                    }
                    record.ax_evidence_path = evidence.clone();
                }
                if let Some(&p) = r.parent_event.as_deref().and_then(|p| by_event.get(p)) {
                    record.context = Self::context_of(&rows[p], &texts);
                }
            }
            out.insert(i, record);
        }
        Ok(out)
    }

    fn matching_events(&self, sources: &[String], from: i64, to: i64, q: &str) -> Result<HashSet<String>> {
        let mut p: Vec<P> = sources.iter().map(text).collect();
        p.extend([int(from), int(to), text(q)]);
        Ok(self
            .db
            .query(
                &format!(
                    "SELECT DISTINCT e.event_id FROM events e JOIN event_text t ON t.event_id=e.event_id JOIN text_chunks c ON c.chunk_id=t.chunk_id
                    WHERE e.source_id IN ({}) AND e.start_ms>=? AND e.start_ms<? AND instr(lower(c.text),lower(?))>0",
                    placeholders(sources.len())
                ),
                &p,
                |r| r.text(0),
            )?
            .into_iter()
            .collect())
    }

    fn activity_summary(&self, rows: &[Row], g: &Group, records: &HashMap<usize, AppRecord>) -> Option<AppActivity> {
        let first = *g.observations.first()?;
        let last = *g.observations.last()?;
        let latest = records.get(&last)?.clone();
        Some(AppActivity {
            anchor: records.get(&first)?.clone(),
            latest: latest.clone(),
            start_ms: rows[first].ts,
            end_ms: group_end(rows, g),
            sample_count: g.observations.len() as i64,
            has_screenshot: g.observations.iter().chain(&g.attachments).any(|&i| rows[i].kind == "screenshot"),
            has_issue: g.observations.iter().chain(&g.attachments).any(|&i| rows[i].kind == "gap"),
            observations: vec![latest],
            observation_offset: g.observations.len() as i64 - 1,
            screenshots: vec![],
            issues: vec![],
        })
    }

    pub fn app_stats(&self) -> Result<AppStats> {
        let sources = self.desktop_sources()?;
        self.refresh_desktop_read_cache(&sources)?;
        if let Some(stats) = &self.desktop_read_cache.borrow().stats { return Ok(stats.clone()); }
        if sources.is_empty() {
            return Ok(AppStats::default());
        }
        let ph = placeholders(sources.len());
        let p: Vec<P> = sources.iter().map(text).collect();
        let mut kinds = p.clone();
        kinds.extend(DESKTOP_KINDS.iter().map(|k| text(*k)));
        let (freed_bytes, freed_screenshots) = self.screenshot_space_freed()?;
        let stats = AppStats {
            freed_bytes,
            freed_screenshots,
            record_count: self.db.scalar(&format!("SELECT COUNT(*) FROM events WHERE source_id IN ({ph}) AND kind IN ({})", placeholders(DESKTOP_KINDS.len())), &kinds)?,
            screenshot_count: self.db.scalar(&format!("SELECT COUNT(*) FROM events WHERE source_id IN ({ph}) AND kind='screen.shot'"), &p)?,
            storage_bytes: self.db.scalar(
                &format!(
                    "SELECT COALESCE(SUM(b.byte_count),0) FROM blobs b WHERE b.sha256 IN
                    (SELECT eb.sha256 FROM event_blobs eb JOIN events e ON e.event_id=eb.event_id WHERE e.source_id IN ({ph}))"
                ),
                &p,
            )?,
        };
        self.desktop_read_cache.borrow_mut().stats = Some(stats.clone());
        Ok(stats)
    }

    /// Activities that overlap `[start_ms, end_ms)`, newest end first.
    pub fn app_activities(&self, req: &AppActivitiesRequest, client_id: &str) -> Result<AppActivitiesResponse> {
        self.audited("app_activities", client_id, req, |r: &AppActivitiesResponse| Some(r.activities.len() as i64), |ctx| {
            self.require_internal(client_id)?;
            let limit = req.limit.unwrap_or(500);
            if !(1..=MAX_LIMIT).contains(&limit) || req.end_ms <= req.start_ms || req.q.as_deref().map(|q| q.len() > 512).unwrap_or(false) {
                return Err(TimelineError::InvalidInput("app_activities 参数".into()).into());
            }
            let sources = self.desktop_sources()?;
            self.refresh_desktop_read_cache(&sources)?;
            let cache_key = serde_json::to_string(&(client_id, &ctx.scope, &ctx.allowed_sources, req)).map_err(TimelineError::from)?;
            if let Some((key, response)) = &self.desktop_read_cache.borrow().activities {
                if key == &cache_key { return Ok(response.clone()); }
            }
            let rows = self.load_rows(&sources, "AND e.start_ms>=? AND e.start_ms<?", vec![int(req.start_ms - LEAD_MS), int(req.end_ms)])?;
            let groups = group_rows(&rows);
            let q = req.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(str::to_lowercase);
            let text_hits = match &q {
                Some(q) => self.matching_events(&sources, req.start_ms - LEAD_MS, req.end_ms, q)?,
                None => HashSet::new(),
            };
            let matches = |i: usize| -> bool {
                let Some(q) = &q else { return true };
                let r = &rows[i];
                if text_hits.contains(&r.event_id) {
                    return true;
                }
                let mut fields = vec![r.app.clone(), r.bundle.clone(), r.title.clone(), r.legacy("detail").unwrap_or_default(), r.trigger().unwrap_or_default()];
                if let Ok(Some(v))=self.visible_annotation(&r.event_id,ctx) {fields.push(v["title"].as_str().unwrap_or("").into());fields.push(v["notes"].as_str().unwrap_or("").into());}
                for k in ["focused_role", "focused_label", "text_status", "label", "note"] {
                    fields.push(json::str_at(&r.payload, k).unwrap_or_default().into());
                }
                fields.iter().any(|f| f.to_lowercase().contains(q.as_str()))
            };
            let mut chosen: Vec<&Group> = groups
                .iter()
                .filter(|g| {
                    let Some(&first) = g.observations.first() else { return false };
                    rows[first].ts < req.end_ms && group_end(&rows, g) >= req.start_ms && g.observations.iter().chain(&g.attachments).any(|&i| matches(i))
                })
                .collect();
            chosen.sort_by(|a, b| {
                group_end(&rows, b).cmp(&group_end(&rows, a)).then_with(|| rows[b.observations[0]].key.cmp(&rows[a.observations[0]].key))
            });
            chosen.truncate(limit as usize);
            let wanted: Vec<usize> = chosen.iter().flat_map(|g| [g.observations[0], *g.observations.last().unwrap_or(&g.observations[0])]).collect();
            let records = self.records(&rows, &wanted,ctx)?;
            let response = AppActivitiesResponse { activities: chosen.iter().filter_map(|g| self.activity_summary(&rows, g, &records)).collect(), stats: self.app_stats()? };
            self.desktop_read_cache.borrow_mut().activities = Some((cache_key, response.clone()));
            Ok(response)
        })
    }

    /// Loads the rows of the activity anchored at record `id`: its explicit `(run, activity)` members, or the legacy
    /// chain that starts at it, plus their screenshots and gaps.
    fn activity_rows(&self, id: &str) -> Result<Option<(Vec<Row>, usize)>> {
        let sources = self.desktop_sources()?;
        if sources.is_empty() {
            return Ok(None);
        }
        let mut sp: Vec<P> = sources.iter().map(text).collect();
        sp.push(text(id));
        let Some((anchor_event, start)) = self.db.first(
            &format!("SELECT event_id,start_ms FROM events WHERE source_id IN ({}) AND source_key=? AND start_ms IS NOT NULL", placeholders(sources.len())),
            &sp,
            |r| (r.text(0), r.int(1)),
        )?
        else {
            return Ok(None);
        };
        let anchor = self.load_rows(&sources, "AND e.event_id=?", vec![text(&anchor_event)])?.pop();
        let Some(anchor) = anchor else { return Ok(None) };
        let mut rows = match (anchor.is_attachment_kind(), anchor.legacy("activity_id")) {
            (false, Some(activity)) => self.load_rows(
                &sources,
                "AND e.start_ms>=? AND json_extract(e.payload_json,'$.legacy.activity_id')=? AND json_extract(e.payload_json,'$.legacy.run_id')=?",
                vec![int(start), text(activity), text(anchor.run())],
            )?,
            // A legacy chain or a single record: everything after the anchor until the chain breaks (bounded to a day).
            _ => self.load_rows(&sources, "AND e.start_ms>=? AND e.start_ms<?", vec![int(start), int(start + 86_400_000)])?,
        };
        // Screenshots and gaps of these observations (they may lie outside the time bound).
        let known: HashSet<String> = rows.iter().map(|r| r.event_id.clone()).collect();
        let ids: Vec<String> = rows.iter().filter(|r| !r.is_attachment_kind()).map(|r| r.event_id.clone()).collect();
        for chunk in ids.chunks(400) {
            let extra = format!("AND e.parent_event_id IN ({})", placeholders(chunk.len()));
            for r in self.load_rows(&sources, &extra, chunk.iter().map(text).collect())? {
                if !known.contains(&r.event_id) {
                    rows.push(r);
                }
            }
        }
        rows.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.key.cmp(&b.key)));
        let groups = group_rows(&rows);
        let Some(pos) = rows.iter().position(|r| r.event_id == anchor_event) else { return Ok(None) };
        let Some(g) = groups.into_iter().position(|g| g.observations.first() == Some(&pos)) else { return Ok(None) };
        Ok(Some((rows, g)))
    }

    /// One activity with a page of observations (default: the newest `limit`), their screenshots and gaps.
    pub fn app_activity(&self, req: &AppActivityRequest, client_id: &str) -> Result<AppActivityResponse> {
        self.audited("app_activity", client_id, req, |r: &AppActivityResponse| Some(r.activity.is_some() as i64), |ctx| {
            self.require_internal(client_id)?;
            let limit = req.limit.unwrap_or(120);
            let cursors = [&req.before, &req.after, &req.around].iter().filter(|c| c.is_some()).count();
            if !(1..=10_000).contains(&limit) || cursors > 1 {
                return Err(TimelineError::InvalidInput("app_activity 参数".into()).into());
            }
            let sources = self.desktop_sources()?;
            self.refresh_desktop_read_cache(&sources)?;
            let cache_key = serde_json::to_string(&(client_id, &ctx.scope, &ctx.allowed_sources, req)).map_err(TimelineError::from)?;
            if let Some((key, response)) = &self.desktop_read_cache.borrow().detail {
                if key == &cache_key { return Ok(response.clone()); }
            }
            let Some((rows, gi)) = self.activity_rows(&req.id)? else { return Ok(AppActivityResponse { activity: None }) };
            let groups = group_rows(&rows);
            let g = &groups[gi];
            let total = g.observations.len() as i64;
            let position = |key: &str| g.observations.iter().position(|&i| rows[i].key == key).map(|p| p as i64);
            let (offset, count) = if let Some(b) = &req.before {
                let p = position(b).ok_or_else(|| QErr::Err(TimelineError::InvalidInput("before 不属于该活动".into())))?;
                let o = (p - limit).max(0);
                (o, p - o)
            } else if let Some(a) = &req.after {
                let p = position(a).ok_or_else(|| QErr::Err(TimelineError::InvalidInput("after 不属于该活动".into())))?;
                (p + 1, limit.min(total - p - 1))
            } else if let Some(c) = &req.around {
                let p = position(c).ok_or_else(|| QErr::Err(TimelineError::InvalidInput("around 不属于该活动".into())))?;
                let o = (p - limit / 2).min((total - limit).max(0)).max(0);
                (o, limit.min(total - o))
            } else {
                let o = (total - limit).max(0);
                (o, total - o)
            };
            let page: Vec<usize> = g.observations[offset as usize..(offset + count.max(0)) as usize].to_vec();
            let page_events: HashSet<&str> = page.iter().map(|&i| rows[i].event_id.as_str()).collect();
            let attached: Vec<usize> = g.attachments.iter().copied().filter(|&i| rows[i].parent_event.as_deref().is_some_and(|p| page_events.contains(p))).collect();
            let first = g.observations[0];
            let last = *g.observations.last().unwrap_or(&first);
            let mut wanted = page.clone();
            wanted.extend(&attached);
            wanted.extend([first, last]);
            let records = self.records(&rows, &wanted,ctx)?;
            let take = |list: &[usize]| list.iter().filter_map(|i| records.get(i).cloned()).collect::<Vec<_>>();
            let shots: Vec<usize> = attached.iter().copied().filter(|&i| rows[i].kind == "screenshot").collect();
            let issues: Vec<usize> = attached.iter().copied().filter(|&i| rows[i].kind == "gap").collect();
            let activity = AppActivity {
                anchor: records.get(&first).cloned().unwrap_or_default(),
                latest: records.get(&last).cloned().unwrap_or_default(),
                start_ms: rows[first].ts,
                end_ms: group_end(&rows, g),
                sample_count: total,
                has_screenshot: g.observations.iter().chain(&g.attachments).any(|&i| rows[i].kind == "screenshot"),
                has_issue: g.observations.iter().chain(&g.attachments).any(|&i| rows[i].kind == "gap"),
                observations: take(&page),
                observation_offset: offset,
                screenshots: take(&shots),
                issues: take(&issues),
            };
            let response = AppActivityResponse { activity: Some(activity) };
            // Full exports may contain 10,000 observations; only retain the normal on-screen page.
            if limit <= 120 { self.desktop_read_cache.borrow_mut().detail = Some((cache_key, response.clone())); }
            Ok(response)
        })
    }

    /// Paths of originals by hash (any source; the App asks only for hashes it was shown).
    pub fn app_media(&self, req: &AppMediaRequest, client_id: &str) -> Result<AppMediaResponse> {
        self.audited("app_media", client_id, req, |r: &AppMediaResponse| Some(r.paths.len() as i64), |_| {
            self.require_internal(client_id)?;
            if req.sha256.len() > 500 {
                return Err(TimelineError::InvalidInput("最多 500 个".into()).into());
            }
            let mut paths = BTreeMap::new();
            for sha in &req.sha256 {
                if let Some(rel) = self.db.first("SELECT rel_path FROM blobs WHERE sha256=?", &[text(sha)], |r| r.text(0))? {
                    let path = self.root.join(rel);
                    if path.exists() {
                        paths.insert(sha.clone(), path.display().to_string());
                    }
                }
            }
            Ok(AppMediaResponse { paths })
        })
    }

    fn recognition_rows(&self, extra: &str, params: Vec<P>) -> Result<Vec<AppRecognition>> {
        let rows = self.db.query(
            &format!(
                "SELECT e.event_id,e.source_key,e.payload_json,
                   (SELECT b.rel_path FROM event_blobs eb JOIN blobs b ON b.sha256=eb.sha256 WHERE eb.event_id=e.event_id AND eb.role='derived' LIMIT 1),
                   (SELECT group_concat(c.text, char(10)||char(10)) FROM (SELECT c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id
                      WHERE t.event_id=e.event_id ORDER BY t.field,t.ord) c)
                FROM events e WHERE e.kind='image.text' {extra}"
            ),
            &params,
            |r| (r.text(0), r.text(1), r.text(2), r.opt_text(3), r.opt_text(4)),
        )?;
        Ok(rows
            .into_iter()
            .map(|(event_id, source_key, payload, rel, body)| {
                let p = json::parse(&payload).unwrap_or(Value::Null);
                AppRecognition {
                    event_id,
                    source_key,
                    method: json::str_at(&p, "method").unwrap_or("vlm").into(),
                    model: json::str_at(&p, "model_id").unwrap_or_default().into(),
                    text: body.unwrap_or_default(),
                    completed_ms: p.get("completed_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                    result_path: rel.map(|r| self.root.join(r).display().to_string()),
                }
            })
            .collect())
    }

    fn screenshot_event(&self, record_id: &str) -> Result<Option<(String, String)>> {
        let sources = self.desktop_sources()?;
        if sources.is_empty() {
            return Ok(None);
        }
        let mut p: Vec<P> = sources.iter().map(text).collect();
        p.push(text(record_id));
        self.db.first(
            &format!(
                "SELECT e.event_id,eb.sha256 FROM events e JOIN event_blobs eb ON eb.event_id=e.event_id AND eb.role='original'
                WHERE e.source_id IN ({}) AND e.source_key=? AND e.kind='screen.shot'",
                placeholders(sources.len())
            ),
            &p,
            |r| (r.text(0), r.text(1)),
        )
    }

    /// Recognition runs on one screenshot: the newest per method, and the older ones still present.
    pub fn app_recognition(&self, req: &AppRecognitionRequest, client_id: &str) -> Result<AppRecognitionResponse> {
        self.audited("app_recognition", client_id, req, |r: &AppRecognitionResponse| Some(r.results.len() as i64), |_| {
            self.require_internal(client_id)?;
            let Some((event, sha)) = self.screenshot_event(&req.screenshot_id)? else {
                return Err(TimelineError::NotFound(req.screenshot_id.clone()).into());
            };
            let mut runs = self.recognition_rows("AND e.parent_event_id=?", vec![text(&event)])?;
            runs.sort_by(|a, b| b.completed_ms.cmp(&a.completed_ms).then_with(|| b.source_key.cmp(&a.source_key)));
            let mut seen = HashSet::new();
            let (mut results, mut older) = (Vec::new(), Vec::new());
            for r in runs {
                if seen.insert(r.method.clone()) {
                    results.push(r);
                } else {
                    older.push(r.event_id);
                }
            }
            let files = self.blobs_for(&[event.as_str()])?.remove(&event).unwrap_or_default();
            Ok(AppRecognitionResponse {
                image_path: files.0.map(|(_, p)| p),
                ax_evidence_path: files.1,
                screenshot_event_id: event,
                image_hash: sha,
                results,
                older,
            })
        })
    }

    /// The earliest desktop screenshot since `since_ms` without a run of `method`, skipping `skip`.
    pub fn app_pending_recognition(&self, req: &AppPendingRequest, client_id: &str) -> Result<AppPendingResponse> {
        self.audited("app_pending_recognition", client_id, req, |r: &AppPendingResponse| Some(r.screenshot.is_some() as i64), |ctx| {
            self.require_internal(client_id)?;
            if req.method != "ocr" && req.method != "vlm" || req.skip.len() > 1000 {
                return Err(TimelineError::InvalidInput("method 只能是 ocr 或 vlm".into()).into());
            }
            let sources = self.desktop_sources()?;
            if sources.is_empty() {
                return Ok(AppPendingResponse { screenshot: None });
            }
            let mut p: Vec<P> = sources.iter().map(text).collect();
            p.extend([int(req.since_ms), text(&req.method)]);
            p.extend(req.skip.iter().map(text));
            let skip = if req.skip.is_empty() { String::new() } else { format!("AND e.source_key NOT IN ({})", placeholders(req.skip.len())) };
            let found = self.db.first(
                &format!(
                    "SELECT e.event_id FROM events e WHERE e.source_id IN ({}) AND e.kind='screen.shot' AND e.start_ms>=?
                    AND EXISTS (SELECT 1 FROM event_blobs eb WHERE eb.event_id=e.event_id AND eb.role='original')
                    AND NOT EXISTS (SELECT 1 FROM events r WHERE r.parent_event_id=e.event_id AND r.kind='image.text' AND json_extract(r.payload_json,'$.method')=?)
                    {skip} ORDER BY e.start_ms, e.source_key LIMIT 1",
                    placeholders(sources.len())
                ),
                &p,
                |r| r.text(0),
            )?;
            let Some(event) = found else { return Ok(AppPendingResponse { screenshot: None }) };
            Ok(AppPendingResponse { screenshot: self.single_record(&sources, &event,ctx)? })
        })
    }

    /// One record with its parent loaded, so a screenshot carries the observation's context.
    fn single_record(&self, sources: &[String], event: &str, ctx: &ClientContext) -> Result<Option<AppRecord>> {
        let mut rows = self.load_rows(sources, "AND e.event_id=?", vec![text(event)])?;
        if rows.is_empty() {
            return Ok(None);
        }
        if let Some(parent) = rows[0].parent_event.clone() {
            rows.extend(self.load_rows(sources, "AND e.event_id=?", vec![text(parent)])?);
        }
        Ok(self.records(&rows, &[0],ctx)?.remove(&0))
    }

    /// The newest OCR run on any screenshot with this original, to reuse it for a repeated image.
    pub fn app_ocr(&self, req: &AppOcrRequest, client_id: &str) -> Result<AppOcrResponse> {
        self.audited("app_ocr", client_id, req, |r: &AppOcrResponse| Some(r.result.is_some() as i64), |_| {
            self.require_internal(client_id)?;
            let mut runs = self.recognition_rows(
                "AND json_extract(e.payload_json,'$.method')='ocr' AND e.parent_event_id IN
                 (SELECT eb.event_id FROM event_blobs eb WHERE eb.sha256=? AND eb.role='original')",
                vec![text(&req.sha256)],
            )?;
            runs.sort_by_key(|r| std::cmp::Reverse(r.completed_ms));
            Ok(AppOcrResponse { result: runs.into_iter().next() })
        })
    }

    /// How many desktop records lie before `before_ms` (what a retention change would delete).
    pub fn app_retention_count(&self, req: &AppCountRequest, client_id: &str) -> Result<AppCountResponse> {
        self.audited("app_retention_count", client_id, req, |_: &AppCountResponse| Some(1), |_| {
            self.require_internal(client_id)?;
            Ok(AppCountResponse { records: self.desktop_count_before(req.before_ms)? })
        })
    }

    /// Per-day amounts for the date picker: one aggregate query; each day is an index range on `events(start_ms)`.
    pub fn app_day_counts(&self, req: &AppDayCountsRequest, client_id: &str) -> Result<AppDayCountsResponse> {
        self.audited("app_day_counts", client_id, req, |r: &AppDayCountsResponse| Some(r.days.len() as i64), |_| {
            self.require_internal(client_id)?;
            if req.days.is_empty() || req.days.len() > MAX_COUNT_DAYS || req.days.iter().any(|[a, b]| b <= a || b - a > 2 * 86_400_000) {
                return Err(TimelineError::InvalidInput("app_day_counts 参数".into()).into());
            }
            let kinds: Vec<&str> = SCREEN_KINDS.iter().chain(MEDIA_KINDS).chain(AI_KINDS).copied().collect();
            let mut p: Vec<P> = Vec::new();
            for (i, [a, b]) in req.days.iter().enumerate() {
                p.extend([int(i as i64), int(*a), int(*b)]);
            }
            p.extend(SCREEN_KINDS.iter().map(|k| text(*k)));
            p.extend(MEDIA_KINDS.iter().map(|k| text(*k)));
            p.extend(AI_KINDS.iter().map(|k| text(*k)));
            p.extend(kinds.iter().map(|k| text(*k)));
            let values = vec!["(?,?,?)"; req.days.len()].join(",");
            let rows = self.db.query(
                &format!(
                    "WITH d(i,a,b) AS (VALUES {values})
                     SELECT d.i,
                       COUNT(DISTINCT CASE WHEN e.kind IN ({screen}) THEN e.start_ms/60000 END),
                       COALESCE(SUM(e.kind IN ({media})),0),
                       COALESCE(SUM(e.kind IN ({ai})),0)
                     FROM d JOIN events e ON e.start_ms>=d.a AND e.start_ms<d.b
                     JOIN sources s ON s.source_id=e.source_id AND s.status<>'revoked'
                     WHERE e.kind IN ({all})
                     GROUP BY d.i ORDER BY d.i",
                    screen = placeholders(SCREEN_KINDS.len()),
                    media = placeholders(MEDIA_KINDS.len()),
                    ai = placeholders(AI_KINDS.len()),
                    all = placeholders(kinds.len()),
                ),
                &p,
                |r| (r.int(0) as usize, r.int(1), r.int(2), r.int(3)),
            )?;
            Ok(AppDayCountsResponse {
                days: rows
                    .into_iter()
                    .filter(|(_, minutes, media, ai)| minutes + media + ai > 0)
                    .map(|(i, screen_minutes, media, ai)| AppDayCount { start_ms: req.days[i][0], screen_minutes, media, ai })
                    .collect(),
            })
        })
    }

    pub(crate) fn desktop_count_before(&self, before_ms: i64) -> Result<i64> {
        let sources = self.desktop_sources()?;
        if sources.is_empty() {
            return Ok(0);
        }
        let mut p: Vec<P> = sources.iter().map(text).collect();
        p.extend(DESKTOP_KINDS.iter().map(|k| text(*k)));
        p.push(int(before_ms));
        self.db.scalar(
            &format!(
                "SELECT COUNT(*) FROM events WHERE source_id IN ({}) AND kind IN ({}) AND start_ms<?",
                placeholders(sources.len()),
                placeholders(DESKTOP_KINDS.len())
            ),
            &p,
        )
    }

    /// Deletes the desktop records before `before_ms` (with their screenshots' recognition), reason `retention`.
    pub fn delete_desktop_before(&self, before_ms: i64) -> Result<usize> {
        let sources = self.desktop_sources()?;
        if sources.is_empty() {
            return Ok(0);
        }
        let mut p: Vec<P> = sources.iter().map(text).collect();
        p.push(int(before_ms));
        let ids = self.db.query(
            &format!("SELECT event_id FROM events WHERE source_id IN ({}) AND start_ms<? AND kind<>'image.text'", placeholders(sources.len())),
            &p,
            |r| r.text(0),
        )?;
        let mut deleted = 0;
        for chunk in ids.chunks(500) {
            deleted += self.delete_events(chunk, "retention")?.events;
        }
        Ok(deleted)
    }

    /// Deletes the whole activity anchored at record `id` (observations, screenshots, gaps, recognition), reason `user`.
    pub fn delete_desktop_activity(&self, id: &str) -> Result<usize> {
        let Some((rows, gi)) = self.activity_rows(id)? else { return Ok(0) };
        let groups = group_rows(&rows);
        let g = &groups[gi];
        let ids: Vec<String> = g.observations.iter().chain(&g.attachments).map(|&i| rows[i].event_id.clone()).collect();
        Ok(self.delete_events(&ids, "user")?.events)
    }

    /// Event IDs of desktop records by their record IDs (missing ones are left out).
    pub fn desktop_event_ids(&self, keys: &[String]) -> Result<Vec<String>> {
        let sources = self.desktop_sources()?;
        let mut out = Vec::new();
        for s in &sources {
            for k in keys {
                if let Some(id) = self.event_id(s, k)? {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// Keeps the event IDs that belong to the desktop sources (the App deletes only its own capture).
    pub fn desktop_owned_events(&self, ids: &[String]) -> Result<Vec<String>> {
        let sources = self.desktop_sources()?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(source) = self.db.first("SELECT source_id FROM events WHERE event_id=?", &[text(id)], |r| r.text(0))? {
                if sources.contains(&source) {
                    out.push(id.clone());
                }
            }
        }
        Ok(out)
    }
}
