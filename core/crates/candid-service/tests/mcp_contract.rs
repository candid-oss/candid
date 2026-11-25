//! The MCP contract external agents depend on: the `initialize` answer (server instructions) and `tools/list` (every
//! tool's name, description and input schema), exactly as the MCP shell writes them. Snapshots live in
//! `core/fixtures/contract/`; any change fails here until regenerated with `UPDATE_SNAPSHOTS=1` and reviewed.
//! The query outputs on the lifecycle fixture are snapshotted by `tests/lifecycle.rs` (`fixtures/lifecycle/snapshots/mcp/`).
#[path = "support/snapshot.rs"]
mod snapshot;

use candid_service::{catalog, McpShell, ToolCallError};
use serde_json::{json, Value};

fn rpc(shell: &McpShell, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let out: Value = serde_json::from_str(&shell.handle(&line).unwrap()).unwrap();
    assert!(out["error"].is_null(), "{method}: {out}");
    out["result"].clone()
}

#[test]
fn mcp_initialize_and_tool_list_match_the_snapshots() {
    let shell = McpShell::new("contract-test", Box::new(|_, _| Err(ToolCallError::new(500, "no service in this test"))));
    let mut snaps = snapshot::Snapshots::default();
    snaps.json("contract/mcp-initialize.json", &rpc(&shell, "initialize", json!({"protocolVersion": "2025-06-18"})));
    let list = rpc(&shell, "tools/list", json!({}));
    snaps.json("contract/mcp-tools.json", &list);

    // Every listed tool has an object schema that parses, and the list is the store's public tool set, in order.
    let names: Vec<&str> = list["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, candid_store::TOOL_NAMES, "catalog and store disagree on the public tools");
    for t in catalog::tools() {
        let schema: Value = serde_json::from_str(t.input_schema).unwrap_or_else(|e| panic!("{}: schema is not JSON: {e}", t.name));
        assert_eq!(schema["type"], "object", "{}", t.name);
    }
    snaps.finish();
}
