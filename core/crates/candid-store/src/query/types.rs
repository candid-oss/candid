//! Wire types for the read-only tools of docs/specs/timeline-v1.md §5.2. Field names are the spec's snake_case names
//! and absent optionals are omitted, matching the Swift encoder.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TimeWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverageGap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    pub reason_code: String,
    pub note: String,
}

/// §5.1 "缺口": separates "nothing was recorded" from "nothing matched".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub has_events: bool,
    pub recording_sources: i64,
    pub gaps: Vec<CoverageGap>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventSummary {
    pub event_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    pub time_basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub source_id: String,
    pub source_label: String,
    pub layer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub sensitivity: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    pub text: String,
    /// Scalar ranges `[start, end)` inside `text`.
    pub highlights: Vec<Vec<i64>>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attention {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_distance: Option<i64>,
    pub is_new: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRef {
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_ord: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_range: Option<Vec<i64>>,
}

// MARK: status

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusStore {
    pub store_id: String,
    pub schema_version: i64,
    pub db_bytes: i64,
    pub blob_bytes: i64,
    pub events: i64,
    pub ingest_seq: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusSource {
    pub source_id: String,
    pub kind: String,
    pub display_name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_seconds: Option<i64>,
    /// When this source last wrote new data (see `last_sync`, the last successful check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ingest: Option<String>,
    /// Finite error code while `status` is `error`; never content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusCapture {
    pub recording: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<String>,
    pub window_minutes: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusAgent {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub store: StatusStore,
    pub sources: Vec<StatusSource>,
    pub capture: StatusCapture,
    pub agent: StatusAgent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct StatusRequest {}

// MARK: overview

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct OverviewRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverviewGroup {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    pub events: i64,
    /// `group_by=app`: how many times the app came to the front (a return within five minutes counts as the same
    /// visit). `events` counts observations (screen text, focus records), not uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switches: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverviewItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverviewResponse {
    pub window: TimeWindow,
    pub group_by: String,
    pub total_events: i64,
    pub groups: Vec<OverviewGroup>,
    pub episodes: Vec<OverviewItem>,
    pub calendar: Vec<OverviewItem>,
    pub recordings: Vec<OverviewItem>,
    pub gaps: Vec<OverviewItem>,
    pub coverage: Coverage,
    pub truncated_by_size: bool,
}

// MARK: search

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SearchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub event_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub source_label: String,
    pub layer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Snippet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<Attention>,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<EventRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub window: TimeWindow,
    pub hits: Vec<SearchHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_new_records: bool,
    pub truncated_by_size: bool,
    /// True when the short-query fallback hit its 5000-paragraph scan bound.
    pub scan_limited: bool,
    pub attention: String,
    pub coverage: Coverage,
}

// MARK: timeline

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TimelineRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeSummary {
    pub episode_id: String,
    pub rev: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub start: String,
    pub end: String,
    pub kind: String,
    pub status: String,
    pub producer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_ref: Option<String>,
    pub evidence_count: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<EventSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<EpisodeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub window: TimeWindow,
    pub level: String,
    pub entries: Vec<TimelineEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_new_records: bool,
    pub truncated_by_size: bool,
    pub coverage: Coverage,
}

// MARK: context

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ContextRequest {
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paragraph {
    pub event_id: String,
    pub field: String,
    pub chunk_ord: i64,
    pub text: String,
    pub truncated: bool,
    pub attention: Attention,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub event_id: String,
    pub sha256: String,
    pub role: String,
    pub mime_type: String,
    pub byte_count: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextResponse {
    pub at: String,
    pub window: TimeWindow,
    pub events: Vec<EventSummary>,
    pub focus_paragraphs: Vec<Paragraph>,
    pub recordings: Vec<EventSummary>,
    pub calendar: Vec<EventSummary>,
    pub media_refs: Vec<MediaRef>,
    pub coverage: Coverage,
    pub truncated_by_size: bool,
}

// MARK: get_event

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct GetEventRequest {
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetEventResponse {
    pub event: EventSummary,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    pub paragraphs: Vec<Paragraph>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadInfo>,
    pub neighbors: Vec<EventSummary>,
    pub media_refs: Vec<MediaRef>,
    pub episodes: Vec<EpisodeSummary>,
    pub truncated_by_size: bool,
}

// MARK: episodes

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct EpisodesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodesResponse {
    pub window: TimeWindow,
    pub episodes: Vec<EpisodeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated_by_size: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct EpisodeRequest {
    pub episode_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeEvidence {
    pub event: EventSummary,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeResponse {
    pub episode: EpisodeSummary,
    pub evidence: Vec<EpisodeEvidence>,
    pub hidden_evidence: i64,
    pub truncated_by_size: bool,
}

// MARK: thread

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ThreadRequest {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub event_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub truncated: bool,
    pub source_label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadResponse {
    pub thread: ThreadInfo,
    pub messages: Vec<ThreadMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_new_records: bool,
    pub truncated_by_size: bool,
}

// MARK: media

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaRequest {
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<MediaRange>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaResponse {
    pub event_id: String,
    pub sha256: String,
    pub mime_type: String,
    pub byte_count: i64,
    pub variant_served: String,
    /// Inline bytes when they fit the client's size budget; otherwise `too_large`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    pub too_large: bool,
    pub range_applied: bool,
}

// MARK: usage

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_external: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageRow {
    /// "app" | "external" | "all"
    pub origin: String,
    pub key: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_actual: f64,
    pub cost_estimated: f64,
    pub cost_unknown_calls: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageResponse {
    pub window: TimeWindow,
    pub group_by: String,
    pub rows: Vec<UsageRow>,
    pub totals: UsageRow,
}

// MARK: propose

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposeRequest {
    pub kind: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposeResponse {
    pub proposal_id: String,
    pub status: String,
}
