//! Synthetic App edit -> authenticated HTTP -> MCP read acceptance.
use candid_service::*;
use candid_store::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
#[test]
fn mcp_reads_manual_metadata_and_original_words_but_cannot_edit() {
    let root = candid_store::fixtures::temp_root("annotation-mcp");
    let store = TimelineStore::open(&root, "SYNTHETIC", true, Clock::default()).unwrap();
    let source = store
        .register_source("mac.desktop", "SYNTHETIC", &SourcePolicy::new(true, false))
        .unwrap();
    let app = store
        .register_client("App", "internal", &ClientScope::unbounded(), None)
        .unwrap()
        .client_id;
    let external = store
        .register_client("SYNTHETIC MCP", "mcp", &ClientScope::unbounded(), None)
        .unwrap();
    let mut event = IngestEvent::new("SYNTHETIC", "screen.text", Some(1000));
    event.title = Some("Original window".into());
    event.text = vec![IngestText::new("body", "SYNTHETIC original words")];
    store
        .ingest(&IngestBatch::events(&source, vec![event]))
        .unwrap();
    let id = store.event_id(&source, "SYNTHETIC").unwrap().unwrap();
    store.record_management(&json!({"action":"annotation_save","event_ids":[id],"request_id":"SYNTHETIC","title":"Manual title","notes":"manualnoteneedle"}),&app).unwrap();
    let store = Arc::new(Mutex::new(store));
    let router = Router::new(store.clone());
    let server = LoopbackServer::start(0, 2, Arc::new(move |r, p| router.handle(r, p))).unwrap();
    let client = ServiceClient::new(
        &format!("http://127.0.0.1:{}", server.port),
        external.token.as_ref().unwrap(),
    );
    let shell = McpShell::new(
        "SYNTHETIC",
        Box::new(move |tool, body| client.call(tool, body)),
    );
    let call = |name: &str, args: Value| -> Value {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        serde_json::from_str(&shell.handle(&request.to_string()).unwrap()).unwrap()
    };
    let detail = call("get_event", json!({"event_id":id}));
    assert_eq!(
        detail["result"]["structuredContent"]["event"]["title"],
        "Manual title"
    );
    assert_eq!(
        detail["result"]["structuredContent"]["payload"]["manual_metadata"]["notes"],
        "manualnoteneedle"
    );
    assert_eq!(
        detail["result"]["structuredContent"]["paragraphs"][0]["text"],
        "SYNTHETIC original words"
    );
    let search = call("search", json!({"q":"manualnoteneedle"}));
    assert_eq!(
        search["result"]["structuredContent"]["hits"][0]["event_id"],
        id
    );
    assert!(call("annotation_save", json!({}))["error"].is_object());
    store
        .lock()
        .unwrap()
        .set_source_status(&source, "revoked", None)
        .unwrap();
    assert_eq!(
        call("get_event", json!({"event_id":id}))["result"]["isError"],
        true
    );
    assert!(
        call("search", json!({"q":"manualnoteneedle"}))["result"]["structuredContent"]["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(shell);
    drop(server);
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}
