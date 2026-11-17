//! Port of Swift `TimelineServerTests` (loopback HTTP, client tokens, MCP), plus the headless serve mode.
//! Synthetic events only.
use candid_service::*;
use candid_store::fixtures::temp_root;
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

struct Env {
    store: Arc<Mutex<TimelineStore>>,
    root: std::path::PathBuf,
    token: String,
    client: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn make_env(scope: ClientScope) -> Env {
    let root = temp_root("server");
    let store = TimelineStore::open(&root, "candid/test", true, Clock::default()).unwrap();
    let source = store.register_source("mac.desktop", "Synthetic Mac", &SourcePolicy::new(true, false)).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let ev = |key: &str, ms: i64, app: &str, name: &str, title: &str, body: &str| {
        let mut e = IngestEvent::new(key, "screen.text", Some(ms));
        e.app_bundle_id = Some(app.into());
        e.app_name = Some(name.into());
        e.title = Some(title.into());
        e.payload = json!({"trigger": "timer"});
        e.text = vec![IngestText::new("body", body)];
        e
    };
    store
        .ingest(&IngestBatch::events(
            &source,
            vec![
                ev("s1", now - 600_000, "com.example.notes", "Notes", "合成周报", "本周合成进度：预算复核完成，接口联调还差两项。"),
                ev("s2", now - 300_000, "com.example.browser", "Browser", "Synthetic docs", "synthetic page about release checklist"),
            ],
        ))
        .unwrap();
    let reg = store.register_client("Synthetic AI", "mcp", &scope, None).unwrap();
    Env { store: Arc::new(Mutex::new(store)), root, token: reg.token.unwrap(), client: reg.client_id }
}

fn request(path: &str, token: Option<&str>, host: &str, origin: Option<&str>, body: &str) -> HttpRequest {
    let mut headers = HashMap::from([("host".to_string(), host.to_string()), ("content-type".to_string(), "application/json".to_string())]);
    if let Some(t) = token {
        headers.insert("authorization".into(), format!("Bearer {t}"));
    }
    if let Some(o) = origin {
        headers.insert("origin".into(), o.into());
    }
    HttpRequest { method: "POST".into(), path: path.into(), headers, body: body.as_bytes().to_vec() }
}

fn req(path: &str, token: Option<&str>, body: &str) -> HttpRequest {
    request(path, token, "127.0.0.1:9", None, body)
}

fn body(r: &HttpResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

fn scalar(env: &Env, sql: &str) -> i64 {
    env.store.lock().unwrap().raw_db().scalar(sql, &[]).unwrap()
}

// MARK: Router

#[test]
fn router_answers_search_for_a_valid_token() {
    let env = make_env(ClientScope::unbounded());
    let router = Router::new(env.store.clone());
    let response = router.handle(&req("/v1/search", Some(&env.token), r#"{"q":"预算","start":"today"}"#), 9);
    assert_eq!(response.status, 200);
    let hits = body(&response)["hits"].as_array().cloned().unwrap_or_default();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["title"], json!("合成周报"));
    assert_eq!(scalar(&env, &format!("SELECT COUNT(*) FROM access_log WHERE tool='search' AND client_id='{}'", env.client)), 1);
}

#[test]
fn router_refuses_missing_or_revoked_tokens_wrong_host_and_browsers() {
    let env = make_env(ClientScope::unbounded());
    let router = Router::new(env.store.clone());
    let t = Some(env.token.as_str());
    assert_eq!(router.handle(&req("/v1/status", None, "{}"), 9).status, 401);
    assert_eq!(router.handle(&req("/v1/status", Some("not-a-token"), "{}"), 9).status, 401);
    assert_eq!(router.handle(&request("/v1/status", t, "evil.example:9", None, "{}"), 9).status, 403);
    assert_eq!(router.handle(&request("/v1/status", t, "127.0.0.1:10", None, "{}"), 9).status, 403);
    assert_eq!(router.handle(&request("/v1/status", t, "localhost:9", None, "{}"), 9).status, 200);
    assert_eq!(router.handle(&request("/v1/status", t, "127.0.0.1:9", Some("https://evil.example"), "{}"), 9).status, 403);
    assert_eq!(router.handle(&req("/v1/nope", t, "{}"), 9).status, 404);
    let mut get = req("/v1/status", t, "{}");
    get.method = "GET".into();
    assert_eq!(router.handle(&get, 9).status, 405);
    let mut big = req("/v1/search", t, "");
    big.body = vec![b' '; MAX_BODY + 1];
    assert_eq!(router.handle(&big, 9).status, 413);
    env.store.lock().unwrap().revoke_client(&env.client).unwrap();
    assert_eq!(router.handle(&req("/v1/status", t, "{}"), 9).status, 401);
}

#[test]
fn router_reports_bad_arguments_and_scope_denials() {
    let env = make_env(ClientScope { media: false, ..ClientScope::unbounded() });
    let router = Router::new(env.store.clone());
    let missing = router.handle(&req("/v1/context", Some(&env.token), "{}"), 9);
    assert_eq!(missing.status, 400);
    assert!(body(&missing)["error"]["message"].as_str().unwrap().contains("at"));
    let bad_type = router.handle(&req("/v1/search", Some(&env.token), r#"{"limit":"ten"}"#), 9);
    assert_eq!(bad_type.status, 400);
    let event = {
        let s = env.store.lock().unwrap();
        let sid = s.sources().unwrap()[0].source_id.clone();
        s.event_id(&sid, "s1").unwrap().unwrap()
    };
    let media = router.handle(&req("/v1/media", Some(&env.token), &format!(r#"{{"event_id":"{event}"}}"#)), 9);
    assert_eq!(media.status, 403);
    assert_eq!(scalar(&env, "SELECT COUNT(*) FROM access_log WHERE tool='media' AND denied IS NOT NULL"), 1);
    let unknown = router.handle(&req("/v1/get_event", Some(&env.token), r#"{"event_id":"nope"}"#), 9);
    assert_eq!(unknown.status, 404);
    let cursor = router.handle(&req("/v1/search", Some(&env.token), r#"{"q":"x","cursor":"bad"}"#), 9);
    assert_eq!(cursor.status, 400);
    assert_eq!(body(&cursor)["error"]["code"], json!("invalid_cursor"));
}

// MARK: HTTP

#[test]
fn http_parser_handles_partial_complete_and_oversized_requests() {
    let head = "POST /v1/status HTTP/1.1\r\nHost: 127.0.0.1:9\r\nContent-Length: 2\r\n\r\n";
    assert_eq!(parse(head.as_bytes()), Parse::Incomplete);
    let Parse::Complete(r) = parse(format!("{head}{{}}").as_bytes()) else { panic!("not parsed") };
    assert!(r.path == "/v1/status" && r.headers["host"] == "127.0.0.1:9" && r.body == b"{}");
    let big = format!("POST /v1/search HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
    assert_eq!(parse(big.as_bytes()), Parse::TooLarge);
    assert_eq!(parse(b"garbage\r\n\r\n"), Parse::Invalid);
    assert_eq!(parse(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"), Parse::Invalid);
    assert_eq!(parse(&vec![b'a'; 16 * 1024 + 1]), Parse::TooLarge);
}

fn start_server(env: &Env) -> LoopbackServer {
    let router = Router::new(env.store.clone());
    LoopbackServer::start(0, 2, Arc::new(move |r, p| router.handle(r, p))).unwrap()
}

#[test]
fn loopback_server_and_client_round_trip() {
    let env = make_env(ClientScope::unbounded());
    let server = start_server(&env);
    let base = format!("http://127.0.0.1:{}", server.port);
    let data = ServiceClient::new(&base, &env.token).call("overview", br#"{"start":"today"}"#).unwrap();
    let object: Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(object["total_events"], json!(2));
    let wrong = ServiceClient::new(&base, "wrong").call("status", b"{}").unwrap_err();
    assert_eq!(wrong.status, 401);
    // a raw request that never finishes its body is answered once complete, and oversized heads get 413
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    s.write_all(format!("POST /v1/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Length: 2\r\n\r\n", server.port, env.token).as_bytes())
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    s.write_all(b"{}").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200 OK\r\n") && out.contains("Cache-Control: no-store"), "{out}");
    let mut s = std::net::TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    s.write_all(format!("POST /v1/search HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1).as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 413 Payload Too Large"), "{out}");
}

// MARK: MCP

#[test]
fn mcp_shell_negotiates_lists_and_forwards_calls() {
    let env = make_env(ClientScope::unbounded());
    let server = start_server(&env);
    let client = ServiceClient::new(&format!("http://127.0.0.1:{}", server.port), &env.token);
    let shell = McpShell::new("test", Box::new(move |tool, body| client.call(tool, body)));
    let send = |line: &str| -> Value { serde_json::from_str(&shell.handle(line).unwrap_or_else(|| "{}".into())).unwrap() };
    let init = send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}"#);
    assert_eq!(init["result"]["protocolVersion"], json!("2025-06-18"));
    assert!(init["result"]["instructions"].as_str().unwrap().contains("证据"));
    for v in ["2025-03-26", "2024-11-05"] {
        let r = send(&format!(r#"{{"jsonrpc":"2.0","id":7,"method":"initialize","params":{{"protocolVersion":"{v}"}}}}"#));
        assert_eq!(r["result"]["protocolVersion"], json!(v));
    }
    let old = send(r#"{"jsonrpc":"2.0","id":8,"method":"initialize","params":{"protocolVersion":"2023-01-01"}}"#);
    assert_eq!(old["result"]["protocolVersion"], json!("2025-06-18"));
    assert_eq!(shell.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), None);

    let list = send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), catalog::tools().len());
    assert!(tools.iter().any(|t| t["name"] == json!("search") && t["inputSchema"].is_object()));
    assert!(tools.iter().any(|t| t["name"] == json!("propose") && t["annotations"]["readOnlyHint"] == json!(false)));

    let call = send(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search","arguments":{"q":"checklist","start":"today"}}}"#);
    assert_eq!(call["result"]["isError"], json!(false));
    assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("release checklist"));
    assert!(call["result"]["structuredContent"]["hits"].is_array());

    let bad = send(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"context","arguments":{}}}"#);
    assert_eq!(bad["result"]["isError"], json!(true));
    let unknown = send(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"drop_tables","arguments":{}}}"#);
    assert_eq!(unknown["error"]["code"], json!(-32602));
    let method = send(r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#);
    assert_eq!(method["error"]["code"], json!(-32601));
    let parse_err = send("{not json");
    assert_eq!(parse_err["error"]["code"], json!(-32700));
    assert_eq!(send(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#)["result"], json!({}));
    let invalid = send(r#"{"jsonrpc":"2.0","id":10}"#);
    assert_eq!(invalid["error"]["code"], json!(-32600));

    // the stdio loop answers line by line and skips notifications
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
    let mut out = Vec::new();
    shell.run(&input[..], &mut out);
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn tool_catalog_matches_the_router_and_has_valid_schemas() {
    let env = make_env(ClientScope::unbounded());
    let router = Router::new(env.store.clone());
    for tool in catalog::tools() {
        assert!(serde_json::from_str::<Value>(tool.input_schema).unwrap().is_object(), "{} schema", tool.name);
        let status = router.handle(&req(&format!("/v1/{}", tool.name), Some(&env.token), "{}"), 9).status;
        assert_ne!(status, 404, "{} not routed", tool.name);
    }
    assert_eq!(catalog::tools().len(), candid_store::TOOL_NAMES.len());
}

// MARK: Headless serve mode

#[test]
fn serve_writes_private_endpoint_and_cli_token_and_cleans_up() {
    let root = temp_root("serve-root");
    let run = temp_root("serve-run");
    let mut opts = ServeOptions::new(root.clone(), run.clone());
    opts.created_by = "candid/test".into();
    let service = Service::start(&opts, |_| {}).unwrap();
    let endpoint_file = RunDirectory::endpoint_file(&run);
    let token_file = RunDirectory::cli_token_file(&run);
    assert_eq!(std::fs::metadata(&endpoint_file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777, 0o600);
    let endpoint = RunDirectory::read_endpoint(&run).unwrap();
    assert_eq!(endpoint.url, format!("http://127.0.0.1:{}", service.port()));
    assert_eq!(endpoint.pid, std::process::id() as i32);
    // discovery through CANDID_RUN_DIR, as the command line does
    let env = HashMap::from([("CANDID_RUN_DIR".to_string(), run.to_string_lossy().into_owned())]);
    let client = ServiceClient::discover(&env).unwrap();
    let status: Value = serde_json::from_slice(&client.call("status", b"{}").unwrap()).unwrap();
    assert_eq!(status["store"]["schema_version"], json!(candid_store::SCHEMA_VERSION));
    // the CLI client has the default scope: no proposals
    let denied = client.call("propose", br#"{"kind":"x","payload":{}}"#).unwrap_err();
    assert_eq!(denied.status, 403);
    // a second start keeps a valid token and revokes nothing
    let token = std::fs::read_to_string(&token_file).unwrap();
    {
        let s = service.store.lock().unwrap();
        serve::ensure_cli_token(&s, &run).unwrap();
        assert_eq!(std::fs::read_to_string(&token_file).unwrap(), token);
        assert_eq!(s.raw_db().query("SELECT COUNT(*) FROM clients WHERE transport='cli' AND revoked_ms IS NULL", &[], |r| r.int(0)).unwrap(), vec![1]);
    }
    service.stop();
    assert!(!endpoint_file.exists());
    assert_eq!(ServiceClient::discover(&env).unwrap_err().status, 503);
    // a stale token is replaced and the old client revoked
    std::fs::write(&token_file, "stale").unwrap();
    let service = Service::start(&opts, |_| {}).unwrap();
    assert_ne!(std::fs::read_to_string(&token_file).unwrap(), "stale");
    {
        let s = service.store.lock().unwrap();
        assert_eq!(s.raw_db().query("SELECT COUNT(*) FROM clients WHERE transport='cli' AND revoked_ms IS NOT NULL", &[], |r| r.int(0)).unwrap(), vec![1]);
    }
    service.stop();
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&run);
}
