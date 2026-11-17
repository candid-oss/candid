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
                return Ok(Db { conn });
            }
        }
        conn.set_prepared_statement_cache_capacity(64);
        Ok(Db { conn })
    }

    /// Runs one statement; rows it returns (e.g. `PRAGMA journal_mode`) are read and dropped.
    pub fn execute(&self, sql: &str, params: &[P]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
        while rows.next()?.is_some() {}
        Ok(())
    }

    /// Multi-statement script (DDL).
    pub fn script(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    pub fn query<T>(&self, sql: &str, params: &[P], mut map: impl FnMut(&Row) -> T) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(map(row));
        }
        Ok(out)
    }

    pub fn first<T>(&self, sql: &str, params: &[P], mut map: impl FnMut(&Row) -> T) -> Result<Option<T>> {
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
