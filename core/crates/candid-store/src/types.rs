//! Stored and ingested structures. Field names are the spec's snake_case names; optional fields are omitted when
//! absent, as the Swift encoder does, so the JSON written by either implementation decodes in the other.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `store.json` (§2.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoreIdentity {
    pub store_id: String,
    pub schema_version: i64,
    pub created_at: String,
    pub created_by: String,
    pub layout: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Exclude {
    pub apps: Vec<String>,
    pub threads: Vec<String>,
    pub patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourcePolicy {
    pub version: i64,
    pub store_text: bool,
    pub store_media: bool,
    pub agent_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<i64>,
    pub exclude: Exclude,
}

impl Default for SourcePolicy {
    fn default() -> Self {
        SourcePolicy { version: 1, store_text: true, store_media: false, agent_visible: true, retention_days: None, exclude: Exclude::default() }
    }
}

impl SourcePolicy {
    pub fn new(store_text: bool, store_media: bool) -> Self {
        SourcePolicy { store_text, store_media, ..Default::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub source_id: String,
    pub kind: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub policy: SourcePolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopeApps {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl Default for ScopeApps {
    fn default() -> Self {
        ScopeApps { allow: vec!["*".into()], deny: vec!["com.apple.keychainaccess".into()] }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ScopeTime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_days_back: Option<i64>,
}

/// §4.10 `scope_json`, enforced before ranking and snippet extraction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientScope {
    #[serde(default)]
    pub agents: Vec<String>,
    pub version: i64,
    pub sources: Vec<String>,
    pub kinds: Vec<String>,
    pub apps: ScopeApps,
    pub time: ScopeTime,
    pub text: bool,
    pub media: bool,
    pub derived: bool,
    pub propose: bool,
    pub max_result_bytes: i64,
    /// Writes inside the store this client may make (agent-platform-v1 §4.1): `annotate`, `reference`, `commitment`,
    /// `draft`. Empty (the default, and every registration before this field) means read and propose only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    /// Proposal kinds this client may file; empty means any kind (the original behaviour).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub propose_kinds: Vec<String>,
}

impl Default for ClientScope {
    fn default() -> Self {
        ClientScope {
            agents: vec![],
            version: 1,
            sources: vec!["*".into()],
            kinds: vec!["*".into()],
            apps: ScopeApps::default(),
            time: ScopeTime { max_days_back: Some(90), ..Default::default() },
            text: true,
            media: false,
            derived: true,
            propose: true,
            max_result_bytes: 65_536,
            writes: vec![],
            propose_kinds: vec![],
        }
    }
}

impl ClientScope {
    /// Default scope with no time limit (what the tests call `ClientScope(time: .init())`).
    pub fn unbounded() -> Self {
        ClientScope { time: ScopeTime::default(), ..Default::default() }
    }
}

// MARK: Ingest (§6.1)

fn default_basis() -> String {
    "observed".into()
}
fn default_actor() -> String {
    "self".into()
}
fn default_sensitivity() -> String {
    "normal".into()
}
fn default_payload_version() -> i64 {
    1
}
fn default_payload() -> Value {
    Value::Object(Default::default())
}
fn default_role() -> String {
    "original".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestBatch {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_after: Option<IngestCursor>,
    #[serde(default)]
    pub threads: Vec<IngestThread>,
    #[serde(default)]
    pub events: Vec<IngestEvent>,
    /// `detach`: an event whose `parent_key` is neither in the batch nor stored (a parent the capture's privacy gate
    /// withheld, or one the user deleted meanwhile) is stored without the link. Absent or `error`: the batch fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_missing_parent: Option<String>,
    /// `true`: fold typing continuations of `screen.text` into the earlier event (see `merge.rs`; provisional rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_text: Option<bool>,
    /// Records the source no longer has (expunged, or only left in its trash): deleted in the same transaction with
    /// tombstones (reason `source_deleted`), so a resend cannot bring them back. Unknown keys are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_source_keys: Vec<String>,
    /// Metadata-only changes of stored events (e.g. a mail moved between mailboxes). The previous payload is kept as a
    /// revision snapshot; text, time and identity are unchanged. Content changes still go to `ingest_conflicts`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payload_revisions: Vec<PayloadRevision>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PayloadRevision {
    pub source_key: String,
    pub payload: Value,
}

impl IngestBatch {
    pub fn events(source_id: &str, events: Vec<IngestEvent>) -> Self {
        IngestBatch { source_id: source_id.into(), events, ..Default::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestCursor {
    pub stream: String,
    pub cursor: String,
}

impl IngestCursor {
    pub fn new(stream: &str, cursor: &str) -> Self {
        IngestCursor { stream: stream.into(), cursor: cursor.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestThread {
    pub source_key: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestTextPart {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_distance: Option<i64>,
}

/// Text for one field. `content` is chunked as a whole; `parts` gives each region its own attention marks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestText {
    pub field: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_distance: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<IngestTextPart>>,
}

impl IngestText {
    pub fn new(field: &str, content: &str) -> Self {
        IngestText { field: field.into(), content: content.into(), ..Default::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestBlob {
    pub staging_path: String,
    pub sha256: String,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_w: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_h: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

impl IngestBlob {
    pub fn new(staging_path: &str, sha256: &str) -> Self {
        IngestBlob {
            staging_path: staging_path.into(),
            sha256: sha256.into(),
            role: "original".into(),
            mime_type: None,
            pixel_w: None,
            pixel_h: None,
            duration_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IngestEvent {
    pub source_key: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz_offset_min: Option<i64>,
    #[serde(default = "default_basis")]
    pub time_basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_uncertainty_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_key: Option<String>,
    /// `source_key` of the parent event (same source); it must already exist or be in the same batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_key: Option<String>,
    #[serde(default = "default_actor")]
    pub actor: String,
    #[serde(default = "default_sensitivity")]
    pub sensitivity: String,
    #[serde(default = "default_payload_version")]
    pub payload_version: i64,
    #[serde(default = "default_payload")]
    pub payload: Value,
    #[serde(default)]
    pub text: Vec<IngestText>,
    #[serde(default)]
    pub blobs: Vec<IngestBlob>,
}

impl IngestEvent {
    /// An event with the Swift initializer's defaults.
    pub fn new(source_key: &str, kind: &str, start_ms: Option<i64>) -> Self {
        IngestEvent {
            source_key: source_key.into(),
            kind: kind.into(),
            start_ms,
            end_ms: None,
            tz_offset_min: None,
            time_basis: default_basis(),
            time_uncertainty_ms: None,
            app_bundle_id: None,
            app_name: None,
            title: None,
            url: None,
            thread_key: None,
            parent_key: None,
            actor: default_actor(),
            sensitivity: default_sensitivity(),
            payload_version: 1,
            payload: default_payload(),
            text: vec![],
            blobs: vec![],
        }
    }
}
