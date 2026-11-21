//! The AI-readable layer (docs/plans/ai-v1-plan.md, 「AI 可读层」): conversations and their messages, and reading
//! excerpts. An AI reads these clean objects by default instead of raw events (window switches, screenshots, OCR
//! blocks); raw records are only reachable as evidence.
//!
//! Every source projects into the same shape: a mail is a message in a `mail` conversation, a transcript segment a
//! message in a `meeting` conversation, and (later) a chat bubble read off a screenshot a message in a `chat`
//! conversation. Each row keeps the events it came from in `message_sources` / `excerpt_sources`; deleting the last
//! of them deletes the row (triggers below), and an empty conversation goes with its last message. Rows can be
//! dropped and rebuilt at any time.
//!
//! Projected rows (`producer` = `projection/1`) are records reshaped without a model; anything else is a model's
//! output and needs the client's `derived` scope as well.

use crate::db::{int, opt_text, text, RowExt};
use crate::{Result, TimelineStore};
use serde_json::{json, Value};

pub(crate) const PROJECTION: &str = "projection/1";
const CURSOR: &str = "curated.cursor/1";
const MEDIA_REVISION: &str = "curated.media_revision/1";
const PROMPT_REVISION: &str = "curated.prompt_revision/1";
const SPEAKER_REVISION: &str = "curated.speaker_revision/1";
const BATCH: i64 = 2000;

/// The conversations table, under a name of its own so the v12 upgrade can rebuild it (kind gained `agent`).
macro_rules! conversations_table {
    ($name:literal) => {
        concat!(
            "CREATE TABLE IF NOT EXISTS ", $name, " (
  conversation_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('chat','mail','meeting','agent')),
  app TEXT,
  title TEXT,
  started_ms INTEGER NOT NULL,
  last_ms INTEGER NOT NULL,
  producer TEXT NOT NULL,
  updated_ms INTEGER NOT NULL
) STRICT;"
        )
    };
}

