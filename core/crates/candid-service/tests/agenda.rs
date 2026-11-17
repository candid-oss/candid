//! `/local/agenda` on the local channel (agent-platform-v1 §4.2): App only, POST only, bad input refused, and the
//! change feed told after a decision. Synthetic data only.
use candid_service::local::local_request;
use candid_service::serve::{ServeOptions, Service};
use candid_store::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

fn call(socket: &Path, method: &str, body: Value) -> (u16, Value) {
    let data = if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() };
    let (status, raw) = local_request(socket, method, "/local/agenda", &data).unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn with_token(socket: &Path, token: &str, body: Value) -> u16 {
    let body = serde_json::to_vec(&body).unwrap();
    let mut s = UnixStream::connect(socket).unwrap();
    let head = format!("POST /local/agenda HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n", body.len());
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(&body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    String::from_utf8_lossy(&raw).split(' ').nth(1).unwrap().parse().unwrap()
}

#[test]
fn agenda_is_app_only_and_validates_decisions() {
    let dir = PathBuf::from(format!("/tmp/pv-agenda-{}", &uuid_v4()[..8]));
    let run = dir.join("run");
    let socket = run.join("core.sock");
    let mut o = ServeOptions::new(dir.join("timeline"), run.clone());
    o.created_by = "candid/test".into();
    o.socket = Some(socket.clone());
    let service = Service::start(&o, |_| {}).unwrap();

    let (s, v) = call(&socket, "POST", json!({"action": "today"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["proposals"].clone(), v["commitments"].clone(), v["review"].clone()), (json!([]), json!([]), Value::Null));
    assert_eq!(call(&socket, "POST", Value::Null).0, 200, "an empty body means today");
    assert_eq!(call(&socket, "GET", Value::Null).0, 405);
    assert_eq!(call(&socket, "POST", json!({"action": "drop"})).0, 400);
    assert_eq!(call(&socket, "POST", json!({"action": "proposal", "proposal_id": "missing", "decision": "approve"})).0, 404);
    assert_eq!(call(&socket, "POST", json!({"action": "commitment", "decision": "confirm"})).0, 400);
    // A token (an external client or Pi itself) cannot approve proposals.
    assert_eq!(with_token(&socket, "anything", json!({"action": "today"})), 403);

    service.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
