//! Product cancels through HTTP and rejects late output; it never signals model processes.
use candid_service::*;
use candid_store::*;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
fn post(socket: &std::path::Path, v: Value) -> Value {
    let (code, body) = local_request(
        socket,
        "POST",
        "/local/media",
        &serde_json::to_vec(&v).unwrap(),
    )
    .unwrap();
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}
#[test]
fn managed_voice_progress_and_cancel_release_the_core_execution_slot() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let acknowledged = Arc::new(AtomicBool::new(false));
    let halt = stopped.clone();
    let ack = acknowledged.clone();
    let server = std::thread::spawn(move || {
        while !halt.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..i]);
                    let size = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= i + 4 + size {
                        break;
                    }
                }
            }
            let body=if bytes.starts_with(b"GET /v1/voice/capabilities "){json!({"format":"candid-voice-jobs/1"})}else if bytes.starts_with(b"DELETE "){ack.store(true,Ordering::SeqCst);json!({"state":"cancelled"})}else{json!({"id":"synthetic-task","state":"running","stage":"transcribing","updated_ms":1790000000000i64,"completed_seconds":1,"total_seconds":4})}.to_string();
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
        }
    });
    let dir = std::env::temp_dir().join(format!("pvj-{}", &uuid_v4()[..8]));
    let socket = dir.join("run/core.sock");
    let store = TimelineStore::open(
        dir.join("root"),
        "managed-voice-test",
        true,
        Clock::default(),
    )
    .unwrap();
    store.model_command(&json!({"action":"save","config":{"version":1,"providers":[{"provider_id":"test","name":"SYNTHETIC","api":"other","base_url":format!("http://{address}/v1"),"destination":"on_device","enabled":true}],"models":[],"roles":[{"role":"asr","model":{"provider_id":"test","model_id":"SYNTHETIC"}}]}})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let id = uuid_v4();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/asr/sample-zh.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    store.media_command(&json!({"action":"import","id":id,"filename":"PUBLIC.wav","kind":"audio","source":"import","path":path})).unwrap();
    drop(store);
    let mut options = ServeOptions::new(dir.join("root"), dir.join("run")); options.power = Some(true);
    options.socket = Some(socket.clone());
    let service = Service::start(&options, |_| {}).unwrap();
    for _ in 0..2 {
        acknowledged.store(false, Ordering::SeqCst);
        post(&socket, json!({"action":"recognize","id":id}));
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let item = post(&socket, json!({"action":"item","id":id}));
            if item["processing"]["state"] == "running"
                && item["processing"]["stage"] == "transcribing"
            {
                break;
            }
            assert!(Instant::now() < deadline, "{item}");
            std::thread::sleep(Duration::from_millis(50));
        }
        post(&socket, json!({"action":"cancel_processing","id":id}));
        loop {
            let item = post(&socket, json!({"action":"item","id":id}));
            if item["processing"]["state"] == "cancelled"
                && item["processing"]["stop_confirmed"] == true
            {
                assert!(item["segments"].as_array().unwrap().is_empty());
                break;
            }
            assert!(Instant::now() < deadline, "{item}");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(acknowledged.load(Ordering::SeqCst));
    }
    service.stop();
    stopped.store(true, Ordering::SeqCst);
    server.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
