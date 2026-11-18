//! Conservative screen text projection. Raw event text/search remain unchanged.
use crate::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenFrame {
    pub event_id: String,
    pub source_id: String,
    pub revision: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub app: String,
    pub window: String,
    /// Stored without it since schema 9; rebuilt from the paragraph table when needed. Digests still cover it.
    #[serde(default)]
    pub text: String,
    pub field: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenLine {
    pub event_id: String,
    pub start: usize,
    pub end: usize,
    /// Stored without it since schema 9: `start..end` (characters) of the frame's text.
    #[serde(default)]
    pub text: String,
    pub field: String,
    pub family: String,
    pub weight: f64,
    pub is_new: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenEpisode {
    pub id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub app: String,
    pub window: String,
    pub frames: Vec<ScreenFrame>,
    pub lines: Vec<ScreenLine>,
    pub input_chars: usize,
    pub raw_chars: usize,
}
fn norm(s: &str) -> String {
    s.nfkc().collect::<String>().trim_end().to_string()
}
fn numbers(s: &str) -> String {
    s.chars().filter(|c| c.is_numeric()).collect()
}
fn simhash(s: &str) -> u64 {
    let chars: Vec<_> = s.chars().collect();
    let mut bits = [0i32; 64];
    for gram in chars.windows(3) {
        let digest = sha256_hex(gram.iter().collect::<String>().as_bytes());
        let hash = u64::from_str_radix(&digest[..16], 16).unwrap();
        for (i, b) in bits.iter_mut().enumerate() {
            *b += if hash & (1 << i) != 0 { 1 } else { -1 };
        }
    }
    bits.iter()
        .enumerate()
        .fold(0, |v, (i, b)| if *b > 0 { v | (1 << i) } else { v })
}
fn near(a: &str, b: &str) -> bool {
    if a.chars().count() < 8 || b.chars().count() < 8 || numbers(a) != numbers(b) {
        return false;
    }
    let ac: Vec<_> = a.chars().collect();
    let bc: Vec<_> = b.chars().collect();
    if ac.len().abs_diff(bc.len()) > 2 {
        return false;
    }
    // Fingerprint grouping also checks edit distance; unrelated short lines never collapse by hash collision.
    let mut row: Vec<usize> = (0..=bc.len()).collect();
    for (i, c) in ac.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, d) in bc.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1).min(old + 1).min(prev + usize::from(c != d));
            prev = old;
        }
    }
    row[bc.len()] * 10 <= ac.len().max(bc.len())
}

