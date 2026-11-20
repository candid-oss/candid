//! The attached Pi agent's side of the local channel (agent-platform-v1 §4.1). The App starts a run, the core
//! registers (or rotates) the dedicated client `candid-pi` with a scope frozen for that run, and Pi's extension
//! calls back with that client's token. Everything Pi can do passes through here:
//!
//! | Method and path | Who | Purpose |
//! |---|---|---|
//! | `POST /local/pi/start` | App (no token) | freeze model (role `agent_chat`), scope, writes and budget; returns the token |
//! | `POST /local/pi/stop` | App | ends the run; the token stops resolving |
//! | `GET /local/pi/run` | App | the frozen run and what it has used |
//! | `POST /local/pi/model` | Pi (token) | one OpenAI-compatible chat completion through the frozen model, logged in `ai_calls` (`client = pi`) |
//! | `POST /v1/<tool>` | Pi (token) | query tools and agent tools, counted against the run's tool budget |
//!
//! The model never sees the provider key: the core adds it. The agent tools (`annotate`, …) are also served to other
//! clients on this channel, where their empty `writes` scope refuses every write.

use crate::http::{HttpRequest, HttpResponse};
use crate::local::Core;
use crate::router::{bearer, lock, run_tool, store_error};
use candid_store::{ClientScope, ScopeTime, AGENT_TOOL_NAMES};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const PI_CLIENT_ID: &str = "candid-pi";
const PI_CLIENT_NAME: &str = "Candid AI 页（Pi）";
/// Writes the agent may be granted; anything else in a start request is refused.
const WRITE_FLAGS: [&str; 4] = ["annotate", "reference", "commitment", "draft"];
/// Proposal kinds the App knows how to execute after approval.
const DEFAULT_PROPOSE_KINDS: [&str; 7] = candid_store::AGENDA_EXECUTABLE_KINDS;
/// Query tools Pi may call (the catalog's others stay with external clients: `usage` and `media` are not needed).
pub const PI_READ_TOOLS: [&str; 15] = ["find", "conversations", "conversation", "evidence", "status", "overview", "search", "timeline", "context", "get_event", "episodes", "episode", "thread", "records", "agent"];

#[derive(Clone, Debug)]
struct Budget {
    max_model_calls: u64,
    max_tool_calls: u64,
    max_output_tokens: u64,
    max_request_bytes: usize,
}

#[derive(Clone, Debug)]
struct Run {
    run_id: String,
    provider: Value,
    model_id: String,
    timeout_ms: u64,
    budget: Budget,
    model_calls: u64,
    tool_calls: u64,
    output_tokens: u64,
    stopped: Option<String>,
    automation: Option<String>,
    active: bool,
}

fn runs() -> &'static Mutex<Option<Run>> {
    static RUN: OnceLock<Mutex<Option<Run>>> = OnceLock::new();
    RUN.get_or_init(|| Mutex::new(None))
}

fn current() -> std::sync::MutexGuard<'static, Option<Run>> {
    runs().lock().unwrap_or_else(|p| p.into_inner())
}

fn ok(v: &Value) -> HttpResponse {
    HttpResponse { status: 200, body: serde_json::to_vec(v).unwrap_or_default() }
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
}

