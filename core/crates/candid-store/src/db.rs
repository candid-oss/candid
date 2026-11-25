//! Thin SQLite wrapper with the conversion rules of the C API (`sqlite3_column_int64` on a REAL truncates, and so on),
//! so queries behave like the Swift wrapper whatever type a column holds.

use crate::{Result, TimelineError};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags, Row};
use std::path::Path;

pub type P = Value;

pub fn text(s: impl Into<String>) -> P {
    Value::Text(s.into())
}
pub fn int(n: i64) -> P {
    Value::Integer(n)
}
pub fn opt_text(s: Option<impl Into<String>>) -> P {
    s.map(|s| Value::Text(s.into())).unwrap_or(Value::Null)
}
pub fn opt_int(n: Option<i64>) -> P {
    n.map(Value::Integer).unwrap_or(Value::Null)
}
pub fn boolean(b: bool) -> P {
    Value::Integer(b as i64)
}

/// Column access in the C API's coercion rules.
pub trait RowExt {
    fn int(&self, i: usize) -> i64;
    fn opt_int(&self, i: usize) -> Option<i64>;
    fn double(&self, i: usize) -> f64;
    fn text(&self, i: usize) -> String;
    fn opt_text(&self, i: usize) -> Option<String>;
    fn blob(&self, i: usize) -> Vec<u8>;
    fn is_null(&self, i: usize) -> bool;
}

fn leading_number(s: &str) -> f64 {
    let t = s.trim_start();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0) || c == 'e' || c == 'E' {
            end = i + 1;
        } else {
            break;
        }
    }
    let mut cut = &t[..end];
    while !cut.is_empty() && cut.parse::<f64>().is_err() {
        cut = &cut[..cut.len() - 1];
    }
    cut.parse().unwrap_or(0.0)
}

impl RowExt for Row<'_> {
    fn int(&self, i: usize) -> i64 {
        match self.get_ref(i) {
            Ok(ValueRef::Integer(n)) => n,
            Ok(ValueRef::Real(f)) => f as i64,
            Ok(ValueRef::Text(t)) => leading_number(&String::from_utf8_lossy(t)) as i64,
            _ => 0,
        }
    }
    fn opt_int(&self, i: usize) -> Option<i64> {
        if self.is_null(i) {
            None
        } else {
            Some(self.int(i))
        }
    }
    fn double(&self, i: usize) -> f64 {
        match self.get_ref(i) {
            Ok(ValueRef::Integer(n)) => n as f64,
            Ok(ValueRef::Real(f)) => f,
            Ok(ValueRef::Text(t)) => leading_number(&String::from_utf8_lossy(t)),
            _ => 0.0,
        }
    }
    fn text(&self, i: usize) -> String {
        match self.get_ref(i) {
            Ok(ValueRef::Text(t)) | Ok(ValueRef::Blob(t)) => String::from_utf8_lossy(t).into_owned(),
            Ok(ValueRef::Integer(n)) => n.to_string(),
            Ok(ValueRef::Real(f)) => format!("{f:?}"),
            _ => String::new(),
        }
    }
    fn opt_text(&self, i: usize) -> Option<String> {
        if self.is_null(i) {
            None
        } else {
            Some(self.text(i))
        }
    }
    fn blob(&self, i: usize) -> Vec<u8> {
        match self.get_ref(i) {
            Ok(ValueRef::Blob(b)) | Ok(ValueRef::Text(b)) => b.to_vec(),
            _ => Vec::new(),
        }
    }
    fn is_null(&self, i: usize) -> bool {
        matches!(self.get_ref(i), Ok(ValueRef::Null) | Err(_))
    }
}

/// One SQLite connection. The owning store serialises all use.
pub struct Db {
    pub conn: Connection,
    /// Rows written through this connection, counted in memory by a pre-update hook (writable connections only).
    pub writes: std::sync::Arc<WriteCounters>,
}

/// Tables whose first column is a key worth telling apart (`store_meta.key`, `background_revisions.domain`): a write
/// to them also counts under `table:<family>`, where the family is the key up to its first `/`.
const KEYED_TABLES: &[&str] = &["store_meta", "background_revisions"];
/// Keyed writes waiting to be resolved to their key; past this many they all count as "any key".
const MAX_PENDING_KEYS: usize = 4096;

