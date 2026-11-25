//! Candid timeline store (docs/specs/timeline-v1.md), the Rust port of the Swift `TimelineStore`.
//!
//! One [`TimelineStore`] owns a data root and is the only writer of `timeline.sqlite` and `blobs/`.
//! The root layout and identity fields retain v1 compatibility; v1 databases upgrade forward to v2.
//! The retired Swift v1 writer refuses v2. All versions take the same exclusive root lock.

mod chunk;
mod clock;
mod compose;
mod activity;
mod item_extract;
mod items;
mod curated;
mod curated_screen;
mod people;
pub use people::{PeopleCandidate, PeopleVerdict, BATCH as PEOPLE_JUDGE_BATCH, PersonCorrectionRequest, SELF_ENTITY};
mod db;
mod delete;
mod derived;
mod speakers;
pub mod recording_speakers;
pub use recording_speakers::RECORDING_JOB;
mod media_processing;
pub use speakers::{MATCH_MARGIN, MATCH_THRESHOLD, MAYBE_BELOW};
mod desktop;
mod error;
#[doc(hidden)]
pub mod fixtures;
mod ids;
mod ingest;
pub mod json;
mod livesync;
mod merge;
mod migration;
mod query;
mod schema;
mod screen_excerpt;
pub mod job_priority;
mod store;
mod types;

pub use chunk::{TextChunk, TextChunker};
pub use clock::{Clock, Zone};
pub use delete::DeletionReport;
pub use error::TimelineError;
pub use ids::{sha256_hex, sha256_hex_file, uuid_v4, uuid_v7};
pub use ingest::IngestResult;
pub use livesync::SyncReport;
pub use merge::{TextMergeConfig, META_TEXT_MERGE};
pub use migration::{KindCount, LedgerCheck, MigrationReport, MigrationVerification, META_CAPTURE_MODE, META_CUTOVER};
pub use query::agent_tools::{AnnotateRequest, CommitmentRequest, MarkReferenceRequest, MemoriesRequest, RunTag, SaveDraftRequest, UndoRequest, AGENT_TOOL_NAMES};
pub use query::agenda::AGENDA_EXECUTABLE_KINDS;
pub use query::app::*;
pub use query::people::PersonRequest;
pub use query::day::DayRequest;
pub use query::items::ItemsRequest;
pub use query::curated::{ConversationRequest, ConversationsRequest, EvidenceRequest, FindRequest};
pub use query::dispatch::{ToolCallFailure, APP_TOOL_NAMES, TOOL_NAMES};
pub use query::types::*;
pub use schema::SCHEMA_VERSION;
pub use store::{
    ClientInfo, ClientRegistration, Change, IntegrityReport, RecoveryReport, TestHooks, TimelineStore,
};
pub use types::*;

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, TimelineError>;

/// Raw SQL access for tests and fixtures in this workspace (not a stable interface).
#[doc(hidden)]
pub mod testing {
    pub use crate::db::{placeholders, Db, RowExt};
    pub use rusqlite;
    use serde_json::{json, Value};

    /// Test support: transcribes a stopped recording through its real `speech.recording` job, answering for the
    /// voice service with `lines` (`start_seconds`, `end_seconds`, `text`, optional `speaker`, `match`,
    /// `voice_ref`). Lines with speakers make a conversation result, otherwise a text-only (quick) one. The model
    /// setup gets a numeric-loopback voice service for the role when it has none.
    pub fn transcribe_recording(store: &crate::TimelineStore, id: &str, lines: &[Value]) -> crate::Result<Value> {
        let conversation = lines.iter().any(|l| l["speaker"].is_string());
        let (mode, role) = if conversation { ("conversation", "diarization") } else { ("quick", "asr") };
        let mut config = store.model_config()?;
        if store.local_voice_provider(role).is_err() {
            let provider = json!({"provider_id":"test-voice","name":"SYNTHETIC voice","api":"other","base_url":"http://127.0.0.1:1/v1","destination":"on_device","enabled":true});
            let providers = config["providers"].as_array_mut().unwrap();
            if !providers.iter().any(|p| p["provider_id"] == "test-voice") { providers.push(provider); }
            if let Some(models) = config["models"].as_array_mut() { models.push(json!({"provider_id":"test-voice","model_id":"SYNTHETIC-voice"})); }
            let roles = config["roles"].as_array_mut().unwrap();
            roles.retain(|r| r["role"] != role);
            roles.push(json!({"role":role,"model":{"provider_id":"test-voice","model_id":"SYNTHETIC-voice"}}));
            store.model_command(&json!({"action":"save","config":config}))?;
        }
        let job = store.rerun_recording(id, Some(mode))?;
        let claimed = store.job_command(&json!({"action":"claim","id":job["id"],"kind":crate::RECORDING_JOB,"owner":"SYNTHETIC","lease_ms":180000}))?;
        // A job this helper claimed before and whose reply was refused is still running: answer it again.
        let job = if claimed.is_null() { store.job_command(&json!({"action":"get","id":job["id"]}))? } else { claimed };
        let request = store.workflow_prepare(&job)?;
        let mut segments = Vec::new();
        let (mut speakers, mut refs) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
        let mut sorted = lines.to_vec();
        sorted.sort_by(|a, b| a["start_seconds"].as_f64().unwrap_or(0.).total_cmp(&b["start_seconds"].as_f64().unwrap_or(0.)));
        for l in &sorted {
            if let Some(s) = l["speaker"].as_str() { speakers.insert(s.to_string()); }
            if let Some(r) = l["voice_ref"].as_str() { refs.insert(r.to_string()); }
            segments.push(json!({"start_seconds":l["start_seconds"],"end_seconds":l["end_seconds"],"text":l["text"],
                "speaker":l["speaker"],"label":if conversation {l["speaker"].clone()} else {Value::Null},
                "voice_ref":l["voice_ref"],"match":l["match"],"correction_key":Value::Null}));
        }
        let output = json!({"format":"recording-speakers/2","mode":mode,"revision":request["input"]["revision"],"segments":segments,
            "speakers":[],"speaker_count":speakers.len(),"unresolved_segments":0,"dropped_segments":0,"voice_refs":refs,
            "producer":{"model_id":"SYNTHETIC","model_revision":"fixture/1","processor_version":"recording-windows/1"}});
        store.recording_complete(&request, &output)
    }
}

mod schema_upgrade;
mod performance;
pub mod background;
mod desktop_read_cache;
pub mod agents;

pub mod records;

pub mod durable;
pub mod media;
pub mod webp;
pub mod media_compaction;
pub mod library_conversion;
pub mod json_blobs;

mod media_projection;

pub mod workflows;
mod feature_settings;
pub mod power;
pub mod automation;
pub mod screen_cleanup;
mod screenshot_budget;
pub use screenshot_budget::{ScreenshotSpaceReport, ScreenshotSpaceRequest, SCREENSHOT_KEEP_RECENT_MS};
mod screen_pipeline;

mod voiceprints;

mod connector_support;
pub use connector_support::{ConnectorSource, MailEventRef};
