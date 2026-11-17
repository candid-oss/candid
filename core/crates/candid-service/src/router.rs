//! `POST /v1/<tool>` → the store's query tools (timeline-v1 §5). Every request needs a client's Bearer token; the
//! `Host` must be the loopback address with this port (no DNS rebinding) and browser requests (`Origin`) are refused.
//! The store applies the client's scope before ranking and writes `access_log` for every call, denied ones included.
//! Administration, the change feed, health and ingest (`/local/…`, `/health`) exist only on the App's local channel
//! ([`crate::local`]) and are refused here whatever the token.

use crate::catalog;
use crate::http::{HttpRequest, HttpResponse, MAX_BODY};
use candid_store::{TimelineError, TimelineStore, ToolCallFailure};
use std::sync::{Arc, Mutex};

pub type SharedStore = Arc<Mutex<TimelineStore>>;

#[derive(Clone)]
pub struct Router {
    pub store: SharedStore,
}

/// Request decoding errors in the Swift router's wording.
pub fn describe(e: &serde_json::Error) -> String {
    let msg = e.to_string();
    let base = msg.split(" at line ").next().unwrap_or(&msg).to_string();
    if let Some(rest) = base.strip_prefix("missing field `") {
        return format!("缺少 {}", rest.trim_end_matches('`'));
    }
    if base.starts_with("invalid type") || base.starts_with("invalid value") {
        return format!("类型不对（{base}）");
    }
    base
}

/// Paths only the App's local channel serves.
pub fn is_local_only(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or("");
    p == "/health" || p == "/local" || p.starts_with("/local/")
}

pub(crate) fn lock(store: &SharedStore) -> std::sync::MutexGuard<'_, TimelineStore> {
    match store.lock() {
        Ok(s) => s,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The bearer token of a request, empty when absent.
pub(crate) fn bearer(request: &HttpRequest) -> String {
    let auth = request.headers.get("authorization").cloned().unwrap_or_default();
    if auth.to_lowercase().starts_with("bearer ") {
        auth.chars().skip(7).collect::<String>().trim().to_string()
    } else {
        String::new()
    }
}

/// Runs one tool for a resolved client; both channels answer through this, so scope filtering and audit are shared.
pub(crate) fn run_tool(store: &TimelineStore, tool: &str, body: &[u8], client: &str) -> HttpResponse {
    match store.call_tool(tool, body, client).and_then(|v| serde_json::to_vec(&v).map_err(|e| ToolCallFailure::Store(TimelineError::Database(e.to_string())))) {
        Ok(data) => HttpResponse { status: 200, body: data },
        Err(ToolCallFailure::Decode(d)) => HttpResponse::error(400, "invalid_input", &format!("参数不符合 {tool} 的格式：{}", describe(&d))),
        Err(ToolCallFailure::Store(e)) => store_error(&e),
    }
}

pub(crate) fn store_error(e: &TimelineError) -> HttpResponse {
    let status = match e {
        TimelineError::Denied(_) => 403,
        TimelineError::NotFound(_) | TimelineError::UnknownSource(_) => 404,
        TimelineError::InvalidInput(_) | TimelineError::InvalidCursor => 400,
        TimelineError::UnknownClient(_) => 401,
        TimelineError::SourceNotWritable(_) => 409,
        _ => 500,
    };
    HttpResponse::error(status, &e.code(), &e.message())
}

impl Router {
    pub fn new(store: SharedStore) -> Router {
        Router { store }
    }

    pub fn handle(&self, request: &HttpRequest, port: u16) -> HttpResponse {
        let host = request.headers.get("host").map(|h| h.to_lowercase()).unwrap_or_default();
        if ![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")].contains(&host) {
            return HttpResponse::error(403, "bad_host", "只接受本机回环地址的请求。");
        }
        if let Some(origin) = request.headers.get("origin").filter(|o| !o.is_empty()) {
            return HttpResponse::error(403, "browser_origin", &format!("不接受浏览器发起的请求（Origin: {origin}）。"));
        }
        if is_local_only(&request.path) {
            return HttpResponse::error(403, "local_only", "这个接口只在 Candid App 的本机通道上提供。");
        }
        if request.method != "POST" {
            return HttpResponse::error(405, "method", "请用 POST /v1/<工具名>。");
        }
        let path = request.path.split('?').next().unwrap_or("");
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let tool = if parts.len() == 2 && parts[0] == "v1" { catalog::tool(parts[1]) } else { None };
        let Some(tool) = tool else {
            return HttpResponse::error(404, "unknown_tool", &format!("没有这个工具：{}", request.path));
        };
        if request.body.len() > MAX_BODY {
            return HttpResponse::error(413, "too_large", "请求体超过 1 MiB。");
        }
        let token = bearer(request);
        let store = lock(&self.store);
        let client = if token.is_empty() { None } else { store.client_id_for_token(&token).ok().flatten() };
        let Some(client) = client else {
            return HttpResponse::error(401, "unauthorized", "访问令牌无效或已撤销。");
        };
        run_tool(&store, tool.name, &request.body, &client)
    }
}
