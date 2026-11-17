//! The attached Pi agent's routes on the local channel (agent-platform-v1 §4.1): run start and stop, the model gateway
//! with its budget and `ai_calls` log, error and refusal paths. A loopback mock stands in for the model; synthetic only.
//! One test function: the run is process-wide state of the core.
use candid_service::local::local_request;
use candid_service::serve::{ServeOptions, Service};
use candid_store::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn call(socket: &Path, method: &str, path: &str, body: Value) -> (u16, Value) {
    let data = if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() };
    let (status, raw) = local_request(socket, method, path, &data).unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn with_token(socket: &Path, path: &str, token: &str, body: Value) -> (u16, Value) {
    let body = serde_json::to_vec(&body).unwrap();
    let mut s = UnixStream::connect(socket).unwrap();
    let head = format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n", body.len());
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(&body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status = String::from_utf8_lossy(&raw[..split]).split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_slice(&raw[split + 4..]).unwrap_or(Value::Null))
}

/// A model that answers every other request with an error, and records what it was sent.
fn mock_model() -> (u16, Arc<std::sync::Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let n = Arc::new(AtomicUsize::new(0));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head_end, len) = loop {
                let k = s.read(&mut chunk).unwrap_or(0);
                if k == 0 { break (0, 0); }
                buf.extend_from_slice(&chunk[..k]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    break (p + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let k = s.read(&mut chunk).unwrap_or(0);
                if k == 0 { break; }
                buf.extend_from_slice(&chunk[..k]);
            }
            let auth = String::from_utf8_lossy(&buf[..head_end]).lines().find(|l| l.to_lowercase().starts_with("authorization:")).map(String::from);
            let mut body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
            body["_auth"] = json!(auth);
            log.lock().unwrap().push(body);
            let i = n.fetch_add(1, Ordering::SeqCst);
            let (status, reply) = if i == 1 {
                ("500 Internal Server Error", json!({"error": "boom"}))
            } else {
                ("200 OK", json!({"id": "x", "choices": [{"index": 0, "message": {"role": "assistant", "content": "好的"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 12, "completion_tokens": 3}}))
            };
            let reply = reply.to_string();
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        }
    });
    (port, seen)
}

