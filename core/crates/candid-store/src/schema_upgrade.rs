//! Transactional upgrades v1 → … → v13. The database commit precedes atomic store.json replacement;
//! a durable marker permits only that particular interrupted replacement to resume.
use crate::{
    db::{text, Db, RowExt},
    schema,
    types::StoreIdentity,
    Result, TimelineError,
};
use std::{fs, path::Path};

pub(crate) fn upgrade(
    db: &Db,
    root: &Path,
    identity: &mut StoreIdentity,
    version: i64,
) -> Result<()> {
    if version == 1 && identity.schema_version == 1 {
        db.execute("PRAGMA foreign_keys=OFF", &[])?;
        let result = db.transaction(|| {
            // SQLite's documented create/copy/drop/rename procedure leaves referencing tables intact.
            rebuild(db, &["sources", "threads", "event_text"], "v2")?;
            db.execute("CREATE UNIQUE INDEX sources_agent ON sources(json_extract(policy_json,'$.agent')) WHERE kind='agent.transcript'",&[])?;
            db.script("CREATE TABLE agent_event_revisions(event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE, revision INTEGER NOT NULL, event_json TEXT NOT NULL, created_ms INTEGER NOT NULL, PRIMARY KEY(event_id,revision)) STRICT;")?;
            for k in schema::KINDS.iter().filter(|k| k.kind == "ai.prompt" || k.kind == "ai.work") {
                db.execute("INSERT INTO event_kinds VALUES(?,?,?,1,?)", &[text(k.kind),text(k.layer),crate::db::boolean(k.is_span),text(k.description)])?;
            }
            if !db.query("PRAGMA foreign_key_check", &[], |_| true)?.is_empty() { return Err(TimelineError::InvalidStore("升级后的来源引用不完整".into())); }
            db.execute("INSERT INTO store_meta(key,value) VALUES('schema.upgrade','1:2')", &[])?;
            db.execute("PRAGMA user_version=2", &[])
        });
        db.execute("PRAGMA foreign_keys=ON", &[])?;
        result?;
    } else if version != identity.schema_version {
        let marker = db.first("SELECT value FROM store_meta WHERE key='schema.upgrade'", &[], |r| r.text(0))?;
        let pending = (version == 13 && identity.schema_version <= 12 && marker.as_deref() == Some("12:13"))
            || (version == 12 && identity.schema_version <= 11 && marker.as_deref() == Some("11:12"))
            || (version == 11 && identity.schema_version <= 10 && marker.as_deref() == Some("10:11"))
            || (version == 10 && identity.schema_version <= 9 && marker.as_deref() == Some("9:10"))
            || (version == 9 && identity.schema_version <= 8 && marker.as_deref() == Some("8:9"))
            || (version == 8 && identity.schema_version <= 7 && marker.as_deref() == Some("7:8"))
            || (version == 7 && identity.schema_version <= 6 && marker.as_deref() == Some("6:7"))
            || (version == 6 && identity.schema_version <= 5 && marker.as_deref() == Some("5:6"))
            || (version == 5 && identity.schema_version <= 4 && marker.as_deref() == Some("4:5"))
            || (version == 4
            && identity.schema_version <= 3
            && db.first("SELECT value FROM store_meta WHERE key='schema.upgrade'", &[], |r| r.text(0))?.as_deref() == Some("3:4"))
            || (version == 3
            && identity.schema_version <= 2
            && db
                .first(
                    "SELECT value FROM store_meta WHERE key='schema.upgrade'",
                    &[],
                    |r| r.text(0),
                )?
                .as_deref()
                == Some("2:3"))
            || version == 2
                && identity.schema_version == 1
                && db
                    .first(
                        "SELECT value FROM store_meta WHERE key='schema.upgrade'",
                        &[],
                        |r| r.text(0),
                    )?
                    .as_deref()
                    == Some("1:2");
        if !pending {
            return Err(TimelineError::InvalidStore(
                "store.json 与数据库版本不一致".into(),
            ));
        }
    }
    if version < 1 || version > schema::SCHEMA_VERSION {
        return Err(TimelineError::InvalidStore("不支持的旧数据库版本".into()));
    }
    let current = db.scalar("PRAGMA user_version", &[])?;
    if current == 2 {
        db.transaction(|| {
            db.script(crate::durable::DDL)?;
            db.execute(
                "INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','2:3')",
                &[],
            )?;
            db.execute("PRAGMA user_version=3", &[])
        })?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 3 {
        // v4: mail/browser source kinds, the `quoted` text field, `source_deleted` tombstones, `browser.visit`.
        db.execute("PRAGMA foreign_keys=OFF", &[])?;
        let result = db.transaction(|| {
            rebuild(db, &["sources", "event_text", "tombstones"], "v4")?;
            for k in schema::KINDS.iter().filter(|k| k.kind == "browser.visit") {
                db.execute("INSERT OR IGNORE INTO event_kinds VALUES(?,?,?,1,?)", &[text(k.kind), text(k.layer), crate::db::boolean(k.is_span), text(k.description)])?;
            }
            if !db.query("PRAGMA foreign_key_check", &[], |_| true)?.is_empty() {
                return Err(TimelineError::InvalidStore("升级后的来源引用不完整".into()));
            }
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','3:4')", &[])?;
            db.execute("PRAGMA user_version=4", &[])
        });
        db.execute("PRAGMA foreign_keys=ON", &[])?;
        result?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 4 {
        // v5: derived layer for the AI design (timeline-v1 §4.8.1): digests, entities, wider memory and
        // correction kinds, and the feature a job belongs to.
        db.execute("PRAGMA foreign_keys=OFF", &[])?;
        let result = db.transaction(|| {
            rebuild(db, &["memories", "corrections"], "v5")?;
            for name in ["digests", "entities", "entity_aliases", "entity_mentions"] {
                create(db, name)?;
            }
            db.execute("ALTER TABLE jobs ADD COLUMN feature TEXT", &[])?;
            if !db.query("PRAGMA foreign_key_check", &[], |_| true)?.is_empty() {
                return Err(TimelineError::InvalidStore("升级后的派生引用不完整".into()));
            }
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','4:5')", &[])?;
            db.execute("PRAGMA user_version=5", &[])
        });
        db.execute("PRAGMA foreign_keys=ON", &[])?;
        result?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 5 {
        db.transaction(|| {
            crate::performance::install(db)?;
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','5:6')", &[])?;
            db.execute("PRAGMA user_version=6", &[])
        })?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 6 {
        db.execute("PRAGMA foreign_keys=OFF", &[])?;
        let result = db.transaction(|| {
            rebuild(db, &["sources"], "v7")?;
            if !db.query("PRAGMA foreign_key_check", &[], |_| true)?.is_empty() {
                return Err(TimelineError::InvalidStore("升级后的来源引用不完整".into()));
            }
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','6:7')", &[])?;
            db.execute("PRAGMA user_version=7", &[])
        });
        db.execute("PRAGMA foreign_keys=ON", &[])?;
        result?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 7 {
        db.transaction(|| {
            db.script(crate::agents::CHECKPOINT_DDL)?;
            crate::performance::install_media_projection(db)?;
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','7:8')", &[])?;
            db.execute("PRAGMA user_version=8", &[])
        })?;
    }
    let mut vacuum = false;
    if db.scalar("PRAGMA user_version", &[])? == 8 {
        // Data model v1: large content is stored once. Revisions of screen inputs were written and never read;
        // inputs keep references instead of text. Old inline session cursors stay: moved row by row into the entry
        // table they grow (every entry repeats its file path and hash), so they wait for interned entries.
        db.transaction(|| {
            db.execute("DELETE FROM store_meta WHERE key LIKE 'screen.revision/%'", &[])?;
            let inputs = db.query("SELECT key,value FROM store_meta WHERE key LIKE 'screen.input/%'", &[], |r| (r.text(0), r.text(1)))?;
            for (key, value) in inputs {
                let value: serde_json::Value = serde_json::from_str(&value).map_err(|e| TimelineError::InvalidStore(e.to_string()))?;
                db.execute("UPDATE store_meta SET value=? WHERE key=?", &[text(crate::screen_pipeline::slim_screen_input(value).to_string()), text(&key)])?;
            }
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','8:9')", &[])?;
            db.execute("PRAGMA user_version=9", &[])
        })?;
        vacuum = true;
    }
    if db.scalar("PRAGMA user_version", &[])? == 9 {
        // Screen inputs and evidence selectors stop repeating the event id and family hash on every line.
        db.transaction(|| {
            let inputs = db.query("SELECT key,value FROM store_meta WHERE key LIKE 'screen.input/%'", &[], |r| (r.text(0), r.text(1)))?;
            for (key, value) in inputs {
                let value: serde_json::Value = serde_json::from_str(&value).map_err(|e| TimelineError::InvalidStore(e.to_string()))?;
                db.execute("UPDATE store_meta SET value=? WHERE key=?", &[text(crate::screen_pipeline::slim_screen_input(value).to_string()), text(&key)])?;
            }
            let selectors = db.query("SELECT rowid,selector_json FROM episode_evidence WHERE json_valid(selector_json) AND json_extract(selector_json,'$.format')='screen-selector/1'", &[], |r| (r.int(0), r.text(1)))?;
            for (rowid, raw) in selectors {
                let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| TimelineError::InvalidStore(e.to_string()))?;
                let lines: Vec<serde_json::Value> = v["lines"].as_array().into_iter().flatten()
                    .map(|l| serde_json::json!([l["start"], l["end"], u8::from(l["is_new"] == true), l["weight"]])).collect();
                let compact = serde_json::json!({"format":"screen-selector/2","source_revision":v["source_revision"],"lines":lines});
                db.execute("UPDATE episode_evidence SET selector_json=? WHERE rowid=?", &[text(compact.to_string()), crate::db::int(rowid)])?;
            }
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','9:10')", &[])?;
            db.execute("PRAGMA user_version=10", &[])
        })?;
        vacuum = true;
    }
    if db.scalar("PRAGMA user_version", &[])? == 10 {
        // The AI-readable layer: conversations, messages and excerpts; filled by `curated_sync` from the records.
        db.transaction(|| {
            db.script(crate::curated::DDL)?;
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','10:11')", &[])?;
            db.execute("PRAGMA user_version=11", &[])
        })?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 11 {
        // v12: agent conversations and the item tables. The conversations rebuild needs foreign keys off.
        // legacy_alter_table: the rename must not re-check triggers that mention the table dropped a moment ago.
        db.execute("PRAGMA foreign_keys=OFF", &[])?;
        db.execute("PRAGMA legacy_alter_table=ON", &[])?;
        let result = db.transaction(|| {
            crate::curated::upgrade_v12(db)?;
            if !db.query("PRAGMA foreign_key_check", &[], |_| true)?.is_empty() {
                return Err(TimelineError::InvalidStore("升级后的整理层引用不完整".into()));
            }
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','11:12')", &[])?;
            db.execute("PRAGMA user_version=12", &[])
        });
        db.execute("PRAGMA legacy_alter_table=OFF", &[])?;
        db.execute("PRAGMA foreign_keys=ON", &[])?;
        result?;
    }
    if db.scalar("PRAGMA user_version", &[])? == 12 {
        // v13 (power v1 §9.1): the energy ledger and the manual flag on jobs. Columns are added only when absent,
        // so a store rebuilt from a newer one by a test (or a resumed upgrade) takes the same path.
        db.transaction(|| {
            let columns = db.query("SELECT name FROM pragma_table_info('jobs')", &[], |r| r.text(0))?;
            for (name, ddl) in [
                ("cpu_ms", "cpu_ms INTEGER"),
                ("wall_ms", "wall_ms INTEGER"),
                ("power_state", "power_state TEXT CHECK (power_state IN ('ac','battery','unknown'))"),
                ("manual", "manual INTEGER NOT NULL DEFAULT 0"),
            ] {
                if !columns.iter().any(|c| c == name) {
                    db.execute(&format!("ALTER TABLE jobs ADD COLUMN {ddl}"), &[])?;
                }
            }
            db.execute("CREATE INDEX IF NOT EXISTS jobs_updated ON jobs(updated_ms)", &[])?;
            db.execute("INSERT OR REPLACE INTO store_meta VALUES('schema.upgrade','12:13')", &[])?;
            db.execute("PRAGMA user_version=13", &[])
        })?;
    }
    if vacuum {
        // Give the freed pages back once; outside the transactions by necessity.
        db.script("VACUUM")?;
    }
    if identity.schema_version != schema::SCHEMA_VERSION {
        identity.schema_version = schema::SCHEMA_VERSION;
        let temp = root.join(".store.json.tmp");
        let file = fs::File::create(&temp)?;
        serde_json::to_writer(&file, &identity)
            .map_err(|e| TimelineError::InvalidStore(e.to_string()))?;
        file.sync_all()?;
        fs::rename(&temp, root.join("store.json"))?;
        fs::File::open(root)?.sync_all()?;
    }
    Ok(())
}

/// SQLite's documented create/copy/drop/rename procedure; the target DDL is the current one, so a table rebuilt
/// at an earlier step is simply copied again. Indices on the table are recreated from their stored SQL.
fn rebuild(db: &Db, names: &[&str], suffix: &str) -> Result<()> {
    for name in names {
        let indices = db.query("SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name=? AND sql IS NOT NULL", &[text(*name)], |r| r.text(0))?;
        let triggers = db.query("SELECT sql FROM sqlite_master WHERE type='trigger' AND tbl_name=? AND sql IS NOT NULL", &[text(*name)], |r| r.text(0))?;
        let marker = format!("CREATE TABLE {name} (");
        let start = schema::DDL.find(&marker).unwrap();
        let end = start + schema::DDL[start..].find(") STRICT;").unwrap() + ") STRICT;".len();
        let ddl = schema::DDL[start..end].replacen(&marker, &format!("CREATE TABLE {name}_{suffix} ("), 1);
        db.script(&ddl)?;
        db.script(&format!("INSERT INTO {name}_{suffix} SELECT * FROM {name}; DROP TABLE {name}; ALTER TABLE {name}_{suffix} RENAME TO {name};"))?;
        for trigger in triggers { db.script(&trigger)?; }
        for index in indices {
            db.script(&index)?;
        }
    }
    Ok(())
}

/// Creates a table added in a later version, with its indices, from the current DDL (one source of truth).
fn create(db: &Db, name: &str) -> Result<()> {
    let marker = format!("CREATE TABLE {name} (");
    let start = schema::DDL.find(&marker).unwrap();
    let end = start + schema::DDL[start..].find(") STRICT;").unwrap() + ") STRICT;".len();
    db.script(&schema::DDL[start..end])?;
    let on = format!(" ON {name}(");
    for line in schema::DDL.lines().filter(|l| l.starts_with("CREATE INDEX") && l.contains(&on)) {
        db.script(line)?;
    }
    Ok(())
}
