mod common;
use common::Fx;
use candid_store::{testing::RowExt, *};
use serde_json::{json, Value};
use std::path::PathBuf;
fn setup() -> (Fx, String) {
    let fx = Fx::make("workflows");
    let id = uuid_v4();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    fx.store.media_command(&json!({"action":"import","id":id,"filename":"PUBLIC.wav","kind":"audio","source":"import","path":path,"occurred_at":"2026-11-12T00:00:00Z"})).unwrap();
    let c = json!({"version":1,"providers":[{"provider_id":"local","name":"SYNTHETIC","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","enabled":true,"destination":"on_device"}],"models":[{"provider_id":"local","model_id":"SYNTHETIC"}],"roles":[{"role":"asr","model":{"provider_id":"local","model_id":"SYNTHETIC"}},{"role":"title","model":{"provider_id":"local","model_id":"SYNTHETIC"}}]});
    fx.store
        .model_command(&json!({"action":"save","config":c}))
        .unwrap();
    (fx, id)
}
/// The recording's one job (quick mode, on the ASR service of `setup`), claimed.
fn speech(fx: &Fx, id: &str) -> Value {
    fx.store
        .media_command(&json!({"action":"recognize","id":id,"mode":"quick"}))
        .unwrap();
    fx.store.job_command(&json!({"action":"claim","kind":RECORDING_JOB,"owner":"SYNTHETIC","lease_ms":180000})).unwrap()
}
/// What the voice service answers for request `r`: one line of text.
fn output(r: &Value) -> Value {
    json!({"format":"recording-speakers/2","mode":"quick","revision":r["input"]["revision"],"segments":[{"start_seconds":0.0,"end_seconds":1.0,"text":"SYNTHETIC exact words","speaker":null,"label":null,"voice_ref":null,"match":null,"correction_key":null}],
        "speakers":[],"speaker_count":0,"unresolved_segments":0,"dropped_segments":0,"voice_refs":[],"producer":{"model_id":"SYNTHETIC","model_revision":"fixture/1"}})
}
#[test]
fn cancelled_deleted_revoked_and_changed_configuration_reject_late_replies() {
    for scenario in ["cancel", "delete", "revoke", "change"] {
        let (fx, id) = setup();
        let job = speech(&fx, &id);
        let r = fx.store.workflow_prepare(&job).unwrap();
        match scenario {
            "cancel" => {
                fx.store
                    .job_command(&json!({"action":"cancel","id":job["id"]}))
                    .unwrap();
            }
            "delete" => {
                fx.store
                    .media_command(&json!({"action":"delete","id":id}))
                    .unwrap();
            }
            "revoke" => {
                let source = fx
                    .store
                    .sources()
                    .unwrap()
                    .into_iter()
                    .find(|s| s.display_name == "资料库")
                    .unwrap();
                let mut policy = source.policy;
                policy.store_text = false;
                fx.store.update_policy(&source.source_id, &policy).unwrap();
            }
            _ => {
                let mut config = fx.store.model_config().unwrap();
                config["providers"][0]["enabled"] = json!(false);
                fx.store
                    .model_command(&json!({"action":"save","config":config}))
                    .unwrap();
            }
        }
        assert!(
            fx.store.workflow_complete(&r, &output(&r)).is_err(),
            "{scenario}"
        );
        assert!(fx
            .store
            .media_command(&json!({"action":"item","id":id}))
            .ok()
            .is_none_or(|i| i["segments"].as_array().is_none_or(|s| s.is_empty())));
    }
}
#[test]
fn budget_and_unknown_cost_are_not_treated_as_zero_and_secrets_are_rejected() {
    let (fx, _) = setup();
    let mut c = fx.store.model_config().unwrap();
    c["roles"][0]["limits_json"] = json!({"daily_budget":1.0});
    fx.store
        .model_command(&json!({"action":"save","config":c}))
        .unwrap();
    let request = json!({"model":{"provider_id":"local","model_id":"SYNTHETIC"},"messages":[{"role":"user","content":"SYNTHETIC"}]});
    assert!(fx.store.model_prepare(&request).is_err());
    c["models"][0]["cost_json"] = json!({"currency":"USD","per_mtok":{"input":1.0,"output":1.0}});
    fx.store
        .model_command(&json!({"action":"save","config":c}))
        .unwrap();
    assert!(fx.store.model_prepare(&request).is_ok());
    c["roles"][0]["limits_json"]["daily_budget"] = json!(0.0);
    fx.store
        .model_command(&json!({"action":"save","config":c}))
        .unwrap();
    assert!(fx.store.model_prepare(&request).is_err());
    c["providers"][0]["headers_json"] = json!({"Authorization":"SYNTHETIC forbidden"});
    assert!(fx
        .store
        .model_command(&json!({"action":"save","config":c}))
        .is_err());
}
#[test]
fn draft_invalidates_on_retranscription_and_unknown_configuration_version_fails() {
    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    let queued = fx
        .store
        .workflow_command(&json!({"action":"enqueue","id":id,"step":"title"}))
        .unwrap();
    let job = fx
        .store
        .job_command(&json!({"action":"claim","kind":"workflow.title","owner":"SYNTHETIC"}))
        .unwrap();
    assert_eq!(queued["id"], job["id"]);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store
        .workflow_complete(&r, &json!("SYNTHETIC generated title"))
        .unwrap();
    assert_eq!(
        fx.store.media_item(&id).unwrap()["title"],
        "SYNTHETIC generated title"
    );
    assert_eq!(fx.texts(&format!("SELECT title FROM events WHERE source_key='media:{id}'")), vec!["SYNTHETIC generated title"]);
    fx.store
        .media_command(&json!({"action":"edit","id":id,"title":"","notes":""}))
        .unwrap();
    assert_eq!(
        fx.store.media_item(&id).unwrap()["title"],
        "",
        "an intentionally cleared manual title stays empty"
    );
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    let mut out = output(&r);
    out["segments"][0]["text"] = json!("SYNTHETIC revised words");
    fx.store.workflow_complete(&r, &out).unwrap();
    assert!(fx
        .store
        .workflow_command(&json!({"action":"drafts","id":id}))
        .unwrap()
        .as_object()
        .unwrap()
        .is_empty());
    assert!(fx
        .store
        .model_command(&json!({"action":"save","config":{"version":99}}))
        .is_err());
}

