//! Optional job API of a local voice deployment. The product never starts or kills its processes.
use crate::router::{lock, SharedStore};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(crate) fn execute(
    store: &SharedStore,
    r: &Value,
    audio: Option<&[u8]>,
) -> Option<Result<(Value, Value), String>> {
    let base = r["provider"]["base_url"].as_str()?.trim_end_matches('/');
    let url = reqwest::Url::parse(base).ok()?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"));
    if !local || url.scheme() != "http" {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let capability = match client.get(format!("{base}/voice/capabilities")).send() {
        Ok(v) => v,
        Err(_) => return Some(Err("offline".into())),
    };
    if capability.status().as_u16() == 404 {
        return None;
    }
    if !capability.status().is_success() {
        return Some(Err(format!("http_{}", capability.status().as_u16())));
    }
    if capability.json::<Value>().ok()?["format"] != "candid-voice-jobs/1" {
        return None;
    }
    Some(run(store, r, audio, &client, base))
}
fn run(
    store: &SharedStore,
    r: &Value,
    audio: Option<&[u8]>,
    client: &reqwest::blocking::Client,
    base: &str,
) -> Result<(Value, Value), String> {
    let bytes = match audio {
        Some(b) => b.to_vec(),
        None => std::fs::read(r["path"].as_str().ok_or("missing_source")?)
            .map_err(|_| "missing_source")?,
    };
    let identity = format!(
        "{}:{}:{}",
        r["job"]["id"].as_str().unwrap_or(""),
        r["job"]["attempt"],
        r["digest"].as_str().unwrap_or("")
    );
    let form = reqwest::blocking::multipart::Form::new()
        .part(
            "file",
            reqwest::blocking::multipart::Part::bytes(bytes).file_name("audio.wav"),
        )
        .text(
            "mode",
            r["mode"]
                .as_str()
                .unwrap_or_else(|| {
                    if r["assignment"]["role"] == "diarization" {
                        "conversation"
                    } else {
                        "quick"
                    }
                })
                .to_string(),
        )
        .text("request_id", identity);
    let response = client
        .post(format!("{base}/voice/jobs"))
        .timeout(Duration::from_secs(15))
        .multipart(form)
        .send()
        .map_err(|_| "offline")?;
    if !response.status().is_success() {
        return Err(format!("http_{}", response.status().as_u16()));
    }
    let mut state: Value = response.json().map_err(|_| "bad_json")?;
    let id = state["id"]
        .as_str()
        .filter(|s| s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .ok_or("bad_job_id")?
        .to_string();
    let started = Instant::now();
    let limit = r["assignment"]["limits_json"]["timeout_ms"]
        .as_u64()
        .unwrap_or(120000)
        .clamp(50, 120000);
    let mut previous = Value::Null;
    let result = (|| {
        loop {
            let allowed = lock(store).workflow_reply_allowed(r).is_ok();
            if !allowed || started.elapsed() > Duration::from_millis(limit) {
                let cancel = client
                    .delete(format!("{base}/voice/jobs/{id}"))
                    .timeout(Duration::from_secs(6))
                    .send();
                let confirmed = cancel.is_ok_and(|v| v.status().is_success());
                if !allowed {
                    let _ = lock(store).media_job_stopped(&r["job"], confirmed);
                }
                return Err(if !allowed {
                    if confirmed {
                        "cancelled"
                    } else {
                        "cancel_unconfirmed"
                    }
                } else {
                    "timeout"
                }
                .into());
            }
            match state["state"].as_str() {
                Some("done") => {
                    return crate::workflows::checked_speech(state["result"].clone(), r)
                }
                Some("failed") => {
                    return Err(state["error"].as_str().unwrap_or("inference_failed").into())
                }
                Some("cancelled") => return Err("cancelled".into()),
                Some("running" | "cancelling") => {}
                _ => return Err("bad_job_state".into()),
            }
            let progress = json!({"stage":state["stage"],"started_ms":r["job"]["updated_ms"],"completed_seconds":state["completed_seconds"].as_f64().map(|s|s+r["audio_offset_seconds"].as_f64().unwrap_or(0.)),"total_seconds":r.get("audio_total_seconds").unwrap_or(&state["total_seconds"]),"updated_ms":state["updated_ms"],"cancellation_supported":true});
            if progress != previous {
                lock(store)
                    .media_job_progress(&r["job"], &progress)
                    .map_err(|e| e.message())?;
                previous = progress;
            }
            std::thread::sleep(Duration::from_millis(250));
            let response = client
                .get(format!("{base}/voice/jobs/{id}"))
                .send()
                .map_err(|_| "offline")?;
            if !response.status().is_success() {
                return Err(format!("http_{}", response.status().as_u16()));
            }
            // Bounded response including transcript; no unlimited JSON allocations.
            use std::io::Read;
            let mut bytes = Vec::new();
            response
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "response_read")?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err("output_limit".into());
            }
            state = serde_json::from_slice(&bytes).map_err(|_| "bad_json")?;
        }
    })();
    if result.is_err() {
        let confirmed = client
            .delete(format!("{base}/voice/jobs/{id}"))
            .timeout(Duration::from_secs(6))
            .send()
            .is_ok_and(|v| v.status().is_success());
        let _ = lock(store).media_job_stopped(&r["job"], confirmed);
    }
    result
}
