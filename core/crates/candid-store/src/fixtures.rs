//! Synthetic fixtures shared by the tests of this workspace and the Swift interop harness. Nothing here is real data.
//!
//! * `fixtures/world.json`: the Swift `QueryTests` world (sources, clients, ingest batches, derived rows).
//! * a desktop ledger and a Hub media library in the shapes the Swift `MigrationTests` build.

use crate::clock::{Clock, Zone};
use crate::db::{text, Db, P};
use crate::desktop::REFERENCE_DATE_OFFSET;
use crate::ids::sha256_hex;
use crate::json;
use crate::store::TimelineStore;
use crate::types::{ClientScope, IngestBatch, IngestEvent, SourcePolicy};
use crate::{Result, TimelineError};
use rusqlite::types::Value as SqlValue;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The fixed test instant: 2027-01-15T08:00:00Z (16:00 in Asia/Shanghai).
pub const FIXED_NOW_MS: i64 = 1_800_000_000_000;
/// Local midnight of the fixed day in Asia/Shanghai.
pub const TODAY_START_MS: i64 = 1_799_942_400_000;

pub fn test_clock() -> Clock {
    Clock::fixed(FIXED_NOW_MS, Zone::named("Asia/Shanghai").expect("tz"))
}

/// `today_start + hours`, like the Swift tests' `at(_:)`.
pub fn at(hours: f64) -> i64 {
    TODAY_START_MS + (hours * 3_600_000.0).round() as i64
}

/// A fresh, not yet existing directory under the system temp dir.
pub fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("timeline-store-synthetic-{label}-{}", crate::ids::uuid_v4()))
}

/// Path of `core/fixtures/<name>`.
pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name)
}

/// Aliases resolved while loading a world.
#[derive(Clone, Debug, Default)]
pub struct WorldIds {
    pub sources: BTreeMap<String, String>,
    pub clients: BTreeMap<String, String>,
    /// source_key → event_id
    pub events: BTreeMap<String, String>,
}

