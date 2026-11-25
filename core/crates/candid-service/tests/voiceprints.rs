use candid_service::*;
use candid_store::*;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
};
fn config(url: &str) -> Value {
    json!({"version":1,"providers":[{"provider_id":"voice","name":"SYNTHETIC","api":"other","base_url":url,"enabled":true,"destination":"on_device"}],"models":[],"roles":[{"role":"diarization","model":{"provider_id":"voice","model_id":"SYNTHETIC"}}]})
}
fn post(socket: &std::path::Path, path: &str, v: Value) -> (u16, Value) {
    let (c, b) = local_request(socket, "POST", path, &serde_json::to_vec(&v).unwrap()).unwrap();
    (c, serde_json::from_slice(&b).unwrap())
}
fn sample_ready() -> bool {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); }
    path.exists()
}
fn start(url: &str) -> (Service, PathBuf, PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("vp-{}", &uuid_v4()[..8]));
    let socket = dir.join("run/core.sock");
    let mut o = ServeOptions::new(dir.join("root"), dir.join("run")); o.power = Some(true);
    o.socket = Some(socket.clone());
    let svc = Service::start(&o, |_| {}).unwrap();
    assert_eq!(
        post(
            &socket,
            "/local/models",
            json!({"action":"save","config":config(url)})
        )
        .0,
        200
    );
    // These tests drive the voice service by hand; automatic transcription would race them for the mock server.
    assert_eq!(post(&socket, "/local/workflows", json!({"action":"settings","settings":{"version":1,"automatic":false,"titles":false}})).0, 200);
    let id = uuid_v4();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
    assert_eq!(post(&socket,"/local/media",json!({"action":"import","id":id,"filename":"PUBLIC.wav","source":"import","kind":"audio","path":path})).0,200);
    (svc, dir, socket, id)
}
fn request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut b = Vec::new();
    let mut tmp = [0; 4096];
    loop {
        let n = stream.read(&mut tmp).unwrap();
        assert!(n > 0);
        b.extend(&tmp[..n]);
        if let Some(i) = b.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&b[..i]);
            let n = head
                .lines()
                .find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if b.len() >= i + 4 + n {
                return b;
            }
        }
    }
}
fn respond(stream: &mut std::net::TcpStream, v: Value) {
    let b = v.to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        b.len(),
        b
    )
    .unwrap();
}
fn analysis(b: &[u8]) -> Value {
    let i = b.windows(4).position(|w| w == b"RIFF").unwrap();
    let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize + 8;
    json!({"format":"candid-voiceprint-analysis/1","analysis":{"analysis_id":"SYNTHETIC-cache","upload_hash":sha256_hex(&b[i..i+n]),"segments":[{"start_s":0.0,"end_s":0.5,"speaker_id":"speaker_01","text":"SYNTHETIC"}],"speakers":[{"speaker_id":"speaker_01","identity":{"status":"unknown","name":null}}]}})
}
#[test]
fn analysis_maps_selection_and_management_requires_confirmation_and_retries() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for index in 0..7 {
            let (mut stream, _) = listener.accept().unwrap();
            let bytes = request(&mut stream);
            if index == 0 {
                respond(&mut stream, analysis(&bytes));
            } else {
                if index == 2 || index == 4 {
                    assert!(String::from_utf8_lossy(&bytes).contains("SYNTHETIC-cache"));
                }
                respond(
                    &mut stream,
                    json!({"format":"candid-voiceprints/1","person":{"id":"SYNTHETIC-person"},"receipt":{"format":"candid-voiceprint-enrollment/1","person_id":"SYNTHETIC-person","templates":[{"source_hash":"SYNTHETIC","start":0,"end":1,"model_id":"SYNTHETIC","model_revision":"1"}]},"capabilities":["template_withdrawal"],"people":[]}),
                );
            }
        }
    });
    let (svc, dir, socket, id) = start(&url);
    let (c, r) = post(
        &socket,
        "/local/voiceprints",
        json!({"action":"analyze","id":id,"start_seconds":1.,"end_seconds":2.}),
    );
    assert_eq!(c, 200, "{r}");
    assert_eq!(r["analysis"]["segments"][0]["original_start_seconds"], 1.);
    let analysis_id = r["analysis"]["analysis_id"].as_str().unwrap();
    let mut cmd = json!({"action":"enroll","analysis_id":analysis_id,"speaker_id":"speaker_01","name":"SYNTHETIC"});
    assert_ne!(post(&socket, "/local/voiceprints", cmd.clone()).0, 200);
    cmd["confirmed"] = json!(true);
    for _ in 0..2 {
        assert_eq!(post(&socket, "/local/voiceprints", cmd.clone()).0, 200);
    }
    let forget = json!({"action":"forget","analysis_id":analysis_id,"confirmed":true});
    assert_eq!(post(&socket, "/local/voiceprints", forget).0, 200);
    assert_ne!(post(&socket, "/local/voiceprints", cmd).0, 200);
    assert_eq!(
        post(&socket, "/local/voiceprints", json!({"action":"list"})).0,
        200
    );
    server.join().unwrap();
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn deleted_and_revoked_source_discard_late_analysis_and_clear_remote_cache() {
    if !sample_ready() { return; }
    for scenario in ["delete", "revoke", "restore", "unrelated"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let bytes = request(&mut stream);
            tx.send(()).unwrap();
            wait.recv().unwrap();
            respond(&mut stream, analysis(&bytes));
            if scenario != "unrelated" {
                let (mut stream, _) = listener.accept().unwrap();
                let b = request(&mut stream);
                assert!(String::from_utf8_lossy(&b).contains("forget"));
                respond(&mut stream, json!({"people":[]}));
            }
        });
        let (svc, dir, socket, id) = start(&url);
        let sock = socket.clone();
        let source_id = id.clone();
        let caller = std::thread::spawn(move || {
            post(
                &sock,
                "/local/voiceprints",
                json!({"action":"analyze","id":source_id,"start_seconds":0.,"end_seconds":1.}),
            )
        });
        rx.recv().unwrap();
        if scenario == "revoke" || scenario == "restore" {
            let s = svc.store.lock().unwrap();
            let source = s
                .sources()
                .unwrap()
                .into_iter()
                .find(|s| s.display_name == "资料库")
                .unwrap();
            let original = source.policy.clone();
            let mut p = source.policy;
            p.store_text = false;
            s.update_policy(&source.source_id, &p).unwrap();
            if scenario == "restore" {
                s.update_policy(&source.source_id, &original).unwrap();
            }
        } else if scenario == "unrelated" {
            let s = svc.store.lock().unwrap();
            let other = s
                .register_source(
                    "mac.region",
                    "SYNTHETIC unrelated",
                    &SourcePolicy::new(true, true),
                )
                .unwrap();
            s.update_policy(&other, &SourcePolicy::new(false, false))
                .unwrap();
            s.raw_db()
                .execute("UPDATE sources SET last_sync_ms=99999", &[])
                .unwrap();
        } else {
            assert_eq!(
                post(&socket, "/local/media", json!({"action":"delete","id":id})).0,
                200
            );
        }
        go.send(()).unwrap();
        assert_eq!(
            caller.join().unwrap().0 == 200,
            scenario == "unrelated",
            "{scenario}"
        );
        server.join().unwrap();
        svc.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn remote_routes_disabled_models_paths_and_offline_are_rejected() {
    if !sample_ready() { return; }
    let (svc, dir, socket, _) = start("http://127.0.0.1:1/v1");
    for v in [
        json!({"action":"list"}),
        json!({"action":"analyze","path":"/etc/passwd","start_seconds":0.,"end_seconds":1.}),
        json!({"action":"delete","person_id":"x"}),
    ] {
        assert_ne!(post(&socket, "/local/voiceprints", v).0, 200);
    }
    let mut c = config("http://127.0.0.1:1@external.invalid/v1");
    let save = post(
        &socket,
        "/local/models",
        json!({"action":"save","config":c}),
    );
    if save.0 == 200 {
        assert_ne!(
            post(&socket, "/local/voiceprints", json!({"action":"list"})).0,
            200
        );
    }
    c = config("http://127.0.0.1:1/v1");
    c["providers"][0]["enabled"] = json!(false);
    assert_eq!(
        post(
            &socket,
            "/local/models",
            json!({"action":"save","config":c})
        )
        .0,
        200
    );
    assert_ne!(
        post(&socket, "/local/voiceprints", json!({"action":"list"})).0,
        200
    );
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
#[ignore = "explicit public model acceptance; independent isolated service on 18769"]
fn public_model_analysis_and_full_management_through_rust() {
    if !sample_ready() { return; }
    let (svc, dir, socket, _) = start("http://127.0.0.1:18769/v1");
    let id = uuid_v4();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/speaker-demo/public-audio/conversation.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    assert_eq!(post(&socket,"/local/media",json!({"action":"import","id":id,"filename":"PUBLIC-conversation.wav","source":"import","kind":"audio","path":path})).0,200);
    let (c, r) = post(
        &socket,
        "/local/voiceprints",
        json!({"action":"analyze","id":id,"start_seconds":0.,"end_seconds":30.}),
    );
    assert_eq!(c, 200, "{r}");
    let speakers = r["analysis"]["speakers"].as_array().unwrap();
    assert!(speakers.len() >= 2, "{r}");
    assert!(!r.to_string().contains("\"vector\""));
    let aid = r["analysis"]["analysis_id"].as_str().unwrap();
    let mut persons = vec![];
    for (index, speaker) in speakers.iter().take(2).enumerate() {
        let cmd = json!({"action":"enroll","analysis_id":aid,"speaker_id":speaker["speaker_id"],"name":format!("PUBLIC acceptance {}",index+1),"confirmed":true});
        let (c, p) = post(&socket, "/local/voiceprints", cmd.clone());
        assert_eq!(c, 200, "{p}");
        let person = p["person"]["id"].clone();
        assert_eq!(
            post(&socket, "/local/voiceprints", cmd).1["person"]["id"],
            person
        );
        persons.push(person);
    }
    assert_eq!(post(&socket,"/local/voiceprints",json!({"action":"enroll","analysis_id":aid,"speaker_id":speakers[0]["speaker_id"],"person_id":persons[0],"confirmed":true})).0,200);
    assert_eq!(
        post(
            &socket,
            "/local/voiceprints",
            json!({"action":"rename","person_id":persons[0],"name":"PUBLIC acceptance renamed"})
        )
        .0,
        200
    );
    for _ in 0..2 {
        assert_eq!(post(&socket,"/local/voiceprints",json!({"action":"merge","person_id":persons[1],"target_id":persons[0],"confirmed":true})).0,200);
    }
    for _ in 0..2 {
        assert_eq!(
            post(
                &socket,
                "/local/voiceprints",
                json!({"action":"delete","person_id":persons[0],"confirmed":true})
            )
            .0,
            200
        );
    }
    for _ in 0..2 {
        assert_eq!(
            post(
                &socket,
                "/local/voiceprints",
                json!({"action":"forget","analysis_id":aid,"confirmed":true})
            )
            .0,
            200
        );
    }
    assert_ne!(
        post(
            &socket,
            "/local/voiceprints",
            json!({"action":"clear_cache"})
        )
        .0,
        200
    );
    assert_eq!(
        post(
            &socket,
            "/local/voiceprints",
            json!({"action":"clear_cache","confirmed":true})
        )
        .0,
        200
    );
    let remaining = post(&socket, "/local/voiceprints", json!({"action":"list"})).1;
    assert!(remaining["people"].as_array().unwrap().is_empty());
    let evidence = json!({"scope":"public_30s_two_speakers_through_rust_local_channel","speakers":speakers.len(),"segments":r["analysis"]["segments"].as_array().unwrap().len(),"mapping":r["source"],"unknown":speakers.iter().all(|s|s["identity"]["name"].is_null()),"management_and_idempotent_retries":true,"remaining_people":0});
    std::fs::write(
        "/tmp/candid-voiceprint-core-public.json",
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn source_removal_withdraws_only_receipted_templates_after_merge() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (svc, dir, _socket, id) = start(&url);
    let store = svc.store.clone();
    svc.stop();
    {
        let s = store.lock().unwrap();
        let source = s.voiceprint_source(&id).unwrap();
        let p = s.voiceprint_provider().unwrap();
        let v = json!({"format":"voiceprint-provenance/1","remote_id":"SYNTHETIC-remote","source":source,"provider":p,"result":{"source":{"id":id},"analysis":{"analysis_id":"SYNTHETIC-local"}},"enrollments":[{"person":{"id":"SYNTHETIC-from"},"receipt":{"format":"candid-voiceprint-enrollment/1","person_id":"SYNTHETIC-from","templates":[{"source_hash":"SYNTHETIC-source-a"}]}}]});
        s.voiceprint_save("SYNTHETIC-local", &v).unwrap();
        s.voiceprint_enrollment(&v).unwrap();
        s.voiceprint_merge_provenance("SYNTHETIC-from", "SYNTHETIC-target")
            .unwrap();
        s.media_command(&json!({"action":"delete","id":id}))
            .unwrap();
    }
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let b = request(&mut stream);
            let text = String::from_utf8_lossy(&b);
            assert!(!text.contains("\"action\":\"delete\""));
            if text.contains("withdraw") {
                assert!(text.contains("SYNTHETIC-target"));
            } else if !text.starts_with("GET") {
                assert!(text.contains("forget"));
            }
            respond(
                &mut stream,
                json!({"format":"candid-voiceprints/1","capabilities":["template_withdrawal"],"people":[]}),
            );
        }
    });
    candid_service::voiceprints::reconcile(&store);
    server.join().unwrap();
    assert!(store
        .lock()
        .unwrap()
        .voiceprint_cleanup_candidates()
        .unwrap()
        .is_empty());
    assert!(store
        .lock()
        .unwrap()
        .voiceprint_load("SYNTHETIC-local")
        .is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn busy_service_can_retry_and_bearer_clients_cannot_manage_library() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        respond(
            &mut stream,
            json!({"format":"candid-voiceprints/1","people":[]}),
        );
    });
    let (svc, dir, socket, _) = start(&url);
    assert_ne!(
        post(&socket, "/local/voiceprints", json!({"action":"list"})).0,
        200
    );
    assert_eq!(
        post(&socket, "/local/voiceprints", json!({"action":"list"})).0,
        200
    );
    let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
    stream.write_all(b"POST /local/voiceprints HTTP/1.1\r\nAuthorization: Bearer SYNTHETIC-external\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    server.join().unwrap();
    svc.stop();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn missing_forget_cache_releases_store_before_slow_model_http() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (svc, dir, _socket, id) = start(&url);
    let store = svc.store.clone();
    svc.stop();
    let (tx, rx) = std::sync::mpsc::channel();
    let (go, wait) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        request(&mut stream);
        tx.send(()).unwrap();
        wait.recv().unwrap();
        respond(&mut stream, json!({"people":[]}));
    });
    let caller_store = store.clone();
    let caller = std::thread::spawn(move || {
        candid_service::voiceprints::command(
            &caller_store,
            &json!({"action":"forget","analysis_id":"missing","confirmed":true}),
        )
    });
    rx.recv().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let query = std::thread::spawn(move || {
        let s = store.lock().unwrap();
        s.media_item(&id).unwrap();
        s.media_command(
            &json!({"action":"edit","id":id,"title":"SYNTHETIC while model busy","notes":"saved"}),
        )
        .unwrap();
        tx.send(()).unwrap();
    });
    let progress = rx.recv_timeout(std::time::Duration::from_secs(2));
    go.send(()).unwrap();
    assert!(progress.is_ok(), "slow model held media lock");
    assert!(caller.join().unwrap().is_ok());
    query.join().unwrap();
    server.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
