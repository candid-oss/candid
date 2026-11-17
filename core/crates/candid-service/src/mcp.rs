//! A Model Context Protocol server over newline-delimited JSON-RPC 2.0 (stdio transport). It holds no data: every
//! `tools/call` is forwarded to the timeline service through `call`, so the service's token, scope and audit apply.

use crate::catalog;
use crate::client::ToolCallError;
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};

pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_LINE: usize = 4 * 1024 * 1024;

/// Calls a tool on the service; returns the response body (JSON) or the service's error.
pub type Call = Box<dyn Fn(&str, &[u8]) -> Result<Vec<u8>, ToolCallError> + Send + Sync>;

pub struct McpShell {
    server_version: String,
    call: Call,
}

struct RpcError {
    code: i64,
    message: String,
}

impl McpShell {
    pub fn new(server_version: &str, call: Call) -> McpShell {
        McpShell { server_version: server_version.into(), call }
    }

    /// Handles one incoming line. Returns the line to write back, or `None` for notifications.
    pub fn handle(&self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }
        let message = match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(m)) => m,
            _ => return Some(encode(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}))),
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return id.map(|id| encode(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "Invalid request"}})));
        };
        let params = message.get("params").and_then(Value::as_object).cloned().unwrap_or_default();
        let id = id?; // notifications (initialized, cancelled) need no answer
        Some(match self.respond(method, &params) {
            Ok(result) => encode(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            Err(e) => encode(json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}})),
        })
    }

    fn respond(&self, method: &str, params: &Map<String, Value>) -> Result<Value, RpcError> {
        match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(SUPPORTED_VERSIONS[0]);
                let version = if SUPPORTED_VERSIONS.contains(&requested) { requested } else { SUPPORTED_VERSIONS[0] };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "candid", "title": "Candid 时间线", "version": self.server_version},
                    "instructions": catalog::INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": catalog::tools().iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": serde_json::from_str::<Value>(t.input_schema).unwrap_or_else(|_| json!({"type": "object"})),
                "annotations": {"readOnlyHint": !t.writes, "openWorldHint": false},
            })).collect::<Vec<_>>()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).filter(|n| catalog::tool(n).is_some());
                let Some(name) = name else {
                    return Err(RpcError { code: -32602, message: "Unknown tool".into() });
                };
                let arguments = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
                let body = serde_json::to_vec(&Value::Object(arguments)).unwrap_or_else(|_| b"{}".to_vec());
                match (self.call)(name, &body) {
                    Ok(response) => {
                        let text = String::from_utf8_lossy(&response).into_owned();
                        let mut result = json!({"content": [{"type": "text", "text": text}], "isError": false});
                        if let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(&response) {
                            result["structuredContent"] = Value::Object(o);
                        }
                        Ok(result)
                    }
                    // Tool failures (bad arguments, denied by scope) go back to the model as a result it can read and correct.
                    Err(e) => Ok(json!({"content": [{"type": "text", "text": e.message}], "isError": true})),
                }
            }
            other => Err(RpcError { code: -32601, message: format!("Method not found: {other}") }),
        }
    }

    /// Reads requests from `input` until it closes, writing each response to `output` on its own line.
    pub fn run(&self, input: impl BufRead, mut output: impl Write) {
        let mut input = input;
        let mut buffer: Vec<u8> = Vec::new();
        let mut dropping = false;
        loop {
            let chunk = match input.fill_buf() {
                Ok([]) => break,
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break, // stdin closed with an error: the client is gone
            };
            let (used, line_done) = match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (chunk.len(), false),
            };
            if !dropping {
                buffer.extend_from_slice(&chunk[..if line_done { used - 1 } else { used }]);
            }
            input.consume(used);
            if buffer.len() > MAX_LINE {
                buffer.clear(); // drop an unbounded line instead of growing forever
                dropping = true;
            }
            if line_done {
                if !dropping {
                    let line = String::from_utf8_lossy(&buffer).into_owned();
                    if let Some(reply) = self.handle(&line) {
                        if output.write_all(reply.as_bytes()).and_then(|_| output.write_all(b"\n")).and_then(|_| output.flush()).is_err() {
                            break;
                        }
                    }
                }
                buffer.clear();
                dropping = false;
            }
        }
    }
}

fn encode(v: Value) -> String {
    serde_json::to_string(&v).unwrap_or_else(|_| r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"encode failed"}}"#.into())
}
