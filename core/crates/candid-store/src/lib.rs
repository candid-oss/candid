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
pub use people::{PeopleVerdict, PersonCorrectionRequest, SELF_ENTITY};
mod db;
mod delete;
mod derived;
mod speakers;
mod recording_speakers;
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
}

mod schema_upgrade;
mod performance;
mod desktop_read_cache;
pub mod agents;

pub mod records;

pub mod durable;
pub mod media;
pub mod webp;
pub mod media_compaction;

mod media_projection;

pub mod workflows;
mod feature_settings;
pub mod automation;
pub mod screen_cleanup;
mod screen_pipeline;

mod voiceprints;

mod connector_support;
pub use connector_support::{ConnectorSource, MailEventRef};
