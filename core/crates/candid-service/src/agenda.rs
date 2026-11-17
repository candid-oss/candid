//! The AI page's 「今天」 column on the local channel (agent-platform-v1 §4.2, §4.3). App only: a request with a token
//! is refused, so neither Pi nor an external client can approve its own proposals.
//!
//! | Method and path | Purpose |
//! |---|---|
//! | `POST /local/agenda` `{"action":"today","since_ms"?}` | open and recently decided proposals, commitment candidates, latest day review |
//! | `POST /local/agenda` `{"action":"proposal","proposal_id","decision":"approve"\|"reject"\|"executed"\|"failed","receipt"?,"error"?}` | move a proposal; `executed` records the system's receipt |
//! | `POST /local/agenda` `{"action":"commitment","memory_id","decision":"confirm"\|"ignore","due"?}` | decide a candidate; confirm files a `create_reminder` proposal |

use crate::http::{HttpRequest, HttpResponse};
use crate::local::Core;
use crate::router::{bearer, lock, store_error};
use serde_json::Value;

/// Serves `/local/agenda`; `None` hands any other path back to the local channel.
pub(crate) fn route(core: &Core, request: &HttpRequest) -> Option<HttpResponse> {
    if request.path.split('?').next() != Some("/local/agenda") {
        return None;
    }
    if !bearer(request).is_empty() {
        return Some(HttpResponse::error(403, "internal_only", "此操作只供 App 使用。"));
    }
    if request.method != "POST" {
        return Some(HttpResponse::error(405, "method", "请用 POST。"));
    }
    let v: Value = match serde_json::from_slice(if request.body.is_empty() { b"{}" } else { &request.body }) {
        Ok(v) => v,
        Err(_) => return Some(HttpResponse::error(400, "json", "请求无效")),
    };
    let result = lock(&core.store).agenda_command(&v, &core.internal_client);
    Some(match result {
        Ok(value) => {
            if v["action"].as_str().unwrap_or("today") != "today" {
                core.feed.publish("data");
            }
            HttpResponse { status: 200, body: serde_json::to_vec(&value).unwrap_or_default() }
        }
        Err(e) => store_error(&e),
    })
}
