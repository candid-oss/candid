//! Data model v1 §9.4: a processed Mac recording part is re-encoded from WAV to AAC with the system encoder; the
//! WAV is deleted, the timeline reference follows, and retried uploads and re-links of the WAV still succeed.
//! Parts still waiting for processing, and imported (non-recording) WAVs, are left alone. Synthetic audio only.
#![cfg(target_os = "macos")]
use candid_service::audio_compaction;
use candid_store::*;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const RECORDING: &str = "50000000-0000-4000-8000-000000000001";
const PART: &str = "50000000-0000-4000-8000-000000000002";
const BUSY: &str = "50000000-0000-4000-8000-000000000003";
const BUSY_RECORDING: &str = "50000000-0000-4000-8000-000000000004";
const IMPORTED: &str = "50000000-0000-4000-8000-000000000005";
const RATE: u32 = 48_000;
const FRAMES: u32 = RATE * 3 + 77;

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("candid-compaction-{}", uuid_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A tone with some noise: realistic enough for the encoder, small enough for a test.
fn wav(path: &Path) {
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + FRAMES * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(RATE.to_le_bytes());
    b.extend((RATE * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((FRAMES * 2).to_le_bytes());
    let mut noise = 12345u32;
    for i in 0..FRAMES {
        noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let t = i as f32 / RATE as f32;
        let v = (t * 440.0 * std::f32::consts::TAU).sin() * 6000.0 + ((noise >> 16) as f32 / 65536.0 - 0.5) * 800.0;
        b.extend((v as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn part(id: &str, recording: &str, path: &Path) -> Value {
    json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":path,
           "occurred_at":"2026-11-12T00:00:00Z",
           "recording":{"recording_id":recording,"segment_index":0,"sample_start":0,"sample_count":FRAMES,"sample_rate":RATE,
                        "recording_started_at":"2026-11-12T00:00:00Z"}})
}

#[test]
fn processed_recording_parts_become_aac_and_the_wav_is_deleted() {
    let d = dir();
    let store = TimelineStore::open(&d.join("timeline"), "compaction", true, Clock::default()).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let source = d.join("SYNTHETIC.wav");
    wav(&source);
    let upload = part(PART, RECORDING, &source);
    store.media_command(&upload).unwrap();
    store.media_command(&json!({"action":"state","id":RECORDING,"state":"stopped"})).unwrap();
    // A second recording whose part still has transcription queued keeps its WAV.
    store.media_command(&part(BUSY, BUSY_RECORDING, &source)).unwrap();
    store.media_command(&json!({"action":"state","id":BUSY_RECORDING,"state":"stopped"})).unwrap();
    store.media_command(&json!({"action":"recognize","id":BUSY,"mode":"quick"})).unwrap();
    // A WAV imported by hand is not a new recording: it stays as it is.
    store.media_command(&json!({"action":"import","id":IMPORTED,"filename":"SYNTHETIC import.wav","kind":"audio","source":"import","path":source})).unwrap();
    store.project_native_media().unwrap();
    let wav_path = PathBuf::from(raw(&store, PART)["path"].as_str().unwrap());
    let wav_digest = sha256_hex_file(&wav_path).unwrap();
    assert!(wav_path.exists());

    // Not yet: the grace period after import has not passed.
    let shared = Arc::new(Mutex::new(store));
    assert_eq!(audio_compaction::pass(&shared, Duration::from_secs(3600), 8), 0);
    assert_eq!(audio_compaction::pass(&shared, Duration::ZERO, 8), 1);
    let store = shared.lock().unwrap();

    let aac_path = PathBuf::from(raw(&store, PART)["path"].as_str().unwrap());
    let aac = std::fs::read(&aac_path).unwrap();
    assert_eq!(&aac[4..8], b"ftyp");
    assert!(aac.len() * 10 < (FRAMES as usize * 2), "AAC {} bytes vs WAV {}", aac.len(), FRAMES * 2);
    // The shared WAV file still backs the busy and imported items, so it remains on disk for them.
    assert!(wav_path.exists());
    let item = store.media_item(RECORDING).unwrap();
    assert_eq!(item["recording_parts"][0]["sample_count"], FRAMES);
    let meta = store.media_command(&json!({"action":"item","id":PART})).unwrap();
    assert_eq!(meta["mime_type"], "audio/mp4", "{meta}");
    assert_eq!(meta["original_digest"], wav_digest.as_str());
    assert!(meta.get("compact_to").is_none_or(Value::is_null));
    assert_eq!(store.media_command(&json!({"action":"item","id":BUSY})).unwrap()["mime_type"], "audio/wav");
    assert_eq!(store.media_command(&json!({"action":"item","id":IMPORTED})).unwrap()["mime_type"], "audio/wav");

    // The timeline reference event now points at the AAC bytes.
    let aac_digest = sha256_hex(&aac);
    let blobs = rows(
        &store,
        "SELECT b.mime_type||'|'||b.rel_path FROM events e JOIN event_blobs eb ON eb.event_id=e.event_id JOIN blobs b ON b.sha256=eb.sha256 WHERE e.source_key=?",
        &format!("media:{PART}"),
    );
    assert_eq!(blobs.len(), 1, "{blobs:?}");
    assert!(blobs[0].starts_with("audio/mp4|") && blobs[0].contains(&aac_digest), "{blobs:?}");

    // A retried upload of the original WAV (lost receipt) and a re-link are both acknowledged.
    store.media_command(&upload).unwrap();
    store.media_command(&json!({"action":"link","id":PART,"recording":upload["recording"]})).unwrap();
    // Processing again reads the AAC (through a temporary WAV copy): the recording's length is unchanged.
    assert!((item["duration_seconds"].as_f64().unwrap() - FRAMES as f64 / RATE as f64).abs() < 1e-6);
    drop(store);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_wav_is_deleted_once_nothing_else_uses_it() {
    let d = dir();
    let store = TimelineStore::open(&d.join("timeline"), "compaction-delete", true, Clock::default()).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    let source = d.join("SYNTHETIC.wav");
    wav(&source);
    store.media_command(&part(PART, RECORDING, &source)).unwrap();
    store.media_command(&json!({"action":"state","id":RECORDING,"state":"stopped"})).unwrap();
    store.project_native_media().unwrap();
    let content = store.media_command(&json!({"action":"content","id":RECORDING})).unwrap();
    let wav_path = PathBuf::from(content["parts"][0]["path"].as_str().unwrap());
    let wav_digest = sha256_hex_file(&wav_path).unwrap();
    let shared = Arc::new(Mutex::new(store));
    // A recording still running is not compacted.
    shared.lock().unwrap().media_command(&json!({"action":"state","id":RECORDING,"state":"recording"})).unwrap();
    assert_eq!(audio_compaction::pass(&shared, Duration::ZERO, 8), 0);
    shared.lock().unwrap().media_command(&json!({"action":"state","id":RECORDING,"state":"stopped"})).unwrap();
    assert_eq!(audio_compaction::pass(&shared, Duration::ZERO, 8), 1);
    let store = shared.lock().unwrap();
    assert!(!wav_path.exists(), "the WAV original must be deleted");
    let stray = rows(&store, "SELECT rel_path FROM blobs WHERE sha256=?", &wav_digest);
    assert!(stray.is_empty(), "{stray:?}");
    let blob_files: Vec<_> = walk(&d.join("timeline")).into_iter().filter(|p| p.to_string_lossy().contains(&wav_digest)).collect();
    assert!(blob_files.is_empty(), "{blob_files:?}");
    // Nothing left to do on the next pass.
    drop(store);
    assert_eq!(audio_compaction::pass(&shared, Duration::ZERO, 8), 0);
    let _ = std::fs::remove_dir_all(d);
}

fn raw(store: &TimelineStore, id: &str) -> Value {
    let content = store.media_command(&json!({"action":"content","id":id})).unwrap();
    content["parts"].get(0).cloned().unwrap_or(content)
}

fn rows(store: &TimelineStore, sql: &str, arg: &str) -> Vec<String> {
    store.raw_db().query(sql, &[candid_store::testing::rusqlite::types::Value::Text(arg.into())], |r| candid_store::testing::RowExt::text(r, 0)).unwrap()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() { out.extend(walk(&p)); } else { out.push(p); }
    }
    out
}
