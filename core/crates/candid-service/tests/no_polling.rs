//! No new fixed-period loops (power v1 §3.5). Background work wakes on events: a write, a change-feed event, a
//! notification, or a computed due time. This test scans the Rust core (`core/crates/*/src`) and the Mac App
//! (`apps/macos/Sources`) for a sleep or timed wait inside a loop, and for repeating timers, and fails on any that
//! is not in `core/fixtures/polling/allowlist.txt`. Every allowlist entry says why that loop is acceptable (a
//! bounded retry, a deadline, an animation that runs only while visible, ...). Adding one is a design decision:
//! prefer an event, then a due time; see CONTRIBUTING.md.
use std::path::{Path, PathBuf};

/// Calls that sleep for a while: flagged when inside a loop.
const RUST_SLEEPS: &[&str] = &["sleep(", "park_timeout(", "wait_timeout(", "recv_timeout(", "sleep_until("];
const SWIFT_SLEEPS: &[&str] = &["Task.sleep(", ".sleep(for:", ".sleep(until:", "usleep(", "Thread.sleep(", "sleep(", "asyncAfter("];
/// Repeating timers: flagged anywhere.
const SWIFT_TIMERS: &[&str] = &["Timer.scheduledTimer(", "Timer.publish(", "TimelineView(.periodic", "TimelineView(.animation", "makeTimerSource(", "repeats: true"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hit {
    path: String,
    line: usize,
    code: String,
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name != "tests" && name != "target" && !name.starts_with('.') {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// The line with string literals and `//` comments blanked, so braces and keywords inside them do not count.
fn code_only(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_string = false;
                out.push('"');
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push('"');
        } else if c == '/' && chars.peek() == Some(&'/') {
            break;
        } else {
            out.push(c);
        }
    }
    out
}

fn is_loop_header(text: &str) -> bool {
    let words: Vec<&str> = text.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
    words.iter().any(|w| matches!(*w, "while" | "loop" | "for" | "repeat"))
}

/// Lines that sleep inside a loop body (any enclosing block opened by `while`/`loop`/`for`/`repeat`), and repeating
/// timers anywhere. Rust code after `#[cfg(test)]` is skipped.
fn scan(path: &Path, rel: &str, swift: bool) -> Vec<Hit> {
    let text = std::fs::read_to_string(path).unwrap();
    let mut hits = vec![];
    // One entry per open brace: whether it opened a loop body.
    let mut stack: Vec<bool> = vec![];
    let mut header = String::new();
    for (i, raw) in text.lines().enumerate() {
        if !swift && raw.trim_start().starts_with("#[cfg(test)]") {
            break;
        }
        let line = code_only(raw);
        let trimmed = raw.trim();
        let in_loop = stack.iter().any(|l| *l) || {
            // A one-line loop: `while x { sleep(...) }`.
            line.find('{').is_some_and(|b| is_loop_header(&line[..b]))
        };
        let sleeps = if swift { SWIFT_SLEEPS } else { RUST_SLEEPS };
        let sleeping = sleeps.iter().any(|s| line.contains(s));
        let timer = swift && SWIFT_TIMERS.iter().any(|s| line.contains(s));
        if timer || (sleeping && in_loop) {
            hits.push(Hit { path: rel.to_string(), line: i + 1, code: trimmed.to_string() });
        }
        for (j, c) in line.char_indices() {
            match c {
                '{' => {
                    let before = format!("{header} {}", &line[..j]);
                    stack.push(is_loop_header(before.rsplit([';', '}']).next().unwrap_or("")));
                    header.clear();
                }
                '}' => {
                    stack.pop();
                    header.clear();
                }
                _ => {}
            }
        }
        // A header can span lines (`while a,\n b {`): keep the text since the last brace or statement end.
        let tail = line.rsplit(['{', '}', ';']).next().unwrap_or("");
        if line.contains(['{', '}', ';']) {
            header = tail.to_string();
        } else {
            header.push(' ');
            header.push_str(tail);
        }
    }
    hits
}

struct Allowed {
    path: String,
    code: String,
    reason: String,
}

fn allowlist(repo: &Path) -> Vec<Allowed> {
    let text = std::fs::read_to_string(repo.join("core/fixtures/polling/allowlist.txt")).unwrap();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let parts: Vec<&str> = l.splitn(3, " | ").collect();
            assert_eq!(parts.len(), 3, "allowlist line needs `path | code | reason`: {l}");
            Allowed { path: parts[0].trim().into(), code: parts[1].trim().into(), reason: parts[2].trim().into() }
        })
        .collect()
}

