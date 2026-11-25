//! One serial consumer of the existing jobs queue. HTTP never holds the shared store mutex.
use crate::router::{lock, SharedStore};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
/// Model calls run in two lanes that never wait for each other. The interactive lane (`complete()`, compose,
/// Pi) is for something a person is waiting on; the background lane is the serial job worker.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Lane {
    Interactive,
    Background,
}
static EXECUTIONS: OnceLock<Mutex<std::collections::HashSet<(usize, Lane)>>> = OnceLock::new();
struct Execution(usize, Lane);
impl Execution {
    fn acquire(store: &SharedStore, lane: Lane) -> Option<Self> {
        let id = std::sync::Arc::as_ptr(store) as usize;
        if EXECUTIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert((id, lane))
        {
            Some(Self(id, lane))
        } else {
            None
        }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        EXECUTIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&(self.0, self.1));
    }
}
static SECRETS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static AUTHORIZATION_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn authorize(v: &Value) {
    let mut secrets = SECRETS.get_or_init(Default::default).lock().unwrap();
    secrets.clear();
    if let Some(values) = v["secrets"].as_object() {
        for (k, v) in values {
            if let Some(s) = v.as_str() {
                secrets.insert(k.clone(), s.into());
            }
        }
    }
    AUTHORIZATION_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn secret(reference: &str) -> Option<String> {
    SECRETS.get_or_init(Default::default).lock().unwrap().get(reference).cloned()
}
fn execute(r: &Value) -> Result<(Value, Value), String> {
    execute_http(r)
}
fn execute_http(r: &Value) -> Result<(Value, Value), String> {
    let p = &r["provider"];
    let raw = p["base_url"]
        .as_str()
        .ok_or("waiting_model")?
        .trim_end_matches('/');
    let ollama = if p["api"] == "ollama" && !raw.ends_with("/v1") {
        format!("{raw}/v1")
    } else {
        raw.into()
    };
    let base = ollama.as_str();
    let anthropic = p["api"] == "anthropic-messages";
    let url = match p["api"].as_str() {
        Some("openai-chat" | "ollama") => format!("{base}/chat/completions"),
        Some("anthropic-messages") => format!("{}/v1/messages", base.trim_end_matches("/v1")),
        _ => return Err("unsupported_model_api".into()),
    };
    let timeout = r["assignment"]["limits_json"]["timeout_ms"]
        .as_u64()
        .unwrap_or(120000)
        .clamp(50, 120000);
    let mut builder = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(timeout))
        .redirect(reqwest::redirect::Policy::none());
    let env = std::env::vars().collect();
    let mode = crate::proxy::ProxyMode::current(&env);
    let route = crate::proxy::route_for(mode, &url);
    let proxy = match route {
        crate::proxy::Route::Direct => None,
        crate::proxy::Route::Http { host, port } => Some(format!("http://{host}:{port}")),
        crate::proxy::Route::Socks { host, port } => Some(format!("socks5h://{host}:{port}")),
    };
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(&proxy).map_err(|_| "proxy_configuration")?);
    }
    let client = builder.build().map_err(|_| "transport")?;
    let mut req = client.post(url);
    if let Some(reference) = p["secret_ref"].as_str() {
        let secret = SECRETS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .ok_or("waiting_authorization")?;
        req = if anthropic {
            req.header("x-api-key", secret)
        } else {
            req.bearer_auth(secret)
        };
    }
    if let Some(headers) = p["headers_json"].as_object() {
        for (k, v) in headers {
            if let Some(s) = v.as_str() {
                req = req.header(k, s);
            }
        }
    }
    {
        let mut messages=r.get("messages").filter(|v|!v.is_null()).cloned().unwrap_or(json!([{"role":"system","content":r["prompt"]},{"role":"user","content":r["content"]}]));
        // Each step states what its answer needs (`max_tokens`); a role limit only lowers it. A fixed 1024 default
        // cut multi-frame chat JSON short, and a thinking model spends part of the budget before it answers.
        let step_cap = r["max_tokens"].as_u64().unwrap_or(1024).min(4096);
        let cap = r["assignment"]["limits_json"]["max_output_tokens"].as_u64().map_or(step_cap, |n| n.min(step_cap));
        let mut body = json!({"model":r["assignment"]["model"]["model_id"],"messages":messages,"max_tokens":cap,"temperature":r["temperature"].as_f64().unwrap_or(0.0)});
        if anthropic {
            let mut systems = Vec::new();
            let mut converted = Vec::new();
            for m in messages.as_array_mut().ok_or("invalid_messages")? {
                if m["role"] == "system" {
                    systems.push(message_text(&m["content"]));
                    continue;
                }
                let mut content = Vec::new();
                if let Some(parts) = m["content"].as_array() {
                    for part in parts {
                        if part["type"] == "text" {
                            content.push(part.clone());
                        } else if let Some(url) = part["image_url"]["url"].as_str() {
                            let (head, data) = url.split_once(',').ok_or("invalid_image")?;
                            let media = head
                                .strip_prefix("data:")
                                .and_then(|s| s.strip_suffix(";base64"))
                                .ok_or("invalid_image")?;
                            content.push(json!({"type":"image","source":{"type":"base64","media_type":media,"data":data}}));
                        }
                    }
                } else {
                    content.push(json!({"type":"text","text":m["content"]}));
                }
                converted.push(json!({"role":m["role"],"content":content}));
            }
            body["system"] = json!(systems.join("\n\n"));
            body["messages"] = json!(converted);
            req = req.header("anthropic-version", "2023-06-01");
        } else {
            if r["response_schema"].is_object() {
                body["response_format"] = json!({"type":"json_schema","json_schema":{"name":"candid_screen_summary","strict":true,"schema":r["response_schema"]}});
            } else if r["json_mode"] == true {
                body["response_format"] = json!({"type":"json_object"});
            }
            if base.starts_with("https://api.openai.com") {
                body.as_object_mut().unwrap().remove("max_tokens");
                body["max_completion_tokens"] = json!(cap);
            }
        }
        req = req.json(&body);
    }
    let response = req
        .send()
        .map_err(|e| if e.is_timeout() { "timeout" } else { "offline" })?;
    if !response.status().is_success() {
        return Err(format!("http_{}", response.status().as_u16()));
    }
    let bytes = std::io::Read::take(response, 4 * 1024 * 1024 + 1);
    use std::io::Read;
    let mut b = Vec::new();
    let mut bytes = bytes;
    bytes.read_to_end(&mut b).map_err(|_| "response_read")?;
    if b.len() > 4 * 1024 * 1024 {
        return Err("output_limit".into());
    }
    let v: Value = serde_json::from_slice(&b).map_err(|_| "bad_json")?;
    if anthropic {
        let text = v["content"]
            .as_array()
            .ok_or("invalid_output")?
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
        Ok((
            json!(text),
            json!({"prompt_tokens":v["usage"]["input_tokens"],"completion_tokens":v["usage"]["output_tokens"],"cache_read_tokens":v["usage"]["cache_read_input_tokens"],"cache_write_tokens":v["usage"]["cache_creation_input_tokens"]}),
        ))
    } else {
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("invalid_output")?;
        Ok((json!(text), v["usage"].clone()))
    }
}
/// A timeout or interrupted attempt is never automatically sent again: charging may already have occurred.
pub fn tick(store: &SharedStore, on_start: &dyn Fn()) {
    tick_inner(store, on_start, true);
}

