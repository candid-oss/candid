//! Loading the lifecycle fixture (`core/fixtures/lifecycle/world.json`) into ingest batches.
#![allow(dead_code)]

use candid_store::*;
use serde_json::{json, Value};
use std::path::Path;

/// `HH:MM[:SS]` on `date` (YYYY-MM-DD) in Asia/Shanghai (UTC+8, no daylight saving), as Unix milliseconds.
pub fn local_ms(date: &str, time: &str) -> i64 {
    let time = if time.len() == 5 { format!("{time}:00") } else { time.to_string() };
    let t = chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M:%S").unwrap();
    t.and_utc().timestamp_millis() - 8 * 3_600_000
}

/// Vision, summary and agent_chat pointed at one loopback provider (the only three roles there are).
pub fn model_config(base_url: &str) -> Value {
    json!({"version": 1,
        "providers": [{"provider_id": "stub", "name": "SYNTHETIC stub", "api": "openai-chat", "base_url": base_url, "destination": "on_device", "enabled": true}],
        "models": [{"provider_id": "stub", "model_id": "stub-vision"}, {"provider_id": "stub", "model_id": "stub-text"}],
        "roles": [
            {"role": "vision", "model": {"provider_id": "stub", "model_id": "stub-vision"}},
            {"role": "summary", "model": {"provider_id": "stub", "model_id": "stub-text"}},
            {"role": "agent_chat", "model": {"provider_id": "stub", "model_id": "stub-text"}}
        ]})
}

/// The records of fixture day `day` (1-based), one batch per record in time order, each with its time: records are
/// replayed as they would arrive. `day`/`at`/`until` become times; a `screen.shot` with `ocr` gets a staged copy of
/// the fixture image and an `image.text` child carrying the OCR, in the same batch.
pub fn day_batches(world: &Value, day: i64, days: &[String], source: &str, image: &Path, store: &TimelineStore) -> Vec<(i64, IngestBatch)> {
    let date = &days[day as usize - 1];
    let mut out = vec![];
    for e in world["events"].as_array().unwrap().iter().filter(|e| e["day"] == day) {
        let mut e = e.clone();
        let m = e.as_object_mut().unwrap();
        let at = local_ms(date, m.remove("at").unwrap().as_str().unwrap());
        m.remove("day");
        m.insert("start_ms".into(), json!(at));
        if let Some(until) = m.remove("until") {
            m.insert("end_ms".into(), json!(local_ms(date, until.as_str().unwrap())));
        }
        let mut events = vec![];
        let ocr = m.remove("ocr");
        if let Some(ocr) = ocr {
            let bytes = std::fs::read(image).unwrap();
            let path = store.staging_directory().join(format!("{}.jpg", uuid_v4()));
            std::fs::write(&path, &bytes).unwrap();
            m.insert("blobs".into(), json!([{"staging_path": path, "sha256": sha256_hex(&bytes), "role": "original", "mime_type": "image/jpeg", "pixel_w": 96, "pixel_h": 64}]));
            let key = m["source_key"].as_str().unwrap().to_string();
            events.push(e.clone());
            events.push(json!({"source_key": format!("{key}-ocr"), "kind": "image.text", "start_ms": at, "parent_key": key,
                "app_name": e["app_name"], "payload": {"image_event_id": "pending", "method": "ocr"},
                "text": [{"field": "ocr", "content": ocr}]}));
        } else {
            events.push(e);
        }
        out.push((at, serde_json::from_value(json!({"source_id": source, "threads": world["threads"], "events": events})).unwrap()));
    }
    out.sort_by_key(|(at, _)| *at);
    out
}
