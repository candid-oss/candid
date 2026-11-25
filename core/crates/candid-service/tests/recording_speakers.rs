//! Optional local acceptance of user-selected recordings. Private input and evidence stay outside Git.
use candid_service::*;
use candid_store::*;
use serde_json::{json,Value};
use std::{path::PathBuf,time::{Duration,Instant}};
#[test]
#[ignore = "requires explicit selected private recordings and an isolated local model service"]
fn selected_recordings_are_reconciled_as_whole_recordings_without_retranscription() {
    let input=std::env::var_os("CANDID_SPEAKER_ACCEPTANCE_INPUT").expect("selected input required");
    let records:Vec<Value>=serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let root=PathBuf::from(std::env::var("CANDID_SPEAKER_ACCEPTANCE_ROOT").unwrap());
    assert!(!root.exists(),"acceptance needs a fresh isolated root");
    let mut config=json!({"version":1,"providers":[{"provider_id":"local","name":"Local acceptance","api":"other","base_url":"http://127.0.0.1:18770/v1","destination":"on_device","enabled":true}],"models":[{"provider_id":"local","model_id":"SYNTHETIC-import"}],"roles":[{"role":"asr","model":{"provider_id":"local","model_id":"SYNTHETIC-import"}}]});
    let store=TimelineStore::open(&root,"recording-speaker-acceptance",true,Clock::default()).unwrap();
    store.model_command(&json!({"action":"save","config":config})).unwrap();
    store.workflow_command(&json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false,"summary":false}})).unwrap();
    for recording in &records {
        for (index,part) in recording["parts"].as_array().unwrap().iter().enumerate() {
            let mut command=json!({"action":"import","id":part["id"],"filename":format!("ACCEPTANCE original {index}.wav"),"source":"mac_microphone","kind":"audio","path":part["path"]});
            if part["recording"].is_object() {let mut p=part["recording"].clone();p["recording_id"]=recording["id"].clone();command["recording"]=p;}
            store.media_command(&command).unwrap();
            store.media_command(&json!({"action":"recognize","id":part["id"],"mode":"quick"})).unwrap();
            let job=store.job_command(&json!({"action":"claim","kind":"speech.recognize","owner":"acceptance","lease_ms":180000})).unwrap();
            let request=store.workflow_prepare(&job).unwrap();
            let mut producer=part["metadata"]["producer"].clone();producer["revision"]=json!("imported-existing-transcript/1");
            store.workflow_complete(&request,&json!({"producer":producer,"segments":part["metadata"]["segments"]})).unwrap();
        }
        if recording["parts"].as_array().unwrap().len()>1 {store.media_command(&json!({"action":"state","id":recording["id"],"state":"stopped"})).unwrap();}
    }
    config["roles"]=json!([{"role":"diarization","model":{"provider_id":"local","model_id":"SYNTHETIC-import"}}]);
    store.model_command(&json!({"action":"save","config":config})).unwrap();
    let originals:Vec<_>=records.iter().map(|r|store.media_item(r["id"].as_str().unwrap()).unwrap()["segments"].clone()).collect();
    let jobs:Vec<_>=records.iter().map(|r|store.enqueue_recording_speakers(r["id"].as_str().unwrap()).unwrap()).collect();
    drop(store);
    let run=root.parent().unwrap().join("run");
    let mut options=ServeOptions::new(root.clone(),run.clone()); options.power = Some(true);options.socket=Some(run.join("core.sock"));
    let service=Service::start(&options, |_|{}).unwrap();
    let deadline=Instant::now()+Duration::from_secs(100);
    loop {
        let complete=jobs.iter().all(|j| {
            let (_,data)=local_request(&service.core.socket,"POST","/local/jobs",&serde_json::to_vec(&json!({"action":"get","id":j["id"]})).unwrap()).unwrap();
            let value:Value=serde_json::from_slice(&data).unwrap();
            assert!(value["state"]!="failed"&&value["state"]!="cancelled","{value}");value["state"]=="done"
        });
        if complete {break;}
        assert!(Instant::now()<deadline,"recording speaker job did not finish");std::thread::sleep(Duration::from_millis(100));
    }
    let mut reports=Vec::new();
    for (i,recording) in records.iter().enumerate() {
        let (_,data)=local_request(&service.core.socket,"POST","/local/media",&serde_json::to_vec(&json!({"action":"item","id":recording["id"]})).unwrap()).unwrap();
        let item:Value=serde_json::from_slice(&data).unwrap();
        assert_eq!(item["speaker_grouping"]["speaker_count"],3,"acceptance recording {i}: {}",item["speaker_grouping"]);
        assert_eq!(item["segments"].as_array().unwrap().len(),originals[i].as_array().unwrap().len());
        for (a,b) in originals[i].as_array().unwrap().iter().zip(item["segments"].as_array().unwrap()) {assert_eq!(a["text"],b["text"]);for key in ["start_seconds","end_seconds"] {assert!((a[key].as_f64().unwrap()-b[key].as_f64().unwrap()).abs()<1e-9);}}
        reports.push(json!({"recording":i,"speaker_grouping":item["speaker_grouping"],"segments":item["segments"].as_array().unwrap().len()}));
    }
    service.stop();
    std::fs::write(root.parent().unwrap().join("acceptance-result.json"),serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
    println!("Local whole-recording acceptance passed: {} recordings; original text/times preserved",reports.len());
}
