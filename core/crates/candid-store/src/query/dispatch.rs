//! Tool calls by name with JSON arguments: the single entry the HTTP router and the tests share.

use crate::store::TimelineStore;
use crate::TimelineError;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// Why a tool call by name failed.
#[derive(Debug)]
pub enum ToolCallFailure {
    /// The arguments do not match the tool's request type.
    Decode(serde_json::Error),
    Store(TimelineError),
}

impl From<TimelineError> for ToolCallFailure {
    fn from(e: TimelineError) -> Self {
        ToolCallFailure::Store(e)
    }
}

/// Names of the query tools (timeline-v1 §5.2).
pub const TOOL_NAMES: [&str; 21] =
    ["day", "find", "person", "conversations", "conversation", "evidence", "source_query", "records", "agent", "status", "overview", "search", "timeline", "context", "get_event", "episodes", "episode", "thread", "media", "usage", "propose"];

/// The App's own read tools (internal client only, never listed in the catalog or MCP).
pub const APP_TOOL_NAMES: [&str; 9] =
    ["app_person_correct", "app_activities", "app_activity", "app_media", "app_recognition", "app_pending_recognition", "app_ocr", "app_retention_count", "app_day_counts"];

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, ToolCallFailure> {
    serde_json::from_slice(body).map_err(ToolCallFailure::Decode)
}

fn value<T: Serialize>(v: T) -> Result<Value, ToolCallFailure> {
    serde_json::to_value(v).map_err(|e| ToolCallFailure::Store(TimelineError::Database(e.to_string())))
}

impl TimelineStore {
    /// Runs tool `tool` for `client_id` with the JSON arguments in `body` (empty means `{}`).
    pub fn call_tool(&self, tool: &str, body: &[u8], client_id: &str) -> Result<Value, ToolCallFailure> {
        let body: &[u8] = if body.is_empty() { b"{}" } else { body };
        // Existing query entry points also accept live agent references.
        if tool == "source_query" { return value(self.source_query(&decode(body)?, client_id)?); }
        if tool == "agent" {
            return value(self.agent_call(&decode(body)?, client_id)?);
        }
        if ["get_event", "thread"].contains(&tool) {
            let args: Value = decode(body)?;
            let reference = args[if tool == "get_event" {
                "event_id"
            } else {
                "thread_id"
            }]
            .as_str()
            .unwrap_or("");
            if reference.starts_with("agent:") {
                let agent = reference.split(':').nth(1).unwrap_or("").to_string();
                return value(self.agent_call(
                    &crate::agents::AgentRequest {
                        agent,
                        action: "read".into(),
                        reference: Some(reference.into()),
                        limit: args["limit"].as_u64().map(|v| v as usize),
                        cursor: args["cursor"].as_str().map(String::from),
                        ..Default::default()
                    },
                    client_id,
                )?);
            }
        }
        if tool=="records" {let args:Value=decode(body)?;let id=args["id"].as_str().filter(|s|!s.is_empty()).ok_or_else(||TimelineError::InvalidInput("缺少修订编号".into()))?;return value(self.record_revision(id,client_id)?);}
        match tool {
            "day" => value(self.day(&decode(body)?, client_id)?),
            "find" => value(self.find(&decode(body)?, client_id)?),
            "person" => value(self.person(&decode(body)?, client_id)?),
            "app_person_correct" => value(self.app_person_correct(&decode(body)?, client_id)?),
            "conversations" => value(self.conversations(&decode(body)?, client_id)?),
            "conversation" => value(self.conversation(&decode(body)?, client_id)?),
            "evidence" => value(self.evidence(&decode(body)?, client_id)?),
            "status" => value(self.status(client_id)?),
            "overview" => value(self.overview(&decode(body)?, client_id)?),
            "search" => self.search_with_agents(body, client_id),
            "timeline" => value(self.timeline(&decode(body)?, client_id)?),
            "context" => value(self.context(&decode(body)?, client_id)?),
            "get_event" => value(self.get_event(&decode(body)?, client_id)?),
            "episodes" => value(self.episodes(&decode(body)?, client_id)?),
            "episode" => value(self.episode(&decode(body)?, client_id)?),
            "thread" => value(self.thread(&decode(body)?, client_id)?),
            "media" => value(self.media(&decode(body)?, client_id)?),
            "usage" => value(self.usage(&decode(body)?, client_id)?),
            "propose" => value(self.propose(&decode(body)?, client_id)?),
            "app_activities" => value(self.app_activities(&decode(body)?, client_id)?),
            "app_activity" => value(self.app_activity(&decode(body)?, client_id)?),
            "app_media" => value(self.app_media(&decode(body)?, client_id)?),
            "app_recognition" => value(self.app_recognition(&decode(body)?, client_id)?),
            "app_pending_recognition" => value(self.app_pending_recognition(&decode(body)?, client_id)?),
            "app_ocr" => value(self.app_ocr(&decode(body)?, client_id)?),
            "memories" => self.memories(&decode(body)?, client_id).map_err(Into::into),
            "annotate" => self.annotate(&decode(body)?, client_id).map_err(Into::into),
            "mark_reference" => self.mark_reference(&decode(body)?, client_id).map_err(Into::into),
            "commitment" => self.commitment(&decode(body)?, client_id).map_err(Into::into),
            "save_draft" => self.save_draft(&decode(body)?, client_id).map_err(Into::into),
            "undo_write" => self.undo_write(&decode(body)?, client_id).map_err(Into::into),
            "app_retention_count" => value(self.app_retention_count(&decode(body)?, client_id)?),
            "app_day_counts" => value(self.app_day_counts(&decode(body)?, client_id)?),
            other => Err(TimelineError::NotFound(other.into()).into()),
        }
    }
}
