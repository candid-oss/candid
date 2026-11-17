//! candid — query the Candid timeline from a shell, serve it to an AI client over MCP (stdio), or run the
//! service without a user interface (`serve`). Except for `serve`, it never opens the database: everything goes
//! through the running service's loopback HTTP with a client token, following the App's 「网络代理」 setting.

use candid_service::catalog::{self, Tool};
use candid_service::serve::{ServeOptions, Service, StartError};
use candid_service::{McpShell, RunDirectory, ServiceClient, VERSION};
use candid_store::json;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "用法:
  candid mcp                         作为 MCP 服务器运行（stdio），供 Claude Code、Codex 等接入
  candid tools                       列出可用工具
  candid <工具> [--参数 值 ...]        调用一个工具，输出 JSON
  candid <工具> --json '{...}'        用 JSON 传参数
  candid serve --root <数据根目录>     不带界面运行时间线服务（回环 HTTP，写 endpoint.json 与 cli.token）
        [--port 0] [--run-dir 目录] [--ledger 桌面账本目录] [--hub Hub目录] [--sync-interval 30] [--workers 2]
        [--socket 本地通道路径] [--parent-pid App进程号]   作为 App 的辅助进程运行时使用

例子:
  candid overview --start today
  candid search --q 预算 --start -3d --limit 5
  candid context --at \"2026-11-10T15:00:00+08:00\"

令牌与地址: 环境变量 CANDID_TOKEN、CANDID_ENDPOINT、CANDID_RUN_DIR；未设置时读取
~/Library/Application Support/Candid/run/ 下的 endpoint.json 与 cli.token。";

fn fail(message: &str, code: i32) -> ! {
    log(message);
    std::process::exit(code)
}