#[ignore = "public isolated model acceptance on 18769; validates withdrawal, not recognition accuracy"]
fn public_two_sources_merge_then_withdraw_preserves_other_templates() {
    if !sample_ready() { return; }
    let (svc, dir, socket, _) = start("http://127.0.0.1:18769/v1");
    let a = uuid_v4();
    let b = uuid_v4();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../data/speaker-demo/public-audio/conversation.wav");
    if !path.exists() { eprintln!("SKIP: public sample {} not downloaded (see services/voice/README.md)", path.display()); return; }
    for id in [&a, &b] {
        assert_eq!(post(&socket,"/local/media",json!({"action":"import","id":id,"filename":"PUBLIC.wav","source":"import","kind":"audio","path":path})).0,200);
    }
    let store = svc.store.clone();
    svc.stop();
    let call = |v: Value| candid_service::voiceprints::command(&store, &v).unwrap();
    let ra = call(json!({"action":"analyze","id":a,"start_seconds":0.,"end_seconds":20.}));
    let rb = call(json!({"action":"analyze","id":b,"start_seconds":0.,"end_seconds":30.}));
    let ea = call(
        json!({"action":"enroll","analysis_id":ra["analysis"]["analysis_id"],"speaker_id":ra["analysis"]["speakers"][0]["speaker_id"],"name":"PUBLIC receipt A","confirmed":true}),
    );
    let eb = call(
        json!({"action":"enroll","analysis_id":rb["analysis"]["analysis_id"],"speaker_id":rb["analysis"]["speakers"][0]["speaker_id"],"person_id":ea["person"]["id"],"confirmed":true}),
    );
    let destination = call(
        json!({"action":"enroll","analysis_id":rb["analysis"]["analysis_id"],"speaker_id":rb["analysis"]["speakers"][1]["speaker_id"],"name":"PUBLIC receipt destination","confirmed":true}),
    );
    assert_eq!(ea["person"]["id"], eb["person"]["id"]);
    let expected = eb["receipt"]["templates"].as_array().unwrap().len()
        + destination["receipt"]["templates"]
            .as_array()
            .unwrap()
            .len();
    assert_ne!(
        ea["receipt"]["templates"][0]["source_hash"],
        eb["receipt"]["templates"][0]["source_hash"]
    );
    call(
        json!({"action":"merge","person_id":ea["person"]["id"],"target_id":destination["person"]["id"],"confirmed":true}),
    );
    // Analysis removal must retain the registration audit and receipt used below.
    call(json!({"action":"forget","analysis_id":ra["analysis"]["analysis_id"],"confirmed":true}));
    store
        .lock()
        .unwrap()
        .media_command(&json!({"action":"delete","id":a}))
        .unwrap();
    candid_service::voiceprints::reconcile(&store);
    candid_service::voiceprints::reconcile(&store);
    let remaining = call(json!({"action":"list"}));
    assert_eq!(remaining["people"].as_array().unwrap().len(), 1);
    assert_eq!(remaining["people"][0]["id"], destination["person"]["id"]);
    assert_eq!(
        remaining["people"][0]["sample_count"].as_u64().unwrap(),
        expected as u64,
        "{remaining}"
    );
    assert_eq!(remaining["cleanup_pending"], 0);
    call(json!({"action":"delete","person_id":destination["person"]["id"],"confirmed":true}));
    call(json!({"action":"forget","analysis_id":rb["analysis"]["analysis_id"],"confirmed":true}));
    assert!(call(json!({"action":"list"}))["people"]
        .as_array()
        .unwrap()
        .is_empty());
    std::fs::write("/tmp/candid-voiceprint-precise-withdrawal.json",json!({"scope":"public_two_different_processing_sources_merge_withdrawal","receipt_a_templates":ea["receipt"]["templates"].as_array().unwrap().len(),"retained_source_b_templates":expected,"merged_person_retained":true,"analysis_forget_retained_registration_audit":true,"retries":true,"remaining_people_after_explicit_cleanup":0}).to_string()).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permission_generation_ignores_sync_health_and_rejects_revoke_then_restore() {
    if !sample_ready() { return; }
    let (svc, dir, _socket, id) = start("http://127.0.0.1:1/v1");
    let store = svc.store.clone();
    svc.stop();
    let s = store.lock().unwrap();
    let snapshot = s.voiceprint_source(&id).unwrap();
    let source = s
        .sources()
        .unwrap()
        .into_iter()
        .find(|s| s.display_name == "资料库")
        .unwrap();
    s.set_source_status(&source.source_id, "error", Some("SYNTHETIC offline"))
        .unwrap();
    s.set_source_status(&source.source_id, "active", None)
        .unwrap();
    s.update_policy(&source.source_id, &source.policy).unwrap();
    assert!(s.voiceprint_check(&snapshot).is_ok());
    s.set_source_status(&source.source_id, "revoked", None)
        .unwrap();
    s.set_source_status(&source.source_id, "active", None)
        .unwrap();
    assert!(s.voiceprint_check(&snapshot).is_err());
    drop(s);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn old_service_keeps_precise_cleanup_pending_without_deleting_people() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (svc, dir, _socket, id) = start(&url);
    let store = svc.store.clone();
    svc.stop();
    {
        let s = store.lock().unwrap();
        let source = s.voiceprint_source(&id).unwrap();
        let p = s.voiceprint_provider().unwrap();
        let audit = json!({"provider":p,"source":source,"result":{"analysis":{"analysis_id":"old"}},"enrollments":[{"person":{"id":"old-person"}}]});
        s.voiceprint_enrollment(&audit).unwrap();
        s.media_command(&json!({"action":"delete","id":id}))
            .unwrap();
    }
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let b = request(&mut stream);
            assert!(b.starts_with(b"GET "));
            respond(
                &mut stream,
                json!({"format":"candid-voiceprints/1","people":[]}),
            );
        }
    });
    candid_service::voiceprints::reconcile(&store);
    let result =
        candid_service::voiceprints::command(&store, &json!({"action":"list"})).unwrap();
    assert_eq!(result["cleanup_pending"], 1);
    assert!(result["cleanup_message"].as_str().unwrap().contains("升级"));
    server.join().unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .voiceprint_cleanup_candidates()
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn late_enrollment_uses_receipt_withdrawal_and_preserves_cleanup_audit() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (svc, dir, _socket, id) = start(&url);
    let store = svc.store.clone();
    svc.stop();
    {
        let s = store.lock().unwrap();
        let source = s.voiceprint_source(&id).unwrap();
        let p = s.voiceprint_provider().unwrap();
        s.voiceprint_save("local",&json!({"format":"voiceprint-provenance/1","remote_id":"remote","provider":p,"source":source,"result":{"source":{"id":id},"analysis":{"analysis_id":"local"}},"enrollments":[]})).unwrap();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (go, wait) = std::sync::mpsc::channel();
    let receipt = json!({"format":"candid-voiceprint-enrollment/1","person_id":"SYNTHETIC-person","templates":[{"source_hash":"SYNTHETIC-source-a","start":0,"end":1,"model_id":"SYNTHETIC","model_revision":"1"}]});
    let expected = receipt.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(request(&mut stream).starts_with(b"GET"));
        respond(
            &mut stream,
            json!({"capabilities":["template_withdrawal"],"people":[]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        assert!(String::from_utf8_lossy(&request(&mut stream)).contains("enroll"));
        tx.send(()).unwrap();
        wait.recv().unwrap();
        respond(
            &mut stream,
            json!({"person":{"id":"SYNTHETIC-person"},"receipt":receipt,"people":[]}),
        );
        let (mut stream, _) = listener.accept().unwrap();
        let b = request(&mut stream);
        let body = b.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let v: Value = serde_json::from_slice(&b[body..]).unwrap();
        assert_eq!(v["action"], "withdraw");
        assert_eq!(v["receipt"], expected);
        respond(&mut stream, json!({"people":[],"removed_templates":1}));
    });
    let caller_store = store.clone();
    let caller = std::thread::spawn(move || {
        candid_service::voiceprints::command(
            &caller_store,
            &json!({"action":"enroll","analysis_id":"local","speaker_id":"speaker_01","name":"SYNTHETIC","confirmed":true}),
        )
    });
    rx.recv().unwrap();
    store
        .lock()
        .unwrap()
        .media_command(&json!({"action":"delete","id":id}))
        .unwrap();
    go.send(()).unwrap();
    assert!(caller.join().unwrap().is_err());
    server.join().unwrap();
    let pending = store
        .lock()
        .unwrap()
        .voiceprint_cleanup_candidates()
        .unwrap();
    let audit = pending
        .iter()
        .find(|v| v["format"] == "voiceprint-enrollment/1")
        .unwrap();
    assert!(audit["enrollments"][0]["receipt"].is_object());
    assert_eq!(audit["cleanup_required"], true);
    std::fs::remove_dir_all(dir).unwrap();
}

/// 「这是谁」 in a recording: the label is saved, and the speaker's longest line becomes library samples through
/// analyze → enroll → forget. Offline, the label is still saved without samples.
#[test]
fn labelling_a_speaker_in_a_recording_saves_samples_from_its_longest_line() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (svc, dir, _socket, id) = start(&url);
    let store = svc.store.clone();
    svc.stop();
    {
        let s = store.lock().unwrap();
        let mut c = config(&url);
        c["roles"].as_array_mut().unwrap().push(json!({"role":"asr","model":{"provider_id":"voice","model_id":"SYNTHETIC"}}));
        s.model_command(&json!({"action":"save","config":c})).unwrap();
        candid_store::testing::transcribe_recording(&s, &id, &[
            json!({"start_seconds":0.1,"end_seconds":0.4,"text":"SYNTHETIC a","speaker":"说话人 1"}),
            json!({"start_seconds":0.6,"end_seconds":0.7,"text":"SYNTHETIC b","speaker":"说话人 1"}),
            json!({"start_seconds":1.0,"end_seconds":1.3,"text":"SYNTHETIC c","speaker":"说话人 2"})]).unwrap();
    }
    let server = std::thread::spawn(move || {
        let mut seen = vec![];
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let bytes = request(&mut stream);
            let text = String::from_utf8_lossy(&bytes).to_string();
            if text.contains("/voiceprints/analyze") {
                seen.push("analyze");
                respond(&mut stream, analysis(&bytes));
            } else if text.starts_with("GET") {
                seen.push("list");
                respond(&mut stream, json!({"format":"candid-voiceprints/1","capabilities":["template_withdrawal"],"people":[]}));
            } else if text.contains("\"enroll\"") {
                assert!(text.contains("speaker_01") && text.contains("SYNTHETIC 王老师"));
                seen.push("enroll");
                respond(&mut stream, json!({"format":"candid-voiceprints/1","person":{"id":"voice-wang","name":"SYNTHETIC 王老师"},"receipt":{"format":"candid-voiceprint-enrollment/1","person_id":"voice-wang","templates":[{"source_hash":"SYNTHETIC","start":0,"end":1,"model_id":"SYNTHETIC","model_revision":"1"}]},"people":[]}));
            } else {
                assert!(text.contains("forget"));
                seen.push("forget");
                respond(&mut stream, json!({"format":"candid-voiceprints/1","people":[]}));
            }
        }
        seen
    });
    let item = candid_service::voiceprints::command(&store, &json!({"action":"label","id":id,"speaker":"说话人 1","name":"SYNTHETIC 王老师"})).unwrap();
    assert_eq!(server.join().unwrap(), vec!["analyze", "list", "enroll", "forget"]);
    let one = item["speakers"].as_array().unwrap().iter().find(|s| s["key"] == "说话人 1").unwrap();
    assert_eq!((one["status"].as_str(), one["name"].as_str(), one["samples_saved"].as_bool()), (Some("confirmed"), Some("SYNTHETIC 王老师"), Some(true)));
    // The voice service is gone now: the label still lands, without samples.
    let item = candid_service::voiceprints::command(&store, &json!({"action":"label","id":id,"speaker":"说话人 2","name":"SYNTHETIC 李娜"})).unwrap();
    let two = item["speakers"].as_array().unwrap().iter().find(|s| s["key"] == "说话人 2").unwrap();
    assert_eq!((two["status"].as_str(), two["samples_saved"].as_bool()), (Some("confirmed"), Some(false)));
    assert!(item["samples_error"].is_string());
    // Renaming in the library reaches the recording.
    store.lock().unwrap().voice_library_changed("rename", "voice-wang", &json!({"name":"SYNTHETIC 王主任"})).unwrap();
    let item = store.lock().unwrap().media_item(&id).unwrap();
    assert_eq!(item["speakers"][0]["name"], "SYNTHETIC 王主任");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Re-matching sends every stored speaker reference to the voice service and names recordings by the answer.
