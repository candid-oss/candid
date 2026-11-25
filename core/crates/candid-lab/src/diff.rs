//! `diff`: what changed between two runs, per conversation (messages aligned by an LCS on speaker + text),
//! plus items, memos, activity titles and the counts.

use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write;

fn key(m: &Value) -> String {
    let who = if m["actor"] == "self" { "我".to_string() } else { m["sender"].as_str().unwrap_or("?").to_string() };
    format!("{who}：{}", m["text"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Lines only in a (`-`) and only in b (`+`), in order.
fn lcs_diff(a: &[String], b: &[String]) -> Vec<(char, String)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, vec![]);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || t[i][j + 1] >= t[i + 1][j]) {
            out.push(('+', b[j].clone()));
            j += 1;
        } else {
            out.push(('-', a[i].clone()));
            i += 1;
        }
    }
    out
}

/// Chat titles are compared the way the store keys them (member count, width, case, spaces and l/I folded), so the
/// same chat read slightly differently in two runs lines up.
fn title_key(t: &str) -> String {
    let t = t.trim();
    let t = match t.rfind(['(', '（']) {
        Some(i) if t[i..].trim_start_matches(['(', '（']).trim_end_matches([')', '）']).trim().chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || ('０'..='９').contains(&c)) => &t[..i],
        _ => t,
    };
    t.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).map(|c| if c == 'l' { 'i' } else { c }).collect()
}

fn by_conv(d: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in d["conversations"].as_array().unwrap() {
        let name = format!("[{}] {} · {}", c["kind"].as_str().unwrap(), c["app"].as_str().unwrap(), title_key(c["title"].as_str().unwrap()));
        out.entry(name).or_default().extend(c["messages"].as_array().unwrap().iter().map(key));
    }
    out
}

fn set_diff(o: &mut String, title: &str, a: Vec<String>, b: Vec<String>) {
    let changes = lcs_diff(&a, &b);
    if changes.is_empty() {
        return;
    }
    let _ = writeln!(o, "\n## {title}\n");
    for (sign, line) in changes {
        let _ = writeln!(o, "{sign} {line}");
    }
}

pub fn diff(a: &Value, b: &Value) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# Diff\n\ncounts A {}\ncounts B {}", a["counts"], b["counts"]);
    let (ca, cb) = (by_conv(a), by_conv(b));
    let mut names: Vec<&String> = ca.keys().chain(cb.keys()).collect();
    names.sort();
    names.dedup();
    let _ = writeln!(o, "\n## Conversations");
    let empty = vec![];
    for name in names {
        let (x, y) = (ca.get(name).unwrap_or(&empty), cb.get(name).unwrap_or(&empty));
        let changes = lcs_diff(x, y);
        if changes.is_empty() {
            continue;
        }
        let _ = writeln!(o, "\n### {name}  (A {} → B {}, -{} +{})", x.len(), y.len(), changes.iter().filter(|c| c.0 == '-').count(), changes.iter().filter(|c| c.0 == '+').count());
        for (sign, line) in changes.iter().take(80) {
            let _ = writeln!(o, "{sign} {}", line.chars().take(160).collect::<String>());
        }
    }
    let items = |d: &Value| d["items"].as_array().unwrap().iter().map(|i| format!("[{} {}] {} · {}", i["kind"].as_str().unwrap(), i["review"].as_str().unwrap(), i["title"].as_str().unwrap(), i["when"].as_str().unwrap())).collect::<Vec<_>>();
    set_diff(&mut o, "Items", items(a), items(b));
    let memos = |d: &Value| d["memos"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    set_diff(&mut o, "Memos", memos(a), memos(b));
    let eps = |d: &Value| d["episodes"].as_array().unwrap().iter().filter(|e| e["producer"] != "screen-cleanup/1").map(|e| format!("{} {} — {}", e["from"].as_str().unwrap(), e["title"].as_str().unwrap(), e["summary"].as_str().unwrap())).collect::<Vec<_>>();
    set_diff(&mut o, "Activity blocks", eps(a), eps(b));
    let people = |d: &Value| d["people"].as_array().unwrap().iter().map(|p| p["aliases"].to_string()).collect::<Vec<_>>();
    set_diff(&mut o, "People (alias sets)", people(a), people(b));
    o
}
