//! Thin HTTP client for the loopback timeline service, used by the command line and the MCP shell.
//!
//! Connections follow Candid's 「网络代理」 setting ([`crate::proxy`]), read on each call as the App does: `system`
//! takes the route macOS would choose for the endpoint, `direct` never uses a proxy.

use crate::endpoint::RunDirectory;
use crate::proxy::{self, ProxyMode, Route};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallError {
    pub status: u16,
    pub message: String,
}

impl ToolCallError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        ToolCallError { status, message: message.into() }
    }
}

impl std::fmt::Display for ToolCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[derive(Clone, Debug)]
pub struct ServiceClient {
    pub base_url: String,
    token: String,
    /// `CANDID_PROXY` when set (otherwise the App's setting is read per call).
    proxy_override: Option<ProxyMode>,
    /// Fixed route for tests.
    route_override: Option<Route>,
}

struct Target {
    host_port: String,
    host: String,
    port: u16,
    path: String,
}

fn parse_base(url: &str) -> Option<Target> {
    let rest = url.strip_prefix("http://")?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if host_port.is_empty() {
        return None;
    }
    let host_port = if host_port.contains(':') && !host_port.ends_with(']') { host_port.to_string() } else { format!("{host_port}:80") };
    let (host, port) = host_port.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    Some(Target { host_port, host, port, path: path.to_string() })
}

impl ServiceClient {
    pub fn new(base_url: &str, token: &str) -> ServiceClient {
        ServiceClient { base_url: base_url.into(), token: token.into(), proxy_override: None, route_override: None }
    }

    /// Uses `route` for every call instead of resolving the proxy setting (tests).
    pub fn with_route(mut self, route: Route) -> ServiceClient {
        self.route_override = Some(route);
        self
    }

    /// The route the next call takes.
    pub fn route(&self) -> Route {
        if let Some(r) = &self.route_override {
            return r.clone();
        }
        let mode = self.proxy_override.unwrap_or_else(|| ProxyMode::current(&HashMap::new()));
        proxy::route_for(mode, &self.base_url)
    }

    /// Resolves the endpoint and token the usual way: `CANDID_ENDPOINT` / `CANDID_TOKEN` when set, otherwise the
    /// run directory's `endpoint.json` and the command-line token file.
    pub fn discover(env: &HashMap<String, String>) -> Result<ServiceClient, ToolCallError> {
        let dir = RunDirectory::path(env);
        let base = match env.get("CANDID_ENDPOINT").filter(|v| !v.is_empty()) {
            Some(b) => b.clone(),
            None => RunDirectory::read_endpoint(&dir).map(|e| e.url).map_err(|_| {
                ToolCallError::new(
                    503,
                    format!("找不到正在运行的 Candid 时间线服务（{}）。请先打开 Candid。", RunDirectory::endpoint_file(&dir).display()),
                )
            })?,
        };
        let mut token = env.get("CANDID_TOKEN").cloned().unwrap_or_default();
        if token.is_empty() {
            token = std::fs::read_to_string(RunDirectory::cli_token_file(&dir)).map(|t| t.trim().to_string()).unwrap_or_default();
        }
        if token.is_empty() {
            return Err(ToolCallError::new(401, "没有访问令牌：设置 CANDID_TOKEN，或在 Candid 里登记这个访问方。"));
        }
        if parse_base(&base).is_none() {
            return Err(ToolCallError::new(400, format!("服务地址无效：{base}")));
        }
        let mut client = ServiceClient::new(&base, &token);
        client.proxy_override = env.get("CANDID_PROXY").filter(|v| !v.is_empty()).map(|v| ProxyMode::from_value(Some(v)));
        Ok(client)
    }

    /// POSTs the arguments to `/v1/<tool>`. A non-2xx answer becomes `ToolCallError` carrying the service's message.
    pub fn call(&self, tool: &str, arguments: &[u8]) -> Result<Vec<u8>, ToolCallError> {
        let target = parse_base(&self.base_url).ok_or_else(|| ToolCallError::new(400, format!("服务地址无效：{}", self.base_url)))?;
        let unreachable = |e: std::io::Error| ToolCallError::new(503, format!("连不上 Candid 时间线服务：{e}"));
        let route = self.route();
        let (connect_host, connect_port) = match &route {
            Route::Direct => (target.host.clone(), target.port),
            Route::Http { host, port } | Route::Socks { host, port } => (host.clone(), *port),
        };
        let addr = (connect_host.as_str(), connect_port)
            .to_socket_addrs()
            .map_err(unreachable)?
            .next()
            .ok_or_else(|| ToolCallError::new(503, "连不上 Candid 时间线服务：地址无法解析"))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(unreachable)?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
        if matches!(route, Route::Socks { .. }) {
            socks5_connect(&mut stream, &target.host, target.port).map_err(unreachable)?;
        }
        let request_target = match route {
            Route::Http { .. } => format!("http://{}{}/v1/{tool}", target.host_port, target.path),
            _ => format!("{}/v1/{tool}", target.path),
        };
        let head = format!(
            "POST {request_target} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            target.host_port,
            self.token,
            arguments.len()
        );
        stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(arguments)).map_err(unreachable)?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(unreachable)?;
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| ToolCallError::new(502, "服务返回的 HTTP 响应无效"))?;
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let status: u16 = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut body = raw[split + 4..].to_vec();
        if let Some(len) = head
            .lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<usize>().ok()))
        {
            body.truncate(len);
        }
        if !(200..300).contains(&status) {
            let message = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).map(String::from));
            return Err(ToolCallError::new(status, message.unwrap_or_else(|| format!("服务返回 {status}"))));
        }
        Ok(body)
    }
}

/// SOCKS5 CONNECT without authentication (RFC 1928).
fn socks5_connect(stream: &mut TcpStream, host: &str, port: u16) -> std::io::Result<()> {
    let bad = |m: &str| std::io::Error::other(format!("SOCKS 代理：{m}"));
    stream.write_all(&[5, 1, 0])?;
    let mut reply = [0u8; 2];
    stream.read_exact(&mut reply)?;
    if reply != [5, 0] {
        return Err(bad("不接受无认证连接"));
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            request.push(1);
            request.extend_from_slice(&ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            request.push(4);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            if host.len() > 255 {
                return Err(bad("主机名过长"));
            }
            request.push(3);
            request.push(host.len() as u8);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request)?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head)?;
    if head[1] != 0 {
        return Err(bad(&format!("连接被拒绝（{}）", head[1])));
    }
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut n = [0u8; 1];
            stream.read_exact(&mut n)?;
            n[0] as usize
        }
        _ => return Err(bad("无效应答")),
    };
    let mut rest = vec![0u8; skip + 2];
    stream.read_exact(&mut rest)
}
