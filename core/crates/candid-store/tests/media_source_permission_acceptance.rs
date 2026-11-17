//! Different capture sources must retain distinct external-query permissions.
mod common;
use candid_store::*;
use serde_json::json;

#[test]
fn importing_permission_does_not_grant_access_to_microphone_recordings() {
    let fx = common::Fx::make("media-source-permission");
    let path = fx.root.join("SYNTHETIC.wav");
    std::fs::write(&path, b"RIFF\x26\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x02\0\0\0\0\0").unwrap();
    fx.store.media_command(&json!({"action":"import","id":"30000000-0000-4000-8000-000000000001","filename":"SYNTHETIC microphone.wav","kind":"audio","source":"mac_microphone","path":path})).unwrap();
    let imports = fx.store.sources().unwrap().into_iter().find(|source| source.kind == "media.import")
        .map(|source| source.source_id).unwrap_or_else(|| fx.store.register_source("media.import", "SYNTHETIC imported files", &SourcePolicy::default()).unwrap());
    let client = fx.store.register_client("SYNTHETIC imported-files reader", "http", &ClientScope {
        sources: vec![imports], ..ClientScope::unbounded()
    }, None).unwrap();
    let response = fx.store.call_tool("search", br#"{"q":"SYNTHETIC microphone","attention":"all"}"#, &client.client_id).unwrap();
    assert!(response["hits"].as_array().unwrap().is_empty(), "a microphone record must not inherit a file-import authorization");
    let microphone_source = fx.store.sources().unwrap().into_iter().find(|source| source.kind == "mac.microphone")
        .expect("the native microphone needs its own source identity").source_id;
    let microphone = fx.store.register_client("SYNTHETIC microphone reader", "http", &ClientScope {
        sources: vec![microphone_source], ..ClientScope::unbounded()
    }, None).unwrap();
    let response = fx.store.call_tool("search", br#"{"q":"SYNTHETIC microphone","attention":"all"}"#, &microphone.client_id).unwrap();
    assert!(!response["hits"].as_array().unwrap().is_empty());
}
