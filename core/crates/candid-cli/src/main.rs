//! candid — query the Candid timeline from a shell, serve it to an AI client over MCP (stdio), or run the
//! service without a user interface (`serve`). Except for `serve`, it never opens the database: everything goes
//! through the running service's loopback HTTP with a client token, following the App's 「网络代理」 setting.

mod migrate;

use candid_service::catalog::{self, Tool};
use candid_service::serve::{ServeOptions, Service, StartError};
use candid_service::{McpShell, RunDirectory, ServiceClient, VERSION};
use candid_store::json;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "用法:
  candid mcp                         作为 MCP 服务器运行（stdio），供 Claude Code、Codex 等接入
  candid tools                       列出可用工具
  candid <工具> [--参数 值 ...]        调用一个工具，输出 JSON
  candid <工具> --json '{...}'        用 JSON 传参数
  candid serve --root <数据根目录>     不带界面运行时间线服务（回环 HTTP，写 endpoint.json 与 cli.token）
        [--port 0] [--run-dir 目录] [--ledger 桌面账本目录] [--hub Hub目录] [--sync-interval 600] [--workers 2]
        [--socket 本地通道路径] [--parent-pid App进程号]   作为 App 的辅助进程运行时使用
        [--power ac|battery]          没有 App 上报时声明电源状态（默认未知，按电池处理）
  candid power status                后台处理档位与电源状态（经 App 核心的本地通道）
  candid power pace <anytime|saver|manual>   设置后台处理：随时 / 省电（默认）/ 手动
  candid energy summary [--start today] [--end now]   后台计算的能耗账（即 energy_summary 工具）
  candid encode-webp --width W --height H [--stride 每行字节] [--quality 60]
                                     从标准输入读 BGRA 像素，向标准输出写 WebP（App 截图用）

数据迁移（见 docs/data-migration.md；默认支持目录 ~/Library/Application Support/Candid，可用 --support 改）:
  candid backup <目标目录> [--no-media]       一致性快照 + 原件 + 清单，App 运行中也能做
  candid restore <备份目录> [--move-existing]  校验后恢复；已有数据默认拒绝，--move-existing 先移到 before-restore-<时间>
  candid move-root <新位置> [--remove-old]    把时间线库搬到新位置（如外置盘），原处留符号链接
  以上命令加 --json 时逐行输出进度和结果 JSON

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
            "--power" => opts.power = Some(match value().as_str() {
                "ac" => true,
                "battery" => false,
                _ => fail("--power 只能是 ac 或 battery", 2),
            }),
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
    wait_for_stop(opts.parent_pid);
    service.stop();
}

/// Blocks until SIGINT/SIGTERM or until the App that launched us exits. On macOS the kernel tells us (kqueue), so
/// the idle core never wakes for this; elsewhere it checks every 200 ms.
#[cfg(target_os = "macos")]
fn wait_for_stop(parent: Option<i32>) {
    let event = |ident: usize, filter: i16, flags: u16, fflags: u32| libc::kevent { ident, filter, flags, fflags, data: 0, udata: std::ptr::null_mut() };
    // SAFETY: a kqueue we own; the events name our own signals and the parent's pid.
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return wait_for_stop_polling(parent);
        }
        let mut changes = vec![event(libc::SIGINT as usize, libc::EVFILT_SIGNAL, libc::EV_ADD, 0), event(libc::SIGTERM as usize, libc::EVFILT_SIGNAL, libc::EV_ADD, 0)];
        if let Some(p) = parent {
            changes.push(event(p as usize, libc::EVFILT_PROC, libc::EV_ADD | libc::EV_ONESHOT, libc::NOTE_EXIT));
        }
        libc::kevent(kq, changes.as_ptr(), changes.len() as i32, std::ptr::null_mut(), 0, std::ptr::null());
        loop {
            // Checked after registering, so an exit just before it is not missed.
            if parent.is_some_and(|p| libc::getppid() != p) {
                log("启动它的 App 已退出，核心随之停止。");
                break;
            }
            if STOP.load(Ordering::SeqCst) {
                break;
            }
            let mut out = event(0, 0, 0, 0);
            // Blocks with no timeout; a signal also interrupts it (EINTR), and the loop checks again.
            libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, std::ptr::null());
        }
        libc::close(kq);
    }
}

