//! Snapshot files under `core/fixtures/`: compare, or rewrite with `UPDATE_SNAPSHOTS=1`.
//!
//! Values are normalised first: every UUID and every run of 24+ hex digits, or 10+ mixing digits and letters or following a prefix like `it-` (ids,
//! hashes that cover ids), becomes `<id>` / `<hex>`, and `sources` lists are sorted, so a snapshot does not change
//! from run to run (ids are random, and lists ordered by id come back in any order).
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn updating() -> bool {
    std::env::var("UPDATE_SNAPSHOTS").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn is_hex(c: char) -> bool {
    c.is_ascii_digit() || ('a'..='f').contains(&c)
}

fn is_uuid(s: &[char]) -> bool {
    s.len() == 36 && s.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == '-' } else { is_hex(*c) })
}

fn normalise_str(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if i + 36 <= chars.len() && is_uuid(&chars[i..i + 36]) && (i == 0 || !is_hex(chars[i - 1])) {
            out += "<id>";
            i += 36;
            continue;
        }
        if is_hex(chars[i]) && (i == 0 || !chars[i - 1].is_ascii_alphanumeric()) {
            let end = (i..chars.len()).find(|&j| !is_hex(chars[j])).unwrap_or(chars.len());
            let run = &chars[i..end];
            let mixed = run.iter().any(|c| c.is_ascii_digit()) && run.iter().any(|c| c.is_ascii_alphabetic());
            // A prefixed id (`it-…`) is random hex that can happen to be all digits (about 1 run in 100).
            let prefixed = i >= 2 && chars[i - 1] == '-' && chars[i - 2].is_ascii_alphabetic();
            if (end - i >= 24 || (end - i >= 10 && (mixed || prefixed))) && (end == chars.len() || !chars[end].is_ascii_alphanumeric()) {
                out += "<hex>";
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub fn normalise(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(normalise_str(s)),
        Value::Array(a) => Value::Array(a.iter().map(normalise).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    let mut x = normalise(x);
                    if let (Value::Array(a), "sources") = (&mut x, k.as_str()) {
                        a.sort_by_key(|e| e.to_string());
                    }
                    (normalise_str(k), x)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Collects mismatches so one run reports every changed snapshot.
#[derive(Default)]
pub struct Snapshots {
    failures: Vec<String>,
}

impl Snapshots {
    /// Compares `value` (normalised, pretty JSON) with `fixtures/<rel>`.
    pub fn json(&mut self, rel: &str, value: &Value) {
        let text = serde_json::to_string_pretty(&normalise(value)).unwrap() + "\n";
        self.text(rel, &text);
    }

    pub fn text(&mut self, rel: &str, text: &str) {
        let path = fixtures_dir().join(rel);
        if updating() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            return;
        }
        match std::fs::read_to_string(&path) {
            Err(_) => self.failures.push(format!("{rel}: snapshot missing")),
            Ok(old) if old != text => {
                let line = old.lines().zip(text.lines()).position(|(a, b)| a != b).unwrap_or(old.lines().count().min(text.lines().count()));
                let was = old.lines().nth(line).unwrap_or("<end>");
                let now = text.lines().nth(line).unwrap_or("<end>");
                self.failures.push(format!("{rel}: line {}\n    was: {was}\n    now: {now}", line + 1));
            }
            Ok(_) => {}
        }
    }

    /// Snapshot files under `fixtures/<dir>` that this run did not produce (left over from an older run).
    pub fn expect_only(&mut self, dir: &str, produced: &[String]) {
        if updating() {
            for e in std::fs::read_dir(fixtures_dir().join(dir)).into_iter().flatten().flatten() {
                let rel = format!("{dir}/{}", e.file_name().to_string_lossy());
                if !produced.contains(&rel) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            return;
        }
        for e in std::fs::read_dir(fixtures_dir().join(dir)).into_iter().flatten().flatten() {
            let rel = format!("{dir}/{}", e.file_name().to_string_lossy());
            if !produced.contains(&rel) {
                self.failures.push(format!("{rel}: stale snapshot (no longer produced)"));
            }
        }
    }

    pub fn finish(self) {
        if !self.failures.is_empty() {
            panic!(
                "{} snapshot(s) differ:\n  {}\n\nIf the change is intended (an interface or prompt really changed), regenerate with\n  UPDATE_SNAPSHOTS=1 cargo test -p candid-service --test lifecycle --test mcp_contract && UPDATE_SNAPSHOTS=1 cargo test -p candid-cli --test contract\nthen review the diff under core/fixtures/ and commit it. External agents depend on these interfaces.",
                self.failures.len(),
                self.failures.join("\n  ")
            );
        }
    }
}
