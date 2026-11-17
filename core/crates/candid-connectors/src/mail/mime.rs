//! MIME decoding through `mail-parser` (multipart, quoted-printable/base64, RFC 2047, legacy charsets via
//! encoding_rs). Mail text is untrusted data: it is decoded and cleaned, never interpreted.
use mail_parser::{Addr, Address, HeaderValue, MessageParser, MimeHeaders};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attachment {
    pub name: Option<String>,
    pub mime: String,
    pub size: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedMail {
    /// Normalised Message-ID (no angle brackets, trimmed); `None` when absent or empty.
    pub message_id: Option<String>,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub from_name: Option<String>,
    /// Lower-cased address; only used in memory (filters, hashing).
    pub from_address: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub date_ms: Option<i64>,
    /// Plain text: the text part, or the HTML part converted to text (styles, scripts and images dropped).
    pub body: String,
    pub body_format: &'static str,
    pub attachments: Vec<Attachment>,
    /// `List-Unsubscribe` or `List-Id` present.
    pub list: bool,
    /// `Auto-Submitted` other than `no`, or `Precedence: bulk|list|junk`.
    pub automated: bool,
}

pub fn normalize_id(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches('<').trim_end_matches('>').trim();
    if t.is_empty() || t.len() > 998 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        None
    } else {
        Some(t.to_string())
    }
}

