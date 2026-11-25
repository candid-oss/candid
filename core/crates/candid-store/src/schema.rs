//! Timeline database v4; v1–v3 roots upgrade transactionally through schema_upgrade.

pub const SCHEMA_VERSION: i64 = 13;
pub const LAYOUT: &str = "timeline-root/1";

pub struct Kind {
    pub kind: &'static str,
    pub layer: &'static str,
    pub is_span: bool,
    pub description: &'static str,
    pub required: &'static [&'static str],
}

/// §4.3. A new kind needs a row here, a payload check, tests, and a schema version bump.
pub const KINDS: &[Kind] = &[
    Kind { kind: "app.focus", layer: "evidence", is_span: true, description: "应用前台时段", required: &[] },
    Kind { kind: "window.focus", layer: "evidence", is_span: true, description: "窗口前台时段", required: &[] },
    Kind { kind: "screen.text", layer: "evidence", is_span: false, description: "屏幕文字观察", required: &[] },
    Kind { kind: "screen.shot", layer: "evidence", is_span: false, description: "屏幕截图", required: &[] },
    Kind { kind: "marker", layer: "evidence", is_span: false, description: "手动或系统标记", required: &[] },
    Kind { kind: "system.state", layer: "evidence", is_span: false, description: "电源、睡眠、锁屏等状态", required: &["change"] },
    Kind { kind: "coverage.gap", layer: "evidence", is_span: true, description: "没有记录的时段", required: &["reason_code"] },
    Kind { kind: "audio.recording", layer: "evidence", is_span: true, description: "录音", required: &[] },
    Kind { kind: "image.capture", layer: "evidence", is_span: false, description: "拍摄或导入的图片", required: &[] },
    Kind { kind: "speech.segment", layer: "perception", is_span: true, description: "语音转写段", required: &["recording_event_id"] },
    Kind { kind: "image.text", layer: "perception", is_span: false, description: "图片文字（AX、OCR、VLM）", required: &["image_event_id"] },
    Kind { kind: "calendar.event", layer: "evidence", is_span: true, description: "日历事件", required: &["calendar_id"] },
    Kind { kind: "reminder.item", layer: "evidence", is_span: false, description: "提醒事项", required: &["list_name"] },
    Kind { kind: "mail.message", layer: "evidence", is_span: false, description: "邮件", required: &["direction"] },
    Kind { kind: "ai.prompt", layer: "evidence", is_span: false, description: "用户发给 AI 的消息", required: &["agent", "session_ref", "message_ref", "origin"] },
    Kind { kind: "ai.work", layer: "evidence", is_span: true, description: "AI 工作摘要", required: &["agent", "session_ref", "prompt_event_id", "status"] },
    Kind { kind: "ai.turn", layer: "evidence", is_span: false, description: "AI 对话消息", required: &["role"] },
    Kind { kind: "note", layer: "evidence", is_span: false, description: "手写笔记或固定的查询结果", required: &[] },
    // v4: registered for the browser-history connector; its import stays disabled (agent-platform-v1 §2.0).
    Kind { kind: "browser.visit", layer: "evidence", is_span: false, description: "浏览器访问记录", required: &["browser", "visit_count"] },
];

pub const SOURCE_KINDS: &[&str] = &["mac.desktop", "mac.microphone", "mac.system_audio", "mac.region", "media.import", "device.pendant", "phone.android", "calendar.system", "reminders.system", "mail.imap", "mail.gmail", "mail.apple", "browser.history", "ai.codex", "ai.claude_code", "agent.transcript", "mcp.remote", "manual"];

pub const DDL: &str = r#"CREATE TABLE sources (
  source_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('mac.desktop','mac.microphone','mac.system_audio','mac.region','media.import',
    'device.pendant','phone.android','calendar.system','reminders.system',
    'mail.imap','mail.gmail','mail.apple','browser.history','ai.codex','ai.claude_code','agent.transcript','mcp.remote','manual')),
  display_name TEXT NOT NULL,
  device_id TEXT, account_ref TEXT, secret_ref TEXT,
  config_json TEXT NOT NULL DEFAULT '{}',
  policy_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active','paused','error','revoked')),
  last_sync_ms INTEGER, last_error TEXT,
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX sources_agent ON sources(json_extract(policy_json,'$.agent')) WHERE kind='agent.transcript';

