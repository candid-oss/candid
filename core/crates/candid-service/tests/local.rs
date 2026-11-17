//! The App's local channel (SPEC-005 M2): Unix socket permissions, local-only administration, the change feed, the
//! internal client, ingest, the mirroring thread, restarts, and the loopback HTTP refusing every local-only path.
//! Synthetic data only.
use candid_service::local::{bind_private, local_request, setup_snippets};
use candid_service::serve::{ServeOptions, Service, StartError};
use candid_service::*;
use candid_store::fixtures::{make_synthetic_ledger};
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Fixture {
    dir: PathBuf,
    root: PathBuf,
    run: PathBuf,
    socket: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(label: &str) -> Fixture {
    // Short base: socket paths must stay under 104 bytes.
    let dir = PathBuf::from(format!("/tmp/pv-{}-{}", label, &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let run = dir.join("run");
    Fixture { root: dir.join("timeline"), socket: run.join("core.sock"), run, dir }
}

fn options(f: &Fixture) -> ServeOptions {
    let mut o = ServeOptions::new(f.root.clone(), f.run.clone());
    o.created_by = "candid/test".into();
    o.socket = Some(f.socket.clone());
    o.cli_path = Some("/Applications/Candid.app/Contents/Helpers/candid".into());
    o
}

fn call(socket: &Path, method: &str, path: &str, body: Value) -> (u16, Value) {
    let data = if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() };
    let (status, raw) = local_request(socket, method, path, &data).unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn loopback(port: u16, method: &str, path: &str, token: &str, body: &str) -> (u16, Value) {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body.as_bytes()).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status = String::from_utf8_lossy(&raw[..split]).split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_slice(&raw[split + 4..]).unwrap_or(Value::Null))
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Reads feed lines until one of `kind` arrives.
fn expect_event(reader: &mut BufReader<UnixStream>, kind: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no {kind} event");
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        if v["type"] == kind {
            assert!(v["seq"].as_u64().unwrap() > 0);
            return;
        }
    }
}

fn subscribe(socket: &Path) -> BufReader<UnixStream> {
    let mut s = UnixStream::connect(socket).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /local/events HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    let mut reader = BufReader::new(s);
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header == "\r\n" {
            break;
        }
    }
    expect_event(&mut reader, "hello");
    reader
}

/// The App's desktop source, after the cutover (without a ledger it switches at once).
fn source(socket: &Path) -> String {
    let (c, cut) = call(socket, "POST", "/local/desktop/cutover", Value::Null);
    assert_eq!(c, 200, "{cut}");
    let (s, v) = call(socket, "POST", "/local/sources", json!({"kind": "mac.desktop", "display_name": "合成桌面"}));
    assert_eq!(s, 200, "{v}");
    v["source_id"].as_str().unwrap().to_string()
}

fn batch(source_id: &str, keys: &[&str]) -> Value {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let events: Vec<Value> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| json!({"source_key": k, "kind": "screen.text", "start_ms": now - 60_000 + i as i64, "app_name": "Notes",
            "app_bundle_id": "com.example.notes", "title": "合成笔记", "text": [{"field": "body", "content": format!("合成段落 {k}：预算复核")}]}))
        .collect();
    json!({"source_id": source_id, "events": events})
}

#[test]
fn socket_is_private_and_a_live_socket_refuses_a_second_core() {
    let f = fixture("perm");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&f.socket), 0o600);
    assert_eq!(mode(&f.run), 0o700);
    // Same socket, other root: the live socket answers, so the second core is busy.
    let mut other = options(&f);
    other.root = f.dir.join("other");
    assert!(matches!(Service::start_checked(&other, |_| {}), Err(StartError::Busy(_))));
    // Same root, other socket: the data-root lock refuses it.
    let mut same_root = options(&f);
    same_root.socket = Some(f.dir.join("b.sock"));
    assert!(matches!(Service::start_checked(&same_root, |_| {}), Err(StartError::Busy(_))));
    service.stop();
    assert!(!f.socket.exists(), "socket removed on stop");
    assert!(!RunDirectory::endpoint_file(&f.run).exists());
}