fn resolve(v: &Value, ids: &WorldIds, staged: &BTreeMap<String, (String, String)>) -> Result<Value> {
    Ok(match v {
        Value::String(s) => {
            if let Some(a) = s.strip_prefix("@source:") {
                json!(ids.sources.get(a).ok_or_else(|| TimelineError::InvalidInput(s.clone()))?)
            } else if let Some(k) = s.strip_prefix("@id:") {
                json!(ids.events.get(k).ok_or_else(|| TimelineError::InvalidInput(s.clone()))?)
            } else if let Some(k) = s.strip_prefix("@stage:") {
                json!(staged.get(k).ok_or_else(|| TimelineError::InvalidInput(s.clone()))?.0)
            } else if let Some(k) = s.strip_prefix("@sha:") {
                json!(staged.get(k).ok_or_else(|| TimelineError::InvalidInput(s.clone()))?.1)
            } else {
                v.clone()
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(|i| resolve(i, ids, staged)).collect::<Result<_>>()?),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve(v, ids, staged)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

/// Loads `world.json` into an open store: sources, clients, the three ingest batches, then the derived rows.
pub fn load_world(store: &TimelineStore, path: &Path) -> Result<WorldIds> {
    let world: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut ids = WorldIds::default();
    for s in world["sources"].as_array().into_iter().flatten() {
        let policy: SourcePolicy = serde_json::from_value(s["policy"].clone())?;
        let id = store.register_source(s["kind"].as_str().unwrap_or(""), s["display_name"].as_str().unwrap_or(""), &policy)?;
        ids.sources.insert(s["alias"].as_str().unwrap_or("").into(), id);
    }
    let empty = BTreeMap::new();
    for c in world["clients"].as_array().into_iter().flatten() {
        let scope: ClientScope = serde_json::from_value(resolve(&c["scope"], &ids, &empty)?)?;
        let reg = store.register_client(c["name"].as_str().unwrap_or(""), c["transport"].as_str().unwrap_or("cli"), &scope, None)?;
        ids.clients.insert(c["alias"].as_str().unwrap_or("").into(), reg.client_id);
    }
    let mut staged = BTreeMap::new();
    for (k, v) in world["staging"].as_object().into_iter().flatten() {
        let bytes = v.as_str().unwrap_or("").as_bytes().to_vec();
        let path = store.staging_directory().join(format!("{}.jpg", crate::ids::uuid_v4()));
        std::fs::write(&path, &bytes)?;
        staged.insert(k.clone(), (path.to_string_lossy().into_owned(), sha256_hex(&bytes)));
    }
    for b in world["batches"].as_array().into_iter().flatten() {
        let source = ids.sources.get(b["source"].as_str().unwrap_or("")).cloned().unwrap_or_default();
        let mut batch_json = json!({"source_id": source, "threads": b["threads"], "events": b["events"]});
        batch_json = resolve(&batch_json, &ids, &staged)?;
        let batch: IngestBatch = serde_json::from_value(batch_json)?;
        let r = store.ingest(&batch)?;
        for (k, v) in r.event_ids {
            ids.events.entry(k).or_insert(v);
        }
    }
    for row in world["sql"].as_array().into_iter().flatten() {
        let params: Vec<P> = row["params"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| resolve(p, &ids, &staged))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .map(|p| match p {
                Value::Null => SqlValue::Null,
                Value::Number(n) if n.is_i64() => SqlValue::Integer(n.as_i64().unwrap_or(0)),
                Value::Number(n) => SqlValue::Real(n.as_f64().unwrap_or(0.0)),
                Value::String(s) => SqlValue::Text(s),
                other => SqlValue::Text(json::canonical(&other)),
            })
            .collect();
        store.raw_db().execute(row["sql"].as_str().unwrap_or(""), &params)?;
    }
    Ok(ids)
}

// MARK: Desktop ledger

/// One synthetic ledger record in the Swift `DesktopRecord` shape.
#[derive(Clone, Debug, Default)]
pub struct LedgerRecord {
    pub id: String,
    /// Seconds since 1970.
    pub unix: f64,
    pub kind: &'static str,
    pub app_name: String,
    pub bundle_id: String,
    pub window_title: String,
    pub detail: String,
    pub image_hash: Option<String>,
    pub image_width: Option<i64>,
    pub image_height: Option<i64>,
    pub parent_id: Option<String>,
    pub run_id: String,
    pub trigger: Option<String>,
    pub context: Option<Value>,
    pub activity_id: Option<String>,
}

impl LedgerRecord {
    /// The payload bytes Swift's ledger writes (sorted keys, dates as seconds since 2001).
    pub fn payload(&self) -> Vec<u8> {
        let mut m = Map::new();
        m.insert("id".into(), json!(self.id));
        m.insert("timestamp".into(), json!(self.unix - REFERENCE_DATE_OFFSET));
        m.insert("kind".into(), json!(self.kind));
        m.insert("appName".into(), json!(self.app_name));
        m.insert("bundleID".into(), json!(self.bundle_id));
        m.insert("windowTitle".into(), json!(self.window_title));
        m.insert("detail".into(), json!(self.detail));
        m.insert("runID".into(), json!(self.run_id));
        let mut opt = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                m.insert(k.into(), v);
            }
        };
        opt("imageHash", self.image_hash.clone().map(Value::from));
        opt("imageWidth", self.image_width.map(Value::from));
        opt("imageHeight", self.image_height.map(Value::from));
        opt("parentID", self.parent_id.clone().map(Value::from));
        opt("trigger", self.trigger.clone().map(Value::from));
        opt("context", self.context.clone());
        opt("activityID", self.activity_id.clone().map(Value::from));
        json::canonical(&Value::Object(m)).into_bytes()
    }
}

pub struct SyntheticLedger {
    pub root: PathBuf,
    pub records: Vec<LedgerRecord>,
    pub image: Vec<u8>,
}

pub const LEDGER_T0: f64 = 1_790_000_000.0;

const LEDGER_DDL: &str = "
CREATE TABLE records(id TEXT PRIMARY KEY, occurred REAL NOT NULL, kind TEXT NOT NULL, image_hash TEXT, search_text TEXT NOT NULL, payload BLOB NOT NULL);
CREATE INDEX records_time ON records(occurred DESC, id DESC);
CREATE TABLE tombstones(id TEXT PRIMARY KEY, deleted_at REAL NOT NULL);
CREATE TABLE image_recognition(screenshot_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('ocr','vision')), result BLOB, PRIMARY KEY(screenshot_id,kind));
PRAGMA user_version=5;
";

fn search_text(r: &LedgerRecord) -> String {
    let ctx = r.context.clone().unwrap_or(Value::Null);
    let s = |k: &str| ctx.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    [r.app_name.clone(), r.bundle_id.clone(), r.window_title.clone(), r.detail.clone(), s("readableText"), s("selectedText"), s("focusedRole"), s("focusedLabel"), s("textStatus")]
        .join("\n")
}

/// Opens (creating if needed) a synthetic ledger database for writing.
pub fn ledger_db(root: &Path) -> Result<Db> {
    std::fs::create_dir_all(root.join("blobs"))?;
    let path = root.join("ledger.sqlite");
    let fresh = !path.exists();
    let db = Db::open(&path, false, true)?;
    if fresh {
        db.execute("PRAGMA journal_mode=WAL", &[])?;
        db.script(LEDGER_DDL)?;
    }
    Ok(db)
}

pub fn ledger_append(db: &Db, r: &LedgerRecord) -> Result<()> {
    db.execute(
        "INSERT INTO records(id,occurred,kind,image_hash,search_text,payload) VALUES(?,?,?,?,?,?)",
        &[text(&r.id), SqlValue::Real(r.unix), text(r.kind), crate::db::opt_text(r.image_hash.clone()), text(search_text(r)), SqlValue::Blob(r.payload())],
    )
}

/// Saves (or replaces) a recognition result like `DesktopLedger.saveRecognition`.
pub fn ledger_save_recognition(db: &Db, screenshot: &str, kind: &str, source: &str, text_: &str, completed_unix: f64) -> Result<()> {
    let payload = json!({"source": source, "text": text_, "completedAt": completed_unix - REFERENCE_DATE_OFFSET, "usedOCRReference": false});
    db.execute(
        "INSERT INTO image_recognition(screenshot_id,kind,result) VALUES(?,?,?) ON CONFLICT(screenshot_id,kind) DO UPDATE SET result=excluded.result",
        &[text(screenshot), text(kind), SqlValue::Blob(serde_json::to_vec(&payload)?)],
    )
}

/// Deletes records like `DesktopLedger.delete(ids:)`: a tombstone, then the row.
pub fn ledger_delete(db: &Db, id: &str, deleted_unix: f64) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO tombstones(id,deleted_at) VALUES(?,?)", &[text(id), SqlValue::Real(deleted_unix)])?;
    db.execute("DELETE FROM records WHERE id=?", &[text(id)])
}

