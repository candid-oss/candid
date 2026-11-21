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
    execute_http(r, None)
}
fn execute_http(r: &Value, audio: Option<Vec<u8>>) -> Result<(Value, Value), String> {
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
    let url = if r["speech"] == true {
        format!("{base}/voice")
    } else {
        match p["api"].as_str() {
            Some("openai-chat" | "ollama") => format!("{base}/chat/completions"),
            Some("anthropic-messages") => format!("{}/v1/messages", base.trim_end_matches("/v1")),
            _ => return Err("unsupported_model_api".into()),
        }
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
    if r["speech"] == true {
        let path = r["path"].as_str().ok_or("missing_source")?;
        let bytes = match audio {
            Some(bytes) => bytes,
            None => std::fs::read(path).map_err(|_| "missing_source")?,
        };
        let form = reqwest::blocking::multipart::Form::new()
            .text("mode", r["mode"].as_str().unwrap_or("quick").to_string())
            .part(
                "file",
                reqwest::blocking::multipart::Part::bytes(bytes).file_name("source.wav"),
            );
        req = req.multipart(form);
    } else {
        let mut messages=r.get("messages").filter(|v|!v.is_null()).cloned().unwrap_or(json!([{"role":"system","content":r["prompt"]},{"role":"user","content":r["content"]}]));
        let cap = r["assignment"]["limits_json"]["max_output_tokens"]
            .as_u64()
            .unwrap_or(1024)
            .min(4096)
            .min(r["max_tokens"].as_u64().unwrap_or(4096));
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
    if r["speech"] == true {
        checked_speech(v,r)
    } else if anthropic {
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
pub(crate) fn checked_speech(mut v:Value,r:&Value)->Result<(Value,Value),String> {
        if v["format"] != "candid-voice-result/1" || v["source_hash"] != r["digest"] {
            return Err("source_mismatch".into());
        }
        for key in [
            "model_id",
            "model_revision",
            "processor_version",
            "timestamp_kind",
        ] {
            if v["producer"][key]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            {
                return Err("invalid_producer".into());
            }
        }
        v["producer"]["revision"] = json!(format!(
            "{}:{}",
            v["producer"]["model_revision"]
                .as_str()
                .unwrap_or("unknown"),
            v["producer"]["processor_version"]
                .as_str()
                .unwrap_or("unknown")
        ));
        Ok((v, Value::Null))
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
                        let _ = s.workflow_wait(
                            j["id"].as_str().unwrap_or(""),
                            reason.split('：').next_back().unwrap_or(&reason),
                        );
                    } else {
                        let _ = s.workflow_invalidate(j["id"].as_str().unwrap_or(""), &reason);
                    }
                }
            }
        }
        prepared
    };
    let Some(r) = request else { return };
    on_start();
    let result = if r["reconcile_speakers"]==true {crate::voiceprints::reconcile_recording_job(store,&r)} else {execute_workflow(store, &r)};
    let unused_refs=result.as_ref().ok().map(|o|o["voice_refs"].clone());
    let s = lock(store);
    let job = &r["job"];
    let error = match result {
        Ok(output) => if r["reconcile_speakers"]==true{s.recording_speakers_complete(&r,&output)}else if r["speech"]==true{s.workflow_complete(&r,&output)}else{s.media_command(&json!({"action":"result","format":"candid-workflow-result/1","request":r,"output":output}))}.err().map(|e|e.message()),
        Err(e) => Some(e),
    };
    if let Some(code) = error {
        if r["reconcile_speakers"]==true {if let Some(refs)=unused_refs {let _=s.discard_recording_speaker_refs(&refs);}}
        let _ = s.screen_request_rejected(&r, &code);
        let attempt = job["attempt"].as_u64().unwrap_or(0);
        // Speaker reconciliation reports "voice service busy/offline" in words rather than the shared codes.
        let transient = if r["reconcile_speakers"]==true && (code.contains("离线")||code.contains("正在处理")||code.contains("声纹服务未完成")) { "offline" } else { code.as_str() };
        match crate::retry::retry_delay_ms(attempt, transient) {
            Some(delay) => {let _=s.job_command(&json!({"action":"retry","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"delay_ms":delay,"error_code":transient}));}
            None => {let _=s.job_command(&json!({"action":"finish","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"error_code":code}));}
        }
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

fn call_once(store: &SharedStore, r: &Value, audio: Option<Vec<u8>>) -> Result<Value, String> {
    let call = lock(store)
        .workflow_call_start(r)
        .map_err(|e| e.message())?;
    let response = if r["speech"] == true {crate::voice_jobs::execute(store,r,audio.as_deref()).unwrap_or_else(||execute_http(r,audio))}else{execute_http(r,audio)};
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
    call_once(store, &again, None)
}

fn execute_workflow(store: &SharedStore, r: &Value) -> Result<Value, String> {
    if r["speech"] != true {
        let output = call_once(store, r, None)?;
        return repair_json_once(store, r, output);
    }
    let path = std::path::Path::new(r["path"].as_str().ok_or("missing_source")?);
    let plan = crate::audio_chunks::AudioChunks::open(path)?;
    let chunk_frames = plan.chunk_frames();
    if plan.frames <= chunk_frames
        && std::fs::metadata(path).map_err(|_| "missing_source")?.len() <= 32 * 1024 * 1024
    {
        return call_once(store, r, None);
    }
    let job = &r["job"];
    let mut checkpoint = job["checkpoint"].clone();
    if checkpoint["format"] != "voice-checkpoint/1"
        || checkpoint["source_digest"] != r["digest"]
        || checkpoint["config_revision"] != r["config_revision"]
    {
        checkpoint = json!({"format":"voice-checkpoint/1","source_digest":r["digest"],"config_revision":r["config_revision"],"next_sample":0,"segments":[],"producer":null});
    }
    let mut start = checkpoint["next_sample"].as_u64().unwrap_or(0);
    if start > plan.frames {
        return Err("invalid_checkpoint".into());
    }
    while start < plan.frames {
        {
            let s = lock(store);
            s.workflow_prepare(job).map_err(|e| e.message())?;
            s.job_command(&json!({"action":"heartbeat","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"lease_ms":180000})).map_err(|e|e.message())?;
        }
        let count = (plan.frames - start).min(chunk_frames);
        let bytes = plan.bytes(start, count)?;
        let mut chunk = r.clone();
        chunk["digest"] = json!(candid_store::sha256_hex(&bytes));
        chunk["audio_total_seconds"] = json!(plan.frames as f64 / plan.rate as f64);
        chunk["audio_offset_seconds"] = json!(start as f64 / plan.rate as f64);
        let output = call_once(store, &chunk, Some(bytes))?;
        let offset = start as f64 / plan.rate as f64;
        let duration = count as f64 / plan.rate as f64;
        let segments = output["segments"].as_array().ok_or("invalid_output")?;
        for segment in segments {
            let a = segment["start_seconds"].as_f64().ok_or("invalid_output")?;
            let b = segment["end_seconds"].as_f64().ok_or("invalid_output")?;
            if a < 0.0 || b <= a || b > duration {
                return Err("chunk_bounds".into());
            }
            let mut segment = segment.clone();
            segment["start_seconds"] = json!(a + offset);
            segment["end_seconds"] = json!(b + offset);
            if let Some(speaker) = segment["speaker"].as_str() {
                let label = speaker.to_string();
                segment["source_speaker"] = json!(label);
                segment["speaker"] = json!(format!("分块{} · {}", start / chunk_frames + 1, label));
                segment["speaker_scope_start_sample"] = json!(start);
            }
            segment["processing_chunk_start_sample"] = json!(start);
            segment["processing_sample_rate"] = json!(plan.rate);
            segment["producer"] = output["producer"].clone();
            checkpoint["segments"]
                .as_array_mut()
                .ok_or("invalid_checkpoint")?
                .push(segment);
        }
        start += count;
        checkpoint["next_sample"] = json!(start);
        checkpoint["producer"] = output["producer"].clone();
        lock(store).job_command(&json!({"action":"checkpoint","id":job["id"],"owner":job["owner"],"attempt":job["attempt"],"checkpoint":checkpoint})).map_err(|e|e.message())?;
    }
    Ok(
        json!({"format":"candid-voice-result/1","source_hash":r["digest"],"producer":checkpoint["producer"],"segments":checkpoint["segments"]}),
    )
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
