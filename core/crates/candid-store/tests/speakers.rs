//! Speaker labels in a recording (agent-platform-v1 §2.6): automatic, 「可能是」, your choice, anonymous.
mod common;
use common::Fx;
use candid_store::*;
use serde_json::{json, Value};
use std::path::PathBuf;

fn recording() -> (Fx, String) {
    let fx = Fx::make("speakers");
    let id = uuid_v4();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    fx.store.media_command(&json!({"action":"import","id":id,"filename":"PUBLIC.wav","kind":"audio","source":"import","path":path,"occurred_at":"2026-11-12T00:00:00Z"})).unwrap();
    let c = json!({"version":1,"providers":[{"provider_id":"local","name":"SYNTHETIC","api":"openai-chat","base_url":"http://127.0.0.1:1/v1","enabled":true,"destination":"on_device"}],"models":[{"provider_id":"local","model_id":"SYNTHETIC"}],"roles":[{"role":"asr","model":{"provider_id":"local","model_id":"SYNTHETIC"}}]});
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    fx.store.media_command(&json!({"action":"recognize","id":id})).unwrap();
    let job = fx.store.job_command(&json!({"action":"claim","kind":"speech.recognize","owner":"SYNTHETIC","lease_ms":180000})).unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    let line = |a: f64, who: &str, m: Value| json!({"start_seconds":a,"end_seconds":a+0.4,"text":format!("SYNTHETIC {who}"),"speaker":who,"match":m});
    let out = json!({"producer":{"model_id":"SYNTHETIC","revision":"fixture/1"},"segments":[
        line(0.0, "说话人 1", json!({"status":"matched","person_id":"v-zhang","name":"张伟","score":0.86,"margin":0.3})),
        line(0.5, "说话人 2", json!({"status":"unmatched","person_id":"v-li","name":"李娜","score":0.68,"margin":0.03})),
        line(1.0, "说话人 3", json!({"status":"unmatched","person_id":"v-wang","name":"王芳","score":0.41,"margin":0.2})),
        line(1.5, "说话人 1", json!({"status":"matched","person_id":"v-zhang","name":"张伟","score":0.86,"margin":0.3})),
        json!({"start_seconds":2.0,"end_seconds":2.4,"text":"SYNTHETIC four","speaker":"说话人 4","voice_ref":"ref-four"}),
    ]});
    fx.store.workflow_complete(&r, &out).unwrap();
    (fx, id)
}

fn speaker<'a>(item: &'a Value, key: &str) -> &'a Value {
    item["speakers"].as_array().unwrap().iter().find(|s| s["key"] == key).unwrap()
}