CREATE TABLE source_cursors (
  source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
  stream TEXT NOT NULL, cursor TEXT NOT NULL, updated_ms INTEGER NOT NULL,
  PRIMARY KEY (source_id, stream)
) STRICT;

CREATE TABLE event_kinds (
  kind TEXT PRIMARY KEY,
  layer TEXT NOT NULL CHECK (layer IN ('evidence','perception')),
  is_span INTEGER NOT NULL,
  payload_version INTEGER NOT NULL,
  description TEXT NOT NULL
) STRICT;

CREATE TABLE threads (
  thread_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(source_id),
  source_key TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('ai_session','agent_session','mail_thread','meeting','recording','calendar_series','chat')),
  title TEXT, started_ms INTEGER, ended_ms INTEGER,
  participants_json TEXT NOT NULL DEFAULT '[]',
  payload_json TEXT NOT NULL DEFAULT '{}',
  UNIQUE (source_id, source_key)
) STRICT;
CREATE INDEX threads_time ON threads(started_ms);

CREATE TABLE events (
  event_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(source_id),
  source_key TEXT NOT NULL,
  kind TEXT NOT NULL REFERENCES event_kinds(kind),
  start_ms INTEGER,
  end_ms INTEGER CHECK (end_ms IS NULL OR start_ms IS NULL OR end_ms >= start_ms),
  tz_offset_min INTEGER,
  time_basis TEXT NOT NULL CHECK (time_basis IN ('observed','source','imported','estimated')),
  time_uncertainty_ms INTEGER,
  app_bundle_id TEXT, app_name TEXT, title TEXT, url TEXT,
  thread_id TEXT REFERENCES threads(thread_id),
  parent_event_id TEXT REFERENCES events(event_id),
  actor TEXT NOT NULL DEFAULT 'self' CHECK (actor IN ('self','other','agent','system','unknown')),
  sensitivity TEXT NOT NULL DEFAULT 'normal' CHECK (sensitivity IN ('normal','sensitive','protected')),
  payload_version INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  ingest_seq INTEGER NOT NULL UNIQUE,
  ingested_ms INTEGER NOT NULL,
  UNIQUE (source_id, source_key)
) STRICT;
CREATE INDEX events_time   ON events(start_ms, event_id);
CREATE INDEX events_app    ON events(app_bundle_id, start_ms) WHERE app_bundle_id IS NOT NULL;
CREATE INDEX events_kind   ON events(kind, start_ms);
CREATE INDEX events_thread ON events(thread_id, start_ms) WHERE thread_id IS NOT NULL;
CREATE INDEX events_source ON events(source_id, start_ms);
CREATE INDEX events_parent ON events(parent_event_id) WHERE parent_event_id IS NOT NULL;

CREATE TABLE ingest_conflicts (
  source_id TEXT NOT NULL, source_key TEXT NOT NULL,
  existing_event_id TEXT NOT NULL, incoming_json TEXT NOT NULL,
  detected_ms INTEGER NOT NULL, resolved_ms INTEGER, resolution TEXT,
  PRIMARY KEY (source_id, source_key, detected_ms)
) STRICT;

CREATE TABLE blobs (
  sha256 TEXT PRIMARY KEY,
  byte_count INTEGER NOT NULL,
  mime_type TEXT NOT NULL,
  rel_path TEXT NOT NULL UNIQUE,
  pixel_w INTEGER, pixel_h INTEGER, duration_ms INTEGER,
  created_ms INTEGER NOT NULL,
  verified_ms INTEGER
) STRICT;

CREATE TABLE event_blobs (
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  sha256 TEXT NOT NULL REFERENCES blobs(sha256),
  role TEXT NOT NULL CHECK (role IN ('original','preview','derived')),
  PRIMARY KEY (event_id, sha256, role)
) STRICT;
CREATE INDEX event_blobs_sha ON event_blobs(sha256);

