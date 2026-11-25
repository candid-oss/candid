//! Independent acceptance of the native-media path through the shared query service.
mod common;
use common::Fx;
use candid_store::*;
use serde_json::{json, Value};

const PART: &str = "20000000-0000-4000-8000-000000000001";
const RECORDING: &str = "20000000-0000-4000-8000-000000000002";

fn import(fx: &Fx, grouped: bool) {
    let path = fx.root.join("SYNTHETIC.wav");
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(16036u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(8000u32.to_le_bytes());
    bytes.extend(16000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(16000u32.to_le_bytes());
    bytes.resize(16044, 0);
    std::fs::write(&path, bytes).unwrap();
    let mut request = json!({"action":"import","id":PART,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":path,"occurred_at":"2026-11-12T00:00:00Z"});
    if grouped {
        request["recording"] = json!({"recording_id":RECORDING,"segment_index":0,"sample_start":0,"sample_count":8000,"sample_rate":8000,"recording_started_at":"2026-11-12T00:00:00Z"});
    }
    fx.store.media_command(&request).unwrap();
}

/// The recording's one job answers with a single line of text.
fn submit(fx: &Fx, id: &str, text: &str, start: f64, end: f64) -> Result<Value> {
    testing::transcribe_recording(&fx.store, id, &[json!({"start_seconds":start,"end_seconds":end,"text":text})])
}

fn recording_jobs(fx: &Fx) -> Vec<Value> {
    fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().iter().filter(|j| j["kind"] == RECORDING_JOB).cloned().collect()
}

#[test]
fn explicitly_recognizing_a_finished_recording_really_queues_a_new_attempt() {
    let fx = Fx::make("native-rerun-acceptance");
    import(&fx, false);
    submit(&fx, PART, "SYNTHETIC first result", 0.0, 1.0).unwrap();
    fx.store.media_command(&json!({"action":"recognize","id":PART,"mode":"quick"})).unwrap();
    let next = recording_jobs(&fx);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0]["state"], "queued", "the explicit re-recognize action must do work after success");
    assert_eq!(next[0]["checkpoint"], json!({}));
    submit(&fx, PART, "SYNTHETIC replacement result", 0.0, 1.0).unwrap();
    let old = fx
        .store
        .call_tool(
            "search",
            br#"{"q":"first result","attention":"all"}"#,
            &fx.client,
        )
        .unwrap();
    assert_eq!(
        old["hits"].as_array().unwrap().len(),
        0,
        "superseded model text must leave the default index"
    );
}

#[test]
fn model_intervals_cannot_extend_beyond_the_source_audio() {
    let fx = Fx::make("native-range-acceptance");
    import(&fx, false);
    assert!(submit(&fx, PART, "SYNTHETIC invalid range", 0.0, 100.0).is_err());
    assert!(submit(&fx, PART, "SYNTHETIC empty range", 0.0, 0.0).is_err());
    submit(&fx, PART, "SYNTHETIC valid range", 0.0, 1.0).unwrap();
}

#[test]
fn a_logical_recording_and_user_edits_are_available_to_shared_queries() {
    let fx = Fx::make("native-logical-acceptance");
    import(&fx, true);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE source_key='media:20000000-0000-4000-8000-000000000002'"), 1, "the logical recording needs a stable queryable source");
    fx.store.media_command(&json!({"action":"edit","id":RECORDING,"title":"SYNTHETIC user chosen title","notes":"SYNTHETIC distinct annotation"})).unwrap();
    for q in ["user chosen title", "distinct annotation"] {
        let response = fx
            .store
            .call_tool(
                "search",
                &serde_json::to_vec(&json!({"q":q,"attention":"all"})).unwrap(),
                &fx.client,
            )
            .unwrap();
        assert!(
            !response["hits"].as_array().unwrap().is_empty(),
            "the shared search must find a user edit"
        );
    }
}

#[test]
fn record_queries_audit_success_and_denial() {
    let fx = Fx::make("records-audit-acceptance");
    let mut event = IngestEvent::new("synthetic", "screen.text", Some(1));
    event
        .text
        .push(IngestText::new("body", "SYNTHETIC evidence"));
    fx.store
        .ingest(&IngestBatch::events(&fx.source, vec![event]))
        .unwrap();
    let id = fx
        .store
        .stage_record_revision(&fx.source, records::Rules::default())
        .unwrap();
    let body = serde_json::to_vec(&json!({"id":id})).unwrap();
    fx.store.call_tool("records", &body, &fx.client).unwrap();
    let denied = fx
        .store
        .register_client(
            "synthetic metadata",
            "http",
            &ClientScope {
                text: false,
                ..ClientScope::unbounded()
            },
            None,
        )
        .unwrap();
    assert!(fx
        .store
        .call_tool("records", &body, &denied.client_id)
        .is_err());
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM access_log WHERE tool='records'"),
        2
    );
}

#[test]
fn a_new_transcript_keeps_human_edits_and_replaces_model_text() {
    let fx = Fx::make("native-provenance-acceptance");
    import(&fx, false);
    fx.store.media_command(&json!({"action":"edit","id":PART,"title":"SYNTHETIC human title","notes":"human correction"})).unwrap();
    submit(&fx, PART, "SYNTHETIC new text", 0.0, 1.0).unwrap();
    fx.store.media_command(&json!({"action":"recognize","id":PART,"mode":"quick"})).unwrap();
    submit(&fx, PART, "SYNTHETIC replacement model text", 0.0, 1.0).unwrap();
    let item = fx.store.media_item(PART).unwrap();
    assert_eq!(item["title"], "SYNTHETIC human title");
    assert_eq!(item["notes"], "human correction");
    assert!(fx
        .store
        .call_tool(
            "search",
            br#"{"q":"new text","attention":"all"}"#,
            &fx.client
        )
        .unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!fx
        .store
        .call_tool(
            "search",
            br#"{"q":"human correction","attention":"all"}"#,
            &fx.client
        )
        .unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn grouped_query_timestamps_samples_references_and_default_visibility_follow_the_recording() {
    let fx = Fx::make("native-offset-acceptance");
    import(&fx, true);
    let other = "20000000-0000-4000-8000-000000000003";
    let path = fx.root.join("SYNTHETIC.wav");
    fx.store.media_command(&json!({"action":"import","id":other,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":path,"occurred_at":"2026-11-12T00:00:02Z","recording":{"recording_id":RECORDING,"segment_index":1,"sample_start":16000,"sample_count":8000,"sample_rate":8000,"recording_started_at":"2026-11-12T00:00:00Z"}})).unwrap();
    fx.store.media_command(&json!({"action":"state","id":RECORDING,"state":"stopped"})).unwrap();
    submit(&fx, RECORDING, "SYNTHETIC second part offset", 2., 3.).unwrap();
    let at = chrono::DateTime::parse_from_rfc3339("2026-11-12T00:00:02Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='audio.recording' AND COALESCE(json_extract(payload_json,'$.native_hidden'),0)=0"),1);
    assert_eq!(fx.scalar(&format!("SELECT COUNT(*) FROM events WHERE kind='speech.segment' AND start_ms={at} AND end_ms={} AND json_extract(payload_json,'$.sample_start')=16000 AND json_extract(payload_json,'$.sample_end')=24000 AND COALESCE(json_extract(payload_json,'$.native_hidden'),0)=0",at+1000)),1);
    let answer = fx
        .store
        .call_tool(
            "search",
            br#"{"q":"second part offset","attention":"all"}"#,
            &fx.client,
        )
        .unwrap();
    assert_eq!(answer["hits"].as_array().unwrap().len(), 1);
    let source = fx
        .store
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.display_name == "资料库")
        .unwrap();
    let physical = fx
        .store
        .event_id(&source.source_id, &format!("media:{other}"))
        .unwrap()
        .unwrap();
    assert!(fx
        .store
        .call_tool(
            "get_event",
            &serde_json::to_vec(&json!({"event_id":physical})).unwrap(),
            &fx.client
        )
        .is_ok());
    let root_event = fx
        .store
        .event_id(&source.source_id, &format!("media:{RECORDING}"))
        .unwrap()
        .unwrap();
    let raw = fx
        .store
        .raw_db()
        .first(
            "SELECT payload_json FROM events WHERE event_id=?",
            &[testing::rusqlite::types::Value::Text(root_event)],
            |r| {
                use testing::RowExt;
                r.text(0)
            },
        )
        .unwrap()
        .unwrap();
    let payload: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(payload["recording_parts"][1]["event_id"], physical);
    fx.store
        .set_source_status(&source.source_id, "revoked", None)
        .unwrap();
    assert!(fx
        .store
        .call_tool(
            "search",
            br#"{"q":"second part offset","attention":"all"}"#,
            &fx.client
        )
        .unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    fx.store
        .set_source_status(&source.source_id, "active", None)
        .unwrap();
    fx.store
        .media_command(&json!({"action":"delete","id":RECORDING}))
        .unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM store_meta WHERE key GLOB 'workflows.speakers.result/1:*'"), 0, "the recording's transcript goes with it");
    assert!(fx
        .store
        .call_tool(
            "get_event",
            &serde_json::to_vec(&json!({"event_id":physical})).unwrap(),
            &fx.client
        )
        .is_err());
}

#[test]
fn a_worker_outside_the_core_completes_the_recording_job_under_its_lease() {
    let fx = Fx::make("native-recording-result");
    import(&fx, false);
    // A first answer through the helper sets up the voice service; then a new attempt is answered over the API.
    submit(&fx, PART, "SYNTHETIC first", 0.0, 1.0).unwrap();
    fx.store.media_command(&json!({"action":"recognize","id":PART,"mode":"quick"})).unwrap();
    let job = fx.store.job_command(&json!({"action":"claim","kind":RECORDING_JOB,"owner":"SYNTHETIC-worker"})).unwrap();
    let mut result = json!({"action":"result","format":"candid-recording-result/1","job_id":job["id"],"attempt":job["attempt"],"owner":"someone else",
        "result":{"speaker_count":0,"unresolved_segments":0,"producer":{"model_id":"SYNTHETIC","model_revision":"fixture/1"},
                  "segments":[{"start_seconds":0.0,"end_seconds":0.5,"text":"SYNTHETIC from the worker"}]}});
    assert!(fx.store.media_command(&result).is_err(), "only the lease holder may answer");
    result["owner"] = json!("SYNTHETIC-worker");
    let item = fx.store.media_command(&result).unwrap();
    assert_eq!(item["processing_status"], "ready");
    assert_eq!(item["segments"][0]["text"], "SYNTHETIC from the worker");
}