/// Serves the agent's routes; `None` hands the request back to the ordinary local channel.
pub(crate) fn route(core: &Core, request: &HttpRequest) -> Option<HttpResponse> {
    let path = request.path.split('?').next().unwrap_or("");
    let token = bearer(request);
    if let Some(rest) = path.strip_prefix("/local/pi/") {
        if rest == "model" {
            return Some(model(core, request, &token));
        }
        if !token.is_empty() {
            return Some(HttpResponse::error(403, "internal_only", "此操作只供 App 使用。"));
        }
        return Some(match (request.method.as_str(), rest) {
            ("POST", "start") => start(core, request),
            ("POST", "stop") => stop(core, request),
            ("POST", "idle") => idle(request),
            ("GET", "run") => ok(&run_json()),
            _ => HttpResponse::error(404, "unknown", "没有这个接口。"),
        });
    }
    let name = path.strip_prefix("/v1/")?;
    let agent_tool = AGENT_TOOL_NAMES.contains(&name);
    let store = lock(&core.store);
    let client = if token.is_empty() { None } else { store.client_id_for_token(&token).ok().flatten() };
    if client.as_deref() != Some(PI_CLIENT_ID) {
        if !agent_tool {
            return None;
        }
        if request.method != "POST" {
            return Some(HttpResponse::error(405, "method", "请用 POST /v1/<工具名>。"));
        }
        let client = match (token.is_empty(), client) {
            (true, _) => core.internal_client.clone(),
            (false, Some(c)) => c,
            (false, None) => return Some(HttpResponse::error(401, "unauthorized", "访问令牌无效或已撤销。")),
        };
        let response = run_tool(&store, name, &strip_run(&request.body, None), &client);
        drop(store);
        if response.status == 200 {
            core.feed.publish("data");
        }
        return Some(response);
    }
    // The agent itself: only its own tools, within the run's tool budget.
    if !agent_tool && !PI_READ_TOOLS.contains(&name) && name != "propose" {
        let _ = store.audit_refusal(PI_CLIENT_ID, name, "tool_not_permitted");
        return Some(HttpResponse::error(403, "not_permitted", &format!("Pi 不能调用 {name}")));
    }
    let tag = {
        let mut run = current();
        let Some(r) = run.as_mut().filter(|r| r.stopped.is_none()) else {
            return Some(HttpResponse::error(409, "no_run", "没有进行中的 AI 运行。"));
        };
        if let Some(id)=&r.automation {
            match store.automation_run(id).and_then(|a|{store.automation_validate(&a)?; if a["state"]!="running"{return Err(candid_store::TimelineError::Denied("run_not_running".into()));}Ok(())}) {
                Ok(())=>{},Err(e)=>return Some(store_error(&e))
            }
        }
        r.tool_calls += 1;
        if r.tool_calls > r.budget.max_tool_calls {
            r.stopped = Some("tool_budget".into());
            return Some(HttpResponse::error(429, "budget_exhausted", "本次运行的工具调用次数已用完。"));
        }
        json!({"run_id": r.run_id, "model_ref": format!("{}/{}", r.provider["provider_id"].as_str().unwrap_or(""), r.model_id)})
    };
    let body = if agent_tool { strip_run(&request.body, Some(tag)) } else { request.body.clone() };
    let response = run_tool(&store, name, &body, PI_CLIENT_ID);
    if let Some(id)=current().as_ref().and_then(|r|r.automation.clone()) {let _=store.automation_audit_tool(&id,name,&response.body);}
    drop(store);
    if agent_tool && response.status == 200 && name != "memories" {
        core.feed.publish("data");
    }
    Some(response)
}

/// Drops any `_run` the caller sent and, for the agent, sets the core's own.
fn strip_run(body: &[u8], tag: Option<Value>) -> Vec<u8> {
    let mut v: Value = serde_json::from_slice(if body.is_empty() { b"{}" } else { body }).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("_run");
        if let Some(tag) = tag {
            o.insert("_run".into(), tag);
        }
        serde_json::to_vec(&v).unwrap_or_default()
    } else {
        body.to_vec()
    }
}

fn run_json() -> Value {
    match current().as_ref() {
        None => json!({"run": null}),
        Some(r) => json!({"run": {
            "run_id": r.run_id, "provider_id": r.provider["provider_id"], "model_id": r.model_id, "destination": r.provider["destination"],
            "active":r.active,"automation":r.automation,"model_calls": r.model_calls, "tool_calls": r.tool_calls, "output_tokens": r.output_tokens, "stopped": r.stopped,
            "budget": {"max_model_calls": r.budget.max_model_calls, "max_tool_calls": r.budget.max_tool_calls, "max_output_tokens": r.budget.max_output_tokens}
        }}),
    }
}

