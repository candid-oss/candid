//! Complete synthetic migration, media, queue and revision acceptance.
mod common;
use common::*;
use candid_store::{records::Rules, *};
use serde_json::{json, Value};
use std::{fs, path::Path};
fn wav(path: &Path) {
    let rate = 8000u32;
    let samples = 8000u32;
    let mut b = vec![];
    b.extend(b"RIFF");
    b.extend((36 + samples * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((samples * 2).to_le_bytes());
    b.resize(44 + samples as usize * 2, 0);
    fs::write(path, b).unwrap();
}
fn upload(fx: &Fx, id: &str) -> Value {
    let p = fx.root.join("SYNTHETIC.wav");
    wav(&p);
    json!({"action":"import","id":id,"filename":"SYNTHETIC.wav","kind":"audio","source":"mac_microphone","path":p,"occurred_at":"2026-11-12T00:00:00Z"})
}
const A: &str = "10000000-0000-4000-8000-000000000001";
const B: &str = "10000000-0000-4000-8000-000000000002";
const R: &str = "10000000-0000-4000-8000-000000000003";
#[test]
fn receive_retry_occurrences_search_ranges_delete_and_reopen() {
    let mut fx = Fx::make("native-media");
    let v = upload(&fx, A);
    let a = fx.store.media_command(&v).unwrap();
    assert_eq!(fx.store.media_command(&v).unwrap(), a);
    let mut other = v.clone();
    other["id"] = json!(B);
    fx.store.media_command(&other).unwrap();
    assert_eq!(
        fx.store.media_command(&json!({"q":"synthetic"})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let read = fx
        .store
        .media_command(&json!({"action":"read","id":A,"offset":44,"length":8}))
        .unwrap();
    assert_eq!(read["data"], "AAAAAAAAAAA=");
    let mut conflict = v.clone();
    conflict["filename"] = json!("different.wav");
    assert!(fx.store.media_command(&conflict).is_err());
    fx.store
        .media_command(&json!({"action":"edit","id":A,"title":"人工标题","notes":"人工备注"}))
        .unwrap();
    assert_eq!(
        fx.store.media_command(&json!({"q":"人工备注"})).unwrap()["items"][0]["id"],
        A
    );
    // Lost receipt/restart, then deletion must invalidate raw queries and forbid resurrection.
    let root = fx.root.clone();
    let id = fx.store.identity.store_id.clone();
    fx.store.close();
    let store = open(&root);
    assert_eq!(store.identity.store_id, id);
    assert_eq!(store.media_item(A).unwrap()["title"], "人工标题");
    store
        .media_command(&json!({"action":"delete","id":A}))
        .unwrap();
    assert!(store.media_command(&v).is_err());
    assert!(store
        .media_command(&json!({"action":"content","id":A}))
        .is_err());
    assert!(store.media_item(B).is_ok());
}
#[test]
fn recording_parts_validate_samples_overlap_and_do_not_concatenate() {
    let fx = Fx::make("native-parts");
    let mut v = upload(&fx, A);
    v["recording"] = json!({"recording_id":R,"segment_index":0,"sample_start":0,"sample_count":8000,"sample_rate":8000,"recording_started_at":"2026-11-12T00:00:00Z"});
    fx.store.media_command(&v).unwrap();
    let mut b = v.clone();
    b["id"] = json!(B);
    assert!(fx.store.media_command(&b).is_err());
    b["recording"]["segment_index"] = json!(1);
    b["recording"]["sample_start"] = json!(16000);
    let item = fx.store.media_command(&b).unwrap();
    assert_eq!(item["duration_seconds"], 3.0);
    assert_eq!(item["gaps"][0]["sample_count"], 8000);
    let content = fx
        .store
        .media_command(&json!({"action":"content","id":R}))
        .unwrap();
    assert_eq!(content["parts"].as_array().unwrap().len(), 2);
    assert_eq!(content["format"], "candid-playlist/1");
    fx.store
        .media_command(&json!({"action":"delete","id":R}))
        .unwrap();
    assert!(fx.store.media_command(&b).is_err());
}
#[test]
fn bad_files_and_revoked_media_are_refused() {
    let fx = Fx::make("bad-media");
    let v = upload(&fx, A);
    fs::write(v["path"].as_str().unwrap(), b"SYNTHETIC corrupt").unwrap();
    assert!(fx.store.media_command(&v).is_err());
    let v = upload(&fx, A);
    fx.store.media_command(&v).unwrap();
    let source = fx
        .store
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.display_name == "资料库")
        .unwrap();
    fx.store
        .set_source_status(&source.source_id, "revoked", None)
        .unwrap();
    assert!(fx
        .store
        .media_command(&json!({"action":"content","id":A}))
        .is_err());
}
#[test]
fn leased_queue_checkpoint_restart_fencing_cancel_and_result() {
    let fx = Fx::make("queue");
    let v = upload(&fx, A);
    fx.store.media_command(&v).unwrap();
    let enqueue = json!({"action":"enqueue","kind":"speech.recognize","input_ref":A,"config_version":"synthetic/1"});
    let a = fx.store.job_command(&enqueue).unwrap();
    assert_eq!(a, fx.store.job_command(&enqueue).unwrap());
    let id = a["id"].as_str().unwrap();
    let claim = fx
        .store
        .job_command(&json!({"action":"claim","owner":"one"}))
        .unwrap();
    assert_eq!(claim["attempt"], 1);
    assert!(fx
        .store
        .job_command(&json!({"action":"claim","owner":"two"}))
        .unwrap()
        .is_null());
    let base =
        json!({"action":"checkpoint","id":id,"owner":"one","attempt":1,"checkpoint":{"part":3}});
    fx.store.job_command(&base).unwrap();
    fx.store
        .raw_db()
        .execute("UPDATE jobs SET lease_until_ms=0", &[])
        .unwrap();
    let next = fx
        .store
        .job_command(&json!({"action":"claim","owner":"two"}))
        .unwrap();
    assert_eq!(next["attempt"], 2);
    assert_eq!(next["checkpoint"]["part"], 3);
    assert!(fx.store.job_command(&base).is_err());
    let result = json!({"action":"result","job_id":id,"owner":"two","attempt":2,"mode":"quick","result":{"producer":{"model_id":"SYNTHETIC-STUB","revision":"fixture/1"},"segments":[{"start_seconds":0.,"end_seconds":1.,"text":"合成转写"}]}});
    fx.store.media_command(&result).unwrap();
    assert_eq!(
        fx.store.media_command(&json!({"q":"合成转写"})).unwrap()["items"][0]["id"],
        A
    );
    assert_eq!(
        fx.store.media_command(&result).unwrap()["processing_status"],
        "ready"
    );
    fx.store
        .media_command(&json!({"action":"recognize","id":A,"mode":"quick"}))
        .unwrap();
    let claim = fx
        .store
        .job_command(&json!({"action":"claim","owner":"two"}))
        .unwrap();
    fx.store
        .job_command(&json!({"action":"cancel","id":claim["id"]}))
        .unwrap();
    assert!(fx
        .store
        .job_command(
            &json!({"action":"finish","id":claim["id"],"owner":"two","attempt":claim["attempt"]})
        )
        .is_err());
}
#[test]
fn persisted_revisions_select_rollback_delete_and_scope() {
    let fx = Fx::make("durable-records");
    let mut e = IngestEvent::new("one", "screen.text", Some(1));
    e.text.push(IngestText::new("body", "SYNTHETIC original"));
    fx.store
        .ingest(&IngestBatch::events(&fx.source, vec![e]))
        .unwrap();
    let a = fx
        .store
        .stage_record_revision(&fx.source, Rules::default())
        .unwrap();
    assert_eq!(
        a,
        fx.store
            .stage_record_revision(&fx.source, Rules::default())
            .unwrap()
    );
    fx.store.select_record_revision(&a, &fx.client).unwrap();
    let mut rules = Rules::default();
    rules.revisit_ms = 100;
    let b = fx.store.stage_record_revision(&fx.source, rules).unwrap();
    fx.store.select_record_revision(&b, &fx.client).unwrap();
    fx.store.select_record_revision(&a, &fx.client).unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM record_audit"), 3);
    let denied = fx
        .store
        .register_client(
            "metadata",
            "http",
            &ClientScope {
                text: false,
                ..ClientScope::unbounded()
            },
            None,
        )
        .unwrap();
    assert!(fx.store.record_revision(&a, &denied.client_id).is_err());
    let e = fx.store.event_id(&fx.source, "one").unwrap().unwrap();
    fx.store.delete_events(&[e], "user").unwrap();
    assert!(fx.store.record_revision(&a, &fx.client).is_err());
    assert!(!fx
        .texts("SELECT keys_json FROM record_revisions")
        .join("")
        .contains("SYNTHETIC original"));
}
#[test]
fn legacy_migration_interrupt_retry_all_modes_edits_tombstones_and_lock() {
    let fx = Fx::make("legacy-native");
    let hub = fx.root.join("OLD-SYNTHETIC");
    let vault = hub.join("vault/media_timeline");
    fs::create_dir_all(vault.join("blobs")).unwrap();
    let db = testing::Db::open(&hub.join("memory.sqlite"), false, true).unwrap();
    db.script("CREATE TABLE media_timeline_schema(slot INTEGER PRIMARY KEY,version INTEGER);INSERT INTO media_timeline_schema VALUES(1,1);CREATE TABLE media_timeline(id TEXT,filename TEXT,kind TEXT,source TEXT,occurred_at TEXT,imported_at TEXT,blob TEXT,processing_status TEXT,processing_mode TEXT);CREATE TABLE media_timeline_recognition(media_id TEXT,mode TEXT,result TEXT);CREATE TABLE media_timeline_deleted(id TEXT);").unwrap();
    db.conn.execute("INSERT INTO media_timeline VALUES(?,'SYNTHETIC.wav','audio','mac_microphone','2026-11-12T00:00:00Z','2026-11-12T00:00:00Z','part.wav','processing','quick')",[A]).unwrap();
    let result=json!({"producer":{"model_id":"SYNTHETIC","revision":"old/1"},"segments":[{"start_seconds":0.,"end_seconds":1.,"text":"旧合成结果"}]}).to_string();
    for mode in ["quick", "conversation"] {
        db.conn
            .execute(
                "INSERT INTO media_timeline_recognition VALUES(?,?,?)",
                [A, mode, &result],
            )
            .unwrap();
    }
    let index = json!({"version":1,"recordings":{R:{"parts":{A:{"index":0,"sample_start":0,"sample_count":8000,"sample_rate":8000,"started_at":"2026-11-12T00:00:00Z"}},"state":"stopped"}},"edits":{R:{"title":"人工旧标题","notes":"保留"}},"deleted":[B]});
    fs::write(vault.join("recordings-v1.json"), index.to_string()).unwrap();
    let lock = testing::Db::open(&vault.join("service.sqlite"), false, true).unwrap();
    lock.execute("BEGIN EXCLUSIVE", &[]).unwrap();
    assert!(fx.store.migrate_media(&hub).is_err());
    drop(lock);
    assert!(fx.store.migrate_media(&hub).is_err());
    assert!(fx.store.media_command(&json!({"action":"list"})).is_err());
    wav(&vault.join("blobs/part.wav"));
    let receipt = fx
        .store
        .media_command(&json!({"action":"migrate","hub":hub}))
        .unwrap();
    assert_eq!(receipt["objects"], 1);
    assert_eq!(fx.store.media_item(R).unwrap()["title"], "人工旧标题");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM media_recognitions"), 2);
    assert_eq!(
        fx.scalar("SELECT COUNT(*) FROM jobs WHERE state='queued'"),
        1
    );
    assert!(
        fx.store
            .media_command(&json!({"action":"state","id":B}))
            .unwrap()["recording_state"]
            == "deleted"
    );
    assert_eq!(fx.store.migrate_media(&hub).unwrap(), receipt);
    assert!(
        db.scalar("SELECT COUNT(*) FROM media_timeline", &[])
            .unwrap()
            == 1
    );
    assert!(db.conn.execute_batch("BEGIN EXCLUSIVE").is_ok());
    drop(db);
    let old_lock = testing::Db::open(&vault.join("service.sqlite"), false, false).unwrap();
    old_lock.execute("PRAGMA busy_timeout=0", &[]).unwrap();
    assert!(old_lock.execute("BEGIN EXCLUSIVE", &[]).is_err());
}

#[test]
fn selected_revision_and_checkpoint_survive_store_reopen() {
    let root=fixtures::temp_root("durable-reopen");
    let store=open(&root);
    let source=store.register_source("mac.desktop","SYNTHETIC",&SourcePolicy::default()).unwrap();
    let client=store.internal_client("synthetic-app",&ClientScope::unbounded()).unwrap();
    let mut event=IngestEvent::new("e","screen.text",Some(1));
    event.text.push(IngestText::new("body","SYNTHETIC durable"));
    store.ingest(&IngestBatch::events(&source,vec![event])).unwrap();
    let revision=store.stage_record_revision(&source,Rules::default()).unwrap();
    store.select_record_revision(&revision,&client).unwrap();
    let job=store.job_command(&json!({"action":"enqueue","kind":"SYNTHETIC","input_ref":"input","config_version":"1"})).unwrap();
    store.job_command(&json!({"action":"claim","owner":"before-restart"})).unwrap();
    store.job_command(&json!({"action":"checkpoint","id":job["id"],"owner":"before-restart","attempt":1,"checkpoint":{"completed_parts":3}})).unwrap();
    drop(store);
    let store=open(&root);
    assert_eq!(store.record_revision(&revision,&client).unwrap().reading[0].text,"SYNTHETIC durable");
    let management=store.record_management(&json!({"action":"list"}),&client).unwrap();
    assert_eq!(management["audit"][0]["selected"],revision);
    let persisted=store.job_command(&json!({"action":"get","id":job["id"]})).unwrap();
    assert_eq!(persisted["checkpoint"]["completed_parts"],3);
    assert_eq!(persisted["attempt"],1);
    assert_eq!(persisted["state"],"running");
    drop(store);fs::remove_dir_all(root).unwrap();
}
#[test]
fn anonymous_speaker_labels_remain_scoped_to_each_physical_recording() {
    let fx=Fx::make("speaker-scope");let mut a=upload(&fx,A);
    a["recording"]=json!({"recording_id":R,"segment_index":0,"sample_start":0,"sample_count":8000,"sample_rate":8000,"recording_started_at":"2026-11-12T00:00:00Z"});
    fx.store.media_command(&a).unwrap();let mut b=a.clone();b["id"]=json!(B);b["recording"]["segment_index"]=json!(1);b["recording"]["sample_start"]=json!(8000);fx.store.media_command(&b).unwrap();
    fx.store.media_command(&json!({"action":"edit","id":R,"title":"人工标题","notes":"人工备注"})).unwrap();
    for id in [A,B] {
        fx.store.media_command(&json!({"action":"recognize","id":id,"mode":"conversation"})).unwrap();
        let j=fx.store.job_command(&json!({"action":"claim","kind":"speech.recognize","owner":"SYNTHETIC"})).unwrap();
        fx.store.media_command(&json!({"action":"result","job_id":j["id"],"owner":"SYNTHETIC","attempt":j["attempt"],"mode":"conversation","result":{"producer":{"model_id":"SYNTHETIC","revision":"test/1"},"segments":[{"start_seconds":0.,"end_seconds":0.5,"text":"合成发言","speaker":"说话人1","identity":"候选人物"}]}})).unwrap();
    }
    let item=fx.store.media_item(R).unwrap();assert_eq!(item["segments"][0]["speaker"],"录音段1 · 说话人1");assert_eq!(item["segments"][1]["speaker"],"录音段2 · 说话人1");assert_eq!(item["segments"][1]["start_seconds"],1.);assert_eq!(item["segments"][1]["source_segment_id"],B);assert_eq!(item["title"],"人工标题");assert_eq!(item["notes"],"人工备注");
    assert!(fx.store.media_command(&json!({"action":"content","id":R})).is_ok());
}

/// Parts recorded straight to AAC (fragmented MP4) link with their exact valid frames, ten minutes long allowed,
/// and are not marked for compaction (they are already AAC).
#[cfg(target_os = "macos")]
#[test]
fn aac_recording_parts_link_with_exact_frames() {
    let fx = Fx::make("native-aac-parts");
    let pcm = fx.root.join("SYNTHETIC-tone.wav");
    let rate = 16000u32;
    let samples = rate * 20;
    let mut b = vec![];
    b.extend(b"RIFF");
    b.extend((36 + samples * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend([1, 0, 1, 0]);
    b.extend(rate.to_le_bytes());
    b.extend((rate * 2).to_le_bytes());
    b.extend([2, 0, 16, 0]);
    b.extend(b"data");
    b.extend((samples * 2).to_le_bytes());
    for i in 0..samples {
        b.extend(((((i as f64) * 0.17).sin() * 8000.0) as i16).to_le_bytes());
    }
    fs::write(&pcm, b).unwrap();
    let aac = fx.root.join("SYNTHETIC-part.m4a");
    assert!(std::process::Command::new("/usr/bin/afconvert").args(["-f", "m4af", "-d", "aac", "-b", "48000", "-c", "1"]).arg(&pcm).arg(&aac).status().unwrap().success());
    let mut v = json!({"action":"import","id":A,"filename":"SYNTHETIC.m4a","kind":"audio","source":"mac_microphone","path":aac,"occurred_at":"2026-11-12T00:00:00Z"});
    v["recording"] = json!({"recording_id":R,"segment_index":0,"sample_start":0,"sample_count":samples + 1,"sample_rate":rate,"recording_started_at":"2026-11-12T00:00:00Z"});
    assert!(fx.store.media_command(&v).is_err(), "a wrong frame count is refused");
    v["recording"]["sample_count"] = json!(samples);
    let item = fx.store.media_command(&v).unwrap();
    assert_eq!(item["duration_seconds"], 20.0);
    assert!(fx.store.media_compaction_candidates(0, 10).unwrap().is_empty(), "AAC parts are not compacted again");
}
