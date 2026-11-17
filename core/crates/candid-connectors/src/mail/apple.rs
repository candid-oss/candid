//! Adapter A: the macOS Mail app's own data, read-only (`~/Library/Mail/V<n>/MailData/Envelope Index` plus the
//! `.emlx` / `.partial.emlx` files). The index layout is recognised at runtime and refused when unknown; no
//! AppleScript, no flag changes, no download requests. Without Full Disk Access the read fails with
//! `full_disk_access_required`, which the caller reports once.
use super::config::{role_of, MailConfig};
use super::{mid_key, mime, IndexMeta, Known, Location, RawMail, Scan};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Layouts whose structure the adapter knows. Only exercised with synthetic data so far; a real read-only trial
/// in the Candid test build reports counts before this is called verified.
pub const KNOWN_LAYOUTS: &[&str] = &["V10"];
pub const FORMAT: &str = "apple-mail-envelope/v10";

const REQUIRED: &[(&str, &[&str])] = &[
    ("messages", &["ROWID", "global_message_id", "sender", "subject", "date_sent", "date_received", "mailbox", "deleted"]),
    ("mailboxes", &["ROWID", "url"]),
    ("subjects", &["ROWID", "subject"]),
    ("addresses", &["ROWID", "address", "comment"]),
    ("message_global_data", &["ROWID", "message_id_header"]),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Cursor {
    v: i64,
    layout: String,
    last_rowid: i64,
}

pub fn default_root() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Mail")
}

fn io_code(e: &std::io::Error) -> String {
    match e.kind() {
        ErrorKind::PermissionDenied => "full_disk_access_required".into(),
        ErrorKind::NotFound => "mail_data_not_found".into(),
        _ => "mail_data_unreadable".into(),
    }
}

/// The newest `V<n>` directory with an index; refuses an unknown layout.
fn locate(root: &Path) -> Result<(String, PathBuf), String> {
    let entries = fs::read_dir(root).map_err(|e| io_code(&e))?;
    let mut found: Vec<(u32, String)> = Vec::new();
    for e in entries {
        let e = e.map_err(|e| io_code(&e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix('V').and_then(|n| n.parse::<u32>().ok()) {
            if e.path().join("MailData/Envelope Index").exists() {
                found.push((n, name));
            }
        }
    }
    found.sort();
    let (_, layout) = found.pop().ok_or("mail_data_not_found")?;
    if !KNOWN_LAYOUTS.contains(&layout.as_str()) {
        return Err("unknown_mail_version".into());
    }
    Ok((layout.clone(), root.join(layout)))
}

fn open_index(path: &Path) -> Result<Connection, String> {
    // Read-only and never creating anything; WAL content of a running Mail app stays visible.
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| {
        if e.to_string().contains("authorization") || e.to_string().contains("permission") {
            "full_disk_access_required".to_string()
        } else {
            "mail_index_unreadable".to_string()
        }
    })?;
    db.busy_timeout(std::time::Duration::from_secs(5)).map_err(|_| "mail_index_unreadable")?;
    db.execute_batch("PRAGMA query_only=ON;").map_err(|_| "mail_index_unreadable")?;
    Ok(db)
}

fn check_schema(db: &Connection) -> Result<(), String> {
    for (table, cols) in REQUIRED {
        let mut stmt = db.prepare(&format!("PRAGMA table_info(\"{table}\")")).map_err(|e| {
            if e.to_string().contains("not authorized") { "full_disk_access_required".to_string() } else { "unknown_mail_schema".to_string() }
        })?;
        let have: BTreeSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|_| "unknown_mail_schema")?
            .filter_map(|r| r.ok())
            .map(|c| c.to_lowercase())
            .collect();
        if have.is_empty() && *table != "messages" {
            return Err("unknown_mail_schema".into());
        }
        for c in cols.iter() {
            // ROWID is implicit unless the table declares an alias.
            if *c != "ROWID" && !have.contains(&c.to_lowercase()) {
                return Err("unknown_mail_schema".into());
            }
        }
    }
    Ok(())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `imap://<account>/INBOX/Sub` → (scheme, account, "INBOX/Sub").
pub fn parse_mailbox_url(url: &str) -> Option<(String, String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let (account, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = percent_decode(path.trim_end_matches('/'));
    Some((scheme.to_string(), account.to_string(), path))
}

fn mailbox_dir(layout_dir: &Path, scheme: &str, account: &str, mailbox: &str) -> PathBuf {
    let base = if scheme == "local" { layout_dir.join("Mailboxes") } else { layout_dir.join(account) };
    mailbox.split('/').filter(|c| !c.is_empty() && *c != ".." && *c != ".").fold(base, |p, c| p.join(format!("{c}.mbox")))
}

/// rowid → (.emlx path, partial) under one mailbox directory; bounded walk, symlinks not followed.
fn index_messages(dir: &Path) -> HashMap<i64, (PathBuf, bool)> {
    let mut out = HashMap::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut budget = 1_000_000usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if budget == 0 {
                return out;
            }
            budget -= 1;
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                // Nested child mailboxes have their own directory and are indexed separately.
                if depth < 8 && !name.ends_with(".mbox") {
                    stack.push((e.path(), depth + 1));
                }
            } else if ft.is_file() {
                let (stem, partial) = if let Some(s) = name.strip_suffix(".partial.emlx") {
                    (s, true)
                } else if let Some(s) = name.strip_suffix(".emlx") {
                    (s, false)
                } else {
                    continue;
                };
                if let Ok(id) = stem.parse::<i64>() {
                    // A complete copy wins over a partial one.
                    if !partial || !out.contains_key(&id) {
                        out.insert(id, (e.path(), partial));
                    }
                }
            }
        }
    }
    out
}