/// Idempotent: the upgrade step can run again after an interrupted metadata write.
pub(crate) const DDL: &str = concat!(conversations_table!("conversations"), r#"
CREATE INDEX IF NOT EXISTS conversations_last ON conversations(last_ms);
CREATE TABLE IF NOT EXISTS messages (
  seq INTEGER PRIMARY KEY,
  message_id TEXT NOT NULL UNIQUE,
  conversation_id TEXT NOT NULL REFERENCES conversations(conversation_id) ON DELETE CASCADE,
  sent_ms INTEGER NOT NULL,
  actor TEXT NOT NULL CHECK (actor IN ('self','other','unknown')),
  sender TEXT,
  sender_entity_id TEXT REFERENCES entities(entity_id) ON DELETE SET NULL,
  text TEXT NOT NULL,
  producer TEXT NOT NULL,
  created_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS messages_conversation ON messages(conversation_id, sent_ms);
CREATE INDEX IF NOT EXISTS messages_time ON messages(sent_ms);
CREATE TABLE IF NOT EXISTS message_sources (
  message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  PRIMARY KEY (message_id, event_id)
) STRICT;
CREATE INDEX IF NOT EXISTS message_sources_event ON message_sources(event_id);
CREATE TABLE IF NOT EXISTS excerpts (
  seq INTEGER PRIMARY KEY,
  excerpt_id TEXT NOT NULL UNIQUE,
  key TEXT NOT NULL,
  app TEXT,
  title TEXT,
  url TEXT,
  start_ms INTEGER NOT NULL,
  end_ms INTEGER NOT NULL,
  text TEXT NOT NULL,
  producer TEXT NOT NULL,
  created_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS excerpts_time ON excerpts(start_ms);
CREATE INDEX IF NOT EXISTS excerpts_key ON excerpts(key, start_ms);
CREATE TABLE IF NOT EXISTS excerpt_sources (
  excerpt_id TEXT NOT NULL REFERENCES excerpts(excerpt_id) ON DELETE CASCADE,
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  PRIMARY KEY (excerpt_id, event_id)
) STRICT;
CREATE INDEX IF NOT EXISTS excerpt_sources_event ON excerpt_sources(event_id);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(text, content='messages', content_rowid='seq', tokenize='trigram');
CREATE VIRTUAL TABLE IF NOT EXISTS excerpts_fts USING fts5(title, text, content='excerpts', content_rowid='seq', tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, text) VALUES (NEW.seq, NEW.text);
END;
CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', OLD.seq, OLD.text);
  DELETE FROM conversations WHERE conversation_id=OLD.conversation_id
    AND NOT EXISTS (SELECT 1 FROM messages WHERE conversation_id=OLD.conversation_id);
END;
CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE OF text ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, text) VALUES ('delete', OLD.seq, OLD.text);
  INSERT INTO messages_fts(rowid, text) VALUES (NEW.seq, NEW.text);
END;
CREATE TRIGGER IF NOT EXISTS message_sources_last AFTER DELETE ON message_sources
  WHEN NOT EXISTS (SELECT 1 FROM message_sources WHERE message_id=OLD.message_id) BEGIN
  DELETE FROM messages WHERE message_id=OLD.message_id;
END;
CREATE TRIGGER IF NOT EXISTS excerpts_fts_insert AFTER INSERT ON excerpts BEGIN
  INSERT INTO excerpts_fts(rowid, title, text) VALUES (NEW.seq, NEW.title, NEW.text);
END;
CREATE TRIGGER IF NOT EXISTS excerpts_fts_delete AFTER DELETE ON excerpts BEGIN
  INSERT INTO excerpts_fts(excerpts_fts, rowid, title, text) VALUES ('delete', OLD.seq, OLD.title, OLD.text);
END;
CREATE TRIGGER IF NOT EXISTS excerpts_fts_update AFTER UPDATE OF title, text ON excerpts BEGIN
  INSERT INTO excerpts_fts(excerpts_fts, rowid, title, text) VALUES ('delete', OLD.seq, OLD.title, OLD.text);
  INSERT INTO excerpts_fts(rowid, title, text) VALUES (NEW.seq, NEW.title, NEW.text);
END;
CREATE TRIGGER IF NOT EXISTS excerpt_sources_last AFTER DELETE ON excerpt_sources
  WHEN NOT EXISTS (SELECT 1 FROM excerpt_sources WHERE excerpt_id=OLD.excerpt_id) BEGIN
  DELETE FROM excerpts WHERE excerpt_id=OLD.excerpt_id;
END;
"#, r#"
CREATE TABLE IF NOT EXISTS items (
  item_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('event','reminder','commitment')),
  owner TEXT NOT NULL CHECK (owner IN ('self','other')),
  title TEXT NOT NULL,
  when_text TEXT,
  start_ms INTEGER,
  end_ms INTEGER,
  all_day INTEGER NOT NULL DEFAULT 0,
  counterpart TEXT,
  conversation_id TEXT REFERENCES conversations(conversation_id) ON DELETE SET NULL,
  review TEXT NOT NULL CHECK (review IN ('candidate','confirmed','rejected')),
  state TEXT NOT NULL CHECK (state IN ('open','done','cancelled','expired')),
  user_fields TEXT NOT NULL DEFAULT '[]',
  external_ref TEXT,
  producer TEXT NOT NULL,
  model_ref TEXT,
  prompt_version TEXT,
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS items_open ON items(state, review, start_ms);
CREATE INDEX IF NOT EXISTS items_conversation ON items(conversation_id);
CREATE TABLE IF NOT EXISTS item_changes (
  change_id TEXT PRIMARY KEY,
  item_id TEXT NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  op TEXT NOT NULL CHECK (op IN ('create','update','done','cancel','merge','expire','user')),
  fields_json TEXT NOT NULL,
  "by" TEXT NOT NULL CHECK ("by" IN ('model','rule','user','calendar')),
  created_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS item_changes_item ON item_changes(item_id, created_ms);
CREATE TABLE IF NOT EXISTS item_sources (
  change_id TEXT NOT NULL REFERENCES item_changes(change_id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
  PRIMARY KEY (change_id, message_id)
) STRICT;
CREATE INDEX IF NOT EXISTS item_sources_message ON item_sources(message_id);
CREATE TRIGGER IF NOT EXISTS item_sources_last AFTER DELETE ON item_sources
  WHEN NOT EXISTS (SELECT 1 FROM item_sources WHERE change_id=OLD.change_id) BEGIN
  DELETE FROM item_changes WHERE change_id=OLD.change_id;
END;
CREATE TRIGGER IF NOT EXISTS item_changes_create_gone AFTER DELETE ON item_changes
  WHEN OLD.op='create' BEGIN
  DELETE FROM items WHERE item_id=OLD.item_id;
END;
"#);

/// v11 → v12: `conversations.kind` accepts `agent` (SQLite cannot alter a CHECK, so the table is rebuilt with
/// foreign keys off by the caller) and the item tables appear. Safe to run again.
pub(crate) fn upgrade_v12(db: &crate::db::Db) -> Result<()> {
    db.script("DROP TABLE IF EXISTS conversations_v12;")?;
    db.script(conversations_table!("conversations_v12"))?;
    db.script("INSERT INTO conversations_v12 SELECT * FROM conversations; DROP TABLE conversations; ALTER TABLE conversations_v12 RENAME TO conversations; CREATE INDEX IF NOT EXISTS conversations_last ON conversations(last_ms);")?;
    db.script(DDL)?;
    // New sources (agent prompts, screen text) project from the beginning; rows that exist are skipped.
    db.script("DELETE FROM store_meta WHERE key IN ('curated.cursor/1','curated.prompt_revision/1','curated.speaker_revision/1','curated.screen_cursor/1')")
}

/// (sender, person entity) of each segment of a recording, by start second.
type SpeakerLabels = std::collections::HashMap<String, (Option<String>, Option<String>)>;

/// A message to write, with the events it came from.
pub(crate) struct NewMessage<'a> {
    pub message_id: String,
    pub conversation_id: &'a str,
    pub sent_ms: i64,
    pub actor: &'a str,
    pub sender: Option<String>,
    pub text: String,
    pub producer: &'a str,
    pub sources: Vec<String>,
}

impl TimelineStore {
    /// Brings projected messages up to date with the records: new mail and transcript segments since the last run,
    /// and transcript segments whose visibility changed when a recording was re-projected. Cheap when nothing changed.
    pub fn curated_sync(&self) -> Result<Value> {
        let mut out = self.sync_records()?;
        let labelled = self.sync_speaker_labels(out["inserted"].as_i64().unwrap_or(0) > 0)?;
        if labelled > 0 {
            out["changed"] = json!(true);
            out["speakers_updated"] = json!(labelled);
        }
        Ok(out)
    }

    /// Who said each transcript line. Labels live in the recording's speaker results, not in the segment events, so
    /// they are read through the recording the way the App shows it (your relabels and voice matches included) and
    /// written onto the messages: `sender` is the shown name or label, `sender_entity_id` the person when known.
    /// Runs again whenever the media projection revision moves (relabel, re-match, re-transcription).
    fn sync_speaker_labels(&self, force: bool) -> Result<usize> {
        let revision = self.db.first("SELECT revision FROM background_revisions WHERE domain='media_projection'", &[], |r| r.int(0))?.unwrap_or(0);
        let seen: i64 = self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(SPEAKER_REVISION)], |r| r.text(0))?.and_then(|v| v.parse().ok()).unwrap_or(-1);
        if !force && revision == seen {
            return Ok(0);
        }
        let rows = self.db.query(
            "SELECT m.message_id,json_extract(e.payload_json,'$.native_projection_owner'),json_extract(e.payload_json,'$.start_seconds'),m.sender,m.sender_entity_id \
             FROM messages m JOIN events e ON e.event_id=substr(m.message_id,4) WHERE m.conversation_id GLOB 'rec:*' AND m.message_id GLOB 'ev:*' AND m.producer=?",
            &[text(PROJECTION)],
            |r| (r.text(0), r.opt_text(1), r.get::<_, Option<f64>>(2).unwrap_or(None), r.opt_text(3), r.opt_text(4)),
        )?;
        let mut labels: std::collections::HashMap<String, SpeakerLabels> = Default::default();
        let mut updates = vec![];
        for (message, owner, start, sender, entity) in rows {
            let (Some(owner), Some(start)) = (owner, start) else { continue };
            let by_start = labels.entry(owner.clone()).or_insert_with(|| self.recording_labels(&owner));
            let Some((name, person)) = by_start.get(&format!("{start:.3}")) else { continue };
            if name.is_some() && (*name != sender || *person != entity) {
                updates.push((message, name.clone(), person.clone()));
            }
        }
        let changed = updates.len();
        self.db.transaction(|| {
            for (message, name, person) in &updates {
                self.db.execute("UPDATE messages SET sender=?,sender_entity_id=? WHERE message_id=?", &[opt_text(name.as_deref()), opt_text(person.as_deref()), text(message)])?;
            }
            self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(SPEAKER_REVISION), text(revision.to_string())])?;
            Ok(())
        })?;
        Ok(changed)
    }

    /// (sender, person entity) of every segment of a recording, by start second; empty when the recording cannot be read.
    fn recording_labels(&self, owner: &str) -> SpeakerLabels {
        let Ok(item) = self.media_item(owner) else { return Default::default() };
        let people: std::collections::HashMap<&str, &str> = item["speakers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| matches!(s["status"].as_str(), Some("confirmed" | "auto")))
            .filter_map(|s| Some((s["key"].as_str()?, s["person_id"].as_str()?)))
            .collect();
        let mut out = std::collections::HashMap::new();
        for s in item["segments"].as_array().into_iter().flatten() {
            let Some(start) = s["start_seconds"].as_f64() else { continue };
            let name = s["speaker_name"].as_str().or_else(|| s["speaker_display_label"].as_str()).filter(|n| !n.is_empty()).map(String::from);
            let person = s["speaker"].as_str().and_then(|k| people.get(k)).filter(|_| s["speaker_name"].is_string()).map(|p| p.to_string());
            out.insert(format!("{start:.3}"), (name, person));
        }
        out
    }

    fn sync_records(&self) -> Result<Value> {
        let mut inserted = 0;
        let cursor: i64 = self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(CURSOR)], |r| r.text(0))?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let rows = self.db.query(
            "SELECT event_id,kind,start_ms,ingested_ms,actor,app_name,title,thread_id,parent_event_id,payload_json,ingest_seq,source_id FROM events \
             WHERE ingest_seq>? AND kind IN ('mail.message','speech.segment','ai.prompt','screen.text') ORDER BY ingest_seq LIMIT ?",
            &[int(cursor), int(BATCH)],
            |r| (r.text(0), r.text(1), r.opt_int(2), r.int(3), r.text(4), r.opt_text(5), r.opt_text(6), r.opt_text(7), r.opt_text(8), r.text(9), r.int(10), r.text(11)),
        )?;
        let last_seq = if (rows.len() as i64) < BATCH { self.db.scalar("SELECT COALESCE(MAX(ingest_seq),0) FROM events", &[])?.max(cursor) } else { rows.last().map(|r| r.10).unwrap_or(cursor) };
        let media_revision = self.db.first("SELECT revision FROM background_revisions WHERE domain='media_projection'", &[], |r| r.int(0))?.unwrap_or(0);
        let seen_media: i64 = self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(MEDIA_REVISION)], |r| r.text(0))?.and_then(|v| v.parse().ok()).unwrap_or(-1);
        let prompt_revision: i64 = self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(PROMPT_REVISION)], |r| r.text(0))?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let latest_prompt_revision = self.db.scalar("SELECT COALESCE(MAX(rowid),0) FROM agent_event_revisions", &[])?;
        if rows.is_empty() && last_seq == cursor && media_revision == seen_media && latest_prompt_revision == prompt_revision {
            return Ok(json!({"changed": false}));
        }
        self.db.transaction(|| {
            let mut screen = vec![];
            for (event_id, kind, start, ingested, actor, app, title, thread, parent, payload, _, source) in &rows {
                let p: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
                let at = start.unwrap_or(*ingested);
                if kind == "mail.message" {
                    inserted += self.project_mail(event_id, at, actor, app.as_deref(), title.as_deref(), thread.as_deref(), &p)?;
                } else if kind == "screen.text" {
                    // Collected: a visit is merged across the batch's frames at once.
                    if app.as_deref().is_some_and(crate::curated_screen::wants) {
                        screen.push((event_id.clone(), *start, None, *ingested, source.clone(), app.clone(), title.clone(), payload.clone()));
                    }
                } else if kind == "ai.prompt" {
                    inserted += self.project_prompt(event_id, at, &p)?;
                } else if p["native_hidden"] != 1 {
                    inserted += self.project_speech(event_id, at, parent.as_deref(), &p)?;
                }
            }
            inserted += self.project_screen_text(&screen)?;
            if media_revision != seen_media {
                // A re-projected recording hides some segments and shows others; the transcript is small.
                self.db.execute(
                    "DELETE FROM messages WHERE producer=? AND message_id IN (SELECT 'ev:'||event_id FROM events WHERE kind='speech.segment' AND json_extract(payload_json,'$.native_hidden')=1)",
                    &[text(PROJECTION)],
                )?;
                let shown = self.db.query(
                    "SELECT event_id,start_ms,ingested_ms,parent_event_id,payload_json FROM events WHERE kind='speech.segment' AND ingest_seq<=? \
                     AND COALESCE(json_extract(payload_json,'$.native_hidden'),0)<>1 AND NOT EXISTS (SELECT 1 FROM messages WHERE message_id='ev:'||events.event_id)",
                    &[int(cursor)],
                    |r| (r.text(0), r.opt_int(1), r.int(2), r.opt_text(3), r.text(4)),
                )?;
                for (event_id, start, ingested, parent, payload) in shown {
                    let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
                    inserted += self.project_speech(&event_id, start.unwrap_or(ingested), parent.as_deref(), &p)?;
                }
                self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(MEDIA_REVISION), text(media_revision.to_string())])?;
            }
            // A prompt that gained follow-ups is revised in place, keeping its sequence number.
            let revised: i64 = self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(PROMPT_REVISION)], |r| r.text(0))?.and_then(|v| v.parse().ok()).unwrap_or(0);
            let latest = self.db.scalar("SELECT COALESCE(MAX(rowid),0) FROM agent_event_revisions", &[])?;
            if latest > revised {
                let again = self.db.query(
                    "SELECT event_id,start_ms,ingested_ms,payload_json FROM events WHERE kind='ai.prompt' AND ingest_seq<=? AND event_id IN (SELECT event_id FROM agent_event_revisions WHERE rowid>?)",
                    &[int(cursor), int(revised)],
                    |r| (r.text(0), r.opt_int(1), r.int(2), r.text(3)),
                )?;
                for (event_id, start, ingested, payload) in again {
                    let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
                    inserted += self.project_prompt(&event_id, start.unwrap_or(ingested), &p)?;
                }
                self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(PROMPT_REVISION), text(latest.to_string())])?;
            }
            self.db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES(?,?)", &[text(CURSOR), text(last_seq.to_string())])?;
            Ok(())
        })?;
        Ok(json!({"changed": inserted > 0, "inserted": inserted}))
    }

    /// The text of one field of an event, paragraphs joined by new lines.
    pub(crate) fn field_text(&self, event_id: &str, field: &str) -> Result<String> {
        Ok(self
            .db
            .query(
                "SELECT c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id WHERE t.event_id=? AND t.field=? ORDER BY t.ord",
                &[text(event_id), text(field)],
                |r| r.text(0),
            )?
            .join("\n")
            .trim()
            .to_string())
    }

    #[allow(clippy::too_many_arguments)]
    fn project_mail(&self, event_id: &str, at: i64, actor: &str, app: Option<&str>, title: Option<&str>, thread: Option<&str>, p: &Value) -> Result<usize> {
        let subject = Some(self.field_text(event_id, "subject")?).filter(|s| !s.is_empty()).or_else(|| title.map(String::from));
        // Quoted history is the previous messages again; the body alone is what this mail says.
        let body = self.field_text(event_id, "body")?;
        let body = if body.is_empty() { subject.clone().unwrap_or_default() } else { body };
        if body.is_empty() {
            return Ok(0);
        }
        let conversation_id = match thread {
            Some(t) => format!("mail:{t}"),
            None => format!("mail:ev:{event_id}"),
        };
        let thread_title = match thread {
            Some(t) => self.db.first("SELECT title FROM threads WHERE thread_id=?", &[text(t)], |r| r.opt_text(0))?.flatten(),
            None => None,
        };
        self.upsert_conversation(&conversation_id, "mail", app.or(Some("邮件")), thread_title.or(subject).as_deref(), at, PROJECTION)?;
        let sender = p["from"]["name"].as_str().filter(|s| !s.is_empty()).map(String::from);
        let actor = match actor {
            "self" => "self",
            "other" => "other",
            _ => "unknown",
        };
        self.insert_message(&NewMessage {
            message_id: format!("ev:{event_id}"),
            conversation_id: &conversation_id,
            sent_ms: at,
            actor,
            sender,
            text: body,
            producer: PROJECTION,
            sources: vec![event_id.into()],
        })
    }

    /// What you wrote to an AI: one `agent` conversation per session, your prompt a message from you. The AI's reply
    /// is not stored (timeline-v1 §6.2), so the conversation is your side only. A revised prompt (follow-ups merged
    /// in) replaces the text of its message.
    fn project_prompt(&self, event_id: &str, at: i64, p: &Value) -> Result<usize> {
        let body = self.field_text(event_id, "prompt")?;
        let Some(session) = p["session_ref"].as_str().filter(|s| !s.is_empty()) else { return Ok(0) };
        if body.is_empty() {
            return Ok(0);
        }
        let agent = p["agent"].as_str().filter(|s| !s.is_empty());
        let project = p["project"].as_str().and_then(|s| s.trim_end_matches('/').rsplit('/').next()).filter(|s| !s.is_empty());
        let title = project.map(String::from).unwrap_or_else(|| body.chars().take(40).collect());
        let conversation_id = if session.starts_with("agent:") { session.to_string() } else { format!("agent:{session}") };
        self.upsert_conversation(&conversation_id, "agent", agent, Some(&title), at, PROJECTION)?;
        let message_id = format!("ev:{event_id}");
        let old = self.db.first("SELECT text FROM messages WHERE message_id=?", &[text(&message_id)], |r| r.text(0))?;
        if let Some(old) = old {
            if old != body {
                self.db.execute("UPDATE messages SET text=? WHERE message_id=?", &[text(&body), text(&message_id)])?;
            }
            return Ok(0);
        }
        self.insert_message(&NewMessage { message_id, conversation_id: &conversation_id, sent_ms: at, actor: "self", sender: None, text: body, producer: PROJECTION, sources: vec![event_id.into()] })
    }

    fn project_speech(&self, event_id: &str, at: i64, parent: Option<&str>, p: &Value) -> Result<usize> {
        let body = self.field_text(event_id, "transcript")?;
        if body.is_empty() {
            return Ok(0);
        }
        // The parent is the logical recording the user sees; the payload id is the file it was cut from.
        let Some(recording) = parent.or(p["recording_event_id"].as_str()) else {
            return Ok(0);
        };
        let title = self.db.first("SELECT title FROM events WHERE event_id=?", &[text(recording)], |r| r.opt_text(0))?.flatten();
        let conversation_id = format!("rec:{recording}");
        self.upsert_conversation(&conversation_id, "meeting", Some("录音"), title.as_deref(), at, PROJECTION)?;
        let sender = ["speaker_display_label", "speaker", "speaker_label"].iter().find_map(|k| p[*k].as_str().filter(|s| !s.is_empty() && *s != "unresolved")).map(String::from);
        self.insert_message(&NewMessage {
            message_id: format!("ev:{event_id}"),
            conversation_id: &conversation_id,
            sent_ms: at,
            actor: "unknown",
            sender,
            text: body,
            producer: PROJECTION,
            sources: vec![event_id.into()],
        })
    }

    pub(crate) fn upsert_conversation(&self, id: &str, kind: &str, app: Option<&str>, title: Option<&str>, at: i64, producer: &str) -> Result<()> {
        let now = self.clock.now_ms();
        self.db.execute(
            "INSERT INTO conversations(conversation_id,kind,app,title,started_ms,last_ms,producer,updated_ms) VALUES(?,?,?,?,?,?,?,?) \
             ON CONFLICT(conversation_id) DO UPDATE SET started_ms=MIN(started_ms,excluded.started_ms), last_ms=MAX(last_ms,excluded.last_ms), \
             title=COALESCE(conversations.title,excluded.title), updated_ms=excluded.updated_ms",
            &[text(id), text(kind), opt_text(app), opt_text(title), int(at), int(at), text(producer), int(now)],
        )?;
        Ok(())
    }

    /// Writes a message unless it exists; returns 1 when written. A message without any existing source is skipped.
    pub(crate) fn insert_message(&self, m: &NewMessage) -> Result<usize> {
        if m.sources.is_empty() {
            return Ok(0);
        }
        if self.db.scalar("SELECT COUNT(*) FROM messages WHERE message_id=?", &[text(&m.message_id)])? > 0 {
            return Ok(0);
        }
        self.db.execute(
            "INSERT INTO messages(message_id,conversation_id,sent_ms,actor,sender,text,producer,created_ms) VALUES(?,?,?,?,?,?,?,?)",
            &[text(&m.message_id), text(m.conversation_id), int(m.sent_ms), text(m.actor), opt_text(m.sender.as_deref()), text(&m.text), text(m.producer), int(self.clock.now_ms())],
        )?;
        for event in &m.sources {
            self.db.execute("INSERT OR IGNORE INTO message_sources(message_id,event_id) SELECT ?,event_id FROM events WHERE event_id=?", &[text(&m.message_id), text(event)])?;
        }
        if self.db.scalar("SELECT COUNT(*) FROM message_sources WHERE message_id=?", &[text(&m.message_id)])? == 0 {
            self.db.execute("DELETE FROM messages WHERE message_id=?", &[text(&m.message_id)])?;
            return Ok(0);
        }
        Ok(1)
    }
}