/// The ledger of the Swift `makeSyntheticLedger()`: eight records, a screenshot with its image, one OCR result.
pub fn make_synthetic_ledger() -> Result<SyntheticLedger> {
    let root = temp_root("ledger");
    let db = ledger_db(&root)?;
    let t0 = LEDGER_T0;
    let run = "synthetic-run".to_string();
    let notes = |id: &str, dt: f64, kind: &'static str| LedgerRecord {
        id: id.into(),
        unix: t0 + dt,
        kind,
        app_name: "Notes".into(),
        bundle_id: "com.example.notes".into(),
        run_id: run.clone(),
        ..Default::default()
    };
    let paragraph = "合成的预算说明：本季度预算需要复核。";
    let app = LedgerRecord { trigger: Some("switch".into()), ..notes("rec-app", 0.0, "application") };
    let window = LedgerRecord { window_title: "预算草稿".into(), activity_id: Some("act-1".into()), ..notes("rec-win", 1.0, "window") };
    let ctx1 = LedgerRecord {
        window_title: "预算草稿".into(),
        trigger: Some("timer".into()),
        activity_id: Some("act-1".into()),
        context: Some(json!({"focusedRole": "AXTextArea", "focusedLabel": "正文", "readableText": format!("{paragraph}\n\nsecond synthetic paragraph"), "textStatus": "ok", "selectedText": "预算"})),
        ..notes("rec-ctx1", 2.0, "context")
    };
    let ctx2 = LedgerRecord {
        window_title: "预算草稿".into(),
        trigger: Some("timer".into()),
        activity_id: Some("act-1".into()),
        context: Some(json!({"readableText": format!("{paragraph}\n\nthird synthetic paragraph"), "textStatus": "ok"})),
        ..notes("rec-ctx2", 62.0, "context")
    };
    let protected = LedgerRecord {
        app_name: "Vault".into(),
        bundle_id: "com.example.vault".into(),
        window_title: "Synthetic secret window".into(),
        trigger: Some("timer".into()),
        activity_id: Some("act-2".into()),
        context: Some(json!({"textStatus": "protected", "isProtected": true})),
        ..notes("rec-prot", 70.0, "context")
    };
    let bare = |id: &str, dt: f64, kind: &'static str| LedgerRecord { id: id.into(), unix: t0 + dt, kind, run_id: run.clone(), ..Default::default() };
    let marker = LedgerRecord { window_title: "synthetic marker".into(), detail: "note body".into(), ..bare("rec-mark", 80.0, "marker") };
    let system = LedgerRecord { detail: "system did wake".into(), ..bare("rec-sys", 90.0, "system") };
    let gap = LedgerRecord { detail: "recording paused".into(), ..bare("rec-gap", 100.0, "gap") };
    let mut records = vec![app, window, ctx1, ctx2, protected, marker, system, gap];
    for r in &records {
        ledger_append(&db, r)?;
    }
    let image: Vec<u8> = (0..200u32).map(|i| (i % 251) as u8).collect();
    let hash = sha256_hex(&image);
    let dir = root.join("blobs").join(&hash[..2]);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{hash}.jpg")), &image)?;
    // AX evidence on the screenshot exercises the Swift default-encoder bytes behind the `derived` blob's hash.
    let evidence = json!({"nodes": [
        {"role": "AXWindow", "label": "预算草稿", "bounds": [0, 0, 1000, 1000]},
        {"parent": 0, "role": "AXLink", "text": "https://example.com/a/b", "destination": "https://example.com/a/b", "focused": true}
    ], "partial": false, "observedAt": LEDGER_T0 + 3.25 - REFERENCE_DATE_OFFSET});
    let shot = LedgerRecord {
        window_title: "预算草稿".into(),
        context: Some(json!({"axEvidence": evidence})),
        image_hash: Some(hash),
        image_width: Some(40),
        image_height: Some(30),
        parent_id: Some("rec-win".into()),
        trigger: Some("timer".into()),
        ..notes("rec-shot", 3.0, "screenshot")
    };
    ledger_append(&db, &shot)?;
    ledger_save_recognition(&db, "rec-shot", "ocr", "synthetic-ocr/1", "OCR 合成文字 预算表", t0 + 10.0)?;
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)", &[])?;
    records.push(shot);
    Ok(SyntheticLedger { root, records, image })
}