/// `.emlx`: decimal byte count line, the RFC 822 message, then an XML property list (ignored).
pub fn emlx_message(data: &[u8]) -> Option<&[u8]> {
    let nl = data.iter().take(24).position(|b| *b == b'\n')?;
    let n: usize = std::str::from_utf8(&data[..nl]).ok()?.trim().parse().ok()?;
    let start = nl + 1;
    data.get(start..start.checked_add(n)?)
}

#[derive(Debug)]
struct Row {
    rowid: i64,
    mailbox: i64,
    message_id: Option<String>,
    deleted: bool,
    received_ms: Option<i64>,
    sent_ms: Option<i64>,
    subject: Option<String>,
    address: Option<String>,
    comment: Option<String>,
}

/// Probes access without reading message content (used by authorize/status).
pub fn probe(config: &MailConfig) -> Result<String, String> {
    let root = config.mail_root.as_ref().map(PathBuf::from).unwrap_or_else(default_root);
    let (layout, dir) = locate(&root)?;
    let db = open_index(&dir.join("MailData/Envelope Index"))?;
    check_schema(&db)?;
    Ok(layout)
}

type Boxes = HashMap<i64, (String, String, String)>;
type FileIndex = HashMap<PathBuf, HashMap<i64, (PathBuf, bool)>>;

