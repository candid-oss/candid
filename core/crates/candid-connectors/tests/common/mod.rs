//! Synthetic fixtures only: invented addresses (example.test), invented text, temporary directories.
//! No test opens the real ~/Library/Mail, browser history or the App's data directory.
#![allow(dead_code)]
use candid_store::{testing::RowExt, Clock, TimelineStore, Zone};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 2026-11-11T12:00:00Z.
pub const NOW: i64 = 1_794_398_400_000;
pub const ACCOUNT: &str = "SYNTHETIC-ACCOUNT";

pub fn temp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("candid-connectors-synthetic-{label}-{}", candid_store::uuid_v4()));
    fs::create_dir_all(&d).unwrap();
    d
}

pub fn store(dir: &Path) -> Mutex<TimelineStore> {
    Mutex::new(TimelineStore::open(dir.join("db"), "synthetic", true, Clock::fixed(NOW, Zone::named("UTC").unwrap())).unwrap())
}

/// A plain UTF-8 synthetic message.
pub fn mail(mid: Option<&str>, from: &str, to: &str, subject: &str, date: &str, extra: &str, body: &str) -> Vec<u8> {
    let mut s = String::new();
    if let Some(m) = mid {
        s.push_str(&format!("Message-ID: <{m}>\r\n"));
    }
    s.push_str(&format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nDate: {date}\r\nMIME-Version: 1.0\r\n{extra}Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body}\r\n"));
    s.into_bytes()
}

pub struct MailTree {
    pub root: PathBuf,
    pub db: Connection,
}