// MARK: Hub media library

pub struct SyntheticHub {
    pub db: PathBuf,
    pub vault: PathBuf,
    pub root: PathBuf,
}

/// The Hub media library and device observations of the Swift `makeSyntheticHub()`.
pub fn make_synthetic_hub() -> Result<SyntheticHub> {
    let root = temp_root("hub");
    let vault = root.join("vault");
    std::fs::create_dir_all(vault.join("blobs"))?;
    let db_path = root.join("hub.sqlite");
    let db = Db::open(&db_path, false, true)?;
    db.script(
        "CREATE TABLE media_timeline (id TEXT PRIMARY KEY, filename TEXT NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('audio','image')), source TEXT NOT NULL,
      occurred_at TEXT, imported_at TEXT NOT NULL, blob TEXT NOT NULL, mime_type TEXT NOT NULL, byte_count INTEGER NOT NULL, filename_search TEXT NOT NULL,
      processing_status TEXT NOT NULL DEFAULT 'idle', processing_mode TEXT, queued_at TEXT, error TEXT);
    CREATE TABLE media_timeline_recognition (media_id TEXT NOT NULL, mode TEXT NOT NULL, result TEXT NOT NULL, transcript_search TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(media_id, mode));
    CREATE TABLE media_timeline_deleted (id TEXT PRIMARY KEY);
    CREATE TABLE observations (event_id TEXT PRIMARY KEY, schema_version TEXT NOT NULL, device_id TEXT NOT NULL, boot_id TEXT NOT NULL, seq INTEGER NOT NULL,
      event_type TEXT NOT NULL, monotonic_ms INTEGER NOT NULL, wall_time_utc TEXT, time_uncertainty_ms INTEGER, payload_json TEXT NOT NULL,
      ingested_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, UNIQUE(device_id, boot_id, seq));",
    )?;
    let add = |id: &str, filename: &str, kind: &str, bytes: &[u8], ext: &str, mime: &str, occurred: Option<&str>, result: Option<&str>| -> Result<()> {
        let sha = sha256_hex(bytes);
        std::fs::write(vault.join("blobs").join(format!("{sha}.{ext}")), bytes)?;
        db.execute(
            "INSERT INTO media_timeline(id,filename,kind,source,occurred_at,imported_at,blob,mime_type,byte_count,filename_search,processing_mode) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            &[
                text(id),
                text(filename),
                text(kind),
                text("synthetic"),
                crate::db::opt_text(occurred),
                text("2026-11-01T00:00:00.000000+00:00"),
                text(format!("{sha}.{ext}")),
                text(mime),
                crate::db::int(bytes.len() as i64),
                text(filename.to_lowercase()),
                text("fast"),
            ],
        )?;
        if let Some(result) = result {
            db.execute(
                "INSERT INTO media_timeline_recognition VALUES(?,?,?,?,?)",
                &[text(id), text("fast"), text(result), text(""), text("2026-11-01T00:01:00.000000+00:00")],
            )?;
        }
        Ok(())
    };
    add(
        "m1",
        "meeting.m4a",
        "audio",
        b"audio-bytes-1",
        "m4a",
        "audio/mp4",
        Some("2026-10-31T09:00:00.123456+08:00"),
        Some(r#"{"producer":{"model_id":"synthetic-asr","revision":"r1"},"segments":[{"start_seconds":0.0,"end_seconds":2.5,"text":"我们讨论预算问题","speaker":"S1"},{"start_seconds":2.5,"end_seconds":4.0,"text":"second synthetic line"}]}"#),
    )?;
    add("m2", "photo.jpg", "image", b"image-bytes", "jpg", "image/jpeg", None, None)?;
    add("m3", "deleted.m4a", "audio", b"gone", "m4a", "audio/mp4", Some("2026-10-31T10:00:00+00:00"), None)?;
    db.execute("INSERT INTO media_timeline_deleted VALUES('m3')", &[])?;
    db.execute(
        "INSERT INTO observations(event_id,schema_version,device_id,boot_id,seq,event_type,monotonic_ms,wall_time_utc,payload_json) VALUES('o1','1','dev','boot1',1,'button.press',10,'2026-10-31T09:00:01+00:00','{\"k\":1}')",
        &[],
    )?;
    db.execute(
        "INSERT INTO observations(event_id,schema_version,device_id,boot_id,seq,event_type,monotonic_ms,wall_time_utc,payload_json) VALUES('o2','1','dev','boot1',2,'battery.low',20,NULL,'{}')",
        &[],
    )?;
    Ok(SyntheticHub { db: db_path, vault, root })
}