/// Index rows of the newest known layout: `(layout, layout dir, mailbox id → (scheme, account, path), rows by ROWID)`.
fn load_index(config: &MailConfig) -> Result<(String, PathBuf, Boxes, Vec<Row>), String> {
    let root = config.mail_root.as_ref().map(PathBuf::from).unwrap_or_else(default_root);
    let (layout, layout_dir) = locate(&root)?;
    let db = open_index(&layout_dir.join("MailData/Envelope Index"))?;
    check_schema(&db)?;
    db.execute_batch("BEGIN").map_err(|_| "mail_index_unreadable")?;
    let mut boxes: Boxes = HashMap::new();
    {
        let mut stmt = db.prepare("SELECT ROWID,url FROM mailboxes").map_err(|_| "unknown_mail_schema")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).map_err(|_| "mail_index_unreadable")?;
        for r in rows.flatten() {
            if let Some((scheme, account, path)) = r.1.as_deref().and_then(parse_mailbox_url) {
                boxes.insert(r.0, (scheme, account, path));
            }
        }
    }
    let mut stmt = db
        .prepare(
            "SELECT m.ROWID,m.mailbox,g.message_id_header,m.deleted,m.date_received,m.date_sent,s.subject,a.address,a.comment
             FROM messages m LEFT JOIN message_global_data g ON g.ROWID=m.global_message_id
             LEFT JOIN subjects s ON s.ROWID=m.subject LEFT JOIN addresses a ON a.ROWID=m.sender ORDER BY m.ROWID",
        )
        .map_err(|_| "unknown_mail_schema")?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                rowid: r.get(0)?,
                mailbox: r.get::<_, Option<i64>>(1)?.unwrap_or(-1),
                message_id: r.get::<_, Option<String>>(2)?,
                deleted: r.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                received_ms: r.get::<_, Option<i64>>(4)?.map(|s| s * 1000),
                sent_ms: r.get::<_, Option<i64>>(5)?.map(|s| s * 1000),
                subject: r.get(6)?,
                address: r.get(7)?,
                comment: r.get(8)?,
            })
        })
        .map_err(|_| "mail_index_unreadable")?
        .collect::<Result<_, _>>()
        .map_err(|_| "mail_index_unreadable")?;
    drop(stmt);
    let _ = db.execute_batch("COMMIT");
    Ok((layout, layout_dir, boxes, rows))
}

fn row_key(row: &Row, account: &str, mailbox: &str) -> String {
    row.message_id
        .as_deref()
        .and_then(mime::normalize_id)
        .map(|id| mid_key(&id))
        .unwrap_or_else(|| format!("local:{account}:{mailbox}:{}", row.rowid))
}

/// `(bytes, partial, unreadable)` of one row's `.emlx`; `bytes` is `None` when the client never downloaded it.
fn load_bytes(layout_dir: &Path, files: &mut FileIndex, location: &(String, String, String), rowid: i64) -> Result<(Option<Vec<u8>>, bool, bool), String> {
    let (scheme, account, mailbox) = location;
    let dir = mailbox_dir(layout_dir, scheme, account, mailbox);
    let index = files.entry(dir.clone()).or_insert_with(|| index_messages(&dir));
    Ok(match index.get(&rowid) {
        Some((path, partial)) => match fs::read(path) {
            Ok(data) => (Some(emlx_message(&data).map(|m| m.to_vec()).unwrap_or_default()), *partial, false),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => return Err("full_disk_access_required".into()),
            Err(_) => (None, false, true),
        },
        None => (None, false, false),
    })
}

fn index_meta(row: &Row) -> IndexMeta {
    IndexMeta {
        subject: row.subject.clone(),
        from_address: row.address.as_ref().map(|a| a.trim().to_lowercase()),
        from_name: row.comment.clone().filter(|c| !c.trim().is_empty()),
        date_ms: row.sent_ms.or(row.received_ms),
    }
}

/// Reads the current copy of the given messages from Mail.app (read-only, nothing stored). A key maps to `None`
/// when the message is no longer in a readable mailbox (deleted, or only left in trash/junk/drafts).
pub fn read_live(config: &MailConfig, keys: &BTreeSet<String>) -> Result<BTreeMap<String, Option<RawMail>>, String> {
    let (_, layout_dir, boxes, rows) = load_index(config)?;
    let mut out: BTreeMap<String, Option<RawMail>> = keys.iter().map(|k| (k.clone(), None)).collect();
    let mut files: FileIndex = HashMap::new();
    for row in rows {
        let Some(location) = boxes.get(&row.mailbox) else { continue };
        let (_, account, mailbox) = location;
        if row.deleted || role_of(mailbox).excluded() {
            continue;
        }
        let key = row_key(&row, account, mailbox);
        if !keys.contains(&key) || out.get(&key).is_some_and(|v| v.is_some()) {
            continue;
        }
        let (bytes, partial, _) = load_bytes(&layout_dir, &mut files, location, row.rowid)?;
        out.insert(
            key.clone(),
            Some(RawMail { key, account: Some(account.clone()), mailbox: mailbox.clone(), bytes, partial, received_ms: row.received_ms, index: index_meta(&row) }),
        );
    }
    Ok(out)
}