/// Poll only indexed revision counters between changes. The bounded fallback covers time-based hooks,
/// model budget resets and offline cleanup without repeatedly hydrating the historical jobs list.
#[derive(Default)]
pub(crate) struct Worker {
    stamp: Option<(i64, i64, i64, u64)>,
    last_poll: Option<Instant>,
    next_due: Option<i64>,
    cleanup: Option<(i64, Instant)>,
}
impl Worker {
    pub fn tick(&mut self, store: &SharedStore, on_start: &dyn Fn()) {
        let (stamp, now) = {
            let s = lock(store);
            let Ok(jobs) = s.background_revision("jobs") else { return };
            let Ok(workflows) = s.background_revision("workflows") else { return };
            let Ok(seq) = s.background_data_revision() else { return };
            ((jobs, workflows, seq, AUTHORIZATION_REVISION.load(std::sync::atomic::Ordering::Relaxed)), s.clock.now_ms())
        };
        if self.stamp == Some(stamp)
            && self.last_poll.is_some_and(|t| t.elapsed() < Duration::from_secs(30))
            && !self.next_due.is_some_and(|t| t <= now)
        { return; }
        let cleanup = self.cleanup.is_none_or(|(revision, t)| revision != stamp.1 || t.elapsed() >= Duration::from_secs(30));
        self.stamp = Some(stamp);
        self.last_poll = Some(Instant::now());
        if cleanup { self.cleanup = Some((stamp.1, Instant::now())); }
        tick_inner(store, on_start, cleanup);
        self.next_due = lock(store).next_workflow_deadline().ok().flatten();
    }
}