// MARK: Query plans (interop)

fn lookup(store: &TimelineStore, sql: &str, key: &str) -> Result<String> {
    store.raw_db().first(sql, &[text(key)], |r| crate::db::RowExt::text(r, 0))?.ok_or_else(|| TimelineError::NotFound(key.into()))
}

fn resolve_plan_value(store: &TimelineStore, v: &Value) -> Result<Value> {
    Ok(match v {
        Value::String(s) => {
            if let Some(k) = s.strip_prefix("@id:") {
                json!(lookup(store, "SELECT event_id FROM events WHERE source_key=?", k)?)
            } else if let Some(k) = s.strip_prefix("@thread:") {
                json!(lookup(store, "SELECT thread_id FROM threads WHERE source_key=?", k)?)
            } else {
                v.clone()
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(|i| resolve_plan_value(store, i)).collect::<Result<_>>()?),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_plan_value(store, v)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

/// Runs every entry of `plan.json` against `store`; client aliases map to the client names in `world.json`.
/// Each result is `{"tool", "client", "ok", "response" | "error"}`.
pub fn run_plan(store: &TimelineStore, world: &Path, plan: &Path) -> Result<Vec<Value>> {
    let world: Value = serde_json::from_slice(&std::fs::read(world)?)?;
    let plan: Value = serde_json::from_slice(&std::fs::read(plan)?)?;
    let mut out = Vec::new();
    for entry in plan["plan"].as_array().into_iter().flatten() {
        let alias = entry["client"].as_str().unwrap_or("");
        let name = world["clients"].as_array().into_iter().flatten().find(|c| c["alias"] == json!(alias)).and_then(|c| c["name"].as_str()).unwrap_or(alias);
        let client = lookup(store, "SELECT client_id FROM clients WHERE name=? ORDER BY created_ms LIMIT 1", name)?;
        let args = resolve_plan_value(store, &entry["args"])?;
        let tool = entry["tool"].as_str().unwrap_or("");
        let body = serde_json::to_vec(&args)?;
        let result = match store.call_tool(tool, &body, &client) {
            Ok(v) => json!({"tool": tool, "client": alias, "ok": true, "response": v}),
            Err(crate::ToolCallFailure::Decode(_)) => json!({"tool": tool, "client": alias, "ok": false, "error": "invalid_input"}),
            Err(crate::ToolCallFailure::Store(e)) => json!({"tool": tool, "client": alias, "ok": false, "error": e.code()}),
        };
        out.push(result);
    }
    Ok(out)
}

/// Makes plan results from two implementations comparable: numbers compared by value, opaque cursors by presence,
/// sizes that depend on the database file left out.
pub fn normalise_result(v: &Value) -> Value {
    fn walk(v: &Value, key: Option<&str>) -> Value {
        match (key, v) {
            (Some("next_cursor"), Value::String(_)) => json!("<cursor>"),
            (Some("db_bytes"), _) => json!("<bytes>"),
            (_, Value::Number(n)) => {
                let f = n.as_f64().unwrap_or(0.0);
                if f.fract() == 0.0 && f.abs() < 9e15 {
                    json!(f as i64)
                } else {
                    json!((f * 1e6).round() / 1e6)
                }
            }
            (_, Value::Array(items)) => Value::Array(items.iter().map(|i| walk(i, None)).collect()),
            (_, Value::Object(m)) => Value::Object(m.iter().map(|(k, v)| (k.clone(), walk(v, Some(k)))).collect()),
            _ => v.clone(),
        }
    }
    walk(v, None)
}

/// Re-sends the world's batches to a store that already holds them (sources found by display name), for
/// cross-implementation idempotency checks. Returns (inserted, duplicates, conflicts, tombstoned).
pub fn reingest_world(store: &TimelineStore, path: &Path) -> Result<(usize, usize, usize, usize)> {
    let world: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut ids = WorldIds::default();
    for s in world["sources"].as_array().into_iter().flatten() {
        let id = lookup(store, "SELECT source_id FROM sources WHERE display_name=?", s["display_name"].as_str().unwrap_or(""))?;
        ids.sources.insert(s["alias"].as_str().unwrap_or("").into(), id);
    }
    let mut staged = BTreeMap::new();
    for (k, v) in world["staging"].as_object().into_iter().flatten() {
        let bytes = v.as_str().unwrap_or("").as_bytes().to_vec();
        let path = store.staging_directory().join(format!("{}.jpg", crate::ids::uuid_v4()));
        std::fs::write(&path, &bytes)?;
        staged.insert(k.clone(), (path.to_string_lossy().into_owned(), sha256_hex(&bytes)));
    }
    let mut totals = (0, 0, 0, 0);
    for b in world["batches"].as_array().into_iter().flatten() {
        let source = ids.sources.get(b["source"].as_str().unwrap_or("")).cloned().unwrap_or_default();
        let batch: IngestBatch = serde_json::from_value(resolve(&json!({"source_id": source, "threads": b["threads"], "events": b["events"]}), &ids, &staged)?)?;
        let r = store.ingest(&batch)?;
        totals = (totals.0 + r.inserted, totals.1 + r.duplicates, totals.2 + r.conflicts, totals.3 + r.tombstoned);
    }
    Ok(totals)
}

/// Every event of a root as implementation-independent facts: event ids are replaced by source keys (also inside
/// payload `*_event_id` fields), text by chunk hashes, blobs by hash and role. Sorted by source key.
pub fn event_projection(db_path: &Path) -> Result<Vec<Value>> {
    let db = Db::open(db_path, true, false)?;
    use crate::db::RowExt;
    let keys: BTreeMap<String, String> = db.query("SELECT event_id, source_key FROM events", &[], |r| (r.text(0), r.text(1)))?.into_iter().collect();
    let rows = db.query(
        "SELECT e.event_id,e.source_key,e.kind,e.start_ms,e.end_ms,e.time_basis,e.app_bundle_id,e.app_name,e.title,e.actor,e.sensitivity,e.payload_json,
                (SELECT source_key FROM events p WHERE p.event_id=e.parent_event_id),(SELECT source_key FROM threads t WHERE t.thread_id=e.thread_id),s.kind
         FROM events e JOIN sources s USING(source_id) ORDER BY e.source_key",
        &[],
        |r| {
            (
                r.text(0),
                json!({"source_key": r.text(1), "kind": r.text(2), "start_ms": r.opt_int(3), "end_ms": r.opt_int(4), "time_basis": r.text(5),
                       "app_bundle_id": r.opt_text(6), "app_name": r.opt_text(7), "title": r.opt_text(8), "actor": r.text(9), "sensitivity": r.text(10),
                       "parent": r.opt_text(12), "thread": r.opt_text(13), "source_kind": r.text(14)}),
                r.text(11),
            )
        },
    )?;
    let mut out = Vec::new();
    for (id, mut fact, payload) in rows {
        let mut p = json::parse(&payload).unwrap_or(Value::Null);
        if let Some(o) = p.as_object_mut() {
            for (k, v) in o.iter_mut() {
                if k.ends_with("_event_id") {
                    if let Some(key) = v.as_str().and_then(|s| keys.get(s)) {
                        *v = json!(format!("@{key}"));
                    }
                }
            }
        }
        fact["payload"] = json::normalise(p);
        fact["text"] = json!(db.query(
            "SELECT t.field||'|'||t.ord||'|'||c.chunk_hash||'|'||COALESCE(t.visible,'-')||'|'||COALESCE(t.focus_distance,'-')||'|'||t.is_new||'|'||COALESCE(t.char_start,'-')
             FROM event_text t JOIN text_chunks c USING(chunk_id) WHERE t.event_id=? ORDER BY t.field, t.ord",
            &[text(&id)],
            |r| r.text(0)
        )?);
        fact["blobs"] = json!(db.query(
            "SELECT eb.sha256||'|'||eb.role||'|'||b.byte_count||'|'||b.mime_type||'|'||b.rel_path FROM event_blobs eb JOIN blobs b USING(sha256) WHERE eb.event_id=? ORDER BY 1",
            &[text(&id)],
            |r| r.text(0)
        )?);
        out.push(fact);
    }
    Ok(out)
}

/// Schema objects of a database (`sqlite_master`), for comparing the two implementations' DDL.
pub fn schema_rows(db_path: &Path) -> Result<Vec<String>> {
    use crate::db::RowExt;
    let db = Db::open(db_path, true, false)?;
    db.query("SELECT type||' '||name||' '||tbl_name||' '||COALESCE(sql,'') FROM sqlite_master ORDER BY type, name", &[], |r| r.text(0))
}

/// The timeline event the ledger mapping makes of one ledger record payload, with staged paths replaced by
/// `<staged>` (the golden file the Swift capture's own mapping is checked against).
pub fn map_ledger_record(store: &TimelineStore, payload: &[u8], parent_exists: bool, blobs_dir: &Path) -> Result<IngestEvent> {
    let record: crate::desktop::DesktopRecord =
        serde_json::from_slice(payload).map_err(|e| crate::TimelineError::InvalidInput(e.to_string()))?;
    let mut staged = Vec::new();
    let mut event = store.desktop_event(&record, parent_exists, blobs_dir, &mut staged)?;
    for f in staged {
        let _ = std::fs::remove_file(f);
    }
    for b in &mut event.blobs {
        b.staging_path = "<staged>".into();
    }
    Ok(event)
}