/// The system account list (`~/Library/Accounts/Accounts4.sqlite`, read-only): account identifier (upper case) →
/// (user name, description). Empty when unreadable; Mail's folder URLs use the same identifiers.
fn system_accounts() -> HashMap<String, (Option<String>, Option<String>)> {
    let path = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Accounts/Accounts4.sqlite");
    let Ok(db) = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX) else {
        return HashMap::new();
    };
    let Ok(mut stmt) = db.prepare("SELECT ZIDENTIFIER, ZUSERNAME, ZACCOUNTDESCRIPTION FROM ZACCOUNT WHERE ZIDENTIFIER IS NOT NULL") else {
        return HashMap::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?.to_uppercase(), (r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Accounts and mailboxes the Mail app holds, with message counts, for choosing the import scope. Reads the index only.
/// `never`: junk, trash and drafts are never read.
pub fn scope_options(config: &MailConfig) -> Result<serde_json::Value, String> {
    // Aggregates only: counting in SQL keeps the scope list quick on large mailboxes (no message rows loaded).
    let root = config.mail_root.as_ref().map(PathBuf::from).unwrap_or_else(default_root);
    let (layout, layout_dir) = locate(&root)?;
    let db = open_index(&layout_dir.join("MailData/Envelope Index"))?;
    check_schema(&db)?;
    let mut boxes: Boxes = HashMap::new();
    let mut counts: HashMap<i64, usize> = HashMap::new();
    {
        let mut stmt = db
            .prepare("SELECT b.ROWID, b.url, (SELECT COUNT(*) FROM messages m WHERE m.mailbox=b.ROWID AND COALESCE(m.deleted,0)=0) FROM mailboxes b")
            .map_err(|_| "unknown_mail_schema")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)))
            .map_err(|_| "mail_index_unreadable")?;
        for (id, url, n) in rows.flatten() {
            if let Some(parsed) = url.as_deref().and_then(parse_mailbox_url) {
                boxes.insert(id, parsed);
                counts.insert(id, n as usize);
            }
        }
    }
    // Each account's own address, so the user sees which mailbox is which: the sender it uses most in its sent
    // folders; the system account list when that is readable; nothing otherwise.
    let mut senders: HashMap<&str, HashMap<String, usize>> = HashMap::new();
    for (id, (_, account, mailbox)) in &boxes {
        if role_of(mailbox) != super::config::Role::Sent {
            continue;
        }
        let Ok(mut stmt) = db.prepare(
            "SELECT a.address, COUNT(*) FROM messages m JOIN addresses a ON a.ROWID=m.sender WHERE m.mailbox=?1 AND COALESCE(m.deleted,0)=0 GROUP BY a.address ORDER BY 2 DESC LIMIT 3",
        ) else {
            continue;
        };
        let rows = stmt.query_map([id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)));
        for (address, n) in rows.into_iter().flatten().flatten() {
            if let Some(address) = address.filter(|a| a.contains('@')) {
                *senders.entry(account.as_str()).or_default().entry(address.trim().to_lowercase()).or_default() += n as usize;
            }
        }
    }
    let system = if config.mail_root.is_none() { system_accounts() } else { HashMap::new() };
    let mut accounts: BTreeMap<String, (String, Vec<serde_json::Value>)> = BTreeMap::new();
    let mut ids: Vec<&i64> = boxes.keys().collect();
    ids.sort();
    for id in ids {
        let (scheme, account, mailbox) = &boxes[id];
        let role = role_of(mailbox);
        accounts.entry(account.clone()).or_insert_with(|| (scheme.clone(), vec![])).1.push(serde_json::json!({
            "name": mailbox, "role": role.as_str(), "never": role.excluded(), "count": counts.get(id).copied().unwrap_or(0),
        }));
    }
    Ok(serde_json::json!({
        "layout": layout,
        "accounts": accounts.into_iter().map(|(id, (scheme, mailboxes))| {
            let sent = senders.get(id.as_str()).and_then(|m| m.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(a, _)| a.clone()));
            let (system_address, description) = system.get(&id.to_uppercase()).cloned().unwrap_or_default();
            let address = sent.or(system_address.filter(|a| a.contains('@')));
            serde_json::json!({"id": id, "scheme": scheme, "address": address, "description": description, "mailboxes": mailboxes})
        }).collect::<Vec<_>>(),
    }))
}