#[test]
fn reprojecting_after_an_unrelated_source_status_change_does_not_rewrite_events() {
    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let request = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&request, &output(&request)).unwrap();
    fx.store.prepare_media(None).unwrap();
    let db = fx.store.raw_db();
    db.script("CREATE TEMP TABLE synthetic_updates(n INTEGER); INSERT INTO synthetic_updates VALUES(0); CREATE TEMP TRIGGER synthetic_event_updates AFTER UPDATE ON main.events BEGIN UPDATE synthetic_updates SET n=n+1; END;").unwrap();
    fx.store.set_source_status(&fx.source, "error", Some("SYNTHETIC offline")).unwrap();
    fx.store.prepare_media(None).unwrap();
    fx.store.set_source_status(&fx.source, "active", None).unwrap();
    fx.store.prepare_media(None).unwrap();
    assert_eq!(fx.scalar("SELECT n FROM synthetic_updates"), 0);
    assert!(fx.store.media_item(&id).is_ok());
}
#[test]
fn draft_and_job_commit_roll_back_together_and_replay_once_after_reopen() {
    let (mut fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    fx.store
        .workflow_command(&json!({"action":"enqueue","id":id,"step":"title"}))
        .unwrap();
    let job = fx
        .store
        .job_command(&json!({"action":"claim","kind":"workflow.title","owner":"SYNTHETIC"}))
        .unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.raw_db().script("CREATE TRIGGER synthetic_fail_draft BEFORE INSERT ON episodes BEGIN SELECT RAISE(ABORT,'SYNTHETIC write failure'); END;").unwrap();
    assert!(fx
        .store
        .workflow_complete(&r, &json!("SYNTHETIC atomic title"))
        .is_err());
    assert_eq!(
        fx.store
            .job_command(&json!({"action":"get","id":job["id"]}))
            .unwrap()["state"],
        "running"
    );
    fx.store
        .raw_db()
        .script("DROP TRIGGER synthetic_fail_draft")
        .unwrap();
    fx.store.close();
    fx.store = common::open(&fx.root);
    fx.store
        .workflow_complete(&r, &json!("SYNTHETIC atomic title"))
        .unwrap();
    let first = fx
        .store
        .workflow_complete(&r, &json!("SYNTHETIC atomic title"))
        .unwrap();
    assert_eq!(first["title"]["text"], "SYNTHETIC atomic title");
    assert_eq!(
        fx.store
            .job_command(&json!({"action":"get","id":job["id"]}))
            .unwrap()["state"],
        "done"
    );
}
#[test]
fn small_features_use_the_same_queue_source_validation_and_atomic_drafts() {
    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    let mut c = fx.store.model_config().unwrap();
    for role in ["summary", "vision"] {
        c["roles"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":role,"model":{"provider_id":"local","model_id":"SYNTHETIC"}}));
    }
    fx.store
        .model_command(&json!({"action":"save","config":c}))
        .unwrap();
    use base64::Engine;
    let png=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=").unwrap();
    let path = fx.root.join("SYNTHETIC.png");
    std::fs::write(&path, png).unwrap();
    let image = uuid_v4();
    fx.store.media_command(&json!({"action":"import","id":image,"filename":"SYNTHETIC.png","kind":"image","source":"import","path":path})).unwrap();
    for (step, input) in [
        ("decisions", &id),
        ("tags", &id),
        ("summary", &id),
        ("screenshot", &image),
    ] {
        fx.store
            .workflow_command(&json!({"action":"enqueue","id":input,"step":step}))
            .unwrap();
        let job = fx
            .store
            .job_command(
                &json!({"action":"claim","kind":format!("workflow.{step}"),"owner":"SYNTHETIC"}),
            )
            .unwrap();
        let r = fx.store.workflow_prepare(&job).unwrap();
        if step == "screenshot" {
            assert!(r["messages"].is_array());
        }
        fx.store
            .workflow_complete(&r, &json!(format!("SYNTHETIC {step} result")))
            .unwrap();
        let drafts = fx
            .store
            .workflow_command(&json!({"action":"drafts","id":input}))
            .unwrap();
        assert!(drafts[step]["text"].as_str().unwrap().contains(step));
    }
    let daily = fx
        .store
        .workflow_command(&json!({"action":"daily","day":"2026-11-12"}))
        .unwrap();
    let job = fx
        .store
        .job_command(&json!({"action":"claim","kind":"workflow.daily","owner":"SYNTHETIC"}))
        .unwrap();
    assert_eq!(job["id"], daily["id"]);
    let r = fx.store.workflow_prepare(&job).unwrap();
    assert!(r["content"].as_str().unwrap().contains("录音1"));
    fx.store
        .workflow_complete(&r, &json!("SYNTHETIC daily result"))
        .unwrap();
}
#[test]
fn legacy_usage_import_preserves_counts_and_is_idempotent() {
    let (fx, _) = setup();
    let call = json!({"call_id":"SYNTHETIC-call","started_ms":1,"ended_ms":9,"feature":"vision","client":"app","run_id":"SYNTHETIC-run","provider_id":"local","model_id":"SYNTHETIC","destination":"on_device","input_tokens":100,"output_tokens":10,"cache_read_tokens":20,"cache_write_tokens":30,"cost":0.25,"currency":"USD","cost_source":"estimated","latency_ms":8,"status":"ok","request_bytes":200,"response_bytes":40});
    for _ in 0..2 {
        fx.store
            .model_command(&json!({"action":"import_usage","calls":[call]}))
            .unwrap();
    }
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM ai_calls"), 1);
    assert_eq!(
        fx.scalar("SELECT cache_read_tokens+cache_write_tokens FROM ai_calls"),
        50
    );
    assert_eq!(fx.texts("SELECT run_id FROM ai_calls")[0], "SYNTHETIC-run");
}
#[test]
fn reply_validation_does_not_reapply_budget_to_its_own_pending_charge() {
    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    let mut c = fx.store.model_config().unwrap();
    c["models"][0]["cost_json"] = json!({"currency":"USD","per_mtok":{"input":1.0,"output":1.0}});
    c["roles"][1]["limits_json"] = json!({"daily_budget":1.0});
    fx.store
        .model_command(&json!({"action":"save","config":c}))
        .unwrap();
    fx.store
        .workflow_command(&json!({"action":"enqueue","id":id,"step":"title"}))
        .unwrap();
    let job = fx
        .store
        .job_command(&json!({"action":"claim","kind":"workflow.title","owner":"SYNTHETIC"}))
        .unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    let call = fx.store.workflow_call_start(&r).unwrap();
    assert!(fx.store.workflow_reply_allowed(&r).is_ok());
    fx.store
        .workflow_call_finish(
            &call,
            "ok",
            None,
            &json!({"prompt_tokens":100,"completion_tokens":10}),
        )
        .unwrap();
    fx.store
        .media_command(&json!({"action":"result","format":"candid-workflow-result/1","request":r,"output":"SYNTHETIC budgeted title"}))
        .unwrap();
    let usage = fx.store.model_command(&json!({"action":"usage"})).unwrap();
    assert_eq!(usage["calls"][0]["cost_source"], "estimated");
    assert_eq!(usage["calls"][0]["currency"], "USD");
}
#[test]
fn retranscription_transport_retry_does_not_start_another_paid_attempt() {
    let (fx, id) = setup();
    let request =
        json!({"action":"recognize","id":id,"mode":"quick","request_id":"SYNTHETIC-one-click"});
    fx.store.media_command(&request).unwrap();
    let job = fx
        .store
        .job_command(&json!({"action":"claim","kind":RECORDING_JOB,"owner":"SYNTHETIC"}))
        .unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    fx.store.media_command(&request).unwrap();
    assert_eq!(
        fx.store
            .job_command(&json!({"action":"get","id":job["id"]}))
            .unwrap()["state"],
        "done"
    );
    assert!(fx
        .store
        .job_command(&json!({"action":"claim","kind":RECORDING_JOB,"owner":"SYNTHETIC"}))
        .unwrap()
        .is_null());
}

fn run_step(fx: &Fx, id: &str, step: &str, text: &str) {
    fx.store.workflow_command(&json!({"action":"enqueue","id":id,"step":step})).unwrap();
    let job = fx.store.job_command(&json!({"action":"claim","kind":format!("workflow.{step}"),"owner":"SYNTHETIC"})).unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &json!(text)).unwrap();
}

/// timeline-v1 §4.8.1: title and summary are one episode, decisions one memory, tags stay a key-value draft;
/// each rerun is a new revision, and deleting the recording removes them.
#[test]
fn workflow_outputs_land_in_the_derived_layer_and_leave_with_the_recording() {
    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    let mut c = fx.store.model_config().unwrap();
    c["roles"].as_array_mut().unwrap().push(json!({"role":"summary","model":{"provider_id":"local","model_id":"SYNTHETIC"}}));
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    run_step(&fx, &id, "title", "SYNTHETIC first title");
    run_step(&fx, &id, "summary", "SYNTHETIC summary");
    run_step(&fx, &id, "decisions", "SYNTHETIC decision");
    run_step(&fx, &id, "tags", "SYNTHETIC tags");
    let db = fx.store.raw_db();
    let one = |sql: &str| db.first(sql, &[], |r| r.text(0)).unwrap().unwrap_or_default();
    let episode = format!("rec:{id}");
    assert_eq!(one(&format!("SELECT title||'|'||summary||'|'||status FROM episodes WHERE episode_id='{episode}' ORDER BY rev DESC LIMIT 1")), "SYNTHETIC first title|SYNTHETIC summary|draft");
    assert_eq!(one(&format!("SELECT kind||'|'||status FROM memories WHERE memory_id='decisions:{id}'")), "decision|candidate");
    assert_eq!(db.scalar("SELECT COUNT(*) FROM memory_sources WHERE ref_type='event'", &[]).unwrap(), 1);
    let meta = one(&format!("SELECT value FROM store_meta WHERE key='workflows.drafts/1:{id}'"));
    assert!(meta.contains("tags") && !meta.contains("first title"), "only steps without a table stay in store_meta");
    let mut c = fx.store.model_config().unwrap();
    c["roles"].as_array_mut().unwrap().push(json!({"role":"vision","model":{"provider_id":"local","model_id":"SYNTHETIC"}}));
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    run_step(&fx, &id, "title", "SYNTHETIC second title");
    assert_eq!(db.scalar(&format!("SELECT COUNT(*) FROM episodes WHERE episode_id='{episode}' AND status='draft'"), &[]).unwrap(), 1);
    let drafts = fx.store.workflow_command(&json!({"action":"drafts","id":id})).unwrap();
    for (step, text) in [("title", "SYNTHETIC second title"), ("summary", "SYNTHETIC summary"), ("decisions", "SYNTHETIC decision"), ("tags", "SYNTHETIC tags")] {
        assert_eq!(drafts[step]["text"], text);
        assert_eq!(drafts[step]["format"], "draft/1");
    }
    assert_eq!(fx.store.media_item(&id).unwrap()["title"], "SYNTHETIC second title");
    fx.store.media_command(&json!({"action":"delete","id":id})).unwrap();
    assert_eq!(db.scalar(&format!("SELECT COUNT(*) FROM episodes WHERE episode_id='{episode}'"), &[]).unwrap(), 0);
    assert_eq!(db.scalar("SELECT COUNT(*) FROM memories", &[]).unwrap(), 0);
}

/// Drafts written before v5 move into the tables once, on open; a rerun of the migration does nothing.
#[test]
fn legacy_drafts_move_into_the_derived_tables_once() {
    let (mut fx, id) = setup();
    // A transcript as versions before the recording job stored it: on the file itself.
    fx.store.raw_db().execute("UPDATE media_objects SET metadata=json_set(metadata,'$.processing_status','ready','$.segments',json(?)) WHERE id=?",
        &[testing::rusqlite::types::Value::Text(json!([{"start_seconds":0.0,"end_seconds":1.0,"text":"SYNTHETIC exact words"}]).to_string()), testing::rusqlite::types::Value::Text(id.clone())]).unwrap();
    let revision = fx.store.workflow_command(&json!({"action":"drafts","id":id})).unwrap();
    assert!(revision.as_object().unwrap().is_empty());
    let segments = fx.store.media_item(&id).unwrap()["segments"].clone();
    let rev = sha256_hex(&serde_json::to_vec(&segments).unwrap());
    let legacy = json!({"title":{"format":"draft/1","text":"SYNTHETIC legacy title","source_revision":rev,"producer":"SYNTHETIC","prompt_version":1,"job_id":"j1"},
                        "tags":{"format":"draft/1","text":"SYNTHETIC legacy tags","source_revision":rev,"producer":"SYNTHETIC","prompt_version":1,"job_id":"j2"}});
    let db = fx.store.raw_db();
    db.execute("DELETE FROM store_meta WHERE key='workflows.drafts.migrated/1'", &[]).unwrap();
    db.execute("INSERT INTO store_meta VALUES(?,?)", &[testing::rusqlite::types::Value::Text(format!("workflows.drafts/1:{id}")), testing::rusqlite::types::Value::Text(legacy.to_string())]).unwrap();
    fx.store.close();
    fx.store = common::open(&fx.root);
    let db = fx.store.raw_db();
    assert_eq!(db.scalar(&format!("SELECT COUNT(*) FROM episodes WHERE episode_id='rec:{id}' AND title='SYNTHETIC legacy title'"), &[]).unwrap(), 1);
    let drafts = fx.store.workflow_command(&json!({"action":"drafts","id":id})).unwrap();
    assert_eq!(drafts["title"]["text"], "SYNTHETIC legacy title");
    assert_eq!(drafts["title"]["job_id"], "j1");
    assert_eq!(drafts["tags"]["text"], "SYNTHETIC legacy tags");
    assert_eq!(fx.store.migrate_workflow_drafts().unwrap(), 0);
    assert_eq!(db.scalar(&format!("SELECT COUNT(*) FROM episodes WHERE episode_id='rec:{id}'"), &[]).unwrap(), 1);
}

/// Recordings only showed a date: titles waited for "automatic" (off by default) and a title-role model.
/// Titles are now on by default, and without a title model the step borrows another text role's model.
#[test]
fn transcribed_recordings_get_titles_by_default_with_a_borrowed_model() {
    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    let mut c = fx.store.model_config().unwrap();
    for a in c["roles"].as_array_mut().unwrap().iter_mut().filter(|a| a["role"] == "title") {
        a["role"] = json!("vision");
        a["prompt"] = json!("SYNTHETIC screenshot prompt");
    }
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    let titles = |fx: &Fx| fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().iter().filter(|j| j["kind"] == "workflow.title" && j["state"] == "queued").count();
    fx.store.workflow_scan().unwrap();
    assert_eq!(titles(&fx), 1, "no saved settings still names recordings");
    assert_eq!(fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().iter().filter(|j| j["kind"] == "workflow.summary").count(), 0);
    let job = fx.store.job_command(&json!({"action":"claim","kind":"workflow.title","owner":"SYNTHETIC"})).unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    assert_eq!(r["assignment"]["role"], "vision");
    assert_ne!(r["prompt"], "SYNTHETIC screenshot prompt", "a borrowed model uses the step's own prompt");
    fx.store.workflow_complete(&r, &json!("SYNTHETIC named")).unwrap();
    fx.store.workflow_scan().unwrap();
    assert_eq!(titles(&fx), 0, "a current title is not asked for again");

    let (fx, id) = setup();
    let job = speech(&fx, &id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &output(&r)).unwrap();
    fx.store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false}})).unwrap();
    fx.store.workflow_scan().unwrap();
    assert_eq!(titles(&fx), 0, "turned off stays off");
}