fn start(core: &Core, request: &HttpRequest) -> HttpResponse {
    static START:OnceLock<Mutex<()>>=OnceLock::new();
    let _start=START.get_or_init(||Mutex::new(())).lock().unwrap_or_else(|e|e.into_inner());
    let mut v: Value = match serde_json::from_slice(&request.body) {
        Ok(v) => v,
        Err(_) => return HttpResponse::error(400, "json", "请求无效"),
    };
    if current().as_ref().is_some_and(|r|r.active&&r.stopped.is_none()) {
        return HttpResponse::error(409,"agent_busy","另一个 AI 任务正在运行。");
    }
    let automation=v["automation_id"].as_str().map(str::to_string);
    if let Some(id)=&automation {
        let frozen=match lock(&core.store).automation_pi_config(id,v["attempt"].as_i64().unwrap_or(0)){Ok(v)=>v,Err(e)=>return store_error(&e)};
        v=frozen;
    }
    let writes = strings(&v["writes"]).unwrap_or_default();
    if writes.iter().any(|w| !WRITE_FLAGS.contains(&w.as_str())) {
        return HttpResponse::error(400, "invalid_input", "未知的写入权限。");
    }
    let propose_kinds = strings(&v["propose_kinds"]).unwrap_or_else(|| DEFAULT_PROPOSE_KINDS.iter().map(|s| s.to_string()).collect());
    let agents = strings(&v["agents"]).unwrap_or_default();
    if agents.iter().any(|a| !candid_store::agents::AGENTS.contains(&a.as_str())) {
        return HttpResponse::error(400, "invalid_input", "不支持的 AI 会话来源。");
    }
    let sources = strings(&v["sources"]).unwrap_or_else(|| vec!["*".into()]);
    let days = v["max_days_back"].as_i64().unwrap_or(90).clamp(1, 3650);
    let budget = &v["budget"];
    let budget = Budget {
        max_model_calls: budget["max_model_calls"].as_u64().unwrap_or(12).clamp(1, 200),
        max_tool_calls: budget["max_tool_calls"].as_u64().unwrap_or(40).clamp(1, 500),
        max_output_tokens: budget["max_output_tokens"].as_u64().unwrap_or(16_000).clamp(256, 400_000),
        max_request_bytes: budget["max_request_bytes"].as_u64().unwrap_or(900_000).clamp(10_000, 1_000_000) as usize,
    };
    let allow_remote = v["allow_remote"].as_bool().unwrap_or(false);
    let store = lock(&core.store);
    // The model is the `agent_chat` role's, resolved now and kept for the whole run.
    let config = match store.model_command(&json!({"action": "get"})) {
        Ok(c) => c,
        Err(e) => return store_error(&e),
    };
    let role = v["role"].as_str().unwrap_or("agent_chat");
    let Some(assignment) = config["roles"].as_array().into_iter().flatten().find(|a| a["role"] == role && a["model"].is_object()).cloned() else {
        return HttpResponse::error(409, "waiting_model", &format!("设置里还没有给角色 {role} 选模型。"));
    };
    let model = &assignment["model"];
    let Some(provider) = config["providers"].as_array().into_iter().flatten().find(|p| p["provider_id"] == model["provider_id"] && p["enabled"] == true).cloned() else {
        return HttpResponse::error(409, "waiting_model", "该角色的模型来源未启用。");
    };
    if !["openai-chat", "ollama"].contains(&provider["api"].as_str().unwrap_or("")) {
        return HttpResponse::error(409, "unsupported_model_api", "Pi 目前只接 OpenAI 兼容的对话接口。");
    }
    if provider["destination"] == "cloud" && !allow_remote {
        return HttpResponse::error(403, "remote_not_allowed", "这个功能未允许使用远程模型。");
    }
    if let Some(id)=&automation { if let Err(e)=store.automation_provider_allowed(id,&provider){return store_error(&e);} }
    if let Some(id)=&automation {if let Err(e)=store.automation_bind_model(id,&json!({"provider_id":provider["provider_id"],"provider_name":provider["name"],"model_id":model["model_id"],"destination":provider["destination"],"role":role,"revision":"provider-managed"})){return store_error(&e);}}
    let usage=automation.as_ref().and_then(|id|store.automation_usage(id).ok()).unwrap_or(json!({}));
    let scope = ClientScope {
        agents,
        sources,
        time: ScopeTime { max_days_back: Some(days), ..Default::default() },
        text: true,
        media: false,
        derived: true,
        propose: automation.is_none(),
        writes: writes.clone(),
        propose_kinds: propose_kinds.clone(),
        // Fit whole tool responses in the extension; never slice JSON or lose a paging cursor.
        max_result_bytes: 18_000,
        ..Default::default()
    };
    // Expose only the capability state, never identities or content from a withheld source.
    let mail = match store.sources() { Ok(s) => s.into_iter().filter(|s|s.kind.starts_with("mail.") && s.status != "revoked").collect::<Vec<_>>(), Err(e) => return store_error(&e) };
    let context_note = if mail.is_empty() {
        Some("仅当用户询问邮件时使用此状态：邮件尚未连接。不要用应用活动代替邮件内容。")
    } else if !mail.iter().any(|s|s.policy.agent_visible && s.policy.store_text) {
        Some("仅当用户询问邮件时使用此状态：邮件已连接，但尚未允许 AI 读取。无需查询；直接用一句话说明在「连接 → 邮件」开启「允许 AI 读取邮件」后才能检查。不要查询无权读取的邮件，不要讨论缺口或据此判断没有邮件、没有待办；其他话题不提此状态。")
    } else if !mail.iter().any(|s|s.policy.agent_visible && s.policy.store_text && scope.sources.iter().any(|id|id=="*" || id==&s.source_id)) {
        Some("仅当用户询问邮件时使用此状态：邮件不在本次运行的来源范围内。说明需调整读取范围，不要用应用活动代替邮件内容。")
    } else { None };
    let token = match store.attach_agent_client(PI_CLIENT_ID, PI_CLIENT_NAME, &scope) {
        Ok(t) => t,
        Err(e) => return store_error(&e),
    };
    drop(store);
    let run_id = v["run_id"].as_str().filter(|s| !s.is_empty() && s.len() <= 80).map(String::from).unwrap_or_else(candid_store::uuid_v4);
    let model_id = model["model_id"].as_str().unwrap_or("").to_string();
    let timeout_ms = assignment["limits_json"]["timeout_ms"].as_u64().unwrap_or(120_000).clamp(1_000, 300_000);
    let tools: Vec<&str> = PI_READ_TOOLS.iter().copied().chain(["memories", "propose"]).chain(AGENT_TOOL_NAMES.iter().copied().filter(|t| match *t {
        "annotate" => writes.iter().any(|w| w == "annotate"),
        "mark_reference" => writes.iter().any(|w| w == "reference"),
        "commitment" => writes.iter().any(|w| w == "commitment"),
        "save_draft" => writes.iter().any(|w| w == "draft"),
        "undo_write" => !writes.is_empty(),
        _ => false,
    })).collect();
    let response = json!({
        "run_id": run_id, "client_id": PI_CLIENT_ID, "token": token,
        "model": {"provider_id": provider["provider_id"], "model_id": model_id, "destination": provider["destination"], "role": role},
        "tools": tools, "writes": writes, "propose_kinds": propose_kinds, "context_note":context_note,
        "budget": {"max_model_calls": budget.max_model_calls, "max_tool_calls": budget.max_tool_calls, "max_output_tokens": budget.max_output_tokens},
    });
    *current() = Some(Run { run_id, provider, model_id, timeout_ms, budget, model_calls: usage["model_calls"].as_u64().unwrap_or(0), tool_calls: usage["tool_calls"].as_u64().unwrap_or(0), output_tokens: usage["output_tokens"].as_u64().unwrap_or(0), stopped: None, automation, active:true });
    ok(&response)
}