pub fn scan(config: &MailConfig, cursor: Option<&str>, known: &Known, limit: usize, now_ms: i64) -> Result<Scan, String> {
    let (layout, layout_dir, boxes, rows) = load_index(config)?;
    let mut cur: Cursor = cursor.and_then(|c| serde_json::from_str(c).ok()).unwrap_or_default();
    if cur.layout != layout {
        cur = Cursor { v: 1, layout: layout.clone(), last_rowid: 0 };
    }
    let cutoff = config.backfill_days.map(|d| now_ms - d * 86_400_000);
    let mut scan = Scan { format: FORMAT.into(), present: Some(vec![]), ..Default::default() };
    let mut new_keys: BTreeSet<String> = BTreeSet::new();
    let mut stopped = false;
    let mut last = cur.last_rowid;
    let mut files: FileIndex = HashMap::new();
    for row in rows {
        let Some(location) = boxes.get(&row.mailbox) else { continue };
        let (_, account, mailbox) = location;
        if row.deleted || (!config.accounts.is_empty() && !config.accounts.contains(account)) {
            if row.rowid > last && !stopped {
                last = row.rowid;
            }
            continue;
        }
        let key = row_key(&row, account, mailbox);
        let role = role_of(mailbox);
        if !role.excluded() {
            scan.present.as_mut().unwrap().push(Location { key: key.clone(), mailbox: mailbox.clone() });
        }
        if row.rowid <= cur.last_rowid || stopped {
            continue;
        }
        let wanted = config.mailboxes.imports(mailbox, role)
            && cutoff.is_none_or(|c| row.received_ms.or(row.sent_ms).is_none_or(|t| t >= c))
            && !known.contains_key(&key);
        if wanted {
            if new_keys.contains(&key) {
                // Same message in a second mailbox: no second body read; presence records the mailbox.
                scan.new.push(RawMail { key: key.clone(), account: Some(account.clone()), mailbox: mailbox.clone(), ..Default::default() });
            } else {
                if new_keys.len() >= limit {
                    stopped = true;
                    scan.more = true;
                    continue;
                }
                let (bytes, partial, unreadable) = load_bytes(&layout_dir, &mut files, location, row.rowid)?;
                if unreadable {
                    scan.issues.push("emlx_unreadable".into());
                }
                new_keys.insert(key.clone());
                scan.new.push(RawMail {
                    key: key.clone(),
                    account: Some(account.clone()),
                    mailbox: mailbox.clone(),
                    bytes,
                    partial,
                    received_ms: row.received_ms,
                    index: index_meta(&row),
                });
            }
        }
        last = row.rowid;
    }
    cur.last_rowid = last;
    cur.v = 1;
    scan.cursor = serde_json::to_string(&cur).unwrap();
    Ok(scan)
}