#[cfg(not(target_os = "macos"))]
fn wait_for_stop(parent: Option<i32>) {
    wait_for_stop_polling(parent)
}

fn wait_for_stop_polling(parent: Option<i32>) {
    while !STOP.load(Ordering::SeqCst) {
        // SAFETY: getppid has no preconditions.
        if parent.is_some_and(|p| unsafe { libc::getppid() } != p) {
            log("启动它的 App 已退出，核心随之停止。");
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `power status` and `power pace <p>`: the running core's power gate, through its local channel in the run
/// directory (the App's core, or `candid serve --socket`). Printed as JSON.
fn power(args: &[String]) {
    let body = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["status"] => serde_json::json!({"action": "status"}),
        ["pace", pace] if candid_store::power::PACES.contains(pace) => serde_json::json!({"action": "set_pace", "pace": pace}),
        _ => fail(&format!("用法：candid power status | candid power pace <anytime|saver|manual>\n\n{USAGE}"), 2),
    };
    let socket = candid_service::local::default_socket(&RunDirectory::path(&env()));
    match candid_service::local_request(&socket, "POST", "/local/power", body.to_string().as_bytes()) {
        Ok((200, data)) => {
            let v: Value = serde_json::from_slice(&data).unwrap_or(Value::Null);
            println!("{}", json::pretty(&v));
        }
        Ok((_, data)) => fail(&String::from_utf8_lossy(&data), 1),
        Err(e) => fail(&format!("连不上核心的本地通道（{}）：{e}。请先打开 Candid，或用 candid serve --socket 启动。", socket.display()), 1),
    }
}

/// `encode-webp`: raw BGRA on stdin, WebP on stdout. The App calls it per screenshot (data model v1 §9.1); it opens
/// nothing else and needs no running service.
fn encode_webp(args: &[String]) {
    let (mut width, mut height, mut stride, mut quality) = (0u32, 0u32, None, candid_store::webp::DEFAULT_QUALITY);
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().unwrap_or_else(|| fail(&format!("{flag} 缺少值"), 2));
        let number = || value.parse::<u64>().unwrap_or_else(|_| fail(&format!("{flag} 需要整数"), 2));
        match flag.as_str() {
            "--width" => width = number().min(u32::MAX as u64) as u32,
            "--height" => height = number().min(u32::MAX as u64) as u32,
            "--stride" => stride = Some(number() as usize),
            "--quality" => quality = value.parse().unwrap_or_else(|_| fail("--quality 需要数字", 2)),
            other => fail(&format!("encode-webp 没有参数 {other}"), 2),
        }
    }
    let stride = stride.unwrap_or(width as usize * 4);
    let limit = stride.saturating_mul(height as usize);
    if limit == 0 || limit > 1 << 30 {
        fail("图片尺寸无效", 2);
    }
    let mut pixels = Vec::with_capacity(limit);
    if std::io::stdin().lock().take(limit as u64 + 1).read_to_end(&mut pixels).is_err() {
        fail("读取像素失败", 1);
    }
    if pixels.len() > limit {
        fail("像素数据多于图片尺寸", 2);
    }
    match candid_store::webp::encode_bgra(&pixels, width, height, stride, quality) {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            if stdout.write_all(&out).and_then(|_| stdout.flush()).is_err() {
                fail("写出 WebP 失败", 1);
            }
        }
        Err(e) => fail(&e, 1),
    }
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
        "power" => power(&argv[1..]),
        "encode-webp" => encode_webp(&argv[1..]),
        "backup" | "restore" | "move-root" => std::process::exit(migrate::run(command, &argv[1..])),
        name => {
            // `candid energy summary` is the `energy_summary` tool.
            let (name, rest) = if name == "energy" && argv.get(1).map(String::as_str) == Some("summary") { ("energy_summary", &argv[2..]) } else { (name, &argv[1..]) };
            let Some(tool) = catalog::tool(name) else { fail(&format!("没有这个工具：{name}\n\n{USAGE}"), 2) };
            let body = arguments(&tool, rest);
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
