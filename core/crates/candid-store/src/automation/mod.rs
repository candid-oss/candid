//! Skill settings, hooks and revisioned interaction runs. Reuses jobs/store_meta (database v5).
use crate::{
    db::{int, text, RowExt},
    sha256_hex, uuid_v4, Result, TimelineError, TimelineStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const PIPELINE: &str = "screen-cleanup/1";
fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}
fn denied(s: &str) -> TimelineError {
    TimelineError::Denied(s.into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPolicy {
    pub enabled: bool,
    pub automatic: bool,
    #[serde(default)]
    pub automatic_configured: bool,
    pub trigger: String,
    pub time: String,
    pub timezone: String,
    pub notify: bool,
    pub sources: Vec<String>,
    pub role: String,
    pub allow_remote: bool,
    pub daily_runs: u64,
    pub max_model_calls: u64,
    pub max_tool_calls: u64,
    pub max_output_tokens: u64,
}
impl Default for SkillPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            automatic: false,
            automatic_configured: false,
            trigger: "manual".into(),
            time: "21:00".into(),
            timezone: "Asia/Singapore".into(),
            notify: false,
            sources: vec!["*".into()],
            role: "agent_chat".into(),
            allow_remote: false,
            daily_runs: 3,
            max_model_calls: 12,
            max_tool_calls: 40,
            max_output_tokens: 16000,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationConfig {
    pub version: u64,
    pub skills: BTreeMap<String, SkillPolicy>,
    pub remote_sources: Vec<String>,
}

mod api;
mod config;
mod hooks;
mod runs;
mod runtime;