fn tick_inner(store: &SharedStore, on_start: &dyn Fn(), cleanup: bool) {
    let Some(_execution) = Execution::acquire(store, Lane::Background) else {
        return;
    };
    if cleanup { crate::voiceprints::reconcile(store); }
    let request = {
        let s = lock(store);
        let _ = s.workflow_scan();
        let _ = s.curated_sync();
        let _ = s.people_sync();
        if s.people_judge_scan().is_ok_and(|v|v["changed"]==true) {on_start();}
        if s.excerpt_scan().is_ok_and(|v|v["changed"]==true) {on_start();}
        if s.items_scan().is_ok_and(|v|v["changed"]==true) {on_start();}
        if s.screen_scan().is_ok_and(|v|v["changed"]==true) {on_start();}
        let _ = s.automation_scan();
        // Kinds that hit their daily limit this round; their remaining jobs are skipped without preparing.
        let mut blocked: std::collections::HashSet<String> = Default::default();
        let _ = s.expire_stale_jobs();
        let queue = match s.pending_job_queue() {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut prepared = None;
        for (id, kind, _) in queue {
            let kind = kind.as_str();
            if blocked.contains(kind) {
                continue;
            }
            let Ok(j) = s.job_command(&json!({"action":"get","id":id})) else { continue };
            let j = &j;
            if j["state"] == "running" {
                if j["lease_until_ms"].as_i64().unwrap_or(i64::MAX)
                    <= chrono::Utc::now().timestamp_millis()
                {
                    let _ = s.job_command(&json!({"action":"cancel","id":j["id"]}));
                }
                continue;
            }
            if j["state"] != "queued" {
                continue;
            }
            // Check route/permissions before claiming. Waiting remains explicitly queued with a reason.
            match s.workflow_prepare(j) {
                Ok(r) => {
                    if let Some(reference) = r["provider"]["secret_ref"].as_str() {
                        if !SECRETS
                            .get_or_init(Default::default)
                            .lock()
                            .unwrap()
                            .contains_key(reference)
                        {
                            let _ = s.workflow_wait(
                                j["id"].as_str().unwrap_or(""),
                                "waiting_authorization",
                            );
                            continue;
                        }
                    }
                    let claimed=match s.job_command(&json!({"action":"claim","id":j["id"],"kind":kind,"owner":"fixed-workflow/1","lease_ms":180000})){Ok(v)if !v.is_null()=>v,_=>continue};
                    let mut r = if claimed["id"] == j["id"] {
                        r
                    } else {
                        match s.workflow_prepare(&claimed) {
                            Ok(r) => r,
                            Err(_) => continue,
                        }
                    };
                    r["job"] = claimed;
                    prepared = Some(r);
                    break;
                }
                Err(e) => {
                    let reason = e.message();
                    if reason.contains("waiting_feature_budget") {
                        blocked.insert(kind.to_string());
                    }
                    if reason.contains("waiting_") {
                        log_waiting(kind, reason.split('：').next_back().unwrap_or(&reason));
                        let _ = s.workflow_wait(
                            j["id"].as_str().unwrap_or(""),
                            reason.split('：').next_back().unwrap_or(&reason),
                        );
                    } else {
                        eprintln!("任务：{kind} 作废 {}", code_only(&reason));
                        let _ = s.workflow_invalidate(j["id"].as_str().unwrap_or(""), &reason);
                    }
                }
            }
        }
        prepared
    };
    let Some(r) = request else { return };
    on_start();
    let started = Instant::now();
    let recording = r["recording"] == true;
    let result = if recording {crate::voiceprints::recording_job(store,&r)} else {execute_workflow(store, &r)};
    let unused_refs=result.as_ref().ok().map(|o|o["voice_refs"].clone());
    // CPU seconds the model service reported for this job's calls (`cpu_seconds`), passed on with the result.
    let model_cpu = result.as_ref().ok().and_then(|o| o["cpu_seconds"].as_f64());
    let s = lock(store);
    let job = &r["job"];
    let error = match result {
        Ok(output) => if recording{s.recording_complete(&r,&output)}else{s.media_command(&json!({"action":"result","format":"candid-workflow-result/1","request":r,"output":output}))}.err().map(|e|e.message()),
        Err(e) => Some(e),
    };
    let kind = job["kind"].as_str().unwrap_or("?");
    let ms = started.elapsed().as_millis();
    if error.is_none() {
        match model_cpu {
            Some(cpu) => eprintln!("任务：{kind} 完成 {ms}ms，模型 CPU {cpu:.1}s"),
            None => eprintln!("任务：{kind} 完成 {ms}ms"),
        }
    }
    if let Some(code) = error {
        if recording {if let Some(refs)=unused_refs {let _=s.discard_recording_speaker_refs(&refs);}}
        let _ = s.screen_request_rejected(&r, &code);
        let attempt = job["attempt"].as_u64().unwrap_or(0);
        // Speaker reconciliation reports "voice service busy/offline" in words rather than the shared codes.
        let transient = if recording && (code.contains("离线")||code.contains("正在处理")||code.contains("声纹服务未完成")) { "offline" } else { code.as_str() };
        match crate::retry::retry_delay_ms(attempt, transient) {
            Some(delay) => {eprintln!("任务：{kind} 失败 {}（第 {} 次，{}s 后重试）{ms}ms", code_only(&code), attempt + 1, delay / 1000);let _=s.job_command(&json!({"action":"retry","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"delay_ms":delay,"error_code":transient}));}
            None => {eprintln!("任务：{kind} 失败 {}（第 {} 次，不再重试）{ms}ms", code_only(&code), attempt + 1);let _=s.job_command(&json!({"action":"finish","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"error_code":code}));}
        }
    }
}

/// Codes, not content: messages from the store may quote input, so the log keeps the leading code word only.
fn code_only(reason: &str) -> String {
    let tail = reason.split('：').next_back().unwrap_or(reason);
    let code: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').take(64).collect();
    if code.is_empty() { "other".into() } else { code }
}

/// A waiting job is re-checked every round; each (kind, reason) is logged once per process.
fn log_waiting(kind: &str, reason: &str) {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let key = format!("{kind} {}", code_only(reason));
    if SEEN.get_or_init(Default::default).lock().unwrap().insert(key.clone()) {
        eprintln!("任务：{key}（等待中，同类只记一次）");
    }
}

pub fn complete(store: &SharedStore, v: &Value) -> Result<Value, String> {
    // Interactive lane: never waits for the background worker, only for another interactive call.
    let mut execution = Execution::acquire(store, Lane::Interactive);
    for _ in 0..100 {
        if execution.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        execution = Execution::acquire(store, Lane::Interactive);
    }
    let _execution = execution.ok_or("模型忙，请稍后重试")?;
    let (r, call) = {
        let s = lock(store);
        let r = s.model_prepare(v).map_err(|e| e.message())?;
        let call = s.workflow_call_start(&r).map_err(|e| e.message())?;
        (r, call)
    };
    match execute(&r) {
        Ok((output, usage)) => {
            lock(store)
                .workflow_call_finish(&call, "ok", None, &usage)
                .map_err(|e| e.message())?;
            Ok(json!({"text":output,"usage":usage,"call_id":call}))
        }
        Err(e) => {
            let _ = lock(store).workflow_call_finish(
                &call,
                if e == "timeout" { "timeout" } else { "error" },
                Some(&e),
                &Value::Null,
            );
            Err(e)
        }
    }
}

fn call_once(store: &SharedStore, r: &Value) -> Result<Value, String> {
    let call = lock(store)
        .workflow_call_start(r)
        .map_err(|e| e.message())?;
    let response = execute_http(r);
    match response {
        Ok((output, usage)) => {
            let s = lock(store);
            let allowed = s.workflow_reply_allowed(r).is_ok();
            s.workflow_call_finish(
                &call,
                if allowed { "ok" } else { "cancelled" },
                if allowed {
                    None
                } else {
                    Some("source_or_configuration_changed")
                },
                &usage,
            )
            .map_err(|e| e.message())?;
            if !allowed {
                return Err("source_or_configuration_changed".into());
            }
            Ok(output)
        }
        Err(code) => {
            let _ = lock(store).workflow_call_finish(
                &call,
                if code == "timeout" {
                    "timeout"
                } else {
                    "error"
                },
                Some(&code),
                &Value::Null,
            );
            Err(code)
        }
    }
}
/// For steps that ask for JSON (`json_repair`): when the reply does not contain a JSON object, ask once more with the
/// bad reply and a short instruction; the second reply is used as is and judged by the step's own parser.
fn repair_json_once(store: &SharedStore, r: &Value, output: Value) -> Result<Value, String> {
    let valid = |o: &Value| o.as_str().and_then(crate::retry::extract_json).is_some_and(|v| v.is_object());
    if r["json_repair"] != true || valid(&output) {
        return Ok(output);
    }
    let mut again = r.clone();
    again["messages"] = crate::retry::json_repair_messages(&r["messages"], output.as_str().unwrap_or(""));
    call_once(store, &again)
}

fn execute_workflow(store: &SharedStore, r: &Value) -> Result<Value, String> {
    let output = call_once(store, r)?;
    repair_json_once(store, r, output)
}

fn message_text(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn store() -> SharedStore {
        let root = std::env::temp_dir().join(format!("lanes-{}", candid_store::uuid_v4()));
        Arc::new(Mutex::new(candid_store::TimelineStore::open(&root, "candid/test", true, candid_store::fixtures::test_clock()).unwrap()))
    }

    #[test]
    fn lanes_do_not_block_each_other_but_each_is_serial() {
        let s = store();
        let background = Execution::acquire(&s, Lane::Background).unwrap();
        let interactive = Execution::acquire(&s, Lane::Interactive).expect("interactive waits for no background call");
        assert!(Execution::acquire(&s, Lane::Background).is_none());
        assert!(Execution::acquire(&s, Lane::Interactive).is_none());
        drop(background);
        assert!(Execution::acquire(&s, Lane::Background).is_some());
        drop(interactive);
        assert!(Execution::acquire(&s, Lane::Interactive).is_some());
    }
}
