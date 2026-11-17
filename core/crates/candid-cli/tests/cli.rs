//! The `candid` binary as the App runs it: `serve` with a local socket, stopping when its parent goes away,
//! exit code 75 when another core holds the data root, and a query from the command line. Synthetic data only.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn dir(label: &str) -> PathBuf {
    let d = PathBuf::from(format!("/tmp/pv-cli-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_for(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn serve(d: &PathBuf, extra: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_candid"));
    c.args(["serve", "--root"]).arg(d.join("timeline")).arg("--run-dir").arg(d.join("run")).arg("--socket").arg(d.join("run/core.sock"));
    c.args(extra).stdout(Stdio::null()).stderr(Stdio::piped());
    c
}

#[test]
fn serve_stops_when_its_parent_is_gone_and_a_second_core_exits_busy() {
    let d = dir("parent");
    // Parent pid 1 is never our parent here, so the core stops by itself, cleaning up the socket and endpoint.
    let mut orphan = serve(&d, &["--parent-pid", "1"]).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(s) = orphan.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "serve kept running without its parent");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success());
    assert!(!d.join("run/core.sock").exists() && !d.join("run/endpoint.json").exists());

    let me = std::process::id().to_string();
    let mut first = serve(&d, &["--parent-pid", &me]).spawn().unwrap();
    wait_for(&d.join("run/core.sock"));
    wait_for(&d.join("run/endpoint.json"));
    let second = serve(&d, &[]).output().unwrap();
    assert_eq!(second.status.code(), Some(75), "{}", String::from_utf8_lossy(&second.stderr));
    // The command line queries the running core over the loopback HTTP, direct by setting.
    let out = Command::new(env!("CARGO_BIN_EXE_candid"))
        .args(["status"])
        .env("CANDID_RUN_DIR", d.join("run"))
        .env("CANDID_PROXY", "direct")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("store_id"));
    // SIGTERM: a clean stop.
    unsafe { libc::kill(first.id() as i32, libc::SIGTERM) };
    assert!(first.wait().unwrap().success());
    assert!(!d.join("run/core.sock").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_app_that_dies_with_the_stderr_pipe_still_gets_a_clean_shutdown() {
    // The shell plays the App: it starts the core with itself as parent and stderr as a pipe, then exits. The pipe is
    // closed before the core notices, so its last log line must not abort the shutdown.
    let d = dir("pipe");
    let script = format!(
        "\"$0\" serve --root '{}' --run-dir '{}' --socket '{}' --parent-pid $$ & sleep 1",
        d.join("timeline").display(), d.join("run").display(), d.join("run/core.sock").display()
    );
    let mut app = Command::new("/bin/sh").args(["-c", &script, env!("CARGO_BIN_EXE_candid")])
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    wait_for(&d.join("run/endpoint.json"));
    drop(app.stderr.take());
    app.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while d.join("run/endpoint.json").exists() || d.join("run/core.sock").exists() {
        assert!(Instant::now() < deadline, "the orphaned core did not clean up");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&d);
}