CREATE TABLE text_chunks (
  chunk_id INTEGER PRIMARY KEY,
  chunk_hash TEXT NOT NULL UNIQUE,
  text TEXT NOT NULL,
  char_count INTEGER NOT NULL,
  lang TEXT,
  first_seen_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE event_text (
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  field TEXT NOT NULL CHECK (field IN ('prompt','body','selection','title','transcript','ocr','vlm','message','subject','note','quoted')),
  ord INTEGER NOT NULL,
  chunk_id INTEGER NOT NULL REFERENCES text_chunks(chunk_id),
  visible INTEGER CHECK (visible IN (0,1)),
  focus_distance INTEGER,
  is_new INTEGER NOT NULL CHECK (is_new IN (0,1)),
  char_start INTEGER,
  PRIMARY KEY (event_id, field, ord)
) STRICT;
CREATE INDEX event_text_chunk ON event_text(chunk_id);

CREATE VIRTUAL TABLE text_fts USING fts5(text, content='text_chunks', content_rowid='chunk_id', tokenize='trigram');
CREATE VIRTUAL TABLE meta_fts USING fts5(title, app_name, url, content='', tokenize='trigram');

CREATE TABLE episodes (
  episode_id TEXT NOT NULL, rev INTEGER NOT NULL,
  start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
  title TEXT, summary TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('activity','meeting','conversation','focus','break','mixed')),
  status TEXT NOT NULL CHECK (status IN ('draft','confirmed','edited','rejected','superseded')),
  producer TEXT NOT NULL, model_ref TEXT, prompt_version TEXT, confidence REAL,
  created_ms INTEGER NOT NULL,
  PRIMARY KEY (episode_id, rev)
) STRICT;
CREATE INDEX episodes_time ON episodes(start_ms) WHERE status NOT IN ('rejected','superseded');

CREATE TABLE episode_evidence (
  episode_id TEXT NOT NULL, rev INTEGER NOT NULL,
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('primary','supporting','context')),
  selector_json TEXT,
  PRIMARY KEY (episode_id, rev, event_id),
  FOREIGN KEY (episode_id, rev) REFERENCES episodes(episode_id, rev) ON DELETE CASCADE
) STRICT;

CREATE TABLE memories (
  memory_id TEXT NOT NULL, rev INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('fact','preference','judgment','commitment','person','project','decision','question','reference')),
  text TEXT NOT NULL,
  valid_from_ms INTEGER, valid_to_ms INTEGER,
  status TEXT NOT NULL CHECK (status IN ('candidate','confirmed','edited','rejected','expired','superseded')),
  producer TEXT NOT NULL, model_ref TEXT, prompt_version TEXT, confidence REAL,
  created_ms INTEGER NOT NULL,
  PRIMARY KEY (memory_id, rev)
) STRICT;

CREATE TABLE memory_sources (
  memory_id TEXT NOT NULL, rev INTEGER NOT NULL,
  ref_type TEXT NOT NULL CHECK (ref_type IN ('event','episode')),
  ref_id TEXT NOT NULL, ref_rev INTEGER, selector_json TEXT,
  PRIMARY KEY (memory_id, rev, ref_type, ref_id),
  FOREIGN KEY (memory_id, rev) REFERENCES memories(memory_id, rev) ON DELETE CASCADE
) STRICT;

CREATE TABLE corrections (
  correction_id TEXT PRIMARY KEY,
  target_type TEXT NOT NULL CHECK (target_type IN ('episode','memory','event_label','speaker','digest','entity')),
  target_id TEXT NOT NULL,
  action TEXT NOT NULL CHECK (action IN ('rename','edit_text','merge','split','reject','confirm','relabel')),
  value_json TEXT NOT NULL,
  created_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE digests (
  digest_id TEXT NOT NULL, rev INTEGER NOT NULL,
  level TEXT NOT NULL CHECK (level IN ('hour','day','week')),
  start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL,
  body_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('draft','confirmed','edited','rejected','superseded')),
  producer TEXT NOT NULL, model_ref TEXT, prompt_version TEXT,
  run_id TEXT,
  created_ms INTEGER NOT NULL,
  PRIMARY KEY (digest_id, rev)
) STRICT;
CREATE INDEX digests_time ON digests(level, start_ms) WHERE status NOT IN ('rejected','superseded');