#[test]
fn speakers_show_status_and_confidence_and_take_your_labels() {
    let (fx, id) = recording();
    let item = fx.store.media_item(&id).unwrap();
    assert_eq!(item["speakers"].as_array().unwrap().len(), 4);
    let one = speaker(&item, "说话人 1");
    assert_eq!((one["status"].as_str(), one["name"].as_str(), one["segments"].as_i64()), (Some("auto"), Some("张伟"), Some(2)));
    assert_eq!(one["score"], 0.86);
    let two = speaker(&item, "说话人 2");
    assert_eq!((two["status"].as_str(), two["name"].as_str()), (Some("maybe"), Some("李娜")), "close but not clear: asked, not shown");
    assert_eq!(speaker(&item, "说话人 3")["status"], "anonymous");
    assert_eq!(speaker(&item, "说话人 4")["status"], "anonymous");
    let names: Vec<_> = item["segments"].as_array().unwrap().iter().map(|s| s["speaker_name"].clone()).collect();
    assert_eq!(names, vec![json!("张伟"), Value::Null, Value::Null, json!("张伟"), Value::Null]);

    // 「是她」 on the maybe, a new name for an anonymous speaker, and keep the last one anonymous.
    fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 2","voice_id":"v-li","name":"李娜","samples_saved":true})).unwrap();
    fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 3","name":"王老师"})).unwrap();
    let item = fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 4","anonymous":true})).unwrap();
    assert_eq!((speaker(&item, "说话人 2")["status"].as_str(), speaker(&item, "说话人 2")["samples_saved"].as_bool()), (Some("confirmed"), Some(true)));
    assert_eq!(speaker(&item, "说话人 3")["name"], "王老师");
    assert_eq!(speaker(&item, "说话人 4")["dismissed"], true);
    let people = fx.store.media_command(&json!({"action":"speaker_people"})).unwrap();
    let listed: Vec<_> = people["people"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    assert!(listed.contains(&"王老师".into()) && listed.contains(&"李娜".into()));
    // An existing person by id; a label nobody said is refused.
    let wang = people["people"].as_array().unwrap().iter().find(|p| p["name"] == "王老师").unwrap()["person_id"].clone();
    fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 4","person_id":wang})).unwrap();
    assert!(fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 9","name":"X"})).is_err());

    // Library changes reach every recording: rename an automatic match, delete a confirmed person.
    fx.store.voice_library_changed("rename", "v-zhang", &json!({"name":"张伟（财务）"})).unwrap();
    fx.store.voice_library_changed("delete", "v-li", &json!({})).unwrap();
    let item = fx.store.media_item(&id).unwrap();
    assert_eq!(speaker(&item, "说话人 1")["name"], "张伟（财务）");
    assert_eq!(speaker(&item, "说话人 2")["status"], "anonymous", "a deleted person's speakers go back to anonymous");
    assert_eq!(speaker(&item, "说话人 4")["name"], "王老师");
}

#[test]
fn merging_in_the_library_moves_labels_to_the_surviving_person() {
    let (fx, id) = recording();
    fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 3","voice_id":"v-wang","name":"王芳"})).unwrap();
    fx.store.voice_library_changed("merge", "v-wang", &json!({"target_id":"v-zhang","target_name":"张伟"})).unwrap();
    let item = fx.store.media_item(&id).unwrap();
    assert_eq!(speaker(&item, "说话人 3")["name"], "张伟");
    assert_eq!(speaker(&item, "说话人 3")["status"], "confirmed");
}

/// After the library changes, a re-match of the stored speaker reference wins over the match made at transcription;
/// deleting the recording queues its references for removal from the voice service.
#[test]
fn rematch_results_name_speakers_and_deleted_recordings_queue_their_references() {
    let (fx, id) = recording();
    let before=fx.store.media_item(&id).unwrap()["content_revision"].clone();
    assert_eq!(fx.store.voice_refs().unwrap(), vec!["ref-four".to_string()]);
    let mut matches = serde_json::Map::new();
    matches.insert("ref-four".into(), json!({"status":"matched","person_id":"v-new","name":"新同事","score":0.81,"margin":0.2}));
    fx.store.voice_matches_save(&matches).unwrap();
    let item = fx.store.media_item(&id).unwrap();
    assert_ne!(item["content_revision"],before,"library changes must invalidate the detail cache");
    assert_eq!((speaker(&item, "说话人 4")["status"].as_str(), speaker(&item, "说话人 4")["name"].as_str()), (Some("auto"), Some("新同事")));
    matches.insert("ref-four".into(), Value::Null);
    fx.store.voice_matches_save(&matches).unwrap();
    assert_eq!(speaker(&fx.store.media_item(&id).unwrap(), "说话人 4")["status"], "anonymous");
    fx.store.media_command(&json!({"action":"delete","id":id})).unwrap();
    let queued = fx.store.voice_refs_to_forget(None).unwrap();
    assert_eq!(queued, vec!["ref-four".to_string()]);
    assert!(fx.store.voice_refs_to_forget(Some(&queued)).unwrap().is_empty());
}

/// Regression: a recording with speakers got a title the library never showed, because the title's revision
/// was taken over segments that carried the display names; labelling a speaker must not hide it either.
#[test]
fn titles_of_recordings_with_speakers_show_and_survive_labelling() {
    let (fx, id) = recording();
    let mut c = fx.store.model_config().unwrap();
    c["roles"].as_array_mut().unwrap().push(json!({"role":"title","model":{"provider_id":"local","model_id":"SYNTHETIC"}}));
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    fx.store.workflow_command(&json!({"action":"enqueue","id":id,"step":"title"})).unwrap();
    let job = fx.store.job_command(&json!({"action":"claim","kind":"workflow.title","owner":"SYNTHETIC"})).unwrap();
    let r = fx.store.workflow_prepare(&job).unwrap();
    fx.store.workflow_complete(&r, &json!("SYNTHETIC 预算周会")).unwrap();
    assert_eq!(fx.store.media_item(&id).unwrap()["title"], "SYNTHETIC 预算周会");
    let listed = fx.store.media_command(&json!({"action":"list"})).unwrap();
    assert_eq!(listed["items"][0]["title"], "SYNTHETIC 预算周会");
    fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 3","name":"王老师"})).unwrap();
    assert_eq!(fx.store.media_item(&id).unwrap()["title"], "SYNTHETIC 预算周会");
    fx.store.workflow_scan().unwrap();
    let queued = fx.store.job_command(&json!({"action":"list"})).unwrap()["jobs"].as_array().unwrap().iter().filter(|j| j["kind"] == "workflow.title" && j["state"] == "queued").count();
    assert_eq!(queued, 0, "a label does not ask for a new title");
}

fn reconciliation(fx:&Fx,id:&str)->(Value,Value) {
    let mut c=fx.store.model_config().unwrap();
    c["roles"].as_array_mut().unwrap().push(json!({"role":"diarization","model":{"provider_id":"local","model_id":"SYNTHETIC"}}));
    fx.store.model_command(&json!({"action":"save","config":c})).unwrap();
    let enqueued=fx.store.enqueue_recording_speakers(id).unwrap();
    assert_eq!(fx.store.enqueue_recording_speakers(id).unwrap()["id"],enqueued["id"]);
    let job=fx.store.job_command(&json!({"action":"claim","id":enqueued["id"],"owner":"SYNTHETIC","lease_ms":180000})).unwrap();
    let r=fx.store.workflow_prepare(&job).unwrap();
    let assignments:Vec<_>=(0..5).map(|i|json!({"index":i,"speaker":if i==4{"unresolved"}else if i==0||i==3{"recording-speaker:A"}else{"recording-speaker:B"},"label":if i==4{"未确定说话人"}else if i==0||i==3{"说话人 1"}else{"说话人 2"},"correction_key":if i==0||i==3{"说话人 1"}else{"说话人 2"},"voice_ref":if i==4{Value::Null}else{json!("SYNTHETIC-ref")},"match":null})).collect();
    let out=json!({"format":"recording-speakers/1","revision":r["input"]["revision"],"policy":r["input"]["policy"],"assignments":assignments,"speaker_count":2,"unresolved_segments":1,"voice_refs":["SYNTHETIC-ref"],"producer":{"model_id":"SYNTHETIC","model_revision":"fixture/1"}});
    (r,out)
}
#[test]
fn whole_recording_clustering_is_idempotent_and_preserves_original_text_times_and_names() {
    let (fx,id)=recording();
    fx.store.label_speaker(&json!({"id":id,"speaker":"说话人 1","name":"SYNTHETIC Human"})).unwrap();
    let before=fx.store.media_item(&id).unwrap();
    let (r,out)=reconciliation(&fx,&id);
    let item=fx.store.recording_speakers_complete(&r,&out).unwrap();
    assert_eq!(item["speaker_grouping"]["speaker_count"],2);
    assert_eq!(item["speakers"].as_array().unwrap().len(),2);
    assert!(item["segments"][4]["speaker"].is_null());
    assert_eq!(item["segments"][0]["speaker_name"],"SYNTHETIC Human");
    for (a,b) in before["segments"].as_array().unwrap().iter().zip(item["segments"].as_array().unwrap()) {
        for key in ["text","start_seconds","end_seconds"] {assert_eq!(a[key],b[key]);}
    }
    assert_eq!(fx.store.recording_speakers_complete(&r,&out).unwrap()["segments"],item["segments"]);
    assert_eq!(fx.store.recording_speaker_input(&id).unwrap()["revision"],r["input"]["revision"]);
    fx.store.media_command(&json!({"action":"delete","id":id})).unwrap();
    assert!(fx.store.voice_refs_to_forget(None).unwrap().contains(&"SYNTHETIC-ref".into()));
    assert!(fx.store.recording_speakers_complete(&r,&out).is_err());
}
#[test]
fn speaker_clustering_refuses_late_cancelled_replies_and_changed_human_labels() {
    let (fx,id)=recording();let (r,out)=reconciliation(&fx,&id);
    fx.store.label_speaker(&json!({"id":id,"speaker":"说话人 2","name":"SYNTHETIC new answer"})).unwrap();
    assert!(fx.store.recording_speakers_complete(&r,&out).is_err());
    fx.store.job_command(&json!({"action":"cancel","id":r["job"]["id"]})).unwrap();
    assert!(fx.store.recording_speakers_complete(&r,&out).is_err());
    assert!(fx.store.workflow_command(&json!({"action":"speaker_policy","id":id,"policy":{"version":1,"preset":"INVALID","max_speakers":0}})).is_err());
}
#[test]
fn speaker_clustering_rejects_duplicate_intervals_and_revoked_sources() {
    let (fx,id)=recording();let (r,mut out)=reconciliation(&fx,&id);
    out["assignments"][4]["index"]=json!(0);
    assert!(fx.store.recording_speakers_complete(&r,&out).is_err());
    let source=fx.store.sources().unwrap().into_iter().find(|s|s.kind=="media.import").unwrap();
    fx.store.set_source_status(&source.source_id,"revoked",None).unwrap();
    assert!(fx.store.recording_speaker_input(&id).is_err());
    assert!(fx.store.recording_speakers_complete(&r,&out).is_err());
}

fn whole_recording(fx:&Fx,id:&str)->(Value,Value) {
    let (r,_)=reconciliation(fx,id);
    let line=|a:f64,b:f64,text:&str,who:&str,correction:&str,m:Value|json!({"start_seconds":a,"end_seconds":b,"text":text,"speaker":format!("recording-speaker:{who}"),"label":if who=="A"{"说话人 1"}else{"说话人 2"},"voice_ref":format!("SYNTHETIC-ref-{who}"),"match":m,"correction_key":correction});
    let out=json!({"format":"recording-speakers/2","revision":r["input"]["revision"],"segments":[
        line(0.0,1.2,"SYNTHETIC 整段第一句","A","说话人 1",Value::Null),
        line(1.2,2.0,"SYNTHETIC 整段第二句","B","说话人 2",json!({"status":"matched","person_id":"v-li","name":"李娜","score":0.81,"margin":0.3,"threshold":0.62,"allow_auto":true})),
        json!({"start_seconds":2.0,"end_seconds":3.8,"text":"SYNTHETIC 没有声音证据的一句","speaker":"unresolved","label":"未确定说话人","voice_ref":null,"match":null,"correction_key":null}),
    ],"speaker_count":2,"unresolved_segments":1,"dropped_segments":2,"voice_refs":["SYNTHETIC-ref-A","SYNTHETIC-ref-B"],
      "producer":{"model_id":"SYNTHETIC","model_revision":"fixture/1","processor_version":"recording-windows/1"}});
    (r,out)
}
#[test]
fn whole_recording_result_replaces_the_transcript_and_keeps_earlier_names() {
    let (fx,id)=recording();
    fx.store.label_speaker(&json!({"id":id,"speaker":"说话人 1","name":"SYNTHETIC Human"})).unwrap();
    let (r,out)=whole_recording(&fx,&id);
    let item=fx.store.recording_speakers_complete(&r,&out).unwrap();
    let texts:Vec<_>=item["segments"].as_array().unwrap().iter().map(|s|s["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(texts,["SYNTHETIC 整段第一句","SYNTHETIC 整段第二句","SYNTHETIC 没有声音证据的一句"]);
    assert_eq!(item["speaker_grouping"]["version"],2);
    assert_eq!(item["speaker_grouping"]["dropped_segments"],2);
    assert_eq!(item["speakers"].as_array().unwrap().len(),2);
    assert_eq!(item["segments"][0]["speaker_name"],"SYNTHETIC Human","a name given before carries over");
    assert_eq!(item["segments"][1]["speaker_status"],"auto");
    assert!(item["segments"][2]["speaker"].is_null());
    assert_eq!(item["segments"][0]["source_segment_id"],json!(id));
    // The job depends on the per-file transcripts, so the replaced transcript does not ask for another run.
    assert_eq!(fx.store.recording_speaker_input(&id).unwrap()["revision"],r["input"]["revision"]);
    assert_eq!(fx.store.enqueue_recording_speakers(&id).unwrap()["id"],r["job"]["id"]);
    assert_eq!(fx.store.recording_speakers_complete(&r,&out).unwrap()["segments"],item["segments"]);
    // A name given on the new speakers sticks.
    fx.store.label_speaker(&json!({"id":id,"speaker":"recording-speaker:B","name":"SYNTHETIC Second"})).unwrap();
    assert_eq!(fx.store.media_item(&id).unwrap()["segments"][1]["speaker_name"],"SYNTHETIC Second");
    fx.store.media_command(&json!({"action":"delete","id":id})).unwrap();
    let forget=fx.store.voice_refs_to_forget(None).unwrap();
    assert!(forget.contains(&"SYNTHETIC-ref-A".into()) && forget.contains(&"SYNTHETIC-ref-B".into()));
}
#[test]
fn whole_recording_result_is_checked() {
    let (fx,id)=recording();
    let (r,out)=whole_recording(&fx,&id);
    let mut bad=out.clone();bad["speaker_count"]=json!(3);
    assert!(fx.store.recording_speakers_complete(&r,&bad).is_err());
    let mut bad=out.clone();bad["voice_refs"]=json!(["SYNTHETIC-ref-A"]);
    assert!(fx.store.recording_speakers_complete(&r,&bad).is_err());
    let mut bad=out.clone();bad["segments"][1]["end_seconds"]=json!(0.5);
    assert!(fx.store.recording_speakers_complete(&r,&bad).is_err());
    let mut bad=out.clone();bad["segments"][0]["start_seconds"]=json!(1.5);
    assert!(fx.store.recording_speakers_complete(&r,&bad).is_err(),"lines are in time order");
    let mut bad=out.clone();bad["revision"]=json!("0".repeat(64));
    assert!(fx.store.recording_speakers_complete(&r,&bad).is_err());
    assert!(fx.store.recording_speakers_complete(&r,&out).is_ok());
}

#[test]
fn recording_messages_carry_speaker_names_and_follow_relabels() {
    let (fx, id) = recording();
    fx.store.curated_sync().unwrap();
    let senders = || fx.texts("SELECT COALESCE(sender,'-')||'|'||COALESCE(sender_entity_id IS NOT NULL,0) FROM messages WHERE conversation_id GLOB 'rec:*' ORDER BY sent_ms");
    // 张伟 matched clearly (a person, but only a voice-library name: no entity yet); the rest are labels.
    assert_eq!(senders(), vec!["张伟|0", "说话人 2|0", "说话人 3|0", "张伟|0", "说话人 4|0"]);
    // You say who 说话人 2 is: the message follows, with the person's entity.
    fx.store.media_command(&json!({"action":"speaker","id":id,"speaker":"说话人 2","voice_id":"v-li","name":"李娜","samples_saved":false})).unwrap();
    fx.store.curated_sync().unwrap();
    let after = senders();
    assert_eq!(after[1], "李娜|1", "{after:?}");
    assert_eq!(after[0], "张伟|0");
    assert_eq!(fx.scalar("SELECT COUNT(DISTINCT sender_entity_id) FROM messages WHERE sender='李娜'"), 1);
}
