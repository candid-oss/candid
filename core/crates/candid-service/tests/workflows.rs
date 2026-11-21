//! Fixed workflows through an isolated real core. Synthetic transport and explicit public-model acceptance.
use candid_service::*;
use candid_store::*;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    time::{Duration, Instant},
};
fn config(url: &str) -> Value {
    json!({"version":1,"providers":[{"provider_id":"voice","name":"SYNTHETIC route","api":"other","base_url":url,"enabled":true,"destination":"on_device"}],"models":[],"roles":[{"role":"asr","model":{"provider_id":"voice","model_id":"public-voice"}},{"role":"title","model":{"provider_id":"voice","model_id":"text"}}]})
}
fn post(socket: &std::path::Path, path: &str, v: Value) -> Value {
    let (code, b) = local_request(socket, "POST", path, &serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(code, 200, "{}", String::from_utf8_lossy(&b));
    serde_json::from_slice(&b).unwrap()
}
fn start() -> (Service, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pw-{}", &uuid_v4()[..8]));
    let socket = dir.join("run/core.sock");
    let mut o = ServeOptions::new(dir.join("root"), dir.join("run"));
    o.socket = Some(socket.clone());
    let svc = Service::start(&o, |_| {}).unwrap();
    (svc, dir, socket)
}
fn import(socket: &std::path::Path, path: PathBuf) -> String {
    let id = uuid_v4();
    post(
        socket,
        "/local/media",
        json!({"action":"import","id":id,"filename":"PUBLIC.wav","kind":"audio","source":"import","path":path,"occurred_at":"2026-11-12T00:00:00Z"}),
    );
    id
}
fn wait(socket: &std::path::Path) -> Value {
    let until = Instant::now() + Duration::from_secs(150);
    loop {
        let v = post(socket, "/local/jobs", json!({"action":"list"}));
        if v["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["state"] == "done" || j["state"] == "failed")
        {
            return v;
        }
        assert!(Instant::now() < until, "{v}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
/// Waits until a job matching `select` reaches a final state and returns that job (other jobs may finish first).
fn wait_job(socket: &std::path::Path, select: impl Fn(&Value) -> bool) -> Value {
    let until = Instant::now() + Duration::from_secs(150);
    loop {
        let v = post(socket, "/local/jobs", json!({"action":"list"}));
        if let Some(job) = v["jobs"].as_array().unwrap().iter()
            .find(|j| select(j) && (j["state"] == "done" || j["state"] == "failed"))
        {
            return job.clone();
        }
        assert!(Instant::now() < until, "{v}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
#[test]
#[ignore = "explicit public sample model acceptance; requires independently running voice service"]
fn public_voice_is_consumed_searched_and_located() {
    let (svc, dir, socket) = start();
    post(
        &socket,
        "/local/models",
        json!({"action":"save","config":config("http://127.0.0.1:8767/v1")}),
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    let id = import(&socket, path);
    post(
        &socket,
        "/local/media",
        json!({"action":"recognize","id":id,"mode":"quick"}),
    );
    let jobs = wait(&socket);
    assert_eq!(jobs["jobs"][0]["state"], "done", "{jobs}");
    let item = post(&socket, "/local/media", json!({"action":"item","id":id}));
    let segment = &item["segments"][0];
    assert!(segment["sample_end"].as_u64().unwrap() > segment["sample_start"].as_u64().unwrap());
    assert_eq!(item["producer"]["timestamp_kind"], "audio_chunk");
    let text = segment["text"].as_str().unwrap();
    let q = text.chars().take(3).collect::<String>();
    let hits = post(&socket, "/v1/search", json!({"q":q,"attention":"all"}));
    assert!(!hits["hits"].as_array().unwrap().is_empty(), "{hits}");
    let content = post(&socket, "/local/media", json!({"action":"content","id":id}));
    assert!(std::path::Path::new(content["path"].as_str().unwrap()).is_file());
    let usage = post(&socket, "/local/models", json!({"action":"usage"}));
    assert_eq!(usage["calls"][0]["status"], "ok");
    assert!(usage["calls"][0]["cost"].is_null());
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn malformed_provider_is_failed_without_hidden_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..i]);
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= i + 4 + length {
                    break;
                }
            }
        }
        if bytes.starts_with(b"GET /v1/voice/capabilities ") {
            stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            continue;
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad")
            .unwrap();
        }
    });
    let (svc, dir, socket) = start();
    post(
        &socket,
        "/local/models",
        json!({"action":"save","config":config(&format!("http://{addr}/v1"))}),
    );
    post(&socket, "/local/workflows", json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}}));
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    let id = import(&socket, path);
    post(
        &socket,
        "/local/media",
        json!({"action":"recognize","id":id}),
    );
    let jobs = wait(&socket);
    assert_eq!(jobs["jobs"][0]["state"], "failed", "{jobs}");
    assert_eq!(jobs["jobs"][0]["error_code"], "bad_json");
    let usage = post(&socket, "/local/models", json!({"action":"usage"}));
    assert_eq!(usage["calls"].as_array().unwrap().len(), 1);
    server.join().unwrap();
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn recording_pipeline_preserves_repetitions_gaps_and_manual_edits() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    let digest = sha256_hex_file(&path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let halt = stop.clone();
    let server = std::thread::spawn(move || {
        while !halt.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream.set_nonblocking(false).unwrap();
            let mut b = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        b.extend_from_slice(&chunk[..n]);
                        if let Some(i) = b.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&b[..i]);
                            let len = head
                                .lines()
                                .find_map(|l| {
                                    l.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|s| s.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if b.len() >= i + 4 + len {
                                break;
                            }
                        }
                    }
                }
            }
            let body=if b.starts_with(b"POST /v1/voice "){json!({"format":"candid-voice-result/1","source_hash":digest,"producer":{"model_id":"SYNTHETIC","model_revision":"1","processor_version":"1","timestamp_kind":"audio_chunk"},"segments":[{"start_seconds":0.0,"end_seconds":1.0,"text":"SYNTHETIC 重复话语"},{"start_seconds":1.0,"end_seconds":2.0,"text":"SYNTHETIC 重复话语"}]})}else{json!({"choices":[{"message":{"content":"SYNTHETIC 草稿"}}],"usage":{"prompt_tokens":100,"completion_tokens":10}})}.to_string();
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
    let (svc, dir, socket) = start();
    let mut c = config(&format!("http://{addr}/v1"));
    c["providers"][0]["api"] = json!("openai-chat");
    c["roles"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"summary","model":{"provider_id":"voice","model_id":"text"}}));
    post(
        &socket,
        "/local/models",
        json!({"action":"save","config":c}),
    );
    let recording = uuid_v4();
    for (index, start) in [(0, 0), (1, 100000)] {
        let id = uuid_v4();
        post(
            &socket,
            "/local/media",
            json!({"action":"import","id":id,"filename":"PUBLIC.wav","kind":"audio","source":"mac_microphone","path":path,"occurred_at":"2026-11-12T00:00:00Z","recording":{"recording_id":recording,"segment_index":index,"sample_start":start,"sample_count":67263,"sample_rate":16000,"recording_started_at":"2026-11-12T00:00:00Z"}}),
        );
        if index == 0 {
            post(
                &socket,
                "/local/media",
                json!({"action":"state","id":recording,"state":"recording"}),
            );
            post(
                &socket,
                "/local/workflows",
                json!({"action":"settings","settings":{"version":1,"automatic":true,"summary":true,"mode":"quick"}}),
            );
            let jobs = wait(&socket);
            assert!(jobs["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|j| j["kind"] == "speech.recognize"));
        }
    }
    post(
        &socket,
        "/local/media",
        json!({"action":"edit","id":recording,"title":"SYNTHETIC 人工标题","notes":"SYNTHETIC 人工备注"}),
    );
    post(
        &socket,
        "/local/media",
        json!({"action":"state","id":recording,"state":"stopped"}),
    );
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        let jobs = post(&socket, "/local/jobs", json!({"action":"list"}));
        let rows = jobs["jobs"].as_array().unwrap();
        if rows.iter().filter(|j| j["state"] == "done").count() == 4 {
            break;
        }
        assert!(Instant::now() < until, "{jobs}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let item = post(
        &socket,
        "/local/media",
        json!({"action":"item","id":recording}),
    );
    assert_eq!(item["title"], "SYNTHETIC 人工标题");
    assert_eq!(item["notes"], "SYNTHETIC 人工备注");
    assert_eq!(item["segments"].as_array().unwrap().len(), 4);
    assert_eq!(item["gaps"].as_array().unwrap().len(), 1);
    let drafts = post(
        &socket,
        "/local/workflows",
        json!({"action":"drafts","id":recording}),
    );
    assert_eq!(drafts["summary"]["text"], "SYNTHETIC 草稿");
    let usage = post(&socket, "/local/models", json!({"action":"usage"}));
    assert_eq!(usage["calls"].as_array().unwrap().len(), 4);
    svc.stop();
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn long_audio_chunks_resume_after_failure_and_core_restart_without_rebilling_completed_chunks() {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let halt = Arc::new(AtomicBool::new(false));
    let stopped = halt.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let sent = requests.clone();
    let server = std::thread::spawn(move || {
        while !stopped.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let mut b = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                b.extend_from_slice(&chunk[..n]);
                if let Some(i) = b.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&b[..i]);
                    let size = header
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if b.len() >= i + 4 + size {
                        break;
                    }
                }
            }
            if b.starts_with(b"GET /v1/voice/capabilities ") {
                stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                continue;
            }
            let index = sent.fetch_add(1, Ordering::SeqCst);
            let (status, body) = if index == 1 {
                ("429 Too Many Requests", json!({"error":"SYNTHETIC"}))
            } else {
                let offset = b.windows(4).position(|w| w == b"RIFF").unwrap();
                let size =
                    u32::from_le_bytes(b[offset + 4..offset + 8].try_into().unwrap()) as usize + 8;
                let hash = sha256_hex(&b[offset..offset + size]);
                (
                    "200 OK",
                    json!({"format":"candid-voice-result/1","source_hash":hash,"producer":{"model_id":"SYNTHETIC chunks","model_revision":"1","processor_version":"1","timestamp_kind":"audio_chunk"},"segments":[{"start_seconds":1.0,"end_seconds":2.0,"text":"SYNTHETIC chunk speech"}]}),
                )
            };
            let body = body.to_string();
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    let (svc, dir, socket) = start();
    let path = dir.join("SYNTHETIC-301-seconds.wav");
    let mut b = b"RIFF".to_vec();
    b.extend(602036u32.to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1000u32.to_le_bytes());
    b.extend(2000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(602000u32.to_le_bytes());
    b.resize(602044, 0);
    std::fs::write(&path, b).unwrap();
    let original = sha256_hex_file(&path).unwrap();
    post(
        &socket,
        "/local/models",
        json!({"action":"save","config":config(&format!("http://{addr}/v1"))}),
    );
    let id = import(&socket, path.clone());
    post(
        &socket,
        "/local/media",
        json!({"action":"recognize","id":id}),
    );
    let first = wait_job(&socket, |j| !j["checkpoint"]["next_sample"].is_null());
    assert_eq!(first["state"], "failed");
    assert_eq!(first["checkpoint"]["next_sample"], 240000);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    let job = first["id"].clone();
    svc.stop();
    let mut options = ServeOptions::new(dir.join("root"), dir.join("run"));
    options.socket = Some(socket.clone());
    let svc = Service::start(&options, |_| {}).unwrap();
    post(&socket, "/local/jobs", json!({"action":"restart","id":job}));
    let second = wait_job(&socket, |j| j["id"] == job && j["state"] == "done");
    assert_eq!(second["state"], "done", "{second}");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        3,
        "first completed chunk must never be sent again"
    );
    let item = post(&socket, "/local/media", json!({"action":"item","id":id}));
    assert_eq!(item["segments"].as_array().unwrap().len(), 2);
    assert_eq!(item["segments"][0]["sample_start"], 1000);
    assert_eq!(item["segments"][1]["sample_start"], 241000);
    assert_eq!(sha256_hex_file(&path).unwrap(), original);
    let usage = post(&socket, "/local/models", json!({"action":"usage"}));
    assert_eq!(usage["calls"].as_array().unwrap().len(), 3);
    svc.stop();
    halt.store(true, Ordering::SeqCst);
    server.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn timeout_and_offline_are_recorded_once_without_fallback() {
    for offline in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = if offline {
            drop(listener);
            None
        } else {
            Some(std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = [0; 4096];
                let _ = stream.read(&mut bytes);
                std::thread::sleep(Duration::from_millis(2000));
            }))
        };
        let (svc, dir, socket) = start();
        let mut c = config(&format!("http://{addr}/v1"));
        c["providers"][0]["api"] = json!("openai-chat");
        for role in c["roles"].as_array_mut().unwrap() {
            role["limits_json"] = json!({"timeout_ms":500});
        }
        post(
            &socket,
            "/local/models",
            json!({"action":"save","config":c}),
        );
        let(code,body)=local_request(&socket,"POST","/local/models/complete",&serde_json::to_vec(&json!({"model":{"provider_id":"voice","model_id":"text"},"feature":"SYNTHETIC timeout","messages":[{"role":"user","content":"SYNTHETIC"}]})).unwrap()).unwrap();
        assert_eq!(code, 409);
        let usage = post(&socket, "/local/models", json!({"action":"usage"}));
        assert_eq!(usage["calls"].as_array().unwrap().len(), 1, "{}", String::from_utf8_lossy(&body));
        assert_eq!(
            usage["calls"][0]["error_code"],
            if offline { "offline" } else { "timeout" }
        );
        assert!(usage["calls"][0]["cost"].is_null());
        if let Some(s) = server {
            s.join().unwrap();
        }
        svc.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
#[ignore = "explicit public two-speaker model acceptance; independently running voice API"]
fn public_conversation_preserves_anonymous_speakers_and_estimated_positions() {
    let (svc, dir, socket) = start();
    let mut c = config("http://127.0.0.1:8767/v1");
    c["roles"].as_array_mut().unwrap().push(
        json!({"role":"diarization","model":{"provider_id":"voice","model_id":"public-voice"}}),
    );
    post(
        &socket,
        "/local/models",
        json!({"action":"save","config":c}),
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/speaker-demo/public-audio/conversation.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    let id = import(&socket, path);
    post(
        &socket,
        "/local/media",
        json!({"action":"recognize","id":id,"mode":"conversation"}),
    );
    let jobs = wait(&socket);
    assert_eq!(jobs["jobs"][0]["state"], "done", "{jobs}");
    let item = post(&socket, "/local/media", json!({"action":"item","id":id}));
    let segments = item["segments"].as_array().unwrap();
    assert!(segments.len() >= 2);
    let speakers: std::collections::HashSet<_> = segments
        .iter()
        .filter_map(|s| s["speaker"].as_str())
        .collect();
    assert!(speakers.len() >= 2);
    assert_ne!(item["producer"]["timestamp_kind"], "word");
    assert!(segments
        .iter()
        .all(|s| s["sample_end"].as_u64().unwrap() > s["sample_start"].as_u64().unwrap()));
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn screen_episode_summary_runs_through_real_queue_and_mock_model_once() {
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();let address=listener.local_addr().unwrap();
    let server=std::thread::spawn(move||{
        let deadline=Instant::now()+Duration::from_secs(20);
        let mut stream=loop {match listener.accept(){Ok((stream,_))=>break stream,Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>{assert!(Instant::now()<deadline,"screen model was never called");std::thread::sleep(Duration::from_millis(20));},Err(e)=>panic!("{e}")}};
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();let mut bytes=Vec::new();let mut buffer=[0;4096];
        let body=loop {
            let n=stream.read(&mut buffer).unwrap();assert!(n>0);bytes.extend_from_slice(&buffer[..n]);
            if let Some(split)=bytes.windows(4).position(|w|w==b"\r\n\r\n"){
                let header=String::from_utf8_lossy(&bytes[..split]);let len=header.lines().find_map(|l|l.to_lowercase().strip_prefix("content-length:").and_then(|s|s.trim().parse::<usize>().ok())).unwrap();
                if bytes.len()>=split+4+len{break serde_json::from_slice::<Value>(&bytes[split+4..split+4+len]).unwrap();}
            }
        };
        assert_eq!(body["response_format"]["type"],"json_schema");
        assert_eq!(body["response_format"]["json_schema"]["schema"]["additionalProperties"],false);
        let content=body["messages"][1]["content"].as_str().unwrap();assert!(content.contains("100")&&content.contains("200"));
        assert!(content.contains("[x1]"),"{content}");let reference="x1";
        let result=json!({"title":"合成预算讨论","summary":"预算从100元改成200元，仍待确认。","citations":[reference]});
        let answer=json!({"choices":[{"message":{"content":result.to_string()}}],"usage":{"prompt_tokens":20,"completion_tokens":30}}).to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",answer.len(),answer).unwrap();
    });
    let dir=std::env::temp_dir().join(format!("pw-screen-{}",&uuid_v4()[..8]));
    let root=dir.join("root");let socket=dir.join("run/core.sock");
    let store=TimelineStore::open(&root,"SYNTHETIC-screen-test",true,Clock::default()).unwrap();
    let source=store.register_source("mac.desktop","SYNTHETIC desktop",&SourcePolicy::default()).unwrap();
    let now=chrono::Utc::now().timestamp_millis()-3600000;
    let events=(0..2).map(|i|{let mut e=IngestEvent::new(&format!("SYNTHETIC-{i}"),"screen.text",Some(now+i*10000));e.title=Some("SYNTHETIC window".into());e.app_name=Some("SYNTHETIC Notes".into());e.text=vec![IngestText{field:"body".into(),content:format!("预算 {} 元，等待确认",100+i*100),visible:Some(true),focus_distance:Some(0),parts:None}];e}).collect();
    store.ingest(&IngestBatch::events(&source,events)).unwrap();
    // The summary reads the AI-readable layer only, so the block needs an excerpt of its app.
    store.raw_db().script(&format!("INSERT INTO excerpts(excerpt_id,key,app,title,url,start_ms,end_ms,text,producer,created_ms) VALUES('ex:1','k','SYNTHETIC Notes','预算',NULL,{now},{e},'预算从 100 元改成 200 元，等待确认','test',{now}); INSERT INTO excerpt_sources SELECT 'ex:1',event_id FROM events WHERE kind='screen.text' LIMIT 1;",e=now+10000)).unwrap();
    store.model_command(&json!({"action":"save","config":{"version":1,"providers":[{"provider_id":"mock","name":"SYNTHETIC mock","api":"openai-chat","base_url":format!("http://{address}/v1"),"destination":"on_device","enabled":true}],"models":[{"provider_id":"mock","model_id":"SYNTHETIC"}],"roles":[{"role":"episode","model":{"provider_id":"mock","model_id":"SYNTHETIC"}}]}})).unwrap();
    let mut features=store.feature_settings().unwrap();features["items"]["screen-episodes"]["enabled"]=json!(true);
    store.workflow_command(&json!({"action":"features","config":features})).unwrap();
    store.screen_scan().unwrap();
    let episode=store.screen_command(&json!({"action":"status"})).unwrap()["episodes"][0]["id"].clone();
    assert!(store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().is_empty());
    store.screen_command(&json!({"action":"run","id":episode})).unwrap();
    drop(store);
    let mut options=ServeOptions::new(root,dir.join("run"));options.socket=Some(socket.clone());
    let service=Service::start(&options, |_|{}).unwrap();
    let jobs=wait(&socket);assert!(jobs["jobs"].as_array().unwrap().iter().any(|j|j["kind"]=="screen.summarize"&&j["state"]=="done"),"{jobs}");
    let screen=post(&socket,"/local/screen",json!({"action":"status"}));assert_eq!(screen["episodes"][0]["summary"],"预算从100元改成200元，仍待确认。");
    let usage=post(&socket,"/local/models",json!({"action":"usage"}));assert_eq!(usage["calls"].as_array().unwrap().iter().filter(|c|c["feature"]=="screen-episodes"&&c["status"]=="ok").count(),1);
    service.stop();server.join().unwrap();std::fs::remove_dir_all(dir).unwrap();
}