CREATE TABLE entities (
  entity_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('person','project','org','place','site','app','topic')),
  name TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('candidate','confirmed','merged','rejected')),
  merged_into TEXT REFERENCES entities(entity_id),
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE entity_aliases (
  entity_id TEXT NOT NULL REFERENCES entities(entity_id) ON DELETE CASCADE,
  alias TEXT NOT NULL,
  alias_kind TEXT NOT NULL CHECK (alias_kind IN ('name','email','handle','domain','bundle','voice','other')),
  source TEXT NOT NULL CHECK (source IN ('rule','model','user')),
  PRIMARY KEY (entity_id, alias, alias_kind)
) STRICT;
CREATE INDEX entity_aliases_lookup ON entity_aliases(alias_kind, alias);

CREATE TABLE entity_mentions (
  entity_id TEXT NOT NULL REFERENCES entities(entity_id) ON DELETE CASCADE,
  ref_type TEXT NOT NULL CHECK (ref_type IN ('event','episode')),
  ref_id TEXT NOT NULL, ref_rev INTEGER,
  selector_json TEXT,
  producer TEXT NOT NULL, confidence REAL,
  PRIMARY KEY (entity_id, ref_type, ref_id)
) STRICT;
CREATE INDEX entity_mentions_ref ON entity_mentions(ref_type, ref_id);

CREATE TABLE ai_providers (
  provider_id TEXT PRIMARY KEY, name TEXT NOT NULL,
  api TEXT NOT NULL CHECK (api IN ('openai-chat','openai-responses','anthropic-messages','gemini','ollama','other')),
  base_url TEXT NOT NULL, secret_ref TEXT,
  destination TEXT NOT NULL CHECK (destination IN ('on_device','self_hosted','cloud')),
  enabled INTEGER NOT NULL,
  headers_json TEXT NOT NULL DEFAULT '{}',
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE ai_models (
  provider_id TEXT NOT NULL REFERENCES ai_providers(provider_id) ON DELETE CASCADE,
  model_id TEXT NOT NULL, display_name TEXT,
  context_window INTEGER, max_output INTEGER,
  modalities_json TEXT NOT NULL DEFAULT '["text"]',
  supports_tools INTEGER, supports_json_schema INTEGER,
  cost_json TEXT, probed_ms INTEGER,
  PRIMARY KEY (provider_id, model_id)
) STRICT;

CREATE TABLE ai_roles (
  role TEXT PRIMARY KEY,
  provider_id TEXT, model_id TEXT,
  fallback_json TEXT NOT NULL DEFAULT '[]',
  limits_json TEXT NOT NULL DEFAULT '{}',
  FOREIGN KEY (provider_id, model_id) REFERENCES ai_models(provider_id, model_id)
) STRICT;

CREATE TABLE ai_calls (
  call_id TEXT PRIMARY KEY,
  started_ms INTEGER NOT NULL, ended_ms INTEGER,
  feature TEXT NOT NULL,
  client TEXT NOT NULL CHECK (client IN ('app','pi','worker')),
  run_id TEXT,
  provider_id TEXT NOT NULL, model_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER,
  cost REAL, currency TEXT,
  cost_source TEXT CHECK (cost_source IN ('provider','estimated','unknown')),
  latency_ms INTEGER,
  status TEXT NOT NULL CHECK (status IN ('ok','error','cancelled','timeout')),
  error_code TEXT,
  request_bytes INTEGER, response_bytes INTEGER
) STRICT;
CREATE INDEX ai_calls_time    ON ai_calls(started_ms);
CREATE INDEX ai_calls_feature ON ai_calls(feature, started_ms);
CREATE INDEX ai_calls_model   ON ai_calls(provider_id, model_id, started_ms);

CREATE TABLE clients (
  client_id TEXT PRIMARY KEY, name TEXT NOT NULL,
  transport TEXT NOT NULL CHECK (transport IN ('mcp','http','cli','internal')),
  token_hash TEXT, scope_json TEXT NOT NULL,
  created_ms INTEGER NOT NULL, revoked_ms INTEGER
) STRICT;

CREATE TABLE access_log (
  log_id TEXT PRIMARY KEY, at_ms INTEGER NOT NULL,
  client_id TEXT NOT NULL REFERENCES clients(client_id),
  tool TEXT NOT NULL, params_json TEXT NOT NULL,
  result_count INTEGER, result_bytes INTEGER, duration_ms INTEGER,
  denied TEXT
) STRICT;
CREATE INDEX access_log_time ON access_log(at_ms);

CREATE TABLE proposals (
  proposal_id TEXT PRIMARY KEY,
  client_id TEXT NOT NULL REFERENCES clients(client_id),
  kind TEXT NOT NULL, payload_json TEXT NOT NULL,
  evidence_json TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','executed','failed','expired')),
  created_ms INTEGER NOT NULL, decided_ms INTEGER, executed_ms INTEGER, result_json TEXT
) STRICT;

CREATE TABLE jobs (
  job_id TEXT PRIMARY KEY, kind TEXT NOT NULL,
  input_ref TEXT NOT NULL, config_version TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('queued','running','done','failed','cancelled')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_run_ms INTEGER, lease_until_ms INTEGER,
  error_code TEXT, output_ref TEXT,
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
  feature TEXT,
  cpu_ms INTEGER, wall_ms INTEGER,
  power_state TEXT CHECK (power_state IN ('ac','battery','unknown')),
  manual INTEGER NOT NULL DEFAULT 0,
  UNIQUE (kind, input_ref, config_version)
) STRICT;
CREATE INDEX jobs_updated ON jobs(updated_ms);

CREATE TABLE changes (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  at_ms INTEGER NOT NULL,
  entity TEXT NOT NULL, entity_id TEXT NOT NULL,
  op TEXT NOT NULL CHECK (op IN ('insert','update','delete'))
) STRICT;

CREATE TABLE tombstones (
  entity TEXT NOT NULL, entity_id TEXT NOT NULL,
  deleted_ms INTEGER NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('user','retention','source_revoked','import_rollback','superseded','source_deleted')),
  PRIMARY KEY (entity, entity_id)
) STRICT;