/// A self-hosted model server on this Mac needs no per-source approval; one on another host still does.
#[test]
fn loopback_model_servers_need_no_source_approval() {
    for (url, approved) in [("http://localhost:1234/v1", true), ("http://192.168.1.20:1234/v1", false)] {
        let (fx, id) = setup();
        let job = speech(&fx, &id);
        let r = fx.store.workflow_prepare(&job).unwrap();
        fx.store.workflow_complete(&r, &output(&r)).unwrap();
        let mut c = fx.store.model_config().unwrap();
        c["providers"].as_array_mut().unwrap().push(json!({"provider_id":"lan","name":"SYNTHETIC","api":"openai-chat","base_url":url,"enabled":true,"destination":"self_hosted"}));
        for a in c["roles"].as_array_mut().unwrap().iter_mut().filter(|a| a["role"] == "title") {
            a["model"]["provider_id"] = json!("lan");
        }
        fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
        fx.store.workflow_command(&json!({"action":"enqueue","id":id,"step":"title"})).unwrap();
        let job = fx.store.job_command(&json!({"action":"claim","kind":"workflow.title","owner":"SYNTHETIC"})).unwrap();
        let prepared = fx.store.workflow_prepare(&job);
        assert_eq!(prepared.is_ok(), approved, "{url}: {prepared:?}");
    }
}