#[test]
fn rematch_names_earlier_recordings_from_the_current_library() {
    if !sample_ready() { return; }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (svc, dir, _socket, id) = start(&url);
    let store = svc.store.clone();
    svc.stop();
    {
        let s = store.lock().unwrap();
        let mut c = config(&url);
        c["roles"].as_array_mut().unwrap().push(json!({"role":"asr","model":{"provider_id":"voice","model_id":"SYNTHETIC"}}));
        s.model_command(&json!({"action":"save","config":c})).unwrap();
        candid_store::testing::transcribe_recording(&s, &id, &[
            json!({"start_seconds":0.1,"end_seconds":0.4,"text":"SYNTHETIC a","speaker":"说话人 1","voice_ref":"SYNTHETIC-ref"})]).unwrap();
    }
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let text = String::from_utf8_lossy(&request(&mut stream)).to_string();
        assert!(text.contains("\"rematch\"") && text.contains("SYNTHETIC-ref"));
        respond(&mut stream, json!({"format":"candid-voiceprints/1","people":[],"matches":{"SYNTHETIC-ref":{"status":"matched","person_id":"voice-1","name":"SYNTHETIC 甲","score":0.9,"margin":0.4}}}));
    });
    assert_eq!(candid_service::voiceprints::rematch(&store).unwrap(), 1);
    server.join().unwrap();
    let item = store.lock().unwrap().media_item(&id).unwrap();
    assert_eq!((item["speakers"][0]["status"].as_str(), item["speakers"][0]["name"].as_str()), (Some("auto"), Some("SYNTHETIC 甲")));
    std::fs::remove_dir_all(dir).unwrap();
}
