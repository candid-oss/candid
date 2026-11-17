//! Every listed encoding with synthetic mail: RFC 2047 headers and bodies in legacy charsets, QP/base64,
//! multipart, HTML-only, attachments, quoted history and signatures.
use base64::Engine;
use candid_connectors::mail::mime::{base_subject, message_id, parse, split_body};

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn encoded(label: &str, text: &str) -> Vec<u8> {
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or_else(|| panic!("label {label}"));
    let (bytes, _, unmappable) = enc.encode(text);
    assert!(!unmappable, "{label} cannot encode the sample");
    bytes.into_owned()
}

/// Subject as RFC 2047 base64 in `charset`, body base64 in the same charset.
fn legacy(charset: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "Message-ID: <{charset}@example.test>\r\nFrom: =?{charset}?B?{}?= <sender@example.test>\r\nTo: me@example.test\r\nSubject: =?{charset}?B?{}?=\r\nDate: Tue, 10 Nov 2026 09:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=\"{charset}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        b64(&encoded(charset, subject)),
        b64(&encoded(charset, subject)),
        b64(&encoded(charset, body))
    )
    .into_bytes()
}

#[test]
fn legacy_charsets_in_headers_and_bodies() {
    let cases: &[(&str, &str, &str)] = &[
        ("GBK", "合成会议纪要", "周五前提交预算草案。"),
        ("GB2312", "合成通知", "请确认合成样例。"),
        ("GB18030", "合成𠀀扩展字", "第二段𠀀正文。"),
        ("Big5", "合成繁體主旨", "請於週五前回覆。"),
        ("ISO-2022-JP", "合成の件名", "金曜日までに返信してください。"),
        ("ISO-8859-1", "Café synthétique", "Réunion à déjà prévue."),
        ("ISO-8859-2", "Zażółć syntetyczny", "Gęślą jaźń próbka."),
        ("ISO-8859-5", "Синтетика", "Пример текста."),
        ("ISO-8859-15", "Prix 100 €", "Œuvre synthétique."),
        ("windows-1252", "Naïve “quotes”", "Synthetic – dash"),
        ("windows-1251", "Синтетический", "Тестовое письмо."),
        ("windows-1250", "Příliš žluťoučký", "Syntetická zpráva."),
        ("windows-1256", "تجريبي", "رسالة تجريبية."),
        ("Shift_JIS", "合成テスト", "本文のサンプル。"),
        ("EUC-KR", "합성 제목", "합성 본문입니다."),
    ];
    for (charset, subject, body) in cases {
        let m = parse(&legacy(charset, subject, body)).unwrap_or_else(|| panic!("{charset} not parsed"));
        assert_eq!(m.subject, *subject, "{charset} subject");
        assert_eq!(m.body.trim(), *body, "{charset} body");
        assert_eq!(m.message_id.as_deref(), Some(format!("{charset}@example.test").as_str()));
    }
}

#[test]
fn quoted_printable_q_words_and_utf8_base64() {
    let raw = "Message-ID: <qp@example.test>\r\nFrom: \"=?UTF-8?Q?Synth=C3=A9tique?=\" <s@example.test>\r\nTo: a@example.test, b@example.test\r\nCc: c@example.test\r\nSubject: =?UTF-8?Q?Caf=C3=A9_?= =?UTF-8?B?5ZCI5oiQ?=\r\nDate: Tue, 10 Nov 2026 09:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nLine one with =C3=A9 and a soft=\r\n break.\r\n";
    let m = parse(raw.as_bytes()).unwrap();
    assert_eq!(m.subject, "Café 合成");
    assert_eq!(m.from_name.as_deref(), Some("Synthétique"));
    assert_eq!(m.from_address.as_deref(), Some("s@example.test"));
    assert_eq!((m.to.len(), m.cc.len()), (2, 1));
    assert!(m.body.contains("Line one with é and a soft break."));
    assert_eq!(m.date_ms, Some(1_794_301_200_000));
}