/// `--key value` pairs become JSON arguments; numbers and booleans are typed by the tool's schema, arrays split on commas.
fn arguments(tool: &Tool, args: &[String]) -> Vec<u8> {
    if let Some(i) = args.iter().position(|a| a == "--json") {
        if i + 1 < args.len() {
            return args[i + 1].clone().into_bytes();
        }
    }
    let schema: Value = serde_json::from_str(tool.input_schema).unwrap_or(Value::Null);
    let props = schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut object = Map::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let Some(key) = flag.strip_prefix("--") else { fail(&format!("无法理解的参数：{flag}\n\n{USAGE}"), 2) };
        let key = key.replace('-', "_");
        let Some(spec) = props.get(&key) else {
            let mut keys: Vec<&String> = props.keys().collect();
            keys.sort();
            fail(&format!("{} 没有参数 {key}。可用：{}", tool.name, keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")), 2)
        };
        let value = rest.next().cloned().unwrap_or_else(|| "true".into());
        let typed = match spec.get("type").and_then(Value::as_str) {
            Some("integer") => Value::from(value.parse::<i64>().unwrap_or_else(|_| fail(&format!("{key} 需要整数"), 2))),
            Some("boolean") => Value::from(["1", "true", "yes"].contains(&value.to_lowercase().as_str())),
            Some("array") => Value::from(value.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()),
            Some("object") => serde_json::from_str(&value).unwrap_or_else(|_| fail(&format!("{key} 需要 JSON 对象"), 2)),
            _ => Value::from(value),
        };
        object.insert(key, typed);
    }
    serde_json::to_vec(&Value::Object(object)).unwrap_or_else(|_| b"{}".to_vec())
}

fn env() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// `EX_TEMPFAIL`: another core already serves this data root or socket.
const EXIT_BUSY: i32 = 75;

/// Writes a line to stderr, ignoring errors: as the App's helper, stderr is a pipe that closes when the App exits, and
/// `eprintln!` would panic before the core could shut down cleanly.
fn log(line: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn serve(args: &[String]) {
    let mut opts = ServeOptions::new(PathBuf::new(), RunDirectory::path(&env()));
    let mut once = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_else(|| fail(&format!("{flag} 需要一个值"), 2));
        match flag.as_str() {
            "--root" => opts.root = PathBuf::from(value()),
            "--run-dir" => opts.run_dir = PathBuf::from(value()),
            "--port" => opts.port = value().parse().unwrap_or_else(|_| fail("--port 需要整数", 2)),
            "--workers" => opts.workers = value().parse().unwrap_or_else(|_| fail("--workers 需要整数", 2)),
            "--ledger" => opts.ledger = Some(PathBuf::from(value())),
            "--hub" => opts.hub = Some(PathBuf::from(value())),
            "--sync-interval" => opts.sync_interval = Duration::from_secs(value().parse().unwrap_or_else(|_| fail("--sync-interval 需要秒数", 2))),
            "--socket" => opts.socket = Some(PathBuf::from(value())),
            "--parent-pid" => opts.parent_pid = Some(value().parse().unwrap_or_else(|_| fail("--parent-pid 需要整数", 2))),
            "--cli-path" => opts.cli_path = Some(value()),
            "--once" => once = true,
            other => fail(&format!("serve 没有参数 {other}\n\n{USAGE}"), 2),
        }
    }
    if opts.root.as_os_str().is_empty() {
        fail("serve 需要 --root <数据根目录>", 2);
    }
    if once {
        // One mirroring pass without serving, for scripts and measurements.
        let store = candid_store::TimelineStore::open(&opts.root, &opts.created_by, true, candid_store::Clock::default())
            .unwrap_or_else(|e| fail(&e.message(), 1));
        for line in candid_service::serve::sync_once(&store, opts.ledger.as_deref(), opts.hub.as_deref()) {
            log(&line);
        }
        return;
    }
    // SAFETY: installing a handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    let service = Service::start_checked(&opts, |line| log(line)).unwrap_or_else(|e| match e {
        // Another core holds the data root or the socket: the App backs off and tries again.
        StartError::Busy(m) => fail(&format!("另一个核心正在运行：{m}"), EXIT_BUSY),
        StartError::Failed(m) => fail(&m, 1),
    });
    log(&format!("candid {VERSION} 服务：{}（{}）", service.endpoint.url, RunDirectory::endpoint_file(&opts.run_dir).display()));
    if let Some(socket) = &opts.socket {
        log(&format!("本地通道：{}", socket.display()));
    }
    while !STOP.load(Ordering::SeqCst) {
        // SAFETY: getppid has no preconditions.
        if opts.parent_pid.is_some_and(|p| unsafe { libc::getppid() } != p) {
            log("启动它的 App 已退出，核心随之停止。");
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    service.stop();
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = argv.first() else {
        println!("{USAGE}");
        return;
    };
    match command.as_str() {
        "-h" | "--help" | "help" => println!("{USAGE}"),
        "--version" | "version" => println!("candid {VERSION}"),
        "tools" => {
            for t in catalog::tools() {
                println!("{}\t{}", t.name, t.description.split('。').next().unwrap_or(""));
            }
        }
        "mcp" => {
            // Endpoint and token are resolved per call, so starting the service after the AI client still works.
            let shell = McpShell::new(VERSION, Box::new(|tool, body| ServiceClient::discover(&env())?.call(tool, body)));
            let stdin = std::io::stdin();
            shell.run(stdin.lock(), std::io::stdout());
        }
        "serve" => serve(&argv[1..]),
        name => {
            let Some(tool) = catalog::tool(name) else { fail(&format!("没有这个工具：{name}\n\n{USAGE}"), 2) };
            let body = arguments(&tool, &argv[1..]);
            let result = ServiceClient::discover(&env()).and_then(|c| c.call(tool.name, &body));
            match result {
                Ok(data) => {
                    let mut out = std::io::stdout();
                    match serde_json::from_slice::<Value>(&data) {
                        Ok(v) => {
                            let _ = writeln!(out, "{}", json::pretty(&v));
                        }
                        Err(_) => {
                            let _ = out.write_all(&data);
                        }
                    }
                }
                Err(e) => fail(&e.message, if e.status == 401 || e.status == 403 { 3 } else { 1 }),
            }
        }
    }
}