fn all_hits(repo: &Path) -> Vec<Hit> {
    let mut hits = vec![];
    let mut rust = vec![];
    for krate in std::fs::read_dir(repo.join("core/crates")).unwrap().flatten() {
        files(&krate.path().join("src"), "rs", &mut rust);
    }
    let mut swift = vec![];
    files(&repo.join("apps/macos/Sources"), "swift", &mut swift);
    for (list, is_swift) in [(rust, false), (swift, true)] {
        for p in list {
            let rel = p.strip_prefix(repo).unwrap().to_string_lossy().to_string();
            hits.extend(scan(&p, &rel, is_swift));
        }
    }
    hits
}

#[test]
fn no_fixed_period_loop_outside_the_allowlist() {
    let repo = repo();
    let allowed = allowlist(&repo);
    let hits = all_hits(&repo);
    let unexplained: Vec<String> = hits
        .iter()
        .filter(|h| !allowed.iter().any(|a| a.path == h.path && a.code == h.code))
        .map(|h| format!("{} | {} | <why this loop is acceptable>    (line {})", h.path, h.code, h.line))
        .collect();
    assert!(
        unexplained.is_empty(),
        "fixed-period loops or timers not in core/fixtures/polling/allowlist.txt. Wake on an event or a computed due time \
         instead; if the loop is truly needed, add the line below with its reason:\n{}",
        unexplained.join("\n")
    );
    let stale: Vec<String> = allowed
        .iter()
        .filter(|a| !hits.iter().any(|h| h.path == a.path && h.code == a.code))
        .map(|a| format!("{} | {}", a.path, a.code))
        .collect();
    assert!(stale.is_empty(), "allowlist entries that no longer match any code (remove them):\n{}", stale.join("\n"));
    for a in &allowed {
        assert!(a.reason.chars().count() >= 8, "allowlist entry needs a real reason: {} | {}", a.path, a.code);
    }
}

#[test]
fn the_scanner_finds_loops_and_timers_but_not_one_shot_waits() {
    let dir = std::env::temp_dir().join(format!("no-polling-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rust = dir.join("a.rs");
    std::fs::write(&rust, "fn a() {\n    loop {\n        std::thread::sleep(d);\n    }\n}\nfn b() { std::thread::sleep(d); }\nfn c() {\n    while x\n        && y {\n        park_timeout(d);\n    }\n}\n#[cfg(test)]\nmod tests { fn t() { loop { sleep(d); } } }\n").unwrap();
    let hits: Vec<usize> = scan(&rust, "a.rs", false).iter().map(|h| h.line).collect();
    assert_eq!(hits, [3, 10]);
    let swift = dir.join("b.swift");
    std::fs::write(&swift, "func a() {\n    Task {\n        while !Task.isCancelled {\n            try? await Task.sleep(for: .seconds(5)) // \"{\"\n        }\n    }\n}\nfunc b() { Task { try? await Task.sleep(for: .milliseconds(300)) } }\nlet t = Timer.publish(every: 1, on: .main, in: .common)\n").unwrap();
    let hits: Vec<usize> = scan(&swift, "b.swift", true).iter().map(|h| h.line).collect();
    assert_eq!(hits, [4, 9]);
    let _ = std::fs::remove_dir_all(&dir);
}
