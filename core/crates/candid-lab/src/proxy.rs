//! A loopback model endpoint for experiments: applies the experiment's request overrides (thinking on/off,
//! sampling), answers from a content-addressed cache, and forwards misses to the real upstream.
//!
//! The copy's model configuration points at this endpoint, so the production worker runs unchanged; replies are
//! cached by the exact request body, so a rule-only change replays for free and deterministically.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Proxy {
    pub upstream: String,
    pub token: Option<String>,
    pub cache: PathBuf,
    pub offline: bool,
    /// Merged into every request body (e.g. `{"chat_template_kwargs":{"enable_thinking":false}}`).
    pub request: Value,
    /// Per model id, merged after `request`.
    pub model_request: Value,
    pub log: Arc<Mutex<std::fs::File>>,
    /// Where requests that missed the cache in offline mode are written, to see why a replay diverged.
    pub misses: Option<PathBuf>,
}

/// Sorted keys, so the cache key does not depend on map order.
pub fn canonical(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<_> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sort(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            _ => v.clone(),
        }
    }
    sort(v).to_string()
}

pub fn merge(into: &mut Value, from: &Value) {
    if let (Some(a), Some(b)) = (into.as_object_mut(), from.as_object()) {
        for (k, v) in b {
            if v.is_object() && a.get(k).is_some_and(Value::is_object) {
                merge(a.get_mut(k).unwrap(), v);
            } else if v.is_null() {
                a.remove(k);
            } else {
                a.insert(k.clone(), v.clone());
            }
        }
    }
}

impl Proxy {
    /// Listens on 127.0.0.1 and on [::1] at the same port, so `lab.localhost` reaches it either way.
    pub fn start(self) -> u16 {
        let v4 = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
        let port = v4.local_addr().unwrap().port();
        let mut listeners = vec![v4];
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
        for listener in listeners {
            let p = self.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let p = p.clone();
                    std::thread::spawn(move || {
                        let _ = p.serve(stream);
                    });
                }
            });
        }
        port
    }

    fn serve(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
        let mut length = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h)?;
            if h == "\r\n" || h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        let (status, out) = if path.ends_with("/chat/completions") {
            match serde_json::from_slice::<Value>(&body) {
                Ok(v) => self.complete(v),
                Err(_) => (400, json!({"error": "bad json"})),
            }
        } else {
            (404, json!({"error": "lab proxy only serves chat completions"}))
        };
        let bytes = out.to_string();
        write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}",
            bytes.len()
        )?;
        stream.flush()
    }

    fn complete(&self, mut body: Value) -> (u16, Value) {
        merge(&mut body, &self.request);
        let model = body["model"].as_str().unwrap_or("").to_string();
        if let Some(extra) = self.model_request.get(&model) {
            merge(&mut body, extra);
        }
        let key = candid_store::sha256_hex(format!("{}\n{}", self.upstream, canonical(&body)).as_bytes());
        let file = self.cache.join(&key[..2]).join(format!("{key}.json"));
        let system = body["messages"][0]["content"].as_str().map(|s| candid_store::sha256_hex(s.as_bytes())[..12].to_string());
        let started = Instant::now();
        let (status, response, hit) = if let Some(cached) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            (200, cached["response"].clone(), true)
        } else if self.offline {
            if let Some(dir) = &self.misses {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join(format!("{key}.json")), serde_json::to_vec(&body).unwrap());
            }
            (503, json!({"error": "offline: not in cache"}), false)
        } else {
            match self.forward(&body) {
                Ok(response) => {
                    let _ = std::fs::create_dir_all(file.parent().unwrap());
                    let record = json!({"request": body, "response": response, "latency_ms": started.elapsed().as_millis() as u64});
                    let _ = std::fs::write(&file, serde_json::to_vec(&record).unwrap());
                    (200, response, false)
                }
                Err((code, e)) => (code, json!({"error": e}), false),
            }
        };
        let message = &response["choices"][0]["message"];
        let reasoning = message["reasoning_content"].as_str().or(message["reasoning"].as_str()).map(|s| s.chars().count()).unwrap_or(0);
        let record = json!({
            "key": key, "hit": hit, "status": status, "model": model, "system": system,
            "ms": started.elapsed().as_millis() as u64,
            "prompt_tokens": response["usage"]["prompt_tokens"], "completion_tokens": response["usage"]["completion_tokens"],
            "reasoning_chars": reasoning, "error": response["error"],
        });
        let _ = writeln!(self.log.lock().unwrap(), "{record}");
        (status, response)
    }

    fn forward(&self, body: &Value) -> Result<Value, (u16, String)> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1800))
            .build()
            .map_err(|e| (502, e.to_string()))?;
        let mut req = client.post(format!("{}/chat/completions", self.upstream.trim_end_matches('/'))).json(body);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().map_err(|e| (502, e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().map_err(|e| (502, e.to_string()))?;
        if status != 200 {
            return Err((status, text.chars().take(500).collect()));
        }
        serde_json::from_str(&text).map_err(|e| (502, e.to_string()))
    }
}