/// In-memory write revisions: how many rows each table has had inserted, updated or deleted since the connection
/// opened. Reading them costs no SQL while nothing is written, so background work can tell "nothing I read has
/// changed" for free. Writes that are later rolled back still count (a revision only has to move when data may have
/// changed, never the reverse).
#[derive(Default)]
pub struct WriteCounters {
    state: std::sync::Mutex<Counts>,
    total: std::sync::atomic::AtomicU64,
    /// SQL statements run through [`Db`]'s helpers (reads included), so tests can assert that idle work runs none.
    statements: std::sync::atomic::AtomicU64,
    /// Called after every counted write (the background thread's wake-up); set once.
    listener: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
}

#[derive(Default)]
struct Counts {
    tables: std::collections::HashMap<Box<str>, u64>,
    /// Rows of keyed tables written since the last resolution, as (table index, rowid). The update hook cannot
    /// run SQL, so their keys are looked up on the next read of a revision.
    pending: Vec<(usize, i64)>,
    overflow: [bool; KEYED_TABLES.len()],
}

impl Counts {
    fn bump(&mut self, name: &str) {
        match self.tables.get_mut(name) {
            Some(n) => *n += 1,
            None => {
                self.tables.insert(name.into(), 1);
            }
        }
    }
}

impl WriteCounters {
    fn record(&self, table: &str, rowid: i64) {
        self.total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut c = self.state.lock().unwrap_or_else(|p| p.into_inner());
        c.bump(table);
        if let Some(i) = KEYED_TABLES.iter().position(|t| *t == table) {
            if c.pending.len() < MAX_PENDING_KEYS {
                c.pending.push((i, rowid));
            } else {
                c.overflow[i] = true;
            }
        }
    }

    /// Calls `f` after every write through this connection, whichever path made it, so whatever depends on the data
    /// can be woken instead of polling. Only the first listener is kept.
    pub fn set_listener(&self, f: impl Fn() + Send + Sync + 'static) -> bool {
        self.listener.set(Box::new(f)).is_ok()
    }

    /// Statements run so far through the connection's helpers.
    pub fn statements(&self) -> u64 {
        self.statements.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Every row written so far.
    pub fn total(&self) -> u64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Per-table counts, for tests and diagnostics (keyed families included once resolved).
    pub fn snapshot(&self) -> std::collections::BTreeMap<String, u64> {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).tables.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn install(self: &std::sync::Arc<Self>, conn: &Connection) {
        let counters = self.clone();
        conn.update_hook(Some(move |_action, db: &str, table: &str, rowid: i64| {
            if db == "main" {
                counters.record(table, rowid);
                if let Some(f) = counters.listener.get() {
                    f();
                }
            }
        }));
    }
}

impl Db {
    /// The sum of the write counters a reader depends on. Each dependency is a table name (`events`) or a key prefix
    /// of a keyed table (`store_meta:features.` matches `features.settings/1`). Counters only grow, so an unchanged
    /// sum means none of them moved. Costs no SQL unless a keyed table was written since the last call.
    pub fn write_revision(&self, reads: &[&str]) -> u64 {
        self.resolve_keyed_writes();
        let c = self.writes.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut sum = 0u64;
        for dep in reads {
            if let Some((table, _)) = dep.split_once(':') {
                // `table:?` counts writes whose key could not be read back (deleted rows): every prefix sees them.
                let unknown = format!("{table}:?");
                for (k, n) in &c.tables {
                    if k.starts_with(dep) || **k == *unknown {
                        sum = sum.wrapping_add(*n);
                    }
                }
            } else {
                sum = sum.wrapping_add(c.tables.get(*dep).copied().unwrap_or(0));
            }
        }
        sum
    }

