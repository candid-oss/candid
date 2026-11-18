//! Loopback HTTP service, MCP stdio shell and HTTP client for the Candid timeline (timeline-v1 §5.4), the Rust
//! port of the Swift `TimelineServer` and `TimelineAPI` modules, plus the App's local channel (Unix domain socket).

pub mod agenda;
pub mod catalog;
pub mod client;
pub mod endpoint;
pub mod http;
pub mod local;
pub mod mcp;
pub mod pi;
pub mod proxy;
pub mod router;
pub mod serve;

pub use client::{ServiceClient, ToolCallError};
pub use endpoint::{RunDirectory, ServiceEndpoint};
pub use http::{parse, HttpRequest, HttpResponse, LoopbackServer, Parse, MAX_BODY};
pub use local::{local_request, Core, LocalServer, SyncStatus, Verification};
pub use mcp::McpShell;
pub use router::Router;
pub use serve::{ServeOptions, Service};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod workflows;

mod audio_chunks;
/// WAV → AAC for processed recordings (data model v1 §9.4).
pub mod audio_compaction;
mod scheduling;

pub mod voiceprints;

mod voice_jobs;