impl MailTree {
    pub fn new(dir: &Path) -> Self {
        Self::with_layout(dir, "V10")
    }
    pub fn with_layout(dir: &Path, layout: &str) -> Self {
        let root = dir.join("Mail");
        fs::create_dir_all(root.join(layout).join("MailData")).unwrap();
        let db = Connection::open(root.join(layout).join("MailData/Envelope Index")).unwrap();
        db.execute_batch(
            "CREATE TABLE mailboxes(ROWID INTEGER PRIMARY KEY, url TEXT, total_count INTEGER);
             CREATE TABLE subjects(ROWID INTEGER PRIMARY KEY, subject TEXT);
             CREATE TABLE addresses(ROWID INTEGER PRIMARY KEY, address TEXT, comment TEXT);
             CREATE TABLE message_global_data(ROWID INTEGER PRIMARY KEY, message_id INTEGER, message_id_header TEXT);
             CREATE TABLE messages(ROWID INTEGER PRIMARY KEY, message_id INTEGER, global_message_id INTEGER, remote_id INTEGER,
               sender INTEGER, subject INTEGER, date_sent INTEGER, date_received INTEGER, mailbox INTEGER, flags INTEGER,
               read INTEGER, deleted INTEGER, size INTEGER);",
        )
        .unwrap();
        MailTree { root, db }
    }
    pub fn mailbox(&self, name: &str) -> i64 {
        let url = format!("imap://{ACCOUNT}/{}", name.replace(' ', "%20"));
        self.db.execute("INSERT INTO mailboxes(url) VALUES(?1)", [url]).unwrap();
        self.db.last_insert_rowid()
    }
    fn dir_of(&self, mailbox: i64) -> PathBuf {
        let url: String = self.db.query_row("SELECT url FROM mailboxes WHERE ROWID=?1", [mailbox], |r| r.get(0)).unwrap();
        let path = url.split_once(&format!("{ACCOUNT}/")).unwrap().1.replace("%20", " ");
        let mut d = self.root.join("V10").join(ACCOUNT);
        for c in path.split('/') {
            d = d.join(format!("{c}.mbox"));
        }
        d.join("SYNTHETIC-STORE-UUID/Data/1/Messages")
    }
    /// Adds an index row; `file`: None = body never downloaded, Some(partial).
    pub fn add(&self, mailbox: i64, raw: &[u8], mid: Option<&str>, subject: &str, from: &str, received_s: i64, file: Option<bool>) -> i64 {
        self.db.execute("INSERT INTO subjects(subject) VALUES(?1)", [subject]).unwrap();
        let subj = self.db.last_insert_rowid();
        self.db.execute("INSERT INTO addresses(address,comment) VALUES(?1,'')", [from]).unwrap();
        let addr = self.db.last_insert_rowid();
        let g = match mid {
            Some(m) => {
                self.db.execute("INSERT INTO message_global_data(message_id,message_id_header) VALUES(1,?1)", [format!("<{m}>")]).unwrap();
                Some(self.db.last_insert_rowid())
            }
            None => None,
        };
        self.db
            .execute(
                "INSERT INTO messages(global_message_id,sender,subject,date_sent,date_received,mailbox,flags,read,deleted,size) VALUES(?1,?2,?3,?4,?4,?5,0,0,0,?6)",
                rusqlite::params![g, addr, subj, received_s, mailbox, raw.len() as i64],
            )
            .unwrap();
        let rowid = self.db.last_insert_rowid();
        if let Some(partial) = file {
            let d = self.dir_of(mailbox);
            fs::create_dir_all(&d).unwrap();
            let mut data = format!("{}\n", raw.len()).into_bytes();
            data.extend_from_slice(raw);
            data.extend_from_slice(b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>flags</key><integer>0</integer></dict></plist>\n");
            let name = if partial { format!("{rowid}.partial.emlx") } else { format!("{rowid}.emlx") };
            fs::write(d.join(name), data).unwrap();
        }
        rowid
    }
    /// Mail.app moves a message by giving it a new row in the target mailbox.
    pub fn move_row(&self, rowid: i64, to_mailbox: i64) -> i64 {
        let raw = self.emlx(rowid);
        let (g, sender, subj, date): (Option<i64>, i64, i64, i64) =
            self.db.query_row("SELECT global_message_id,sender,subject,date_received FROM messages WHERE ROWID=?1", [rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        self.db.execute("DELETE FROM messages WHERE ROWID=?1", [rowid]).unwrap();
        self.db
            .execute(
                "INSERT INTO messages(global_message_id,sender,subject,date_sent,date_received,mailbox,flags,read,deleted,size) VALUES(?1,?2,?3,?4,?4,?5,0,0,0,0)",
                rusqlite::params![g, sender, subj, date, to_mailbox],
            )
            .unwrap();
        let new = self.db.last_insert_rowid();
        if let Some(raw) = raw {
            let d = self.dir_of(to_mailbox);
            fs::create_dir_all(&d).unwrap();
            let mut data = format!("{}\n", raw.len()).into_bytes();
            data.extend_from_slice(&raw);
            fs::write(d.join(format!("{new}.emlx")), data).unwrap();
        }
        new
    }
    fn emlx(&self, rowid: i64) -> Option<Vec<u8>> {
        for e in walk(&self.root) {
            if e.file_name().unwrap().to_string_lossy() == format!("{rowid}.emlx") {
                let data = fs::read(&e).unwrap();
                return candid_connectors::mail::apple::emlx_message(&data).map(|m| m.to_vec());
            }
        }
        None
    }
    pub fn expunge(&self, rowid: i64) {
        self.db.execute("DELETE FROM messages WHERE ROWID=?1", [rowid]).unwrap();
    }
    pub fn config(&self) -> Value {
        json!({"version": 1, "mail_root": self.root.to_string_lossy(), "self_addresses": ["me@example.test"]})
    }
}

pub fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(d) {
        for e in rd.flatten() {
            if e.file_type().unwrap().is_dir() {
                out.extend(walk(&e.path()));
            } else {
                out.push(e.path());
            }
        }
    }
    out
}

pub fn cmd(store: &Mutex<TimelineStore>, dir: &Path, v: Value) -> Value {
    candid_connectors::command(store, &v, dir).unwrap_or_else(|f| panic!("{v} failed: {f:?}"))
}

pub fn cmd_err(store: &Mutex<TimelineStore>, dir: &Path, v: Value) -> String {
    candid_connectors::command(store, &v, dir).unwrap_err().code
}

pub fn count(store: &Mutex<TimelineStore>, sql: &str) -> i64 {
    store.lock().unwrap().raw_db().scalar(sql, &[]).unwrap()
}

pub fn texts(store: &Mutex<TimelineStore>, key: &str, field: &str) -> Vec<String> {
    store
        .lock()
        .unwrap()
        .raw_db()
        .query(
            "SELECT c.text FROM event_text t JOIN text_chunks c USING(chunk_id) JOIN events e USING(event_id) WHERE e.source_key=? AND t.field=? ORDER BY t.ord",
            &[candid_store::testing::rusqlite::types::Value::Text(key.into()), candid_store::testing::rusqlite::types::Value::Text(field.into())],
            |r| r.text(0),
        )
        .unwrap()
}

pub fn payload(store: &Mutex<TimelineStore>, key: &str) -> Value {
    let s: String = store
        .lock()
        .unwrap()
        .raw_db()
        .first("SELECT payload_json FROM events WHERE source_key=?", &[candid_store::testing::rusqlite::types::Value::Text(key.into())], |r| r.text(0))
        .unwrap()
        .unwrap_or_else(|| panic!("no event {key}"));
    serde_json::from_str(&s).unwrap()
}