    fn resolve_keyed_writes(&self) {
        let (pending, overflow) = {
            let mut c = self.writes.state.lock().unwrap_or_else(|p| p.into_inner());
            if c.pending.is_empty() && !c.overflow.iter().any(|o| *o) {
                return;
            }
            (std::mem::take(&mut c.pending), std::mem::take(&mut c.overflow))
        };
        let mut names: Vec<String> = vec![];
        for (i, table) in KEYED_TABLES.iter().enumerate() {
            if overflow[i] {
                names.push(format!("{table}:?"));
            }
            let mut rowids: Vec<i64> = pending.iter().filter(|(t, _)| *t == i).map(|(_, r)| *r).collect();
            rowids.sort_unstable();
            rowids.dedup();
            for chunk in rowids.chunks(500) {
                let sql = format!("SELECT rowid,CAST({} AS TEXT) FROM {table} WHERE rowid IN ({})", key_column(table), placeholders(chunk.len()));
                let params: Vec<P> = chunk.iter().map(|r| int(*r)).collect();
                let found: std::collections::HashMap<i64, String> = self.query(&sql, &params, |r| (r.int(0), r.text(1))).unwrap_or_default().into_iter().collect();
                for r in chunk {
                    match found.get(r) {
                        Some(key) => names.push(format!("{table}:{}", key.split('/').next().unwrap_or(key))),
                        None => names.push(format!("{table}:?")),
                    }
                }
            }
        }
        let mut c = self.writes.state.lock().unwrap_or_else(|p| p.into_inner());
        for n in names {
            c.bump(&n);
        }
    }
}

fn key_column(table: &str) -> &'static str {
    match table {
        "background_revisions" => "domain",
        _ => "key",
    }
}

fn percent_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Db {
    pub fn open(path: &Path, read_only: bool, create: bool) -> Result<Db> {
        let mut flags = OpenFlags::SQLITE_OPEN_NO_MUTEX;
        flags |= if read_only { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE };
        if create && !read_only {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let conn = Connection::open_with_flags(path, flags)?;
        if read_only {
            // A WAL database whose writer is not running has no -shm file, and a read-only connection cannot create
            // one. With no -wal file there is nothing uncommitted to read, so the file is opened as immutable instead.
            let probe = conn.query_row("SELECT 1 FROM sqlite_master LIMIT 1", [], |_| Ok(())).map(|_| ());
            let cant_open = matches!(&probe, Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::CannotOpen);
            let wal = format!("{}-wal", path.display());
            if cant_open && !Path::new(&wal).exists() {
                drop(conn);
                let uri = format!("file:{}?mode=ro&immutable=1", percent_path(&path.to_string_lossy()));
                let conn = Connection::open_with_flags(
                    uri,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(|_| {
                    TimelineError::Database(format!(
                        "无法只读打开 {}",
                        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    ))
                })?;
                conn.set_prepared_statement_cache_capacity(64);
                return Ok(Db { conn, writes: Default::default() });
            }
        }
        conn.set_prepared_statement_cache_capacity(64);
        let writes: std::sync::Arc<WriteCounters> = Default::default();
        if !read_only {
            writes.install(&conn);
        }
        Ok(Db { conn, writes })
    }

    /// Runs one statement; rows it returns (e.g. `PRAGMA journal_mode`) are read and dropped.
    pub fn execute(&self, sql: &str, params: &[P]) -> Result<()> {
        self.writes.statements.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
        while rows.next()?.is_some() {}
        Ok(())
    }

    /// Multi-statement script (DDL).
    pub fn script(&self, sql: &str) -> Result<()> {
        self.writes.statements.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    pub fn query<T>(&self, sql: &str, params: &[P], mut map: impl FnMut(&Row) -> T) -> Result<Vec<T>> {
        self.writes.statements.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map(row));
        }
        Ok(out)
    }

    pub fn first<T>(&self, sql: &str, params: &[P], mut map: impl FnMut(&Row) -> T) -> Result<Option<T>> {
        self.writes.statements.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
        Ok(rows.next()?.map(&mut map))
    }

    pub fn scalar(&self, sql: &str, params: &[P]) -> Result<i64> {
        Ok(self.first(sql, params, |r| r.opt_int(0).unwrap_or(0))?.unwrap_or(0))
    }

    pub fn changes(&self) -> u64 {
        self.conn.changes()
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// `BEGIN IMMEDIATE` … `COMMIT`, rolled back on any error.
    pub fn transaction<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        self.execute("BEGIN IMMEDIATE", &[])?;
        match work() {
            Ok(v) => {
                if let Err(e) = self.execute("COMMIT", &[]) {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(e);
                }
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

pub fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}
