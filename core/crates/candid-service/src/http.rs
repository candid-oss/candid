//! A minimal HTTP/1.1 server bound to 127.0.0.1 only: one request per connection, bodies up to [`MAX_BODY`].
//! Enough for local tools; anything reachable from other hosts needs authentication and TLS first (project rule 9).
//! Connections are served by a fixed pool of worker threads (no async runtime).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const MAX_BODY: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    /// Header names lowercased.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn error(status: u16, code: &str, message: &str) -> HttpResponse {
        let body = serde_json::json!({"error": {"code": code, "message": message}});
        HttpResponse { status, body: serde_json::to_vec(&body).unwrap_or_default() }
    }

    pub fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Payload Too Large",
            409 => "Conflict",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Status",
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            self.status,
            self.reason(),
            self.body.len()
        )
        .into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parse {
    Incomplete,
    Complete(HttpRequest),
    TooLarge,
    Invalid,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn parse(buffer: &[u8]) -> Parse {
    let Some(head_end) = find(buffer, b"\r\n\r\n") else {
        return if buffer.len() > 16 * 1024 { Parse::TooLarge } else { Parse::Incomplete };
    };
    let Ok(head) = std::str::from_utf8(&buffer[..head_end]) else { return Parse::Invalid };
    let mut lines = head.split("\r\n");
    let start: Vec<&str> = lines.next().unwrap_or("").split(' ').filter(|s| !s.is_empty()).collect();
    if start.len() != 3 || !start[2].starts_with("HTTP/1.") {
        return Parse::Invalid;
    }
    let mut headers = HashMap::new();
    for line in lines {
        let Some(colon) = line.find(':') else { return Parse::Invalid };
        headers.insert(line[..colon].to_lowercase().trim().to_string(), line[colon + 1..].trim().to_string());
    }
    if headers.contains_key("transfer-encoding") {
        return Parse::Invalid; // chunked bodies are not needed by any local client
    }
    let length: i64 = match headers.get("content-length") {
        Some(v) => v.parse().unwrap_or(-1),
        None => 0,
    };
    if length < 0 {
        return Parse::Invalid;
    }
    if length as usize > MAX_BODY {
        return Parse::TooLarge;
    }
    let body_start = head_end + 4;
    if buffer.len() - body_start < length as usize {
        return Parse::Incomplete;
    }
    Parse::Complete(HttpRequest {
        method: start[0].into(),
        path: start[1].into(),
        headers,
        body: buffer[body_start..body_start + length as usize].to_vec(),
    })
}

pub type Handler = Arc<dyn Fn(&HttpRequest, u16) -> HttpResponse + Send + Sync>;

pub struct LoopbackServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    workers: Vec<JoinHandle<()>>,
}

pub(crate) const CONNECTION_DEADLINE: Duration = Duration::from_secs(30);

/// A connected byte stream: TCP on loopback or a Unix domain socket.
pub trait Conn: Read + Write {
    fn read_timeout(&self, t: Option<Duration>) -> std::io::Result<()>;
    fn write_timeout(&self, t: Option<Duration>) -> std::io::Result<()>;
    fn close(&self);
}

impl Conn for TcpStream {
    fn read_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(t)
    }
    fn write_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        self.set_write_timeout(t)
    }
    fn close(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

impl Conn for std::os::unix::net::UnixStream {
    fn read_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(t)
    }
    fn write_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        self.set_write_timeout(t)
    }
    fn close(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

/// Reads one request within [`CONNECTION_DEADLINE`]. `None`: the client went away or never finished; `Err`: a
/// response to send instead (malformed or too large).
pub fn read_request<S: Conn>(stream: &mut S) -> Option<Result<HttpRequest, HttpResponse>> {
    let deadline = Instant::now() + CONNECTION_DEADLINE;
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.read_timeout(Some(left)).is_err() {
            return None; // a client that never finishes its request is dropped
        }
        let n = stream.read(&mut chunk).ok()?;
        buffer.extend_from_slice(&chunk[..n]);
        match parse(&buffer) {
            Parse::Complete(request) => return Some(Ok(request)),
            Parse::TooLarge => return Some(Err(HttpResponse::error(413, "too_large", "请求过大。"))),
            Parse::Invalid => return Some(Err(HttpResponse::error(400, "bad_request", "无法解析的 HTTP 请求。"))),
            Parse::Incomplete if n == 0 => return None,
            Parse::Incomplete => {}
        }
    }
}

pub fn write_response<S: Conn>(stream: &mut S, response: &HttpResponse) {
    let _ = stream.write_timeout(Some(CONNECTION_DEADLINE));
    let _ = stream.write_all(&response.to_bytes());
    let _ = stream.flush();
    stream.close();
}

fn serve(mut stream: TcpStream, handler: &Handler, port: u16) {
    let response = match read_request(&mut stream) {
        None => return,
        Some(Ok(request)) => handler(&request, port),
        Some(Err(response)) => response,
    };
    write_response(&mut stream, &response);
}

impl LoopbackServer {
    /// Starts listening on 127.0.0.1:`preferred_port` (0 = any free port) with `workers` serving threads.
    pub fn start(preferred_port: u16, workers: usize, handler: Handler) -> std::io::Result<LoopbackServer> {
        let listener = TcpListener::bind(("127.0.0.1", preferred_port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<TcpStream>();
        let rx = Arc::new(Mutex::new(rx));
        let mut pool = Vec::new();
        for i in 0..workers.max(1) {
            let rx = rx.clone();
            let handler = handler.clone();
            pool.push(
                std::thread::Builder::new()
                    .name(format!("candid-http-{i}"))
                    .spawn(move || loop {
                        let next = rx.lock().map(|r| r.recv());
                        match next {
                            Ok(Ok(stream)) => serve(stream, &handler, port),
                            _ => break,
                        }
                    })?,
            );
        }
        let stop_flag = stop.clone();
        let accept = std::thread::Builder::new().name("candid-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(s) = stream {
                    if tx.send(s).is_err() {
                        break;
                    }
                }
            }
        })?;
        Ok(LoopbackServer { port, stop, accept: Some(accept), workers: pool })
    }

    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // wakes the accept loop
        if let Some(a) = self.accept.take() {
            let _ = a.join();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop();
    }
}