pub fn clean(mut frames: Vec<ScreenFrame>, important: &HashSet<String>) -> Vec<ScreenEpisode> {
    frames.sort_by(|a, b| (a.start_ms, &a.event_id).cmp(&(b.start_ms, &b.event_id)));
    let mut windows: HashMap<String, usize> = HashMap::new();
    let mut episodes: Vec<ScreenEpisode> = Vec::new();
    let mut seen: HashMap<String, Vec<(String, String, i64, Option<u64>, String)>> = HashMap::new();
    let mut frequency: HashMap<(String, String), HashSet<String>> = HashMap::new();
    let mut totals: HashMap<String, usize> = HashMap::new();
    for f in frames {
        let window = format!("{}\u{1f}{}\u{1f}{}", f.source_id, f.app, f.window);
        *totals.entry(window.clone()).or_default() += 1;
        let idx = if let Some(i) = windows
            .get(&window)
            .copied()
            .filter(|i| f.start_ms - episodes[*i].end_ms <= 300000)
        {
            i
        } else {
            let id = format!(
                "screen:{}",
                &sha256_hex(format!("{}:{}", window, f.event_id).as_bytes())[..24]
            );
            episodes.push(ScreenEpisode {
                id,
                start_ms: f.start_ms,
                end_ms: f.end_ms,
                app: f.app.clone(),
                window: f.window.clone(),
                frames: vec![],
                lines: vec![],
                input_chars: 0,
                raw_chars: 0,
            });
            let idx = episodes.len() - 1;
            windows.insert(window.clone(), idx);
            idx
        };
        let memory = seen.entry(window.clone()).or_default();
        memory.retain(|(_, _, at, _, _)| f.start_ms - *at <= 1800000);
        let mut offset = 0;
        let mut previous = String::new();
        for line in f.text.split_inclusive('\n') {
            let text = line.trim_matches(['\r', '\n']).to_string();
            let normalized = norm(&text);
            let context = if normalized.chars().count() < 4 {
                format!("{previous}\u{1f}{normalized}")
            } else {
                normalized.clone()
            };
            let punctuation = !text.chars().any(char::is_alphanumeric);
            let digits = numbers(&context);
            let fingerprint = if (8..=256).contains(&context.chars().count()) {
                Some(simhash(&context))
            } else {
                None
            };
            let exact = memory.iter().position(|(k, _, _, _, _)| *k == context);
            let index = exact.or_else(|| {
                memory
                    .iter()
                    .enumerate()
                    .rev()
                    .take(200)
                    .find(|(_, (k, _, at, hash, nums))| {
                        f.start_ms - *at <= 600000
                            && *nums == digits
                            && fingerprint
                                .zip(*hash)
                                .is_some_and(|(a, b)| (a ^ b).count_ones() <= 3)
                            && near(k, &context)
                    })
                    .map(|(i, _)| i)
            });
            let existing = index.map(|i| &mut memory[i]);
            let (family, is_new) = if let Some((key, family, at, _, _)) = existing {
                *at = f.start_ms;
                (family.clone(), *key != context)
            } else {
                let family = sha256_hex(format!("{window}:{context}").as_bytes());
                if memory.len() >= 2048 {
                    memory.remove(0);
                }
                memory.push((
                    context.clone(),
                    family.clone(),
                    f.start_ms,
                    fingerprint,
                    digits,
                ));
                (family, true)
            };
            // A near match relates variants; it never hides an actual edit (negation, code or otherwise).
            if is_new && index.is_some() {
                memory.push((
                    context.clone(),
                    family.clone(),
                    f.start_ms,
                    fingerprint,
                    numbers(&context),
                ));
            }
            frequency
                .entry((window.clone(), family.clone()))
                .or_default()
                .insert(f.event_id.clone());
            let len = text.chars().count();
            if !normalized.is_empty() {
                episodes[idx].lines.push(ScreenLine {
                    event_id: f.event_id.clone(),
                    start: offset,
                    end: offset + len,
                    text,
                    field: f.field.clone(),
                    family,
                    weight: if punctuation { 0.1 } else { 1. },
                    is_new,
                });
            }
            offset += line.chars().count();
            previous = normalized;
        }
        episodes[idx].raw_chars += f.text.chars().count();
        episodes[idx].end_ms = episodes[idx].end_ms.max(f.end_ms);
        episodes[idx].frames.push(f);
    }
    for ep in &mut episodes {
        let window = format!(
            "{}\u{1f}{}\u{1f}{}",
            ep.frames[0].source_id, ep.app, ep.window
        );
        for l in &mut ep.lines {
            let count = frequency
                .get(&(window.clone(), l.family.clone()))
                .map(HashSet::len)
                .unwrap_or(0);
            if !important.contains(&l.family)
                && ((count >= 20
                    && count as f64 / *totals.get(&window).unwrap_or(&1) as f64 >= 0.3)
                    || l.text.chars().count() < 4)
            {
                l.weight = l.weight.min(0.2);
            }
            if important.contains(&l.family) {
                l.weight = 1.;
            }
        }
        ep.input_chars = ep
            .lines
            .iter()
            .filter(|l| l.is_new && l.weight >= 0.5)
            .map(|l| l.text.chars().count())
            .sum();
    }
    episodes
}
