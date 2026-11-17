//! §4.6 normalisation and chunking. Offsets are Unicode scalar offsets into the normalised text. Chunk hashes are
//! shared with the Swift implementation (`text_chunks.chunk_hash` is UNIQUE), so this must match it exactly.

use crate::ids::sha256_hex;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq)]
pub struct TextChunk {
    pub text: String,
    pub char_start: usize,
    pub hash: String,
    pub scalar_count: usize,
    pub lang: Option<&'static str>,
}

pub struct TextChunker;

const MAX_CHUNK_SCALARS: usize = 512;

fn is_terminator(c: char) -> bool {
    matches!(c, '。' | '.' | '！' | '？' | '!' | '?' | '\n')
}

impl TextChunker {
    pub const MAX_CHUNK_SCALARS: usize = MAX_CHUNK_SCALARS;

    /// NFC, unified newlines, no trailing whitespace on lines, runs of blank lines folded to one, outer blank lines trimmed.
    pub fn normalise(raw: &str) -> String {
        let text: String = raw.nfc().collect();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut lines: Vec<&str> = Vec::new();
        let mut previous_blank = true; // drops leading blank lines
        for line in text.split('\n') {
            let l = line.trim_end_matches(char::is_whitespace);
            if l.is_empty() {
                if !previous_blank {
                    lines.push("");
                }
                previous_blank = true;
            } else {
                lines.push(l);
                previous_blank = false;
            }
        }
        while lines.last() == Some(&"") {
            lines.pop();
        }
        lines.join("\n")
    }

    pub fn chunk(raw: &str) -> Vec<TextChunk> {
        let scalars: Vec<char> = Self::normalise(raw).chars().collect();
        let mut out = Vec::new();
        let n = scalars.len();
        let mut i = 0;
        while i < n {
            while i < n && scalars[i] == '\n' {
                i += 1;
            }
            if i >= n {
                break;
            }
            let mut j = i;
            while j < n {
                if scalars[j] == '\n' && j + 1 < n && scalars[j + 1] == '\n' {
                    break;
                }
                j += 1;
            }
            append_paragraph(&scalars[i..j], i, &mut out);
            i = j;
        }
        out
    }

    pub fn language(scalars: &[char]) -> Option<&'static str> {
        let (mut cjk, mut latin) = (false, false);
        for &s in scalars {
            let v = s as u32;
            if (0x4E00..=0x9FFF).contains(&v) || (0x3400..=0x4DBF).contains(&v) {
                cjk = true;
            } else if s.is_alphabetic() {
                latin = true;
            }
            if cjk && latin {
                return Some("mixed");
            }
        }
        if cjk {
            Some("zh")
        } else if latin {
            Some("en")
        } else {
            None
        }
    }
}

fn append_paragraph(p: &[char], start: usize, out: &mut Vec<TextChunk>) {
    if p.len() <= MAX_CHUNK_SCALARS {
        emit(p, start, out);
        return;
    }
    // cut into sentence-ish units (terminator stays with the preceding unit), then pack greedily
    let mut units: Vec<(usize, usize)> = Vec::new();
    let mut unit_start = 0;
    for (k, &c) in p.iter().enumerate() {
        if is_terminator(c) {
            units.push((unit_start, k + 1));
            unit_start = k + 1;
        }
    }
    if unit_start < p.len() {
        units.push((unit_start, p.len()));
    }
    let mut cur: Option<(usize, usize)> = None;
    let flush = |cur: &mut Option<(usize, usize)>, out: &mut Vec<TextChunk>| {
        if let Some(c) = cur.take() {
            emit(&p[c.0..c.1], start + c.0, out);
        }
    };
    for u in units {
        let mut unit = u;
        while unit.1 - unit.0 > MAX_CHUNK_SCALARS {
            // no terminator: hard split
            flush(&mut cur, out);
            cur = Some((unit.0, unit.0 + MAX_CHUNK_SCALARS));
            flush(&mut cur, out);
            unit.0 += MAX_CHUNK_SCALARS;
        }
        if let Some(c) = cur {
            if unit.1 - c.0 > MAX_CHUNK_SCALARS {
                flush(&mut cur, out);
            }
        }
        cur = Some(match cur {
            Some(c) => (c.0, unit.1),
            None => unit,
        });
    }
    flush(&mut cur, out);
}

fn emit(raw: &[char], start: usize, out: &mut Vec<TextChunk>) {
    let (mut lo, mut hi) = (0, raw.len());
    while lo < hi && raw[lo].is_whitespace() {
        lo += 1;
    }
    while hi > lo && raw[hi - 1].is_whitespace() {
        hi -= 1;
    }
    if lo >= hi {
        return;
    }
    let text: String = raw[lo..hi].iter().collect();
    out.push(TextChunk {
        hash: sha256_hex(text.as_bytes()),
        text,
        char_start: start + lo,
        scalar_count: hi - lo,
        lang: TextChunker::language(&raw[lo..hi]),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_normalises_splits_long_paragraphs_and_dedupes() {
        let sentence = "这是一个合成的长句子用来测试切分。";
        let long = sentence.repeat(60);
        let chunks = TextChunker::chunk(&format!("  第一段  \r\n\r\n\r\n\r\n第二段\n{long}"));
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|c| c.scalar_count <= 512));
        assert_eq!(chunks[0].text, "第一段");
        assert_eq!(chunks[0].char_start, 2);
        assert!(chunks[1].text.starts_with("第二段"));
        assert_eq!(TextChunker::normalise("a  \n\n\n\nb\n\n"), "a\n\nb");
        assert_eq!(TextChunker::normalise("e\u{0301}"), "é");
        let same = TextChunker::chunk("same\n\nsame");
        assert_eq!(same.len(), 2);
        assert_eq!(same[0].hash, same[1].hash);
        let no_terminator = "x".repeat(1300);
        assert_eq!(TextChunker::chunk(&no_terminator).iter().map(|c| c.scalar_count).collect::<Vec<_>>(), vec![512, 512, 276]);
        assert_eq!(TextChunker::language(&"预算 budget".chars().collect::<Vec<_>>()), Some("mixed"));
    }
}
