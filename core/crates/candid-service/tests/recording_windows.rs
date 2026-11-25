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

/// A synthetic voice service that records every request: windows answer with two speakers (or text only in quick
/// mode), the first window can be refused as too dense, and stitch returns three lines.
fn voice_service(dense_once: bool) -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut dense_once = dense_once;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (head, body) = read_request(&mut stream);
            let line = head.lines().next().unwrap_or("").to_string();
            if line.starts_with("GET /v1/voiceprints") {
                respond(&mut stream, 200, json!({"format":"candid-voiceprints/1","capabilities":["recording_windows","recording_windows_quick"],"people":[]}));
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
                if meta["mode"] == "quick" {
                    respond(&mut stream, 200, json!({"format":"speaker-window/1","mode":"quick","duration":seconds,"cpu_seconds":1.5,
                        "segments":[{"start":0.0,"end":seconds.min(30.0),"text":"SYNTHETIC 文字","speaker":null}],"speakers":{}}));
                    continue;
                }
                respond(&mut stream, 200, json!({"format":"speaker-window/1","duration":seconds,"cpu_seconds":2.0,
                    "segments":[{"start":1.0,"end":9.0,"text":"SYNTHETIC 甲","speaker":"S01"},{"start":10.0,"end":14.0,"text":"SYNTHETIC 乙","speaker":"S02"}],
                    "speakers":{"S01":{"ref":format!("w{}a",meta["index"]),"seconds":8.0},"S02":{"ref":format!("w{}b",meta["index"]),"seconds":4.0}}}));
            } else if line.starts_with("POST /v1/voiceprints/stitch") {
                let request: Value = serde_json::from_slice(&body).unwrap();
                log.lock().unwrap().push(("stitch".into(), request.clone()));
                if request["mode"] == "quick" {
                    respond(&mut stream, 200, json!({"format":"recording-speakers/2","mode":"quick","revision":request["revision"],"cpu_seconds":0.5,
                        "segments":[{"start_seconds":0.0,"end_seconds":30.0,"text":"SYNTHETIC 文字","speaker":null,"label":null,"voice_ref":null,"match":null,"correction_key":null}],
                        "speakers":[],"speaker_count":0,"unresolved_segments":0,"dropped_segments":0,"voice_refs":[],
                        "producer":{"model_id":"SYNTHETIC-asr","model_revision":"1","processor_version":"recording-windows/1"}}));
                    continue;
                }
                let line = |a: f64, text: &str, who: &str| json!({"start_seconds":a,"end_seconds":a+8.0,"text":text,"speaker":format!("recording-speaker:{who}"),"label":if who=="A"{"说话人 1"}else{"说话人 2"},"voice_ref":format!("SYNTHETIC-{who}"),"match":null,"correction_key":null});
                respond(&mut stream, 200, json!({"format":"recording-speakers/2","revision":request["revision"],"cpu_seconds":0.5,
                    "segments":[line(1.0,"SYNTHETIC 整段甲","A"),line(20.0,"SYNTHETIC 整段乙","B"),line(40.0,"SYNTHETIC 整段甲再说","A")],
                    "speakers":[],"speaker_count":2,"unresolved_segments":0,"dropped_segments":4,"voice_refs":["SYNTHETIC-A","SYNTHETIC-B"],
                    "producer":{"model_id":"SYNTHETIC","model_revision":"1","processor_version":"recording-windows/1"}}));
            } else {
                log.lock().unwrap().push((line.clone(), Value::Null));
                respond(&mut stream, 200, json!({"format":"candid-voiceprints/1","people":[],"matches":{},"forgotten":0}));
            }
        }
    });
    (url, seen)
}

fn voice_config(url: &str) -> Value {
    json!({"version":1,"providers":[{"provider_id":"voice","name":"SYNTHETIC","api":"other","base_url":url,"destination":"on_device","enabled":true}],
        "models":[{"provider_id":"voice","model_id":"SYNTHETIC"}],
        "roles":[{"role":"asr","model":{"provider_id":"voice","model_id":"SYNTHETIC"}},{"role":"diarization","model":{"provider_id":"voice","model_id":"SYNTHETIC"}}]})
}