fn idle(request:&HttpRequest)->HttpResponse {
    let v:Value=serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    if let Some(r)=current().as_mut().filter(|r|r.run_id==v["run_id"].as_str().unwrap_or("")){r.active=false;}
    ok(&json!({"ok":true}))
}
fn stop(core: &Core, request:&HttpRequest) -> HttpResponse {
    let v:Value=serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let mut guard=current();
    if v["run_id"].as_str().is_some_and(|id|guard.as_ref().is_none_or(|r|r.run_id!=id)){return ok(&json!({"stopped":null}));}
    let ended = guard.take();
    drop(guard);
    if let Err(e) = lock(&core.store).detach_agent_client(PI_CLIENT_ID) {
        return store_error(&e);
    }
    ok(&json!({"stopped": ended.map(|r| r.run_id)}))
}

/// One chat completion for Pi through the run's frozen model. Only the conversation and tool definitions pass through;
/// the model, token cap and key are the core's. The answer is not streamed (the extension replays it to Pi as one
/// server-sent-event burst).
fn model(core: &Core, request: &HttpRequest, token: &str) -> HttpResponse {
    if request.method != "POST" {
        return HttpResponse::error(405, "method", "请用 POST。");
    }
    let client = if token.is_empty() { None } else { lock(&core.store).client_id_for_token(token).ok().flatten() };
    if client.as_deref() != Some(PI_CLIENT_ID) {
        return HttpResponse::error(401, "unauthorized", "访问令牌无效或已撤销。");
    }
    let v: Value = match serde_json::from_slice(&request.body) {
        Ok(v) => v,
        Err(_) => return HttpResponse::error(400, "json", "请求无效"),
    };
    if !v["messages"].is_array() {
        return HttpResponse::error(400, "invalid_input", "缺少 messages。");
    }
    let (run, cap) = {
        let mut guard = current();
        let Some(r) = guard.as_mut().filter(|r| r.stopped.is_none()) else {
            return HttpResponse::error(409, "no_run", "没有进行中的 AI 运行。");
        };
        if request.body.len() > r.budget.max_request_bytes {
            return HttpResponse::error(413, "too_large", "对话过长，超过本次运行的上限。");
        }
        if r.model_calls >= r.budget.max_model_calls || r.output_tokens >= r.budget.max_output_tokens {
            r.stopped = Some("model_budget".into());
            return HttpResponse::error(429, "budget_exhausted", "本次运行的模型预算已用完。");
        }
        r.model_calls += 1;
        let asked = v["max_tokens"].as_u64().or(v["max_completion_tokens"].as_u64()).unwrap_or(4096);
        let cap = asked.min(r.budget.max_output_tokens - r.output_tokens).clamp(1, 16_384);
        (r.clone(), cap)
    };
    if let Some(id)=&run.automation {let s=lock(&core.store);match s.automation_run(id).and_then(|r|{s.automation_validate(&r)?;if r["state"]!="running"{return Err(candid_store::TimelineError::Denied("run_not_running".into()));}s.automation_provider_allowed(id,&run.provider)}){Ok(())=>{},Err(e)=>return store_error(&e)}}
    let mut body = json!({"model": run.model_id, "messages": v["messages"], "max_tokens": cap, "stream": false});
    for key in ["tools", "tool_choice", "temperature", "parallel_tool_calls"] {
        if !v[key].is_null() {
            body[key] = v[key].clone();
        }
    }
    let payload = serde_json::to_vec(&body).unwrap_or_default();
    let call = {
        let store = lock(&core.store);
        let destination = run.provider["destination"].as_str().unwrap_or("");
        match store.agent_call_start(run.automation.as_deref().unwrap_or("agent_chat"), &run.run_id, run.provider["provider_id"].as_str().unwrap_or(""), &run.model_id, destination, payload.len() as i64) {
            Ok(c) => c,
            Err(e) => return store_error(&e),
        }
    };
    let result = send(&run, payload);
    let store = lock(&core.store);
    match result {
        Ok(mut answer) => {
            if let Some(id)=&run.automation {if store.automation_run(id).and_then(|r|{store.automation_validate(&r)?;if r["state"]!="running"{return Err(candid_store::TimelineError::Denied("run_not_running".into()));}Ok(())}).is_err(){let _=store.agent_call_finish(&call,"cancelled",Some("source_or_configuration_changed"),&answer["usage"],None);return HttpResponse::error(409,"run_cancelled","运行已取消或来源权限已改变。");}}
            let usage = answer["usage"].clone();
            let out = usage["completion_tokens"].as_u64().unwrap_or(0);
            if let Some(r) = current().as_mut().filter(|r| r.run_id == run.run_id) {
                r.output_tokens += out;
            }
            let bytes = serde_json::to_vec(&answer).map(|b| b.len() as i64).ok();
            if let Err(e) = store.agent_call_finish(&call, "ok", None, &usage, bytes) {
                return store_error(&e);
            }
            answer["candid"] = json!({"call_id": call, "run_id": run.run_id});
            ok(&answer)
        }
        Err(code) => {
            let _ = store.agent_call_finish(&call, if code == "timeout" { "timeout" } else { "error" }, Some(&code), &Value::Null, None);
            HttpResponse::error(502, "model_call_failed", &code)
        }
    }
}