#[test]
fn feature_controls_persist_reject_unknown_versions_and_stop_queueing() {
    let (fx, _) = setup();
    let mut config = fx.store.feature_settings().unwrap();
    assert_eq!(config["items"]["transcribe"]["enabled"], true);
    config["items"]["transcribe"]["enabled"] = json!(false);
    config["items"]["title"]["enabled"] = json!(false);
    fx.store
        .workflow_command(&json!({"action":"features","config":config}))
        .unwrap();
    fx.store.workflow_scan().unwrap();
    assert!(
        fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fx.store
            .workflow_command(&json!({"action":"status"}))
            .unwrap()["features"],
        config
    );
    for invalid in [
        json!({"version":2,"items":{}}),
        json!({"version":1,"items":{"unknown":{}}}),
        json!({"version":1,"items":{"transcribe":{"enabled":true}}}),
    ] {
        assert!(fx
            .store
            .workflow_command(&json!({"action":"features","config":invalid}))
            .is_err());
        assert_eq!(fx.store.feature_settings().unwrap(), config);
    }
    config["items"]["transcribe"]["enabled"] = json!(true);
    fx.store
        .workflow_command(&json!({"action":"features","config":config}))
        .unwrap();
    fx.store.workflow_scan().unwrap();
    assert!(
        !fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn feature_sources_remote_budget_and_late_replies_obey_saved_controls() {
    for scenario in ["sources", "remote", "budget", "disable", "late-sources"] {
        let (fx, id) = setup();
        let job = speech(&fx, &id);
        let r = fx.store.workflow_prepare(&job).unwrap();
        let mut config = fx.store.feature_settings().unwrap();
        match scenario {
            "sources" | "late-sources" => config["items"]["transcribe"]["sources"] = json!([]),
            "remote" => {
                config["items"]["transcribe"]["allow_remote"] = json!(false);
                let mut models = fx.store.model_config().unwrap();
                models["providers"][0]["base_url"] = json!("https://model.example/v1");
                models["providers"][0]["destination"] = json!("self_hosted");
                fx.store
                    .model_command(&json!({"action":"save","config":models}))
                    .unwrap();
            }
            "budget" => config["items"]["transcribe"]["daily_calls"] = json!(0),
            _ => config["items"]["transcribe"]["enabled"] = json!(false),
        }
        fx.store
            .workflow_command(&json!({"action":"features","config":config}))
            .unwrap();
        let reason = match scenario {
            "sources" | "late-sources" => "waiting_feature_source",
            "remote" => "waiting_feature_remote",
            "budget" => "waiting_feature_budget",
            _ => "feature_disabled",
        };
        let error = fx.store.workflow_prepare(&job).unwrap_err();
        assert!(error.message().contains(reason), "{scenario}: {}", error.message());
        assert!(fx.store.workflow_reply_allowed(&r).is_err(), "{scenario}");
    }
}

#[test]
fn feature_reenable_resumes_only_jobs_cancelled_by_that_switch() {
    let (fx,id) = setup();
    let job = speech(&fx,&id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    let mut config = fx.store.feature_settings().unwrap();
    config["items"]["transcribe"]["enabled"] = json!(false);
    fx.store.save_feature_settings(&config).unwrap();
    assert_eq!(fx.store.job_command(&json!({"action":"get","id":job["id"]})).unwrap()["state"], "cancelled");
    assert!(fx.store.workflow_complete(&r,&output(&r)).is_err());
    config["items"]["transcribe"]["enabled"] = json!(true);
    fx.store.save_feature_settings(&config).unwrap();
    assert_eq!(fx.store.job_command(&json!({"action":"get","id":job["id"]})).unwrap()["state"], "queued");
    assert!(fx.store.workflow_complete(&r,&output(&r)).is_err());
    fx.store.job_command(&json!({"action":"cancel","id":job["id"]})).unwrap();
    fx.store.save_feature_settings(&config).unwrap();
    assert_eq!(fx.store.job_command(&json!({"action":"get","id":job["id"]})).unwrap()["state"], "cancelled");
}

#[test]
fn automatic_summary_works_when_automatic_transcription_is_off() {
    let (fx,id) = setup();
    let job = speech(&fx,&id);
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r,&output(&r)).unwrap();
    fx.store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":true}})).unwrap();
    fx.store.workflow_scan().unwrap();
    let jobs = fx.store.job_command(&json!({"action":"list"})).unwrap();
    assert!(jobs["jobs"].as_array().unwrap().iter().any(|j|j["kind"] == "workflow.summary" && j["state"] == "queued"));
}

/// Jobs queued by earlier versions (one per 30-second file, plus a whole-recording speaker pass) are cancelled
/// once; the stopped recording then gets its one job instead.
#[test]
fn queued_jobs_of_earlier_versions_are_retired_for_one_recording_job() {
    let (fx, id) = setup();
    let old = [("speech.recognize", "external-api/1:conversation"), ("workflow.speakers", "SYNTHETIC-old")];
    for (kind, config) in old {
        fx.store.job_command(&json!({"action":"enqueue","kind":kind,"input_ref":id,"config_version":config})).unwrap();
    }
    fx.store.workflow_scan().unwrap();
    let jobs = fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().clone();
    for (kind, _) in old {
        let job = jobs.iter().find(|j| j["kind"] == kind).unwrap();
        assert_eq!((job["state"].as_str(), job["error_code"].as_str()), (Some("cancelled"), Some("superseded")), "{kind}");
    }
    let new: Vec<_> = jobs.iter().filter(|j| j["kind"] == RECORDING_JOB).collect();
    assert_eq!(new.len(), 1, "{jobs:?}");
    assert_eq!((new[0]["state"].as_str(), new[0]["input_ref"].as_str()), (Some("queued"), Some(id.as_str())));
    // A second scan with nothing changed queues nothing more.
    fx.store.workflow_scan().unwrap();
    assert_eq!(fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().len(), jobs.len());
}