struct Running {
    service: Option<Service>,
    socket: PathBuf,
}

impl Running {
    fn start(root: &Path, run: &Path) -> Running {
        let mut options = ServeOptions::new(root.to_path_buf(), run.to_path_buf()); options.power = Some(true);
        options.socket = Some(run.join("core.sock"));
        Running { service: Some(Service::start(&options, |_| {}).unwrap()), socket: run.join("core.sock") }
    }
    fn get(&self, path: &str, v: Value) -> Value {
        let (_, data) = local_request(&self.socket, "POST", path, &serde_json::to_vec(&v).unwrap()).unwrap();
        serde_json::from_slice(&data).unwrap()
    }
    fn jobs(&self, kind: &str) -> Vec<Value> {
        self.get("/local/jobs", json!({"action":"list"}))["jobs"].as_array().unwrap().iter().filter(|j| j["kind"] == kind).cloned().collect()
    }
    fn wait_done(&self, id: &Value) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let state = self.get("/local/jobs", json!({"action":"get","id":id}));
            assert!(state["state"] != "failed" && state["state"] != "cancelled", "{state}");
            if state["state"] == "done" {
                return state;
            }
            assert!(Instant::now() < deadline, "recording job did not finish: {state}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(s) = self.service.take() {
            s.stop();
        }
    }
}