#[test]
fn pi_runs_freeze_the_model_log_calls_and_refuse_what_they_were_not_granted() {
    let dir = PathBuf::from(format!("/tmp/pv-pi-{}", &uuid_v4()[..8]));
    let run = dir.join("run");
    let socket = run.join("core.sock");
    let mut o = ServeOptions::new(dir.join("timeline"), run.clone());
    o.created_by = "candid/test".into();
    o.socket = Some(socket.clone());
    let service = Service::start(&o, |_| {}).unwrap();

    // No model for the role yet.
    let (s, v) = call(&socket, "POST", "/local/pi/start", json!({}));
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("waiting_model")));

    let (port, seen) = mock_model();
    let provider = |destination: &str| json!({"version": 1, "providers": [{"provider_id": "mock", "name": "Mock", "api": "openai-chat", "base_url": format!("http://127.0.0.1:{port}/v1"), "secret_ref": "mock-key", "destination": destination, "enabled": true, "headers_json": {}}], "models": [{"provider_id": "mock", "model_id": "frozen-model"}], "roles": [{"role": "agent_chat", "model": {"provider_id": "mock", "model_id": "frozen-model"}}]});
    assert_eq!(call(&socket, "POST", "/local/models", json!({"action": "save", "config": provider("cloud")})).0, 200);
    let (s, v) = call(&socket, "POST", "/local/pi/start", json!({}));
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("remote_not_allowed")), "a cloud model needs the feature's permission");
    assert_eq!(call(&socket, "POST", "/local/models", json!({"action": "save", "config": provider("on_device")})).0, 200);
    assert_eq!(call(&socket, "POST", "/local/pi/start", json!({"writes": ["delete_everything"]})).0, 400);
    assert_eq!(call(&socket, "POST", "/local/models/authorize", json!({"secrets": {"mock-key": "sk-synthetic"}})).0, 200);

    let mut withheld_mail = SourcePolicy::default();withheld_mail.agent_visible = false;
    assert_eq!(call(&socket,"POST","/local/sources",json!({"kind":"mail.apple","display_name":"SYNTHETIC private mailbox identity","policy":withheld_mail})).0,200);
    let (s, started) = call(&socket, "POST", "/local/pi/start", json!({"run_id": "r1", "writes": ["draft"], "budget": {"max_model_calls": 3, "max_tool_calls": 2}}));
    assert_eq!(s, 200, "{started}");
    assert!(started["context_note"].as_str().unwrap().contains("尚未允许 AI 读取"));
    assert!(!started["context_note"].as_str().unwrap().contains("private mailbox"));
    let token = started["token"].as_str().unwrap().to_string();
    let tools: Vec<String> = serde_json::from_value(started["tools"].clone()).unwrap();
    assert!(tools.contains(&"save_draft".into()) && !tools.contains(&"annotate".into()) && tools.contains(&"propose".into()));

    // Model calls: the core's model and key, whatever Pi asks for; failures are logged too.
    let ask = json!({"model": "anything", "messages": [{"role": "user", "content": "你好"}], "max_tokens": 999_999, "stream": true, "api_key": "leak"});
    let (s, v) = with_token(&socket, "/local/pi/model", &token, ask.clone());
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["choices"][0]["message"]["content"], "好的");
    let (s, v) = with_token(&socket, "/local/pi/model", &token, ask.clone());
    assert_eq!((s, v["error"]["code"].as_str()), (502, Some("model_call_failed")));
    assert_eq!(with_token(&socket, "/local/pi/model", &token, ask.clone()).0, 200);
    let (s, v) = with_token(&socket, "/local/pi/model", &token, ask.clone());
    assert_eq!((s, v["error"]["code"].as_str()), (429, Some("budget_exhausted")), "the fourth call exceeds max_model_calls");
    {
        let sent = seen.lock().unwrap();
        assert_eq!(sent.len(), 3);
        for r in sent.iter() {
            assert_eq!(r["model"], "frozen-model");
            assert_eq!(r["stream"], false);
            assert!(r["max_tokens"].as_u64().unwrap() <= 16_384);
            assert!(r.get("api_key").is_none());
            assert_eq!(r["_auth"], "authorization: Bearer sk-synthetic");
        }
    }
    let (_, usage) = call(&socket, "POST", "/local/models", json!({"action": "usage"}));
    let calls: Vec<String> = usage["calls"].as_array().unwrap().iter().map(|c| format!("{}/{}/{}/{}", c["status"].as_str().unwrap(), c["client"].as_str().unwrap(), c["run_id"].as_str().unwrap(), c["model_id"].as_str().unwrap())).collect();
    assert_eq!(calls, vec!["ok/pi/r1/frozen-model", "error/pi/r1/frozen-model", "ok/pi/r1/frozen-model"]);

    // An exhausted budget ends the run: nothing more runs under it.
    let (_, state) = call(&socket, "GET", "/local/pi/run", Value::Null);
    assert_eq!((state["run"]["model_calls"].as_u64(), state["run"]["stopped"].as_str()), (Some(3), Some("model_budget")));
    assert_eq!(with_token(&socket, "/v1/status", &token, json!({})).0, 409);

    // A new run rotates the token. Tools: counted, and only those granted. The tool budget is 2.
    let old = token;
    let (s, started) = call(&socket, "POST", "/local/pi/start", json!({"run_id": "r2", "writes": ["draft"], "budget": {"max_tool_calls": 2}}));
    assert_eq!(s, 200);
    let token = started["token"].as_str().unwrap().to_string();
    assert_eq!(with_token(&socket, "/v1/status", &old, json!({})).0, 401);
    assert_eq!(with_token(&socket, "/v1/status", &token, json!({})).0, 200);
    let (s, v) = with_token(&socket, "/v1/annotate", &token, json!({"event_ids": ["x"], "title": "t"}));
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("write_not_permitted")));
    let (s, v) = with_token(&socket, "/v1/status", &token, json!({}));
    assert_eq!((s, v["error"]["code"].as_str()), (429, Some("budget_exhausted")));
    let (s, v) = with_token(&socket, "/v1/usage", &token, json!({}));
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("not_permitted")), "usage is not one of Pi's tools");
    assert_eq!(with_token(&socket, "/local/pi/stop", &token, json!({})).0, 403, "only the App ends a run");

    let (_, state) = call(&socket, "GET", "/local/pi/run", Value::Null);
    assert_eq!(state["run"]["stopped"], "tool_budget");
    assert_eq!(call(&socket, "POST", "/local/pi/stop", json!({})).0, 200);
    assert_eq!(with_token(&socket, "/local/pi/model", &token, ask).0, 401, "the token ends with the run");
    service.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
