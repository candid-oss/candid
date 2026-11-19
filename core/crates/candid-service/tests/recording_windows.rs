//! Whole-recording speakers in resumable windows, against a synthetic voice service and synthetic audio.
use candid_service::voiceprints::{next_window, WINDOW_OVERLAP_SECONDS, WINDOW_SECONDS};
use candid_service::*;
use candid_store::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn windows_cover_the_recording_with_overlap_and_no_tiny_tail() {
    assert_eq!(next_window(None, 240.0, WINDOW_SECONDS), Some((0.0, 240.0)));
    assert_eq!(next_window(Some(240.0), 240.0, WINDOW_SECONDS), None);
    // A recording slightly over one window stays one call instead of leaving a two-minute tail.
    assert_eq!(next_window(None, 700.0, WINDOW_SECONDS), Some((0.0, 700.0)));
    let total = 3600.0;
    let (mut end, mut windows) = (None, vec![]);
    while let Some(w) = next_window(end, total, WINDOW_SECONDS) {
        windows.push(w);
        end = Some(w.1);
    }
    assert_eq!(windows.first().unwrap().0, 0.0);
    assert_eq!(windows.last().unwrap().1, total);
    for pair in windows.windows(2) {
        assert!((pair[0].1 - pair[1].0 - WINDOW_OVERLAP_SECONDS).abs() < 1e-9, "{pair:?}");
    }
    assert!(windows.iter().all(|(a, b)| b - a <= 12.0 * 60.0));
}

fn synthetic_wav(path: &Path, seconds: u32) {
    let frames = seconds * 16000;
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + frames * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]);
    wav.extend(16000u32.to_le_bytes());
    wav.extend(32000u32.to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend((frames * 2).to_le_bytes());
    for i in 0..frames {
        wav.extend((((i % 160) as i16 - 80) * 50).to_le_bytes());
    }
    std::fs::write(path, wav).unwrap();
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    let mut b = Vec::new();
    let mut tmp = [0; 65536];
    loop {
        let n = stream.read(&mut tmp).unwrap();
        if n == 0 {
            return (String::new(), b);
        }
        b.extend(&tmp[..n]);
        if let Some(i) = b.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&b[..i]).to_string();
            let len = head
                .lines()
                .find_map(|l| l.to_lowercase().strip_prefix("content-length:").and_then(|s| s.trim().parse::<usize>().ok()))
                .unwrap_or(0);
            if b.len() >= i + 4 + len {
                return (head, b[i + 4..i + 4 + len].to_vec());
            }
        }
    }
}

fn respond(stream: &mut std::net::TcpStream, status: u16, v: Value) {
    let b = v.to_string();
    let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len());
}

