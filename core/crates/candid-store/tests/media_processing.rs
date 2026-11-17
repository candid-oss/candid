mod common;
use common::Fx;
use candid_store::*;
use serde_json::json;
#[test]
fn recording_progress_shows_waiting_reason_and_cancel_rejects_late_results() {
    let fx = Fx::make("media-task-feedback");
    let id = uuid_v4();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/asr/sample-zh.wav");
    fx.store.media_command(&json!({"action":"import","id":id,"filename":"PUBLIC.wav","kind":"audio","source":"import","path":path})).unwrap();
    fx.store.model_command(&json!({"action":"save","config":{"version":1,"providers":[{"provider_id":"test","name":"SYNTHETIC","api":"other","base_url":"http://127.0.0.1:1/v1","enabled":true,"destination":"on_device"}],"models":[{"provider_id":"test","model_id":"SYNTHETIC"}],"roles":[{"role":"asr","model":{"provider_id":"test","model_id":"SYNTHETIC"}}]}})).unwrap();
    let queued = fx
        .store
        .media_command(&json!({"action":"recognize","id":id}))
        .unwrap();
    assert_eq!(queued["processing"]["state"], "queued");
    let job = fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["input_ref"] == id)
        .unwrap()
        .clone();
    fx.store.raw_db().execute(&format!("UPDATE jobs SET created_ms=created_ms-600000 WHERE job_id='{}'",job["id"].as_str().unwrap()),&[]).unwrap();
    fx.store
        .workflow_wait(job["id"].as_str().unwrap(), "waiting_feature_source")
        .unwrap();
    assert_eq!(
        fx.store.media_item(&id).unwrap()["processing"]["reason"],
        "waiting_feature_source"
    );
    let job = fx
        .store
        .job_command(
            &json!({"action":"claim","id":job["id"],"owner":"SYNTHETIC","lease_ms":180000}),
        )
        .unwrap();
    assert_eq!(fx.store.media_item(&id).unwrap()["processing"]["elapsed_seconds"],0,"a fresh attempt must not include ten minutes of prior history");
    let request = fx.store.workflow_prepare(&job).unwrap();
    fx.store.media_job_progress(&job,&json!({"stage":"transcribing","completed_seconds":1.0,"total_seconds":4.0,"updated_ms":42,"cancellation_supported":true})).unwrap();
    assert_eq!(
        fx.store.media_item(&id).unwrap()["processing"]["stage"],
        "transcribing"
    );
    let cancelled = fx
        .store
        .media_command(&json!({"action":"cancel_processing","id":id}))
        .unwrap();
    assert_eq!(cancelled["processing"]["state"], "cancelling");
    fx.store.media_job_stopped(&job, true).unwrap();
    assert_eq!(
        fx.store
            .media_command(&json!({"action":"cancel_processing","id":id}))
            .unwrap()["processing"]["state"],
        "cancelled"
    );
    assert!(fx.store.workflow_complete(&request,&json!({"producer":{"model_id":"SYNTHETIC","revision":"1"},"segments":[{"start_seconds":0,"end_seconds":1,"text":"SYNTHETIC late reply"}]})).is_err());
    assert!(fx.store.media_item(&id).unwrap()["segments"]
        .as_array()
        .unwrap()
        .is_empty());
}
