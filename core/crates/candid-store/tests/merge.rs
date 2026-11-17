//! Typing continuations folded at ingest (`merge_text`, provisional thresholds): growth, repeats, empty reads in
//! between, focus and time limits, resends, absorbed parents, deletion. Synthetic text only.
mod common;
use common::*;
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::{json, Value};

const T: i64 = 1_800_000_000_000;

fn sample(key: &str, ms: i64, label: &str, body: &str, status: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "screen.text", Some(ms));
    e.app_bundle_id = Some("com.example.notes".into());
    e.app_name = Some("Notes".into());
    e.title = Some("合成草稿".into());
    e.payload = json!({"trigger": "content", "text_status": status, "focused_role": "AXTextArea", "focused_label": label,
        "legacy": {"run_id": "run-1", "activity_id": "act-1"}});
    if !body.is_empty() {
        e.text = vec![IngestText::new("body", body)];
    }
    e
}

/// A page of `n` stable lines plus a growing last line, as the reader returns it while someone types.
fn page(typed: &str) -> String {
    let mut lines: Vec<String> = (0..8).map(|i| format!("合成背景第 {i} 行，内容不变")).collect();
    lines.push(typed.to_string());
    lines.join("\n")
}

fn merging(source: &str, events: Vec<IngestEvent>) -> IngestBatch {
    IngestBatch { merge_text: Some(true), ..IngestBatch::events(source, events) }
}

fn text_events(fx: &Fx) -> Vec<(String, Option<i64>, Value)> {
    fx.store
        .raw_db()
        .query("SELECT event_id, end_ms, payload_json FROM events WHERE kind='screen.text' ORDER BY start_ms", &[], |r| {
            (r.text(0), r.opt_int(1), serde_json::from_str(&r.text(2)).unwrap())
        })
        .unwrap()
}

fn body(fx: &Fx, event: &str) -> String {
    fx.texts(&format!("SELECT c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id WHERE t.event_id='{event}' ORDER BY t.ord")).join("\n")
}

#[test]
fn typing_growth_becomes_one_event_with_the_latest_text() {
    let fx = Fx::make("merge-grow");
    let typed = ["今", "今天", "今天下午", "今天下午三点", "今天下午三点开会", "今天下午三点开会，", "今天下午三点开会，带样稿"];
    // One sample per batch, as capture sends them; then a batch holding several.
    for (i, t) in typed.iter().take(4).enumerate() {
        let r = fx.store.ingest(&merging(&fx.source, vec![sample(&format!("s{i}"), T + i as i64 * 10_000, "正文", &page(t), "captured")])).unwrap();
        assert_eq!((r.inserted, r.merged), if i == 0 { (1, 0) } else { (0, 1) });
    }
    let rest: Vec<IngestEvent> = typed.iter().enumerate().skip(4).map(|(i, t)| sample(&format!("s{i}"), T + i as i64 * 10_000, "正文", &page(t), "captured")).collect();
    assert_eq!(fx.store.ingest(&merging(&fx.source, rest)).unwrap().merged, 3);
    let events = text_events(&fx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, Some(T + 60_000), "the end follows the last sample");
    assert_eq!(events[0].2["merge"]["revisions"], 6);
    assert_eq!(events[0].2["merge"]["keys"].as_array().unwrap().len(), 6);
    assert!(body(&fx, &events[0].0).ends_with("今天下午三点开会，带样稿"));
    // Paragraphs of the earlier versions are gone from the index; search finds the latest text once.
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM text_chunks WHERE text LIKE '%今天下午三点开会，' OR text = '今'"), 0);
    // A resend of an absorbed sample is a duplicate.
    let again = fx.store.ingest(&merging(&fx.source, vec![sample("s3", T + 30_000, "正文", &page(typed[3]), "captured")])).unwrap();
    assert_eq!((again.inserted, again.merged, again.duplicates), (0, 0, 1));
    assert_eq!(again.event_ids["s3"], events[0].0);
    // So is a resend of the first sample, although its event has changed since.
    let first = fx.store.ingest(&merging(&fx.source, vec![sample("s0", T, "正文", &page(typed[0]), "captured")])).unwrap();
    assert_eq!((first.duplicates, first.conflicts), (1, 0));
}

#[test]
fn repeats_and_empty_reads_extend_without_breaking_the_chain() {
    let fx = Fx::make("merge-repeat");
    let a = page("第一版");
    let b = page("第二版");
    let batch = vec![
        sample("a", T, "正文", &a, "captured"),
        sample("a-again", T + 5_000, "正文", &a, "captured"),
        sample("blip", T + 10_000, "正文", "合", "captured"),
        sample("failed", T + 15_000, "正文", "", "unavailable"),
        sample("b", T + 20_000, "正文", &b, "captured"),
    ];
    let r = fx.store.ingest(&merging(&fx.source, batch)).unwrap();
    assert_eq!((r.inserted, r.merged), (1, 4));
    let events = text_events(&fx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, Some(T + 20_000));
    assert_eq!((events[0].2["merge"]["revisions"].as_i64(), events[0].2["merge"]["samples"].as_i64()), (Some(1), Some(4)));
    assert!(body(&fx, &events[0].0).ends_with("第二版"));
}

#[test]
fn another_focus_a_long_pause_or_a_new_page_starts_a_new_event() {
    let fx = Fx::make("merge-break");
    let batch = vec![
        sample("a", T, "正文", &page("一"), "captured"),
        sample("label", T + 10_000, "标题栏", &page("一二"), "captured"),
        sample("late", T + 10_000 + 121_000, "标题栏", &page("一二三"), "captured"),
        sample("other", T + 140_000, "标题栏", "完全不同的另一页\n没有共同的行", "captured"),
    ];
    let r = fx.store.ingest(&merging(&fx.source, batch)).unwrap();
    assert_eq!((r.inserted, r.merged), (4, 0));
    // Without `merge_text` nothing is folded.
    let fx2 = Fx::make("merge-off");
    let r = fx2.store.ingest(&IngestBatch::events(&fx2.source, vec![sample("a", T, "正文", &page("一"), "captured"), sample("b", T + 1000, "正文", &page("一二"), "captured")])).unwrap();
    assert_eq!((r.inserted, r.merged), (2, 0));
}

#[test]
fn screenshots_of_absorbed_samples_attach_and_deletion_blocks_late_resends() {
    let fx = Fx::make("merge-parent");
    fx.store.ingest(&merging(&fx.source, vec![sample("a", T, "正文", &page("一"), "captured"), sample("b", T + 5_000, "正文", &page("一二"), "captured")])).unwrap();
    let head = text_events(&fx)[0].0.clone();
    // A screenshot whose parent sample was absorbed, sent in a later batch.
    let (path, sha) = write_staging(&fx.store, &[1, 2, 3], "jpg");
    let mut s = shot("shot", T + 6_000, &path, &sha);
    s.parent_key = Some("b".into());
    fx.store.ingest(&IngestBatch { on_missing_parent: Some("detach".into()), ..merging(&fx.source, vec![s]) }).unwrap();
    assert_eq!(fx.texts("SELECT parent_event_id FROM events WHERE source_key='shot'"), vec![head.clone()]);
    fx.store.delete_events(&[head], "user").unwrap();
    let late = fx.store.ingest(&merging(&fx.source, vec![sample("b", T + 5_000, "正文", &page("一二"), "captured")])).unwrap();
    assert_eq!((late.inserted, late.tombstoned), (0, 1));
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events"), 0);
}