#[test]
fn multipart_prefers_text_and_html_only_is_converted_without_styles_scripts_or_images() {
    let alt = "Message-ID: <alt@example.test>\r\nFrom: s@example.test\r\nSubject: alt\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlain version.\r\n--b1\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>HTML version.</p>\r\n--b1--\r\n";
    let m = parse(alt.as_bytes()).unwrap();
    assert_eq!((m.body.trim(), m.body_format), ("Plain version.", "text"));
    let html = "Message-ID: <html@example.test>\r\nFrom: s@example.test\r\nSubject: html\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n".to_string()
        + &b64("<html><head><style>.x{color:red}</style></head><body><script>steal()</script><p>Visible synthetic text.</p><img src=\"https://tracker.example.test/p.gif\" width=1 height=1></body></html>".as_bytes())
        + "\r\n";
    let m = parse(html.as_bytes()).unwrap();
    assert_eq!(m.body_format, "html");
    assert!(m.body.contains("Visible synthetic text."));
    assert!(!m.body.contains("color:red") && !m.body.contains("steal()") && !m.body.contains("tracker"));
}

#[test]
fn attachments_are_name_type_and_size_only() {
    let raw = format!(
        "Message-ID: <att@example.test>\r\nFrom: s@example.test\r\nSubject: files\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n--m\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSee attached.\r\n--m\r\nContent-Type: application/pdf; name=\"=?UTF-8?B?{}?=\"\r\nContent-Disposition: attachment; filename*=UTF-8''%E5%90%88%E6%88%90.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--m--\r\n",
        b64("合成.pdf".as_bytes()),
        b64(&[7u8; 300])
    );
    let m = parse(raw.as_bytes()).unwrap();
    assert_eq!(m.attachments.len(), 1);
    assert_eq!(m.attachments[0].name.as_deref(), Some("合成.pdf"));
    assert_eq!((m.attachments[0].mime.as_str(), m.attachments[0].size), ("application/pdf", 300));
    assert_eq!(m.body.trim(), "See attached.");
}

#[test]
fn list_and_automated_headers_and_missing_message_id() {
    let raw = "From: noreply@example.test\r\nSubject: news\r\nList-Unsubscribe: <mailto:u@example.test>\r\nAuto-Submitted: auto-generated\r\n\r\nbody\r\n";
    let m = parse(raw.as_bytes()).unwrap();
    assert!(m.list && m.automated);
    assert_eq!(m.message_id, None);
    assert_eq!(message_id(raw.as_bytes()), None);
    assert_eq!(message_id(b"Message-ID:   <x@example.test>  \r\n\r\n").as_deref(), Some("x@example.test"));
}

#[test]
fn quoted_history_and_signatures_are_split_from_new_text() {
    let (new, quoted, sig) = split_body("Thanks, see below.\n\n-- \nSynthetic Person\nExample Org\n");
    assert_eq!((new.as_str(), quoted.as_str(), sig.as_str()), ("Thanks, see below.", "", "-- \nSynthetic Person\nExample Org"));
    let (new, quoted, _) = split_body("Agreed.\n\nOn Tue, 10 Nov 2026, Someone <s@example.test> wrote:\n> earlier text\n> more\n");
    assert_eq!(new, "Agreed.");
    assert!(quoted.contains("wrote:") && quoted.contains("> earlier text"));
    let (new, quoted, _) = split_body("好的，周五前完成。\n\n在 2026年11月10日 09:00，某人 写道：\n> 之前的内容\n");
    assert_eq!(new, "好的，周五前完成。");
    assert!(quoted.contains("之前的内容"));
    let (new, quoted, _) = split_body("Reply on top\n\n-----Original Message-----\nFrom: x@example.test\nSent: Monday\n\nold body\n");
    assert_eq!(new, "Reply on top");
    assert!(quoted.contains("old body"));
    let (new, _, sig) = split_body("Short answer.\n\nSent from my iPhone");
    assert_eq!((new.as_str(), sig.as_str()), ("Short answer.", "Sent from my iPhone"));
    let (new, quoted, sig) = split_body("Inline:\n> question?\nanswer.\n");
    assert_eq!((new.as_str(), quoted.as_str(), sig.as_str()), ("Inline:\nanswer.", "> question?", ""));
}

#[test]
fn reply_prefixes_are_removed_for_thread_titles() {
    assert_eq!(base_subject("Re: Fwd: RE: 预算"), "预算");
    assert_eq!(base_subject("回复：答复: 周报"), "周报");
    assert_eq!(base_subject("Re[2]: plan"), "plan");
    assert_eq!(base_subject("Regarding plan"), "Regarding plan");
}

#[test]
fn garbage_is_not_a_message() {
    assert!(parse(b"").is_none());
}