CREATE TABLE agent_event_revisions (
  event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  revision INTEGER NOT NULL, event_json TEXT NOT NULL, created_ms INTEGER NOT NULL,
  PRIMARY KEY(event_id,revision)
) STRICT;

CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;"#;

pub const DEFAULT_META: &[(&str, &str)] = &[
    ("ingest_seq_next", "1"),
    ("access_log_params", "full"), // "full" | "redacted_q": keep or drop search keywords in params_json
    ("rank_weights", r#"{"visible":1.2,"near_focus":1.2,"is_new":1.3,"recency_half_life_days":14}"#),
];

pub fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.kind == name)
}

pub(crate) fn create(db: &crate::db::Db) -> crate::Result<()> {
    use crate::db::{boolean, int, text};
    db.transaction(|| {
        db.script(DDL)?;
        db.script(crate::durable::DDL)?;
        crate::performance::install(db)?;
        crate::performance::install_media_projection(db)?;
        db.script(crate::agents::CHECKPOINT_DDL)?;
        db.script(crate::curated::DDL)?;
        for k in KINDS {
            db.execute(
                "INSERT INTO event_kinds(kind,layer,is_span,payload_version,description) VALUES(?,?,?,?,?)",
                &[text(k.kind), text(k.layer), boolean(k.is_span), int(1), text(k.description)],
            )?;
        }
        for (k, v) in DEFAULT_META {
            db.execute("INSERT INTO store_meta(key,value) VALUES(?,?)", &[text(*k), text(*v)])?;
        }
        db.execute(&format!("PRAGMA user_version={SCHEMA_VERSION}"), &[])?;
        Ok(())
    })
}
