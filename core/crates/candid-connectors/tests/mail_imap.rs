//! IMAP adapter (email-lib) against an in-process fake IMAP server on 127.0.0.1 with synthetic mailboxes.
//! The server records every command so the test can prove the adapter is read-only.
mod common;
use base64::Engine;
use common::*;
use candid_connectors::sync_all;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Box_ {
    attrs: String,
    uidvalidity: u32,
    messages: Vec<(u32, Vec<u8>)>,
}

#[derive(Default)]
struct State {
    boxes: BTreeMap<String, Box_>,
    commands: Vec<String>,
    password: String,
}

type Shared = Arc<Mutex<State>>;

fn start(state: Shared) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { break };
            let state = state.clone();
            std::thread::spawn(move || serve(conn, state));
        }
    });
    port
}

fn read_command(r: &mut BufReader<TcpStream>, w: &mut TcpStream) -> Option<String> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    // Synchronising literals: {n}\r\n → continuation, then n bytes and the rest of the line.
    while let Some(start) = line.trim_end().strip_suffix('}').and_then(|l| l.rfind('{').map(|i| (i, l))) {
        let (i, l) = start;
        let n: usize = l[i + 1..].trim_end_matches('+').parse().ok()?;
        line.truncate(i);
        w.write_all(b"+ go\r\n").ok()?;
        let mut buf = vec![0; n];
        r.read_exact(&mut buf).ok()?;
        line.push_str(&format!("\"{}\"", String::from_utf8_lossy(&buf)));
        let mut rest = String::new();
        r.read_line(&mut rest).ok()?;
        line.push_str(&rest);
    }
    Some(line.trim_end().to_string())
}