#[test]
fn stale_socket_files_are_replaced_and_long_paths_refused() {
    let f = fixture("stale");
    std::fs::create_dir_all(&f.run).unwrap();
    // A socket file nobody listens on, as a crashed core leaves behind.
    drop(std::os::unix::net::UnixListener::bind(&f.socket).unwrap());
    assert!(f.socket.exists());
    let service = Service::start(&options(&f), |_| {}).unwrap();
    assert_eq!(call(&f.socket, "GET", "/health", Value::Null).0, 200);
    service.stop();
    let long = f.dir.join("x".repeat(120));
    assert_eq!(bind_private(&long).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn restart_on_the_same_paths_keeps_clients_and_data() {
    let f = fixture("restart");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let src = source(&f.socket);
    assert_eq!(call(&f.socket, "POST", "/local/ingest", batch(&src, &["a"])).0, 200);
    let (_, setup) = call(&f.socket, "POST", "/local/clients", json!({"name": "Synthetic AI"}));
    let token = setup["token"].as_str().unwrap().to_string();
    // A crash leaves the socket and endpoint files; dropping without stop() is the closest in-process equivalent.
    service.stop();
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let (_, health) = call(&f.socket, "GET", "/health", Value::Null);
    assert_eq!(health["ok"], true);
    assert_eq!(health["root"], f.root.display().to_string());
    let (status, v) = loopback(service.port(), "POST", "/v1/search", &token, r#"{"q":"预算"}"#);
    assert_eq!(status, 200, "{v}");
    assert_eq!(source(&f.socket), src, "source registration is idempotent across restarts");
    service.stop();
}

#[test]
fn loopback_http_refuses_every_local_only_path_even_with_a_valid_token() {
    let f = fixture("refuse");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let src = source(&f.socket);
    let (_, setup) = call(&f.socket, "POST", "/local/clients", json!({"name": "Synthetic AI"}));
    let token = setup["token"].as_str().unwrap().to_string();
    let body = serde_json::to_string(&batch(&src, &["x1"])).unwrap();
    for (method, path) in [
        ("GET", "/health"),
        ("GET", "/local/status"),
        ("GET", "/local/events"),
        ("GET", "/local/clients"),
        ("POST", "/local/clients"),
        ("POST", "/local/clients/revoke"),
        ("POST", "/local/sync"),
        ("POST", "/local/verify"),
        ("GET", "/local/sources"),
        ("POST", "/local/sources"),
        ("POST", "/local/ingest"),
        ("POST", "/local/ingest?x=1"),
    ] {
        let payload = if path.starts_with("/local/ingest") { body.as_str() } else { r#"{"name":"intruder","client_id":"x"}"# };
        let (status, v) = loopback(service.port(), method, path, &token, payload);
        assert_eq!(status, 403, "{method} {path}: {v}");
        assert_eq!(v["error"]["code"], "local_only", "{method} {path}");
    }
    let db = service.store.lock().unwrap();
    assert_eq!(db.raw_db().scalar("SELECT COUNT(*) FROM events", &[]).unwrap(), 0, "nothing ingested over HTTP");
    assert_eq!(db.raw_db().scalar("SELECT COUNT(*) FROM clients WHERE name='intruder'", &[]).unwrap(), 0);
    drop(db);
    service.stop();
}

#[test]
fn administration_registers_lists_and_revokes_clients() {
    let f = fixture("admin");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let (s, setup) = call(&f.socket, "POST", "/local/clients", json!({"name": "  Codex  "}));
    assert_eq!(s, 200, "{setup}");
    let token = setup["token"].as_str().unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(setup["name"], "Codex");
    let cli = "/Applications/Candid.app/Contents/Helpers/candid";
    let (claude, codex, generic) = setup_snippets(cli, token);
    assert_eq!(setup["claude_code"], claude);
    assert_eq!(setup["codex"], codex);
    assert_eq!(setup["json"], generic);
    assert_eq!(claude, format!("claude mcp add candid --scope user --env CANDID_TOKEN={token} -- \"{cli}\" mcp"));
    assert!(codex.starts_with("[mcp_servers.candid]\ncommand = \"") && codex.ends_with(&format!("CANDID_TOKEN = \"{token}\" }}")));
    let parsed: Value = serde_json::from_str(&generic).unwrap();
    assert_eq!(parsed["mcpServers"]["candid"]["env"]["CANDID_TOKEN"], token);
    // The token works on the loopback HTTP right away; the listing shows the client and the command-line client.
    assert_eq!(loopback(service.port(), "POST", "/v1/status", token, "{}").0, 200);
    let (_, list) = call(&f.socket, "GET", "/local/clients", Value::Null);
    let clients = list["clients"].as_array().unwrap();
    let codex_row = clients.iter().find(|c| c["name"] == "Codex").unwrap();
    assert_eq!(codex_row["transport"], "mcp");
    assert_eq!(codex_row["calls_7d"], 1);
    assert!(codex_row["revoked_ms"].is_null());
    let cli_row = clients.iter().find(|c| c["transport"] == "cli").unwrap();
    assert!(clients.iter().all(|c| c["transport"] != "internal"), "the App's own client is not listed");
    // Rejections: bad input, unknown client, the managed command-line client, wrong method, unknown path.
    assert_eq!(call(&f.socket, "POST", "/local/clients", json!({"name": "   "})).0, 400);
    assert_eq!(call(&f.socket, "POST", "/local/clients", json!({"nom": "x"})).0, 400);
    assert_eq!(call(&f.socket, "POST", "/local/clients/revoke", json!({"client_id": "nope"})).0, 404);
    assert_eq!(call(&f.socket, "POST", "/local/clients/revoke", json!({"client_id": cli_row["client_id"]})).0, 409);
    assert_eq!(call(&f.socket, "DELETE", "/local/clients", Value::Null).0, 405);
    assert_eq!(call(&f.socket, "GET", "/local/nothing", Value::Null).0, 404);
    // Revoke: the token stops working; revoking again is harmless.
    let id = codex_row["client_id"].clone();
    assert_eq!(call(&f.socket, "POST", "/local/clients/revoke", json!({"client_id": id})).0, 200);
    assert_eq!(loopback(service.port(), "POST", "/v1/status", token, "{}").0, 401);
    assert_eq!(call(&f.socket, "POST", "/local/clients/revoke", json!({"client_id": id})).0, 200);
    let (_, list) = call(&f.socket, "GET", "/local/clients", Value::Null);
    assert!(!list["clients"].as_array().unwrap().iter().find(|c| c["client_id"] == id).unwrap()["revoked_ms"].is_null());
    service.stop();
}

#[test]
fn tools_on_the_local_channel_share_scope_filtering_and_audit() {
    let f = fixture("tools");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let src = source(&f.socket);
    let mut b = batch(&src, &["recent"]);
    let mut old = b["events"][0].clone();
    old["source_key"] = json!("old");
    old["start_ms"] = json!(b["events"][0]["start_ms"].as_i64().unwrap() - 200 * 86_400_000);
    b["events"].as_array_mut().unwrap().push(old);
    assert_eq!(call(&f.socket, "POST", "/local/ingest", b).1["inserted"], 2);
    let (_, setup) = call(&f.socket, "POST", "/local/clients", json!({"name": "Scoped"}));
    let token = setup["token"].as_str().unwrap().to_string();
    let query = json!({"start": "-400d", "end": "now", "kinds": ["screen.text"]});
    // The App (internal, unbounded) sees both events; a 90-day client sees one on either channel.
    let (s, mine) = call(&f.socket, "POST", "/v1/timeline", query.clone());
    assert_eq!(s, 200, "{mine}");
    let count = |v: &Value| v["entries"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(count(&mine), 2, "{mine}");
    let data = serde_json::to_vec(&query).unwrap();
    let (s, raw) = local_request_with_token(&f.socket, "/v1/timeline", &token, &data);
    assert_eq!(s, 200);
    let scoped_local: Value = serde_json::from_slice(&raw).unwrap();
    let (_, scoped_http) = loopback(service.port(), "POST", "/v1/timeline", &token, &String::from_utf8(data).unwrap());
    assert_eq!(count(&scoped_local), 1);
    assert_eq!(scoped_local["entries"], scoped_http["entries"]);
    // A bad token on the local channel is refused, not silently downgraded to the App's client.
    assert_eq!(local_request_with_token(&f.socket, "/v1/status", "bogus", b"{}").0, 401);
    assert_eq!(call(&f.socket, "POST", "/v1/nope", json!({})).0, 404);
    assert_eq!(call(&f.socket, "GET", "/v1/status", Value::Null).0, 405);
    assert_eq!(call(&f.socket, "POST", "/v1/search", json!({"q": 5})).0, 400);
    // Every call is audited under its own client, the App's included.
    let db = service.store.lock().unwrap();
    let internal: i64 = db.raw_db().scalar("SELECT COUNT(*) FROM access_log a JOIN clients c USING(client_id) WHERE c.transport='internal'", &[]).unwrap();
    let scoped: i64 = db.raw_db().scalar("SELECT COUNT(*) FROM access_log a JOIN clients c USING(client_id) WHERE c.name='Scoped'", &[]).unwrap();
    assert_eq!(internal, 1, "the timeline call; argument, path and method errors never reach the store (as on HTTP)");
    assert_eq!(scoped, 2);
    drop(db);
    service.stop();
}

fn local_request_with_token(socket: &Path, path: &str, token: &str, body: &[u8]) -> (u16, Vec<u8>) {
    use std::io::Read;
    let mut s = UnixStream::connect(socket).unwrap();
    let head = format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n", body.len());
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status = String::from_utf8_lossy(&raw[..split]).split(' ').nth(1).unwrap().parse().unwrap();
    (status, raw[split + 4..].to_vec())
}

#[test]
fn ingest_is_idempotent_and_rejects_bad_batches() {
    let f = fixture("ingest");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let src = source(&f.socket);
    let sent = batch(&src, &["k1", "k2"]);
    let (s, first) = call(&f.socket, "POST", "/local/ingest", sent.clone());
    assert_eq!(s, 200, "{first}");
    assert_eq!(first["inserted"], 2);
    assert_eq!(first["event_ids"].as_object().unwrap().len(), 2);
    // Retrying the same batch (e.g. after a timeout) is a no-op: duplicates, no conflicts.
    let (_, again) = call(&f.socket, "POST", "/local/ingest", sent);
    assert_eq!((again["inserted"].as_i64(), again["duplicates"].as_i64(), again["conflicts"].as_i64()), (Some(0), Some(2), Some(0)));
    assert_eq!(again["event_ids"], first["event_ids"]);
    let (_, status) = call(&f.socket, "GET", "/local/status", Value::Null);
    assert_eq!(status["events"], 2);
    assert_eq!(call(&f.socket, "POST", "/local/ingest", batch("no-such-source", &["k3"])).0, 404);
    assert_eq!(call(&f.socket, "POST", "/local/ingest", json!({"events": "x"})).0, 400);
    let mut bad_kind = batch(&src, &["k4"]);
    bad_kind["events"][0]["kind"] = json!("not.a.kind");
    assert_eq!(call(&f.socket, "POST", "/local/ingest", bad_kind).0, 400);
    assert_eq!(call(&f.socket, "POST", "/local/sources", json!({"kind": "nope", "display_name": "x"})).0, 400);
    let (_, sources) = call(&f.socket, "GET", "/local/sources", Value::Null);
    assert_eq!(sources["sources"].as_array().unwrap().len(), 1);
    service.stop();
}

#[test]
fn change_feed_reports_data_clients_and_status() {
    let f = fixture("feed");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    let mut feed = subscribe(&f.socket);
    let src = source(&f.socket);
    call(&f.socket, "POST", "/local/ingest", batch(&src, &["e1"]));
    expect_event(&mut feed, "data");
    call(&f.socket, "POST", "/local/clients", json!({"name": "Feed test"}));
    expect_event(&mut feed, "clients");
    let mut second = subscribe(&f.socket);
    wait_until("two subscribers", || call(&f.socket, "GET", "/health", Value::Null).1["subscribers"] == 2);
    // A subscriber that goes away is dropped at the next event.
    drop(second.get_mut().shutdown(std::net::Shutdown::Both));
    drop(second);
    call(&f.socket, "POST", "/local/ingest", batch(&src, &["e2"]));
    expect_event(&mut feed, "data");
    wait_until("dropped subscriber", || call(&f.socket, "GET", "/health", Value::Null).1["subscribers"] == 1);
    service.stop();
    // Stopping closes the feed; events already on their way may still arrive first.
    let mut closed = false;
    for _ in 0..16 {
        let mut rest = String::new();
        if feed.read_line(&mut rest).unwrap_or(0) == 0 {
            closed = true;
            break;
        }
    }
    assert!(closed, "the feed stays open after stop");
}

#[test]
fn mirroring_runs_in_the_core_with_first_pass_verification_and_sync_now() {
    let f = fixture("sync");
    let ledger = make_synthetic_ledger().unwrap();
    let mut o = options(&f);
    o.ledger = Some(ledger.root.clone());
    o.sync_interval = Duration::from_secs(3600);
    let service = Service::start(&o, |_| {}).unwrap();
    wait_until("first pass and verification", || {
        let s = call(&f.socket, "GET", "/local/status", Value::Null).1;
        s["phase"] == "ready" && !s["verification"].is_null()
    });
    let (_, s) = call(&f.socket, "GET", "/local/status", Value::Null);
    assert_eq!(s["verification"]["ok"], true, "{s}");
    assert_eq!(s["verification"]["hashed"], true, "a new library is verified with hashes");
    assert_eq!(s["verification"]["blobs_checked"], 1);
    assert!(s["events"].as_i64().unwrap() >= 9, "{s}");
    assert!(s["last_change"].as_str().unwrap().starts_with("桌面 +"));
    assert_eq!(s["endpoint"], service.endpoint.url);
    let first_sync = s["last_sync_ms"].as_i64().unwrap();
    // 立即同步: a new record arrives in the ledger and is mirrored without waiting for the interval.
    let mut feed = subscribe(&f.socket);
    let db = candid_store::fixtures::ledger_db(&ledger.root).unwrap();
    let mut extra = ledger.records[5].clone();
    extra.id = "rec-mark-2".into();
    extra.unix += 5.0;
    candid_store::fixtures::ledger_append(&db, &extra).unwrap();
    drop(db);
    let (s202, queued) = call(&f.socket, "POST", "/local/sync", Value::Null);
    assert_eq!((s202, queued["queued"].as_bool()), (202, Some(true)));
    expect_event(&mut feed, "data");
    let (_, s) = call(&f.socket, "GET", "/local/status", Value::Null);
    assert!(s["last_sync_ms"].as_i64().unwrap() >= first_sync);
    assert_eq!(s["last_change"], "桌面 +1");
    // 重新核对 with and without hashes.
    let (code, v) = call(&f.socket, "POST", "/local/verify", json!({"hash_blobs": false}));
    assert_eq!((code, v["ok"].as_bool(), v["hashed"].as_bool()), (200, Some(true), Some(false)));
    let (_, v) = call(&f.socket, "POST", "/local/verify", Value::Null);
    assert_eq!(v["hashed"], true);
    service.stop();
    let _ = std::fs::remove_dir_all(&ledger.root);
}

#[test]
fn verification_reports_a_missing_screenshot_and_no_ledger_is_a_conflict() {
    let f = fixture("verify");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    assert_eq!(call(&f.socket, "POST", "/local/verify", Value::Null).0, 409);
    assert_eq!(call(&f.socket, "GET", "/local/status", Value::Null).1["phase"], "ready", "nothing to mirror");
    service.stop();

    let g = fixture("verify2");
    let ledger = make_synthetic_ledger().unwrap();
    let mut o = options(&g);
    o.ledger = Some(ledger.root.clone());
    o.sync_interval = Duration::from_secs(3600);
    let service = Service::start(&o, |_| {}).unwrap();
    wait_until("first verification", || !call(&g.socket, "GET", "/local/status", Value::Null).1["verification"].is_null());
    // A screenshot the ledger gained since the last pass: verification reports the count and the missing event.
    let db = candid_store::fixtures::ledger_db(&ledger.root).unwrap();
    let mut shot = ledger.records.last().unwrap().clone();
    shot.id = "rec-shot-2".into();
    shot.unix += 30.0;
    candid_store::fixtures::ledger_append(&db, &shot).unwrap();
    drop(db);
    let (_, v) = call(&g.socket, "POST", "/local/verify", json!({"hash_blobs": false}));
    assert_eq!(v["ok"], false, "{v}");
    let differences: Vec<&str> = v["differences"].as_array().unwrap().iter().map(|d| d.as_str().unwrap()).collect();
    assert!(differences.contains(&"screenshot：账本 2，时间线 1"), "{v}");
    assert!(differences.contains(&"1 张截图未进入时间线"), "{v}");
    let (_, s) = call(&g.socket, "GET", "/local/status", Value::Null);
    assert_eq!(s["verification"], v, "the status carries the latest verification");
    service.stop();
    let _ = std::fs::remove_dir_all(&ledger.root);
}

#[test]
fn health_reports_identity_and_the_app_client_is_reused() {
    let f = fixture("health");
    let mut o = options(&f);
    o.parent_pid = Some(4242);
    let service = Service::start(&o, |_| {}).unwrap();
    let (_, h) = call(&f.socket, "GET", "/health", Value::Null);
    assert_eq!(h["pid"], std::process::id());
    assert_eq!(h["parent_pid"], 4242);
    assert_eq!(h["store_id"], service.endpoint.store_id);
    assert_eq!(h["socket"], f.socket.display().to_string());
    assert_eq!(h["staging_dir"], f.root.join("staging").display().to_string());
    service.stop();
    let service = Service::start(&o, |_| {}).unwrap();
    let n = service.store.lock().unwrap().raw_db().scalar("SELECT COUNT(*) FROM clients WHERE transport='internal'", &[]).unwrap();
    assert_eq!(n, 1);
    let _ = service.store.lock().unwrap().raw_db().first("SELECT 1", &[], |r| r.int(0));
    service.stop();
}

#[test]
fn stop_completes_even_when_the_run_directory_was_deleted() {
    let f = fixture("gone");
    let service = Service::start(&options(&f), |_| {}).unwrap();
    std::fs::remove_dir_all(&f.run).unwrap();
    let started = Instant::now();
    service.stop();
    assert!(started.elapsed() < Duration::from_secs(3), "stop took {:?}", started.elapsed());
}

#[test]
fn agent_consent_is_local_only_and_scheduler_syncs_without_ledger_or_hub() {
    let f = fixture("agents");
    let mut o = options(&f);
    o.sync_interval = Duration::from_millis(20);
    let service = Service::start_checked(&o, |_| {}).unwrap();
    let (_, settings) = call(&f.socket, "GET", "/local/agents", Value::Null);
    assert!(settings["consents"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["read"] == false && c["activity"] == false));
    let source = f.dir.join("SYNTHETIC-transcripts");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("synthetic.jsonl"),concat!("{\"type\":\"session\",\"version\":3,\"id\":\"synthetic\",\"cwd\":\"/SYNTHETIC\"}\n", "{\"type\":\"message\",\"id\":\"u1\",\"timestamp\":\"2026-11-11T01:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"SYNTHETIC prompt\"}}\n")).unwrap();
    let c = json!({"agent":"pi","read":true,"search":true,"activity":true,"paths":[source]});
    assert_eq!(
        call(
            &f.socket,
            "POST",
            "/local/agents/consent",
            json!({"consent":c})
        )
        .0,
        200
    );
    wait_until("AI scheduler", || {
        service
            .store
            .lock()
            .unwrap()
            .raw_db()
            .scalar("SELECT COUNT(*) FROM events WHERE kind='ai.prompt'", &[])
            .unwrap()
            == 1
    });
    let count = service
        .store
        .lock()
        .unwrap()
        .raw_db()
        .scalar("SELECT COUNT(*) FROM events", &[])
        .unwrap();
    service.core.sync_pass();
    assert_eq!(
        service
            .store
            .lock()
            .unwrap()
            .raw_db()
            .scalar("SELECT COUNT(*) FROM events", &[])
            .unwrap(),
        count
    );
    let (_, r) = call(
        &f.socket,
        "POST",
        "/local/agents/query",
        json!({"agent":"pi","action":"read","reference":"agent:pi:synthetic"}),
    );
    assert_eq!(r["messages"][0]["text"], "SYNTHETIC prompt");
    let token = std::fs::read_to_string(f.run.join("cli.token")).unwrap();
    assert_eq!(
        loopback(
            service.port(),
            "POST",
            "/local/agents/consent",
            token.trim(),
            "{}"
        )
        .0,
        403
    );
    assert_eq!(
        loopback(
            service.port(),
            "POST",
            "/v1/agent",
            token.trim(),
            r#"{"agent":"pi","action":"read","reference":"agent:pi:synthetic"}"#
        )
        .0,
        403
    );
    let (code, registration)=call(&f.socket,"POST","/local/clients",json!({"name":"SYNTHETIC scoped AI","agents":["pi"]}));
    assert_eq!(code,200);
    let scoped_token=registration["token"].as_str().unwrap();
    assert_eq!(loopback(service.port(),"POST","/v1/agent",scoped_token,r#"{"agent":"pi","action":"read","reference":"agent:pi:synthetic"}"#).0,200);
    assert_eq!(loopback(service.port(),"POST","/local/agents/consent",scoped_token,"{}").0,403);
    assert_eq!(call(&f.socket,"POST","/local/clients",json!({"name":"SYNTHETIC invalid","agents":["unknown"]})).0,400);
    assert_eq!(call(&f.socket,"GET","/local/agents/query",Value::Null).0,405);
    assert_eq!(call(&f.socket,"POST","/local/agents/consent",json!({})).0,400);
    assert_eq!(
        call(
            &f.socket,
            "POST",
            "/local/agents/consent",
            json!({"consent":{"agent":"pi","paths":[source]},"delete_stored":true})
        )
        .0,
        200
    );
    assert_eq!(
        call(
            &f.socket,
            "POST",
            "/local/agents/query",
            json!({"agent":"pi","action":"read","reference":"agent:pi:synthetic"})
        )
        .0,
        403
    );
    assert_eq!(
        service
            .store
            .lock()
            .unwrap()
            .raw_db()
            .scalar("SELECT COUNT(*) FROM events", &[])
            .unwrap(),
        0
    );
    service.stop();
}

#[test]
fn native_management_is_local_and_external_records_are_read_only() {
    let f=fixture("native-api");
    let service=Service::start(&options(&f), |_|{}).unwrap();
    let (_,setup)=call(&f.socket,"POST","/local/clients",json!({"name":"Synthetic external"}));
    let token=setup["token"].as_str().unwrap();
    for path in ["/local/media","/local/jobs","/local/records"] {
        assert_eq!(loopback(service.port(),"POST",path,token,"{}").0,403);
        assert_eq!(call(&f.socket,"GET",path,Value::Null).0,405);
        assert_eq!(local_request_with_token(&f.socket,path,token,br#"{"action":"list"}"#).0,403);
    }
    assert_eq!(local_request_with_token(&f.socket,"/local/records","invalid",br#"{"action":"query","id":"missing"}"#).0,401);
    assert_eq!(call(&f.socket,"POST","/local/media",json!({"action":"import","id":"bad"})).0,400);
    assert_eq!(call(&f.socket,"POST","/local/jobs",json!({"action":"enqueue"})).0,400);
    let (status,job)=call(&f.socket,"POST","/local/jobs",json!({"action":"enqueue","kind":"SYNTHETIC","input_ref":"synthetic","config_version":"1"}));
    assert_eq!(status,200);
    assert_eq!(call(&f.socket,"POST","/local/jobs",json!({"action":"get","id":job["id"]})).1["state"],"queued");
    service.stop();
}

/// Minimal synthetic Mail.app tree: one INBOX message with its .emlx.
fn synthetic_mail_root(dir: &Path) -> PathBuf {
    let root = dir.join("SYNTHETIC-Mail");
    let data = root.join("V10/MailData");
    std::fs::create_dir_all(&data).unwrap();
    let db = candid_store::testing::rusqlite::Connection::open(data.join("Envelope Index")).unwrap();
    db.execute_batch(
        "CREATE TABLE mailboxes(ROWID INTEGER PRIMARY KEY, url TEXT);
         CREATE TABLE subjects(ROWID INTEGER PRIMARY KEY, subject TEXT);
         CREATE TABLE addresses(ROWID INTEGER PRIMARY KEY, address TEXT, comment TEXT);
         CREATE TABLE message_global_data(ROWID INTEGER PRIMARY KEY, message_id_header TEXT);
         CREATE TABLE messages(ROWID INTEGER PRIMARY KEY, global_message_id INTEGER, sender INTEGER, subject INTEGER, date_sent INTEGER, date_received INTEGER, mailbox INTEGER, deleted INTEGER);
         INSERT INTO mailboxes VALUES(1,'imap://SYNTHETIC-ACCOUNT/INBOX');
         INSERT INTO subjects VALUES(1,'SYNTHETIC subject');
         INSERT INTO addresses VALUES(1,'sender@example.test','');
         INSERT INTO message_global_data VALUES(1,'<svc@example.test>');",
    )
    .unwrap();
    let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    db.execute("INSERT INTO messages VALUES(1,1,1,1,?1,?1,1,0)", [now_s]).unwrap();
    let raw = "Message-ID: <svc@example.test>\r\nFrom: sender@example.test\r\nSubject: SYNTHETIC subject\r\n\r\nSYNTHETIC body\r\n";
    let msgs = root.join("V10/SYNTHETIC-ACCOUNT/INBOX.mbox/STORE/Data/Messages");
    std::fs::create_dir_all(&msgs).unwrap();
    std::fs::write(msgs.join("1.emlx"), format!("{}\n{raw}", raw.len())).unwrap();
    root
}

#[test]
fn connectors_are_local_only_and_the_scheduler_imports_mail() {
    let f = fixture("conn");
    let mut o = options(&f);
    o.sync_interval = Duration::from_millis(20);
    let service = Service::start_checked(&o, |_| {}).unwrap();
    let (code, d) = call(&f.socket, "POST", "/local/connectors", json!({"action": "describe"}));
    assert_eq!((code, d["connectors"].as_array().unwrap().len()), (200, 3));
    let root = synthetic_mail_root(&f.dir);
    let (code, r) = call(&f.socket, "POST", "/local/connectors", json!({"action": "authorize", "kind": "mail.apple", "config": {"version": 1, "mail_root": root, "rules": [{"when": {"from": "sender@example.test"}, "level": "normal"}]}}));
    assert_eq!((code, r["probe"]["ok"].as_bool()), (200, Some(true)), "{r}");
    let source_id = r["source_id"].as_str().unwrap().to_string();
    wait_until("mail import", || call(&f.socket, "GET", "/local/connectors", Value::Null).1["sources"][0]["events"] == 1);
    assert_eq!(call(&f.socket, "POST", "/local/connectors", json!({"action": "nope"})).0, 400);
    assert_eq!(call(&f.socket, "PUT", "/local/connectors", Value::Null).0, 405);
    let (_, reg) = call(&f.socket, "POST", "/local/clients", json!({"name": "SYNTHETIC external"}));
    let token = reg["token"].as_str().unwrap();
    assert_eq!(local_request_with_token(&f.socket, "/local/connectors", token, br#"{"action":"list"}"#).0, 403);
    assert_eq!(loopback(service.port(), "POST", "/local/connectors", token, r#"{"action":"list"}"#).0, 403);
    // Mail sources are hidden from external clients unless granted by source id; the App sees them.
    assert_eq!(loopback(service.port(), "POST", "/v1/search", token, r#"{"q":"SYNTHETIC body"}"#).1["hits"].as_array().map(|a| a.len()), Some(0));
    let (_, mine) = call(&f.socket, "POST", "/v1/search", json!({"q": "SYNTHETIC body"}));
    assert_eq!(mine["hits"].as_array().map(|a| a.len()), Some(1), "{mine}");
    let (code, r) = call(&f.socket, "POST", "/local/connectors", json!({"action": "revoke", "source_id": source_id, "delete_stored": true}));
    assert_eq!((code, r["deleted_events"].as_u64()), (200, Some(1)));
    service.stop();
}

#[test]
fn automation_and_screen_routes_are_app_only_versioned_and_retryable() {
    let f=fixture("automation");let service=Service::start(&options(&f), |_|{}).unwrap();
    for path in ["/local/automation","/local/screen"] {
        assert_eq!(call(&f.socket,"GET",path,Value::Null).0,405);
        assert_eq!(call(&f.socket,"POST",path,json!({"action":"status"})).0,200);
        assert_eq!(loopback(service.port(),"POST",path,"any","{}").0,403);
        use std::io::Read;
        let body=b"{\"action\":\"status\"}";let mut socket=UnixStream::connect(&f.socket).unwrap();
        write!(socket,"POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer any\r\nContent-Length: {}\r\n\r\n",body.len()).unwrap();socket.write_all(body).unwrap();
        let mut response=String::new();socket.read_to_string(&mut response).unwrap();assert!(response.starts_with("HTTP/1.1 403"));
    }
    assert_eq!(call(&f.socket,"POST","/local/automation",json!({"action":"config","config":{"version":999,"skills":{},"remote_sources":[]}})).0,400);
    let (s,run)=call(&f.socket,"POST","/local/automation",json!({"action":"run","skill":"daily-review","key":"synthetic-repeat"}));assert_eq!(s,200);
    let (_,same)=call(&f.socket,"POST","/local/automation",json!({"action":"run","skill":"daily-review","key":"synthetic-repeat"}));assert_eq!(same["id"],run["id"]);
    assert_eq!(call(&f.socket,"POST","/local/automation",json!({"action":"cancel","id":run["id"]})).1["state"],"cancelled");
    assert_eq!(call(&f.socket,"POST","/local/automation",json!({"action":"retry","id":run["id"]})).1["state"],"queued");
    service.stop();
}