fn ids(v: &HeaderValue) -> Vec<String> {
    let raw: Vec<String> = match v {
        HeaderValue::Text(t) => t.split_whitespace().map(String::from).collect(),
        HeaderValue::TextList(l) => l.iter().flat_map(|t| t.split_whitespace().map(String::from).collect::<Vec<_>>()).collect(),
        _ => vec![],
    };
    let mut out: Vec<String> = Vec::new();
    for id in raw.iter().filter_map(|r| normalize_id(r)) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn addresses(a: Option<&Address>) -> Vec<String> {
    a.map(|a| a.iter().filter_map(|x: &Addr| x.address()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn present(v: &HeaderValue) -> bool {
    !matches!(v, HeaderValue::Empty)
}

/// Reads only what the header needs (cheap, used by adapters to derive the source key).
pub fn message_id(bytes: &[u8]) -> Option<String> {
    MessageParser::default().parse_headers(bytes).and_then(|m| m.message_id().and_then(normalize_id))
}

fn header_fields(m: &mail_parser::Message) -> ParsedMail {
    let from = m.from().and_then(|a| a.first());
    let auto = m
        .header_raw("Auto-Submitted")
        .map(|v| !v.trim().eq_ignore_ascii_case("no"))
        .unwrap_or(false)
        || m.header_raw("Precedence").map(|v| ["bulk", "list", "junk"].contains(&v.trim().to_lowercase().as_str())).unwrap_or(false);
    ParsedMail {
        message_id: m.message_id().and_then(normalize_id),
        in_reply_to: ids(m.in_reply_to()),
        references: ids(m.references()),
        subject: m.subject().unwrap_or("").trim().to_string(),
        from_name: from.and_then(|a| a.name()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        from_address: from.and_then(|a| a.address()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()),
        to: addresses(m.to()),
        cc: addresses(m.cc()),
        date_ms: m.date().map(|d| d.to_timestamp() * 1000).filter(|t| *t > 0),
        list: present(m.list_unsubscribe()) || present(m.list_id()),
        automated: auto,
        ..Default::default()
    }
}

/// Header fields only (preview counts): the body is not decoded, so `body` is empty and `body_format` is `none`.
pub fn parse_headers(bytes: &[u8]) -> Option<ParsedMail> {
    let m = MessageParser::default().parse_headers(bytes)?;
    if m.headers().is_empty() {
        return None;
    }
    Some(ParsedMail { body_format: "none", ..header_fields(&m) })
}

pub fn parse(bytes: &[u8]) -> Option<ParsedMail> {
    let m = MessageParser::default().parse(bytes)?;
    if m.headers().is_empty() {
        return None;
    }
    let (body, body_format) = if m.text_body_count() > 0 && m.text_bodies().any(|p| !p.is_text_html()) {
        (m.body_text(0).map(|c| c.into_owned()).unwrap_or_default(), "text")
    } else if m.html_body_count() > 0 {
        // body_text on an HTML-only message converts it (head/style/script/template removed, tags stripped).
        (m.body_text(0).map(|c| c.into_owned()).unwrap_or_default(), "html")
    } else {
        (String::new(), "none")
    };
    Some(ParsedMail {
        body: body.replace("\r\n", "\n"),
        body_format,
        attachments: m
            .attachments()
            .map(|a| Attachment {
                name: a.attachment_name().map(String::from),
                mime: a
                    .content_type()
                    .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")).to_lowercase())
                    .unwrap_or_else(|| "application/octet-stream".into()),
                size: a.len(),
            })
            .collect(),
        ..header_fields(&m)
    })
}

/// Subject without reply/forward prefixes (English, Chinese, common European forms), for thread titles.
pub fn base_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let mut cut = None;
        for p in ["re:", "fw:", "fwd:", "aw:", "wg:", "sv:", "vs:", "tr:", "回复:", "回复：", "答复:", "答复：", "转发:", "转发：", "回覆:", "回覆：", "轉寄:", "轉寄："] {
            if lower.starts_with(p) {
                cut = Some(s.char_indices().nth(p.chars().count()).map(|(i, _)| i).unwrap_or(s.len()));
                break;
            }
        }
        // "Re[2]:" style
        if cut.is_none() && lower.starts_with("re[") {
            if let Some(end) = s.find("]:") {
                cut = Some(end + 2);
            }
        }
        match cut {
            Some(i) => s = s[i..].trim_start(),
            None => break,
        }
    }
    s.to_string()
}

/// Splits a plain-text body into the new part, quoted history and signature. Quoted history: `>` lines, and
/// everything after an attribution/forward separator (top posting). Signature: after `-- ` or a trailing
/// "sent from my …" line. Detection is conservative; unmatched text stays in the new part.
pub fn split_body(body: &str) -> (String, String, String) {
    let lines: Vec<&str> = body.lines().collect();
    let mut cut_history = lines.len();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        let lower = t.to_lowercase();
        let attribution = (lower.starts_with("on ") && (lower.ends_with("wrote:") || lower.ends_with("wrote：")))
            || (t.starts_with('在') && (t.ends_with("写道：") || t.ends_with("写道:") || t.ends_with("寫道：") || t.ends_with("寫道:")))
            || (lower.starts_with("am ") && lower.ends_with("schrieb:"))
            || (lower.starts_with("le ") && lower.ends_with("a écrit :"))
            || lower.starts_with("-----original message-----")
            || t.starts_with("-------- 原始邮件 --------")
            || t.starts_with("------------------ 原始邮件 ------------------")
            || lower.starts_with("---------- forwarded message")
            || lower.starts_with("begin forwarded message:")
            || (lower.starts_with("from:") && lines.get(i + 1).map(|n| { let n = n.trim().to_lowercase(); n.starts_with("sent:") || n.starts_with("date:") }).unwrap_or(false))
            || (t.starts_with("发件人:") || t.starts_with("发件人："));
        if attribution {
            cut_history = i;
            break;
        }
    }
    let head = &lines[..cut_history];
    let mut history: Vec<&str> = lines[cut_history..].to_vec();
    let mut sig_at = head.len();
    for (i, l) in head.iter().enumerate() {
        if *l == "-- " || *l == "--" {
            sig_at = i;
            break;
        }
    }
    let mut new_lines: Vec<&str> = Vec::new();
    let mut quoted: Vec<&str> = Vec::new();
    for l in &head[..sig_at] {
        if l.trim_start().starts_with('>') {
            quoted.push(l);
        } else {
            new_lines.push(l);
        }
    }
    let mut signature: Vec<&str> = head[sig_at..].to_vec();
    // Trailing mobile signature, after trimming blank lines.
    while new_lines.last().is_some_and(|l| l.trim().is_empty()) {
        new_lines.pop();
    }
    if let Some(last) = new_lines.last() {
        let lower = last.trim().to_lowercase();
        if lower.starts_with("sent from my ") || lower.starts_with("发自我的") || lower.starts_with("從我的") || lower.starts_with("sent from outlook") {
            signature.insert(0, new_lines.pop().unwrap());
        }
    }
    quoted.append(&mut history);
    let join = |v: &[&str]| v.join("\n").trim().to_string();
    (join(&new_lines), join(&quoted), join(&signature))
}