fn serve(conn: TcpStream, state: Shared) {
    let mut w = conn.try_clone().unwrap();
    let mut r = BufReader::new(conn);
    let caps = "IMAP4rev1 AUTH=PLAIN";
    w.write_all(format!("* OK [CAPABILITY {caps}] synthetic ready\r\n").as_bytes()).unwrap();
    let mut current: Option<String> = None;
    while let Some(line) = read_command(&mut r, &mut w) {
        let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let upper = rest.to_uppercase();
        state.lock().unwrap().commands.push(upper.split(' ').take(if upper.starts_with("UID ") { 2 } else { 1 }).collect::<Vec<_>>().join(" "));
        let mut out = String::new();
        if upper.starts_with("CAPABILITY") {
            out.push_str(&format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n"));
        } else if upper.starts_with("AUTHENTICATE PLAIN") {
            let inline = rest.split_whitespace().nth(2).map(String::from);
            let data = match inline {
                Some(d) => d,
                None => {
                    w.write_all(b"+ \r\n").unwrap();
                    let mut l = String::new();
                    r.read_line(&mut l).unwrap();
                    l.trim().to_string()
                }
            };
            let decoded = base64::engine::general_purpose::STANDARD.decode(data).unwrap_or_default();
            let parts: Vec<&[u8]> = decoded.split(|b| *b == 0).collect();
            let ok = parts.len() == 3 && parts[2] == state.lock().unwrap().password.as_bytes();
            out.push_str(&if ok { format!("{tag} OK [CAPABILITY {caps}] authenticated\r\n") } else { format!("{tag} NO [AUTHENTICATIONFAILED] invalid\r\n") });
        } else if upper.starts_with("LOGIN") {
            out.push_str(&format!("{tag} NO use AUTHENTICATE\r\n"));
        } else if upper.starts_with("LIST") {
            for (name, b) in &state.lock().unwrap().boxes {
                out.push_str(&format!("* LIST ({}) \"/\" \"{}\"\r\n", b.attrs, name));
            }
            out.push_str(&format!("{tag} OK done\r\n"));
        } else if upper.starts_with("EXAMINE") {
            let name = rest[8..].trim().trim_matches('"').to_string();
            let s = state.lock().unwrap();
            match s.boxes.get(&name) {
                Some(b) => {
                    let next = b.messages.iter().map(|m| m.0).max().unwrap_or(0) + 1;
                    out.push_str(&format!("* FLAGS (\\Seen \\Answered)\r\n* {} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {}] v\r\n* OK [UIDNEXT {next}] n\r\n{tag} OK [READ-ONLY] examined\r\n", b.messages.len(), b.uidvalidity));
                    current = Some(name);
                }
                None => out.push_str(&format!("{tag} NO no such mailbox\r\n")),
            }
        } else if upper.starts_with("UID SEARCH") {
            let s = state.lock().unwrap();
            let b = &s.boxes[current.as_ref().unwrap()];
            let hits: Vec<String> = if upper.contains("HEADER") {
                let want = rest.split_whitespace().last().unwrap_or("").trim_matches('"').to_lowercase();
                assert!(want.starts_with('<') && want.ends_with('>'), "unexpected search {rest}");
                b.messages.iter().filter(|(_, raw)| String::from_utf8_lossy(raw).to_lowercase().contains(&format!("message-id: {want}"))).map(|m| m.0.to_string()).collect()
            } else {
                b.messages.iter().map(|m| m.0.to_string()).collect()
            };
            out.push_str(&format!("* SEARCH{}{}\r\n{tag} OK done\r\n", if hits.is_empty() { "" } else { " " }, hits.join(" ")));
        } else if upper.starts_with("UID FETCH") {
            let set = rest.split_whitespace().nth(2).unwrap_or("");
            let s = state.lock().unwrap();
            let b = &s.boxes[current.as_ref().unwrap()];
            let wanted: Vec<u32> = set
                .split(',')
                .flat_map(|p| match p.split_once(':') {
                    Some((a, z)) => {
                        let a: u32 = a.parse().unwrap();
                        let z: u32 = if z == "*" { u32::MAX } else { z.parse().unwrap() };
                        (a.min(z)..=a.max(z)).take(10_000).collect::<Vec<_>>()
                    }
                    None => vec![p.parse().unwrap()],
                })
                .collect();
            let mut w2 = Vec::new();
            for (i, (uid, raw)) in b.messages.iter().enumerate() {
                if wanted.contains(uid) {
                    w2.extend_from_slice(format!("* {} FETCH (UID {uid} BODY[] {{{}}}\r\n", i + 1, raw.len()).as_bytes());
                    w2.extend_from_slice(raw);
                    w2.extend_from_slice(b")\r\n");
                }
            }
            w2.extend_from_slice(format!("{tag} OK fetched\r\n").as_bytes());
            drop(s);
            w.write_all(&w2).unwrap();
            continue;
        } else if upper.starts_with("LOGOUT") {
            let _ = w.write_all(format!("* BYE bye\r\n{tag} OK done\r\n").as_bytes());
            return;
        } else if upper.starts_with("NOOP") || upper.starts_with("ID") || upper.starts_with("ENABLE") {
            out.push_str(&format!("{tag} OK done\r\n"));
        } else {
            out.push_str(&format!("{tag} BAD not supported by the synthetic server\r\n"));
        }
        if w.write_all(out.as_bytes()).is_err() {
            return;
        }
    }
}

fn world(label: &str) -> (std::path::PathBuf, std::sync::Mutex<candid_store::TimelineStore>, Shared, u16) {
    let dir = temp(label);
    let st = store(&dir);
    let state: Shared = Arc::new(Mutex::new(State { password: "synthetic-app-password".into(), ..Default::default() }));
    {
        let mut s = state.lock().unwrap();
        let a = mail(Some("i1@example.test"), "Alice <alice@example.test>", "me@example.test", "合同", "Tue, 10 Nov 2026 09:00:00 +0000", "", "请审阅合同第三条。");
        let b = mail(Some("i2@example.test"), "me@example.test", "alice@example.test", "Re: 合同", "Tue, 10 Nov 2026 10:00:00 +0000", "In-Reply-To: <i1@example.test>\r\nReferences: <i1@example.test>\r\n", "已审阅，没有问题。");
        let c = mail(Some("i3@example.test"), "Bob <bob@example.test>", "me@example.test", "周会", "Tue, 10 Nov 2026 11:00:00 +0000", "", "周会改到周四。");
        s.boxes.insert("INBOX".into(), Box_ { attrs: "\\HasNoChildren".into(), uidvalidity: 7, messages: vec![(1, a.clone()), (2, c)] });
        s.boxes.insert("[Gmail]/Sent Mail".into(), Box_ { attrs: "\\HasNoChildren \\Sent".into(), uidvalidity: 9, messages: vec![(1, b)] });
        s.boxes.insert("[Gmail]/All Mail".into(), Box_ { attrs: "\\HasNoChildren \\All".into(), uidvalidity: 11, messages: vec![(5, a)] });
        s.boxes.insert("[Gmail]/Spam".into(), Box_ { attrs: "\\HasNoChildren \\Junk".into(), uidvalidity: 13, messages: vec![] });
        s.boxes.insert("[Gmail]/Trash".into(), Box_ { attrs: "\\HasNoChildren \\Trash".into(), uidvalidity: 15, messages: vec![] });
    }
    let port = start(state.clone());
    (dir, st, state, port)
}

fn authorize(dir: &std::path::Path, st: &std::sync::Mutex<candid_store::TimelineStore>, port: u16, secret_ref: &str) -> String {
    let r = cmd(st, dir, json!({"action": "authorize", "kind": "mail.imap", "account_ref": "me@example.test", "secret_ref": secret_ref,
        "config": {"version": 1, "imap": {"host": "127.0.0.1", "port": port, "tls": "none"}}}));
    r["source_id"].as_str().unwrap().to_string()
}

const FORBIDDEN: &[&str] = &["SELECT", "STORE", "UID STORE", "COPY", "UID COPY", "MOVE", "UID MOVE", "EXPUNGE", "UID EXPUNGE", "APPEND", "DELETE", "CREATE", "RENAME", "FETCH", "SEARCH"];

#[test]
fn imap_sync_is_read_only_idempotent_and_tracks_moves_and_deletions() {
    let (dir, st, state, port) = world("imap-sync");
    let sid = authorize(&dir, &st, port, "keychain:synthetic-imap");
    // No credentials in memory yet: waiting, nothing read, no secret anywhere in the database.
    let out = sync_all(&st, None, &dir).sources.remove(0);
    assert_eq!(out.code.as_deref(), Some("waiting_authorization"));
    assert!(state.lock().unwrap().commands.is_empty());
    cmd(&st, &dir, json!({"action": "secrets", "secrets": {"keychain:synthetic-imap": "synthetic-app-password"}}));
    let out = sync_all(&st, None, &dir).sources.remove(0);
    assert_eq!((out.state.as_str(), out.inserted), ("synced", 3), "{out:?}");
    assert_eq!(payload(&st, "mid:i2@example.test")["direction"], "out", "login address counts as self");
    assert_eq!(payload(&st, "mid:i1@example.test")["mailboxes"], json!(["INBOX"]));
    assert_eq!(count(&st, "SELECT COUNT(DISTINCT thread_id) FROM events"), 2);
    let again = sync_all(&st, None, &dir).sources.remove(0);
    assert_eq!((again.inserted, again.revised, again.removed), (0, 0, 0));
    // Archive i1 (gone from INBOX, still in All Mail) and expunge i3.
    {
        let mut s = state.lock().unwrap();
        s.boxes.get_mut("INBOX").unwrap().messages.clear();
    }
    let moved = sync_all(&st, None, &dir).sources.remove(0);
    assert_eq!((moved.revised, moved.removed), (1, 1), "{moved:?}");
    assert_eq!(payload(&st, "mid:i1@example.test")["mailboxes"], json!(["[Gmail]/All Mail"]));
    assert_eq!(count(&st, "SELECT COUNT(*) FROM tombstones WHERE reason='source_deleted' AND entity='event'"), 1);
    let s = state.lock().unwrap();
    for c in &s.commands {
        assert!(!FORBIDDEN.contains(&c.as_str()), "issued {c}");
    }
    assert!(s.commands.iter().any(|c| c == "EXAMINE") && s.commands.iter().any(|c| c == "UID FETCH"));
    drop(s);
    let db = std::fs::read(dir.join("db/timeline.sqlite")).unwrap();
    let wal = std::fs::read(dir.join("db/timeline.sqlite-wal")).unwrap_or_default();
    for blob in [db, wal] {
        assert!(!String::from_utf8_lossy(&blob).contains("synthetic-app-password"));
    }
    let list = cmd(&st, &dir, json!({"action": "list"}));
    assert_eq!((list["sources"][0]["has_secret"].as_bool(), list["sources"][0]["source_id"].as_str()), (Some(true), Some(sid.as_str())));
}

#[test]
fn wrong_password_and_insecure_remote_transport_are_refused() {
    let (dir, st, state, port) = world("imap-auth");
    authorize(&dir, &st, port, "keychain:synthetic-wrong");
    cmd(&st, &dir, json!({"action": "secrets", "secrets": {"keychain:synthetic-wrong": "wrong"}}));
    let out = sync_all(&st, None, &dir).sources.remove(0);
    assert_eq!(out.code.as_deref(), Some("imap_auth_failed"), "{out:?}");
    assert!(!state.lock().unwrap().commands.iter().any(|c| c == "EXAMINE"));
    assert_eq!(count(&st, "SELECT COUNT(*) FROM events"), 0);
    let code = cmd_err(&st, &dir, json!({"action": "authorize", "kind": "mail.imap", "account_ref": "x@example.test", "secret_ref": "k",
        "config": {"version": 1, "imap": {"host": "imap.example.test", "port": 143, "tls": "none"}}}));
    assert_eq!(code, "insecure_transport_refused");
    let code = cmd_err(&st, &dir, json!({"action": "authorize", "kind": "mail.imap", "account_ref": "x@example.test",
        "config": {"version": 1, "imap": {"host": "imap.example.test", "port": 993}}}));
    assert_eq!(code, "secret_ref_required");
}

#[test]
fn dry_run_counts_without_writing() {
    let (dir, st, _state, port) = world("imap-dry");
    cmd(&st, &dir, json!({"action": "secrets", "secrets": {"keychain:dry": "synthetic-app-password"}}));
    let r = cmd(&st, &dir, json!({"action": "dry_run", "kind": "mail.imap", "account_ref": "me@example.test", "secret_ref": "keychain:dry",
        "config": {"version": 1, "imap": {"host": "127.0.0.1", "port": port, "tls": "none"}}}));
    assert_eq!((r["would_insert"].as_u64(), r["threads"].as_u64()), (Some(3), Some(2)), "{r}");
    assert_eq!(count(&st, "SELECT COUNT(*) FROM events") + count(&st, "SELECT COUNT(*) FROM sources"), 0);
}