#[test]
fn a_long_recording_is_processed_in_saved_windows_and_shown_as_one_transcript() {
    let dir = std::env::temp_dir().join(format!("rw-{}", &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let audio = dir.join("SYNTHETIC-13min.wav");
    synthetic_wav(&audio, 13 * 60);
    let (url, seen) = voice_service(true);

    let root = dir.join("root");
    let store = TimelineStore::open(&root, "recording-windows", true, Clock::default()).unwrap();
    store.power_declare(Some(true)).unwrap();
    store.model_command(&json!({"action":"save","config":voice_config(&url)})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let id = uuid_v4();
    store.media_command(&json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","source":"import","kind":"audio","path":audio})).unwrap();
    // Asking for a transcript queues the recording's one job.
    store.media_command(&json!({"action":"recognize","id":id,"mode":"conversation"})).unwrap();
    let job = store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().iter().find(|j| j["kind"] == RECORDING_JOB).cloned().unwrap();
    drop(store);

    let core = Running::start(&root, &dir.join("run"));
    let finished = core.wait_done(&job["id"]);
    let calls = seen.lock().unwrap().clone();
    let windows: Vec<&Value> = calls.iter().filter(|(k, _)| k == "window").map(|(_, v)| v).collect();
    // The first ten-minute window was too dense for one call; the job halved the window and went on.
    assert!((windows[0]["seconds"].as_f64().unwrap() - 600.0).abs() < 0.01, "{windows:?}");
    let saved = finished["checkpoint"]["speaker_windows"]["windows"].as_array().unwrap();
    let spans: Vec<(f64, f64)> = saved.iter().map(|w| (w["start"].as_f64().unwrap(), w["end"].as_f64().unwrap())).collect();
    assert_eq!(spans, vec![(0.0, 300.0), (240.0, 540.0), (480.0, 780.0)]);
    assert_eq!(windows.len(), 4);
    assert!(windows.iter().all(|w| w["meta"]["mode"] == "conversation"));
    let stitch = &calls.iter().find(|(k, _)| k == "stitch").unwrap().1;
    assert_eq!(stitch["windows"].as_array().unwrap().len(), 3);
    assert!(stitch.to_string().find("path").is_none(), "the voice service never sees file paths");

    let item = core.get("/local/media", json!({"action":"item","id":id}));
    let texts: Vec<_> = item["segments"].as_array().unwrap().iter().map(|s| s["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["SYNTHETIC 整段甲", "SYNTHETIC 整段乙", "SYNTHETIC 整段甲再说"]);
    assert_eq!(item["processing_status"], "ready");
    assert_eq!(item["processing_mode"], "conversation");
    assert_eq!(item["speaker_grouping"]["speaker_count"], 2);
    assert_eq!(item["speakers"].as_array().unwrap().len(), 2);
    // Each window and the stitch are in the call ledger.
    let usage = core.get("/local/models", json!({"action":"usage"}));
    assert_eq!(usage["calls"].as_array().unwrap().iter().filter(|c| c["feature"] == "transcribe").count(), 5);
    // Nothing asks for the recording to be transcribed again.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(core.jobs(RECORDING_JOB).len(), 1);
    drop(core);
    let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
}

const RECORDING: &str = "60000000-0000-4000-8000-000000000001";

fn part(store: &TimelineStore, dir: &Path, index: u32) {
    let path = dir.join(format!("SYNTHETIC-part-{index}.wav"));
    synthetic_wav(&path, 30);
    let id = format!("60000000-0000-4000-8000-0000000001{index:02}");
    store.media_command(&json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":path,
        "occurred_at":"2026-11-12T00:00:00Z",
        "recording":{"recording_id":RECORDING,"segment_index":index,"sample_start":index*30*16000,"sample_count":30*16000,"sample_rate":16000,
                     "recording_started_at":"2026-11-12T00:00:00Z"}})).unwrap();
}

#[test]
fn one_recording_is_one_windowed_job_after_it_stops_and_nothing_heavy_runs_while_recording() {
    let dir = std::env::temp_dir().join(format!("rw-one-{}", &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let (url, seen) = voice_service(false);
    let root = dir.join("root");
    let store = TimelineStore::open(&root, "recording-one-job", true, Clock::default()).unwrap();
    store.power_declare(Some(true)).unwrap();
    store.model_command(&json!({"action":"save","config":voice_config(&url)})).unwrap();
    // Automatic transcription on (the default), in conversation mode.
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":true,"titles":false,"summary":false,"mode":"conversation"}})).unwrap();
    part(&store, &dir, 0);
    store.media_command(&json!({"action":"state","id":RECORDING,"state":"recording"})).unwrap();
    drop(store);

    let core = Running::start(&root, &dir.join("run"));
    // While recording, more 30-second files arrive; none of them is transcribed on its own.
    for index in 1..3 {
        let path = dir.join(format!("SYNTHETIC-part-{index}.wav"));
        synthetic_wav(&path, 30);
        core.get("/local/media", json!({"action":"import","id":format!("60000000-0000-4000-8000-0000000001{index:02}"),"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":path,
            "occurred_at":"2026-11-12T00:00:00Z",
            "recording":{"recording_id":RECORDING,"segment_index":index,"sample_start":index*30*16000,"sample_count":30*16000,"sample_rate":16000,"recording_started_at":"2026-11-12T00:00:00Z"}}));
    }
    // A new recording's first file arrives before the app reports that it is recording: still nothing runs.
    let early = dir.join("SYNTHETIC-early.wav");
    synthetic_wav(&early, 30);
    core.get("/local/media", json!({"action":"import","id":"60000000-0000-4000-8000-000000000201","filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":early,
        "occurred_at":"2026-11-12T01:00:00Z",
        "recording":{"recording_id":"60000000-0000-4000-8000-000000000002","segment_index":0,"sample_start":0,"sample_count":30*16000,"sample_rate":16000,"recording_started_at":"2026-11-12T01:00:00Z"}}));
    std::thread::sleep(Duration::from_millis(2500));
    let heavy: Vec<Value> = core.get("/local/jobs", json!({"action":"list"}))["jobs"].as_array().unwrap().iter()
        .filter(|j| j["kind"].as_str().is_some_and(|k| k.starts_with("speech.") || k == "workflow.speakers") && j["state"] != "cancelled").cloned().collect();
    assert!(heavy.is_empty(), "no heavy job while recording: {heavy:?}");
    assert!(seen.lock().unwrap().iter().all(|(k, _)| !k.contains("window") && !k.contains("/voice")), "no model call while recording");

    core.get("/local/media", json!({"action":"state","id":RECORDING,"state":"stopped"}));
    let deadline = Instant::now() + Duration::from_secs(30);
    let job = loop {
        let jobs = core.jobs(RECORDING_JOB);
        if let Some(j) = jobs.iter().find(|j| j["state"] == "done") {
            break j.clone();
        }
        assert!(Instant::now() < deadline, "no recording job finished: {jobs:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(job["input_ref"], RECORDING);
    std::thread::sleep(Duration::from_millis(300));
    let calls = seen.lock().unwrap().clone();
    // One window covers the 90-second recording, then one stitch; no per-file or per-job transcription calls.
    let kinds: Vec<&str> = calls.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["window", "stitch"], "{calls:?}");
    assert!((calls[0].1["seconds"].as_f64().unwrap() - 90.0).abs() < 0.01);
    let live: Vec<Value> = core.jobs(RECORDING_JOB).into_iter().filter(|j| j["state"] != "cancelled").collect();
    assert_eq!(live.len(), 1, "exactly one job for the recording: {live:?}");
    let item = core.get("/local/media", json!({"action":"item","id":RECORDING}));
    assert_eq!(item["processing_status"], "ready");
    assert_eq!(item["segments"].as_array().unwrap().len(), 3);
    drop(core);
    let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
}

#[test]
fn quick_mode_is_the_same_job_on_the_small_model_without_speakers() {
    let dir = std::env::temp_dir().join(format!("rw-quick-{}", &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let (url, seen) = voice_service(false);
    let root = dir.join("root");
    let store = TimelineStore::open(&root, "recording-quick", true, Clock::default()).unwrap();
    store.power_declare(Some(true)).unwrap();
    store.model_command(&json!({"action":"save","config":voice_config(&url)})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":true,"titles":false,"summary":false,"mode":"quick"}})).unwrap();
    for index in 0..2 {
        part(&store, &dir, index);
    }
    store.media_command(&json!({"action":"state","id":RECORDING,"state":"stopped"})).unwrap();
    drop(store);
    let core = Running::start(&root, &dir.join("run"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let job = loop {
        if let Some(j) = core.jobs(RECORDING_JOB).into_iter().find(|j| j["state"] == "done") {
            break j;
        }
        assert!(Instant::now() < deadline, "quick job did not finish");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(job["config_version"].as_str().unwrap().starts_with("recording/1:quick:"));
    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["window", "stitch"]);
    assert_eq!(calls[0].1["meta"]["mode"], "quick");
    assert_eq!(calls[1].1["mode"], "quick");
    let item = core.get("/local/media", json!({"action":"item","id":RECORDING}));
    assert_eq!(item["processing_status"], "ready");
    assert_eq!(item["processing_mode"], "quick");
    assert!(item["segments"][0]["speaker"].is_null() && item["segments"][0]["text"] == "SYNTHETIC 文字");
    assert!(item.get("speaker_grouping").is_none_or(Value::is_null));
    drop(core);
    let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
}

#[test]
fn a_restarted_job_resumes_after_its_saved_windows() {
    let dir = std::env::temp_dir().join(format!("rw-resume-{}", &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let audio = dir.join("SYNTHETIC-13min.wav");
    synthetic_wav(&audio, 13 * 60);
    let (url, seen) = voice_service(false);
    let root = dir.join("root");
    let store = TimelineStore::open(&root, "recording-resume", true, Clock::default()).unwrap();
    store.power_declare(Some(true)).unwrap();
    store.model_command(&json!({"action":"save","config":voice_config(&url)})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let id = uuid_v4();
    store.media_command(&json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","source":"import","kind":"audio","path":audio})).unwrap();
    let job = store.enqueue_recording(&id, "conversation").unwrap();
    // An earlier attempt finished the first window before the app quit.
    let r = store.workflow_prepare(&job).unwrap();
    let first = json!({"start":0.0,"end":600.0,"result":{"format":"speaker-window/1","duration":600.0,"cpu_seconds":4.0,
        "segments":[{"start":1.0,"end":9.0,"text":"SYNTHETIC 甲","speaker":"S01"}],"speakers":{"S01":{"ref":"w0a","seconds":8.0}}}});
    let checkpoint = json!({"speaker_windows":{"identity":{"format":"speaker-windows/1","mode":"conversation","revision":r["input"]["revision"],"config_revision":r["config_revision"],"total":780.0},
        "size":600.0,"windows":[first]}});
    store.raw_db().execute("INSERT INTO job_checkpoints(job_id,owner,checkpoint) VALUES(?,NULL,?) ON CONFLICT(job_id) DO UPDATE SET checkpoint=excluded.checkpoint",
        &[candid_store::testing::rusqlite::types::Value::Text(job["id"].as_str().unwrap().into()), candid_store::testing::rusqlite::types::Value::Text(checkpoint.to_string())]).unwrap();
    drop(store);

    let core = Running::start(&root, &dir.join("run"));
    core.wait_done(&job["id"]);
    let calls = seen.lock().unwrap().clone();
    let windows: Vec<&Value> = calls.iter().filter(|(k, _)| k == "window").map(|(_, v)| v).collect();
    // Only the rest of the recording is sent again: one window from 9 to 13 minutes.
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!(windows[0]["meta"]["index"], 1);
    assert!((windows[0]["seconds"].as_f64().unwrap() - 240.0).abs() < 0.01);
    let stitch = &calls.iter().find(|(k, _)| k == "stitch").unwrap().1;
    assert_eq!(stitch["windows"].as_array().unwrap().len(), 2);
    drop(core);
    let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
}

/// Power v1 §2.2: on battery (no report: unknown counts as battery) automatic work waits, but a transcription the
/// user asked for is a manual run, and a result an outside worker hands back is accepted whatever the power state.
#[test]
fn an_explicit_transcription_is_manual_and_outside_results_are_accepted_on_battery() {
    let dir = std::env::temp_dir().join(format!("rw-manual-{}", &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let audio = dir.join("SYNTHETIC-30s.wav");
    synthetic_wav(&audio, 30);
    let store = TimelineStore::open(&dir.join("root"), "recording-manual", true, Clock::default()).unwrap();
    store.model_command(&json!({"action":"save","config":voice_config("http://127.0.0.1:1/v1")})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let id = uuid_v4();
    store.media_command(&json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","source":"import","kind":"audio","path":audio})).unwrap();
    assert_eq!(store.power_status().unwrap()["reason"], "waiting_power");
    store.media_command(&json!({"action":"recognize","id":id,"mode":"quick"})).unwrap();
    let jobs = store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().clone();
    let job = jobs.iter().find(|j| j["kind"] == RECORDING_JOB).expect("the recording job");
    assert_eq!((job["state"].as_str(), job["manual"].as_bool()), (Some("queued"), Some(true)), "{job}");
    // An outside worker answers; the result is taken although the Mac is on battery.
    let claimed = store.job_command(&json!({"action":"claim","id":job["id"],"kind":RECORDING_JOB,"owner":"SYNTHETIC"})).unwrap();
    store.media_command(&json!({"action":"result","format":"candid-recording-result/1","job_id":claimed["id"],"attempt":claimed["attempt"],"owner":"SYNTHETIC",
        "result":{"speaker_count":0,"unresolved_segments":0,"producer":{"model_id":"SYNTHETIC","model_revision":"fixture/1"},
            "segments":[{"start_seconds":0.0,"end_seconds":1.0,"text":"SYNTHETIC 手动"}]}})).unwrap();
    let item = store.media_command(&json!({"action":"item","id":id})).unwrap();
    assert_eq!((item["processing_status"].as_str(), item["segments"][0]["text"].as_str()), (Some("ready"), Some("SYNTHETIC 手动")), "{item}");
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