#[test]
fn a_long_recording_is_processed_in_saved_windows_and_shown_as_one_transcript() {
    let dir = std::env::temp_dir().join(format!("rw-{}", &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let audio = dir.join("SYNTHETIC-13min.wav");
    synthetic_wav(&audio, 13 * 60);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut dense_once = true;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (head, body) = read_request(&mut stream);
            let line = head.lines().next().unwrap_or("").to_string();
            if line.starts_with("GET /v1/voiceprints") {
                respond(&mut stream, 200, json!({"format":"candid-voiceprints/1","capabilities":["recording_windows"],"people":[]}));
            } else if line.starts_with("POST /v1/voiceprints/window") {
                let text = String::from_utf8_lossy(&body).to_string();
                let meta: Value = text.split("name=\"meta\"\r\n\r\n").nth(1).and_then(|s| s.split("\r\n").next()).and_then(|s| serde_json::from_str(s).ok()).unwrap();
                let riff = body.windows(4).position(|w| w == b"RIFF").unwrap();
                let seconds = u32::from_le_bytes(body[riff + 40..riff + 44].try_into().unwrap()) as f64 / 32000.0;
                log.lock().unwrap().push(("window".into(), json!({"meta":meta,"seconds":seconds})));
                if dense_once {
                    dense_once = false;
                    respond(&mut stream, 422, json!({"detail":"window_too_dense"}));
                    continue;
                }
                respond(&mut stream, 200, json!({"format":"speaker-window/1","duration":seconds,
                    "segments":[{"start":1.0,"end":9.0,"text":"SYNTHETIC 甲","speaker":"S01"},{"start":10.0,"end":14.0,"text":"SYNTHETIC 乙","speaker":"S02"}],
                    "speakers":{"S01":{"ref":format!("w{}a",meta["index"]),"seconds":8.0},"S02":{"ref":format!("w{}b",meta["index"]),"seconds":4.0}}}));
            } else if line.starts_with("POST /v1/voiceprints/stitch") {
                let request: Value = serde_json::from_slice(&body).unwrap();
                log.lock().unwrap().push(("stitch".into(), request.clone()));
                let line = |a: f64, text: &str, who: &str| json!({"start_seconds":a,"end_seconds":a+8.0,"text":text,"speaker":format!("recording-speaker:{who}"),"label":if who=="A"{"说话人 1"}else{"说话人 2"},"voice_ref":format!("SYNTHETIC-{who}"),"match":null,"correction_key":null});
                respond(&mut stream, 200, json!({"format":"recording-speakers/2","revision":request["revision"],
                    "segments":[line(1.0,"SYNTHETIC 整段甲","A"),line(400.0,"SYNTHETIC 整段乙","B"),line(700.0,"SYNTHETIC 整段甲再说","A")],
                    "speakers":[],"speaker_count":2,"unresolved_segments":0,"dropped_segments":4,"voice_refs":["SYNTHETIC-A","SYNTHETIC-B"],
                    "producer":{"model_id":"SYNTHETIC","model_revision":"1","processor_version":"recording-windows/1"}}));
            } else {
                respond(&mut stream, 200, json!({"format":"candid-voiceprints/1","people":[],"matches":{},"forgotten":0}));
            }
        }
    });

    let root = dir.join("root");
    let store = TimelineStore::open(&root, "recording-windows", true, Clock::default()).unwrap();
    let mut config = json!({"version":1,"providers":[{"provider_id":"voice","name":"SYNTHETIC","api":"other","base_url":url,"destination":"on_device","enabled":true}],
        "models":[{"provider_id":"voice","model_id":"SYNTHETIC"}],"roles":[{"role":"asr","model":{"provider_id":"voice","model_id":"SYNTHETIC"}}]});
    store.model_command(&json!({"action":"save","config":config})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let id = uuid_v4();
    store.media_command(&json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","source":"import","kind":"audio","path":audio})).unwrap();
    store.media_command(&json!({"action":"recognize","id":id,"mode":"quick"})).unwrap();
    let job = store.job_command(&json!({"action":"claim","kind":"speech.recognize","owner":"fixture","lease_ms":180000})).unwrap();
    let request = store.workflow_prepare(&job).unwrap();
    // A per-file transcript in four-minute pieces, the way long files are transcribed.
    let pieces: Vec<Value> = (0..4).map(|n| json!({"start_seconds":n as f64*200.0,"end_seconds":n as f64*200.0+10.0,"text":format!("SYNTHETIC 分块{n}"),"speaker":format!("分块{} · 说话人 1",n+1)})).collect();
    store.workflow_complete(&request, &json!({"producer":{"model_id":"SYNTHETIC","revision":"fixture/1"},"segments":pieces})).unwrap();
    config["roles"] = json!([{"role":"diarization","model":{"provider_id":"voice","model_id":"SYNTHETIC"}}]);
    store.model_command(&json!({"action":"save","config":config})).unwrap();
    let job = store.enqueue_recording_speakers(&id).unwrap();
    drop(store);

    let run = dir.join("run");
    let mut options = ServeOptions::new(root.clone(), run.clone());
    options.socket = Some(run.join("core.sock"));
    let service = Service::start(&options, |_| {}).unwrap();
    let socket = run.join("core.sock");
    let get = |path: &str, v: Value| -> Value {
        let (_, data) = local_request(&socket, "POST", path, &serde_json::to_vec(&v).unwrap()).unwrap();
        serde_json::from_slice(&data).unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    let finished = loop {
        let state = get("/local/jobs", json!({"action":"get","id":job["id"]}));
        assert!(state["state"] != "failed" && state["state"] != "cancelled", "{state}");
        if state["state"] == "done" {
            break state;
        }
        assert!(Instant::now() < deadline, "speaker job did not finish: {state}");
        std::thread::sleep(Duration::from_millis(100));
    };

    let calls = seen.lock().unwrap().clone();
    let windows: Vec<&Value> = calls.iter().filter(|(k, _)| k == "window").map(|(_, v)| v).collect();
    // The first ten-minute window was too dense for one call; the job halved the window and went on.
    assert!((windows[0]["seconds"].as_f64().unwrap() - 600.0).abs() < 0.01, "{windows:?}");
    let saved = finished["checkpoint"]["speaker_windows"]["windows"].as_array().unwrap();
    let spans: Vec<(f64, f64)> = saved.iter().map(|w| (w["start"].as_f64().unwrap(), w["end"].as_f64().unwrap())).collect();
    assert_eq!(spans, vec![(0.0, 300.0), (240.0, 540.0), (480.0, 780.0)]);
    assert_eq!(windows.len(), 4);
    let stitch = &calls.iter().find(|(k, _)| k == "stitch").unwrap().1;
    assert_eq!(stitch["windows"].as_array().unwrap().len(), 3);
    assert!(stitch.to_string().find("path").is_none(), "the voice service never sees file paths");

    let item = get("/local/media", json!({"action":"item","id":id}));
    let texts: Vec<_> = item["segments"].as_array().unwrap().iter().map(|s| s["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["SYNTHETIC 整段甲", "SYNTHETIC 整段乙", "SYNTHETIC 整段甲再说"]);
    assert_eq!(item["speaker_grouping"]["speaker_count"], 2);
    assert_eq!(item["speakers"].as_array().unwrap().len(), 2);
    assert!(item["segments"].as_array().unwrap().iter().all(|s| !s["speaker"].as_str().unwrap_or("").contains("分块")));
    // Nothing asks for the recording to be grouped again.
    std::thread::sleep(Duration::from_millis(500));
    let jobs = get("/local/jobs", json!({"action":"list"}));
    let speaker_jobs = jobs["jobs"].as_array().unwrap().iter().filter(|j| j["kind"] == "workflow.speakers").count();
    assert_eq!(speaker_jobs, 1);
    service.stop();
    let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
}
