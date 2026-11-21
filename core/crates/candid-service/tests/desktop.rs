//! Direct desktop capture over the local channel: the cutover from the ledger (pass and fail), direct ingest with
//! staged screenshots (normal, repeated, detached parent), the App's read tools (activities, detail paging, search,
//! media paths, recognition, pending work), deletion and retention, and the tools staying off the loopback HTTP.
//! Synthetic data only.
use candid_service::local::local_request;
use candid_service::serve::{ServeOptions, Service};
use candid_store::fixtures::{ledger_db, make_synthetic_ledger};
use candid_store::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Fixture {
    dir: PathBuf,
    root: PathBuf,
    run: PathBuf,
    socket: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(label: &str) -> Fixture {
    let dir = PathBuf::from(format!("/tmp/pv-{}-{}", label, &uuid_v4()[..8]));
    std::fs::create_dir_all(&dir).unwrap();
    let run = dir.join("run");
    Fixture { root: dir.join("timeline"), socket: run.join("core.sock"), run, dir }
}

fn options(f: &Fixture, ledger: Option<&Path>) -> ServeOptions {
    let mut o = ServeOptions::new(f.root.clone(), f.run.clone());
    o.created_by = "candid/test".into();
    o.socket = Some(f.socket.clone());
    o.ledger = ledger.map(Path::to_path_buf);
    o.sync_interval = Duration::from_secs(3600);
    o
}

fn call(socket: &Path, method: &str, path: &str, body: Value) -> (u16, Value) {
    let data = if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() };
    let (status, raw) = local_request(socket, method, path, &data).unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn ok(socket: &Path, path: &str, body: Value) -> Value {
    let (s, v) = call(socket, "POST", path, body);
    assert_eq!(s, 200, "{path}: {v}");
    v
}

fn desktop_source(socket: &Path) -> String {
    let policy = json!({"version": 1, "store_text": true, "store_media": true, "agent_visible": true, "exclude": {"apps": [], "threads": [], "patterns": []}});
    ok(socket, "/local/sources", json!({"kind": "mac.desktop", "display_name": "Mac 桌面", "policy": policy}))["source_id"].as_str().unwrap().to_string()
}

/// Writes bytes into the core's staging directory the way the App does (temporary name, then rename).
fn stage(socket: &Path, name: &str, bytes: &[u8]) -> (String, String) {
    let (_, health) = call(socket, "GET", "/health", Value::Null);
    let dir = PathBuf::from(health["staging_dir"].as_str().unwrap());
    let tmp = dir.join(format!(".{name}.part"));
    std::fs::write(&tmp, bytes).unwrap();
    let path = dir.join(name);
    std::fs::rename(&tmp, &path).unwrap();
    (path.display().to_string(), sha256_hex(bytes))
}

const T: i64 = 1_800_000_000_000;

fn context(key: &str, at: i64, activity: &str, title: &str, body: &str) -> Value {
    json!({"source_key": key, "kind": "screen.text", "start_ms": at, "app_bundle_id": "com.example.notes", "app_name": "Notes", "title": title,
        "payload": {"trigger": "timer", "text_status": "captured", "focused_role": "AXTextArea", "legacy": {"run_id": "run-1", "activity_id": activity}},
        "text": [{"field": "body", "content": body}]})
}

fn screenshot(socket: &Path, key: &str, at: i64, parent: &str, activity: &str, bytes: &[u8]) -> Value {
    let (path, sha) = stage(socket, &format!("app-{key}.jpg"), bytes);
    json!({"source_key": key, "kind": "screen.shot", "start_ms": at, "app_bundle_id": "com.example.notes", "app_name": "Notes", "title": "合成草稿",
        "parent_key": parent, "payload": {"trigger": "timer", "pixel_w": 2, "pixel_h": 2, "legacy": {"run_id": "run-1", "activity_id": activity}},
        "blobs": [{"staging_path": path, "sha256": sha, "role": "original", "mime_type": "image/jpeg", "pixel_w": 2, "pixel_h": 2}]})
}

#[test]
fn cutover_imports_the_rest_checks_hashes_then_stops_mirroring() {
    let f = fixture("cut");
    let ledger = make_synthetic_ledger().unwrap();
    let service = Service::start(&options(&f, Some(&ledger.root)), |_| {}).unwrap();
    let src = desktop_source(&f.socket);
    // Before the cutover the App's desktop batches wait.
    let (code, v) = call(&f.socket, "POST", "/local/ingest", json!({"source_id": src, "events": [context("early", T, "a", "合成", "正文")]}));
    assert_eq!((code, v["error"]["code"].as_str()), (409, Some("cutover_pending")));
    let (code, cut) = call(&f.socket, "POST", "/local/desktop/cutover", Value::Null);
    assert_eq!(code, 200, "{cut}");
    assert_eq!(cut["mode"], "direct");
    assert_eq!(cut["cutover"]["verification"]["ok"], true);
    assert_eq!(cut["cutover"]["screenshots"], 1);
    assert_eq!(cut["cutover"]["records"], 9);
    let status = call(&f.socket, "GET", "/local/status", Value::Null).1;
    assert_eq!(status["capture_mode"], "direct");
    assert!(status["cutover_error"].is_null());
    // A record the ledger gains afterwards is no longer mirrored: the ledger is a read-only backup now.
    let db = ledger_db(&ledger.root).unwrap();
    let mut late = ledger.records[5].clone();
    late.id = "rec-after".into();
    candid_store::fixtures::ledger_append(&db, &late).unwrap();
    drop(db);
    let before = status["events"].as_i64().unwrap();
    service.core.sync_pass();
    assert_eq!(call(&f.socket, "GET", "/local/status", Value::Null).1["events"].as_i64().unwrap(), before);
    // Direct writes are accepted now; a second cutover is a no-op that returns the same record.
    assert_eq!(ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": [context("c1", T, "a", "合成", "正文")]}))["inserted"], 1);
    assert_eq!(ok(&f.socket, "/local/desktop/cutover", Value::Null)["cutover"], cut["cutover"]);
    // The mode survives a restart, with the cutover's verification in the status.
    service.stop();
    let service = Service::start(&options(&f, Some(&ledger.root)), |_| {}).unwrap();
    let status = call(&f.socket, "GET", "/local/status", Value::Null).1;
    assert_eq!(status["capture_mode"], "direct");
    assert_eq!(status["verification"]["ok"], true);
    service.stop();
    let _ = std::fs::remove_dir_all(&ledger.root);
}

#[test]
fn a_failed_check_keeps_the_ledger_mode_and_says_why() {
    let f = fixture("cutfail");
    let ledger = make_synthetic_ledger().unwrap();
    let service = Service::start(&options(&f, Some(&ledger.root)), |_| {}).unwrap();
    // Mirror once, then damage the imported screenshot: the hash check must refuse the switch.
    service.core.sync_pass();
    let rel: String = service
        .store
        .lock()
        .unwrap()
        .raw_db()
        .first("SELECT rel_path FROM blobs WHERE mime_type='image/jpeg'", &[], |r| candid_store::testing::RowExt::text(r, 0))
        .unwrap()
        .unwrap();
    std::fs::write(f.root.join(rel), b"damaged").unwrap();
    let (code, v) = call(&f.socket, "POST", "/local/desktop/cutover", Value::Null);
    assert_eq!(code, 409, "{v}");
    assert_eq!(v["mode"], "ledger");
    assert!(v["error"]["message"].as_str().unwrap().contains("hash 不一致"), "{v}");
    let status = call(&f.socket, "GET", "/local/status", Value::Null).1;
    assert_eq!(status["capture_mode"], "ledger");
    assert!(status["cutover_error"].as_str().unwrap().starts_with("未切换到直写"));
    let src = desktop_source(&f.socket);
    assert_eq!(call(&f.socket, "POST", "/local/ingest", json!({"source_id": src, "events": [context("c1", T, "a", "合成", "正文")]})).0, 409);
    // The App reads the ledger's capture preferences without touching the ledger (the synthetic one has none).
    assert_eq!(call(&f.socket, "GET", "/local/desktop/legacy-preferences", Value::Null).0, 404);
    service.stop();
    let _ = std::fs::remove_dir_all(&ledger.root);
}

#[test]
fn direct_capture_reads_back_as_activities_with_pages_search_and_paths() {
    let f = fixture("app");
    let service = Service::start(&options(&f, None), |_| {}).unwrap();
    ok(&f.socket, "/local/desktop/cutover", Value::Null);
    let src = desktop_source(&f.socket);
    let image = vec![7u8; 64];
    let mut events = vec![
        json!({"source_key": "app-1", "kind": "app.focus", "start_ms": T, "app_bundle_id": "com.example.notes", "app_name": "Notes",
            "payload": {"reason": "application", "legacy": {"run_id": "run-1", "activity_id": "app-1"}}}),
    ];
    for i in 0..5 {
        events.push(context(&format!("c{i}"), T + 1000 * (i + 1), "act-1", "合成草稿", &format!("合成段落 {i}：预算复核")));
    }
    events.push(screenshot(&f.socket, "s1", T + 3500, "c2", "act-1", &image));
    events.push(json!({"source_key": "g1", "kind": "coverage.gap", "start_ms": T + 3600, "parent_key": "c2",
        "payload": {"reason_code": "unknown", "legacy": {"run_id": "run-1", "activity_id": "act-1", "detail": "截图未保存：合成原因"}}}));
    events.push(context("other", T + 9000, "act-2", "另一个窗口", "无关内容"));
    let r = ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": events.clone(), "on_missing_parent": "detach"}));
    assert_eq!(r["inserted"], 9, "{r}");
    // Re-sending the same batch (a retry after a lost answer) changes nothing; the staged copies are removed.
    let mut again = events.clone();
    again[6] = screenshot(&f.socket, "s1", T + 3500, "c2", "act-1", &image);
    let r = ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": again, "on_missing_parent": "detach"}));
    assert_eq!((r["inserted"].as_i64(), r["duplicates"].as_i64(), r["conflicts"].as_i64()), (Some(0), Some(9), Some(0)));
    assert_eq!(std::fs::read_dir(f.root.join("staging")).unwrap().count(), 0);
    // A child whose parent never arrived is kept without the link.
    let r = ok(&f.socket, "/local/ingest", json!({"source_id": src, "on_missing_parent": "detach",
        "events": [{"source_key": "g2", "kind": "coverage.gap", "start_ms": T + 9500, "parent_key": "withheld",
            "payload": {"reason_code": "unknown", "legacy": {"run_id": "run-1"}}}]}));
    assert_eq!(r["inserted"], 1);
    let day = ok(&f.socket, "/v1/app_activities", json!({"start_ms": T - 1000, "end_ms": T + 86_400_000}));
    let acts = day["activities"].as_array().unwrap();
    assert_eq!(acts.len(), 4, "{day}");
    assert_eq!(acts[0]["anchor"]["id"], "g2", "newest end first");
    let act = acts.iter().find(|a| a["anchor"]["id"] == "c0").unwrap();
    assert_eq!((act["sample_count"].as_i64(), act["has_screenshot"].as_bool(), act["has_issue"].as_bool()), (Some(5), Some(true), Some(true)));
    assert_eq!(act["latest"]["id"], "c4");
    assert_eq!(act["latest"]["context"]["readable_text"], "合成段落 4：预算复核");
    assert_eq!(day["stats"]["record_count"], 10);
    assert_eq!(day["stats"]["screenshot_count"], 1);
    assert_eq!(day["stats"]["storage_bytes"], 64);
    // Search matches text and titles; two-character Chinese words work.
    let found = ok(&f.socket, "/v1/app_activities", json!({"start_ms": T - 1000, "end_ms": T + 86_400_000, "q": "预算"}));
    assert_eq!(found["activities"].as_array().unwrap().len(), 1);
    assert_eq!(ok(&f.socket, "/v1/app_activities", json!({"start_ms": T - 1000, "end_ms": T + 86_400_000, "q": "另一个"}))["activities"].as_array().unwrap().len(), 1);
    // Detail: newest page by default, then the page before, around a chosen observation.
    let page = ok(&f.socket, "/v1/app_activity", json!({"id": "c0", "limit": 2}))["activity"].clone();
    assert_eq!(page["observations"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(), ["c3", "c4"]);
    assert_eq!(page["observation_offset"], 3);
    let earlier = ok(&f.socket, "/v1/app_activity", json!({"id": "c0", "before": "c3", "limit": 2}))["activity"].clone();
    assert_eq!(earlier["observations"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(), ["c1", "c2"]);
    let shot = &earlier["screenshots"][0];
    assert_eq!((shot["id"].as_str(), shot["parent_id"].as_str()), (Some("s1"), Some("c2")));
    assert_eq!(shot["image_hash"], sha256_hex(&image));
    let path = shot["image_path"].as_str().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), image, "the App reads the original by path");
    assert_eq!(shot["context"]["readable_text"], "合成段落 2：预算复核", "a screenshot carries its observation's context");
    assert_eq!(earlier["issues"][0]["detail"], "截图未保存：合成原因");
    let around = ok(&f.socket, "/v1/app_activity", json!({"id": "c0", "around": "c1", "limit": 3}))["activity"].clone();
    assert_eq!(around["observation_offset"], 0);
    assert_eq!(call(&f.socket, "POST", "/v1/app_activity", json!({"id": "c0", "before": "other"})).0, 400);
    assert!(ok(&f.socket, "/v1/app_activity", json!({"id": "missing"}))["activity"].is_null());
    let media = ok(&f.socket, "/v1/app_media", json!({"sha256": [sha256_hex(&image), "0".repeat(64)]}));
    assert_eq!(media["paths"].as_object().unwrap().len(), 1);
    // Only the App may call these: a token on the local channel and the loopback HTTP are refused.
    let (code, _) = local_request_with_token(&f.socket, "/v1/app_activities", b"{}");
    assert_eq!(code, 403);
    let port = service.port();
    assert_eq!(loopback_status(port, "/v1/app_activities"), 404);
    // Over the budget, only screenshots older than 7 days lose their original: this one is newer, so it stays.
    let freed = ok(&f.socket, "/local/desktop/free-space", json!({"target_bytes": 0}));
    assert_eq!((freed["screenshots"].as_i64(), freed["reached_target"].as_bool()), (Some(0), Some(false)), "{freed}");
    assert_eq!(call(&f.socket, "POST", "/local/desktop/free-space", json!({"target_bytes": -1})).0, 400);
    // Retention counts, then deletes, records before a time; deleting an activity removes all of it.
    assert_eq!(ok(&f.socket, "/v1/app_retention_count", json!({"before_ms": T + 2500}))["records"], 3);
    assert_eq!(ok(&f.socket, "/local/desktop/retention", json!({"before_ms": T + 500}))["deleted"], 1);
    let deleted = ok(&f.socket, "/local/desktop/delete", json!({"activity_id": "c0"}))["deleted"].as_i64().unwrap();
    assert_eq!(deleted, 7, "five observations, the screenshot and the gap");
    let day = ok(&f.socket, "/v1/app_activities", json!({"start_ms": T - 1000, "end_ms": T + 86_400_000}));
    assert_eq!(day["activities"].as_array().unwrap().len(), 2, "{day}");
    assert_eq!(day["stats"]["storage_bytes"], 0, "the unreferenced original is gone");
    // A record of a deleted activity sent late (still queued in the App) is not brought back.
    let r = ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": [context("c1", T + 2000, "act-1", "合成草稿", "合成段落 1：预算复核")]}));
    assert_eq!(r["tombstoned"], 1);
    service.stop();
}

#[test]
fn recognition_runs_are_read_revised_and_found_by_hash() {
    let f = fixture("rec");
    let service = Service::start(&options(&f, None), |_| {}).unwrap();
    ok(&f.socket, "/local/desktop/cutover", Value::Null);
    let src = desktop_source(&f.socket);
    let image = vec![9u8; 32];
    let events = vec![context("c1", T, "act-1", "合成", "正文"), screenshot(&f.socket, "s1", T + 10, "c1", "act-1", &image)];
    ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": events}));
    let pending = ok(&f.socket, "/v1/app_pending_recognition", json!({"method": "ocr", "since_ms": 0}));
    assert_eq!(pending["screenshot"]["id"], "s1");
    assert_eq!(ok(&f.socket, "/v1/app_pending_recognition", json!({"method": "ocr", "since_ms": 0, "skip": ["s1"]}))["screenshot"], Value::Null);
    let info = ok(&f.socket, "/v1/app_recognition", json!({"screenshot_id": "s1"}));
    let shot_event = info["screenshot_event_id"].as_str().unwrap().to_string();
    assert_eq!(std::fs::read(info["image_path"].as_str().unwrap()).unwrap(), image);
    let run = |completed: i64, body: &str| {
        let result = serde_json::to_vec(&json!({"source": "合成 OCR", "text": body, "completedAt": 0, "usedOCRReference": false})).unwrap();
        let (path, sha) = stage(&f.socket, &format!("app-r{completed}.json"), &result);
        json!({"source_key": format!("recognition:s1:ocr@{completed}"), "kind": "image.text", "start_ms": T + 10, "parent_key": "s1",
            "payload": {"image_event_id": shot_event, "method": "ocr", "model_id": "合成 OCR", "completed_ms": completed, "result_ref": sha},
            "text": [{"field": "ocr", "content": body}],
            "blobs": [{"staging_path": path, "sha256": sha, "role": "derived", "mime_type": "application/json"}]})
    };
    ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": [run(T + 100, "第一次识别")]}));
    assert_eq!(ok(&f.socket, "/v1/app_pending_recognition", json!({"method": "ocr", "since_ms": 0}))["screenshot"], Value::Null);
    assert_eq!(ok(&f.socket, "/v1/app_pending_recognition", json!({"method": "vlm", "since_ms": 0}))["screenshot"]["id"], "s1");
    ok(&f.socket, "/local/ingest", json!({"source_id": src, "events": [run(T + 200, "第二次识别")]}));
    let info = ok(&f.socket, "/v1/app_recognition", json!({"screenshot_id": "s1"}));
    assert_eq!(info["results"][0]["text"], "第二次识别");
    assert!(std::fs::read(info["results"][0]["result_path"].as_str().unwrap()).is_ok());
    let older = info["older"].as_array().unwrap().clone();
    assert_eq!(older.len(), 1);
    // The revision supersedes the earlier run (a tombstone, so it cannot come back).
    assert_eq!(ok(&f.socket, "/local/desktop/delete", json!({"event_ids": older, "reason": "superseded"}))["deleted"], 1);
    assert!(ok(&f.socket, "/v1/app_recognition", json!({"screenshot_id": "s1"}))["older"].as_array().unwrap().is_empty());
    assert_eq!(ok(&f.socket, "/v1/app_ocr", json!({"sha256": sha256_hex(&image)}))["result"]["text"], "第二次识别");
    // The App may delete only its own capture.
    assert_eq!(call(&f.socket, "POST", "/local/desktop/delete", json!({"event_ids": ["x"], "reason": "retention"})).0, 400);
    // Deleting the screenshot by its record ID takes its recognition with it.
    assert_eq!(ok(&f.socket, "/local/desktop/delete", json!({"record_ids": ["s1"], "reason": "user"}))["deleted"], 2);
    assert_eq!(call(&f.socket, "POST", "/v1/app_recognition", json!({"screenshot_id": "s1"})).0, 404);
    service.stop();
}

fn local_request_with_token(socket: &Path, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    use std::io::{Read, Write};
    let mut s = std::os::unix::net::UnixStream::connect(socket).unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer abc\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let status = String::from_utf8_lossy(&raw).split(' ').nth(1).unwrap().parse().unwrap();
    (status, raw)
}

fn loopback_status(port: u16, path: &str) -> u16 {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer abc\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
    s.write_all(head.as_bytes()).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    String::from_utf8_lossy(&raw).split(' ').nth(1).unwrap().parse().unwrap()
}
