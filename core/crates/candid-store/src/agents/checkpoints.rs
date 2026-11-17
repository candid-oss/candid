//! Append-sized durable session state. The progress cursor and changed entries commit with admitted events.
use super::*;
use crate::{db::{text, RowExt}, IngestCursor, TimelineStore};

pub(crate) const CHECKPOINT_DDL: &str = "CREATE TABLE agent_session_entries (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    session_stream TEXT NOT NULL, entry_id TEXT NOT NULL, entry_kind TEXT NOT NULL,
    entry_json TEXT NOT NULL,
    PRIMARY KEY(source_id,session_stream,entry_id,entry_kind)
) STRICT;
CREATE TABLE agent_file_counts (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    file_stream TEXT NOT NULL, content_hash TEXT NOT NULL, occurrences INTEGER NOT NULL,
    PRIMARY KEY(source_id,file_stream,content_hash)
) STRICT;";

#[derive(Default)]
pub(crate) struct CheckpointDelta {
    pub entries: Vec<(String,String,String,String)>,
    pub counts: Vec<(String,String,usize)>,
    pub reset_counts: Vec<String>,
}

impl TimelineStore {
    pub(super) fn load_agent_file(&self, source: &str, stream: &str) -> Result<Option<Session>> {
        let Some(raw) = self.cursor(source, stream)? else { return Ok(None); };
        let mut state: Session = serde_json::from_str(&raw).map_err(|_| failure("invalid_checkpoint"))?;
        if state.counts_external {
            state.fallback_counts = self.db.query("SELECT content_hash,occurrences FROM agent_file_counts WHERE source_id=? AND file_stream=?",
                &[text(source),text(stream)], |r| (r.text(0),r.int(1) as usize))?.into_iter().collect();
            state.saved_counts = state.fallback_counts.clone();
        }
        Ok(Some(state))
    }
    pub(super) fn load_agent_session(&self, source: &str, stream: &str) -> Result<Option<Session>> {
        let Some(raw) = self.cursor(source, stream)? else { return Ok(None); };
        let mut state: Session = serde_json::from_str(&raw).map_err(|_| failure("invalid_checkpoint"))?;
        if state.entries_external {
            // Keep same-line order without SQLite spilling large JSON sort keys to temporary files.
            let mut rows = self.db.query("SELECT rowid,entry_json FROM agent_session_entries WHERE source_id=? AND session_stream=?",
                &[text(source), text(stream)], |r| (r.int(0),r.text(1)))?;
            rows.sort_unstable_by_key(|row| row.0);
            state.entries = rows.into_iter()
                .map(|(_,s)| serde_json::from_str(&s).map_err(|_| failure("invalid_checkpoint")))
                .collect::<Result<Vec<Entry>>>()?;
            state.entries.sort_by(|a,b| (a.order_ms.unwrap_or(a.ts), &a.source_file, a.source_line)
                .cmp(&(b.order_ms.unwrap_or(b.ts), &b.source_file, b.source_line)));
            state.saved_entries = state.entries.iter().map(|e| ((e.id.clone(),e.kind.clone()),e.clone())).collect();
        }
        // Legacy inline entries remain in the old cursor until the whole new batch commits.
        Ok(Some(state))
    }
}

pub(super) fn session_checkpoint(stream: &str, state: &Session, delta: &mut CheckpointDelta) -> IngestCursor {
    let changed: Vec<_> = state.entries.iter().filter(|e| state.saved_entries.get(&(e.id.clone(),e.kind.clone())) != Some(*e))
        .map(|e| (stream.to_owned(),e.id.clone(),e.kind.clone(),crate::json::canonical_of(e))).collect();
    delta.entries.extend(changed);
    let mut compact = state.clone();
    compact.entries.clear();
    compact.fallback_counts.clear();
    compact.entries_external = true;
    IngestCursor::new(stream, &crate::json::canonical_of(&compact))
}

pub(super) fn file_checkpoint(stream: &str, mut state: Session, delta: &mut CheckpointDelta) -> IngestCursor {
    if state.reset_counts { delta.reset_counts.push(stream.into()); }
    delta.counts.extend(state.fallback_counts.iter()
        .filter(|(hash,n)| state.saved_counts.get(*hash) != Some(*n))
        .map(|(hash,n)| (stream.into(),hash.clone(),*n)));
    state.entries.clear();
    state.fallback_counts.clear();
    state.counts_external = true;
    IngestCursor::new(stream, &crate::json::canonical_of(&state))
}