fn send(run: &Run, payload: Vec<u8>) -> Result<Value, String> {
    let p = &run.provider;
    let raw = p["base_url"].as_str().ok_or("waiting_model")?.trim_end_matches('/');
    let base = if p["api"] == "ollama" && !raw.ends_with("/v1") { format!("{raw}/v1") } else { raw.to_string() };
    let url = format!("{base}/chat/completions");
    let mut builder = reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_millis(run.timeout_ms)).redirect(reqwest::redirect::Policy::none());
    let env = std::env::vars().collect();
    match crate::proxy::route_for(crate::proxy::ProxyMode::current(&env), &url) {
        crate::proxy::Route::Direct => {}
        crate::proxy::Route::Http { host, port } => builder = builder.proxy(reqwest::Proxy::all(format!("http://{host}:{port}")).map_err(|_| "proxy_configuration")?),
        crate::proxy::Route::Socks { host, port } => builder = builder.proxy(reqwest::Proxy::all(format!("socks5h://{host}:{port}")).map_err(|_| "proxy_configuration")?),
    }
    let client = builder.build().map_err(|_| "transport")?;
    let mut req = client.post(url).header("content-type", "application/json").body(payload);
    if let Some(reference) = p["secret_ref"].as_str() {
        req = req.bearer_auth(crate::workflows::secret(reference).ok_or("waiting_authorization")?);
    }
    for (k, v) in p["headers_json"].as_object().into_iter().flatten() {
        if let Some(s) = v.as_str() {
            req = req.header(k, s);
        }
    }
    let response = req.send().map_err(|e| if e.is_timeout() { "timeout".to_string() } else { "transport".to_string() })?;
    let status = response.status().as_u16();
    let text = response.text().map_err(|_| "transport")?;
    if !(200..300).contains(&status) {
        return Err(format!("http_{status}"));
    }
    let answer: Value = serde_json::from_str(&text).map_err(|_| "invalid_response")?;
    if !answer["choices"].is_array() {
        return Err("invalid_response".into());
    }
    Ok(answer)
}
