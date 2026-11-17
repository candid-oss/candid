//! Port of Swift `IngestTests`.
mod common;
use common::*;
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::json;

#[test]
fn reingest_is_idempotent_and_adds_no_rows() {
    let fx = Fx::make("idem");
    let mut turn = IngestEvent::new("b", "ai.turn", Some(2000));
    turn.time_basis = "source".into();
    turn.thread_key = Some("t1".into());
    turn.payload = json!({"role": "user"});
    turn.text = vec![IngestText::new("message", "hello")];
    let batch = IngestBatch {
        on_missing_parent: None,
        merge_text: None, removed_source_keys: vec![], payload_revisions: vec![],
        source_id: fx.source.clone(),
        cursor_after: Some(IngestCursor::new("s", "c1")),
        threads: vec![IngestThread { source_key: "t1".into(), kind: "ai_session".into(), title: Some("Synthetic session".into()), ..Default::default() }],
        events: vec![text_event("a", 1000, "第一段预算说明\n\n第二段 second paragraph"), turn],
    };
    let first = fx.store.ingest(&batch).unwrap();
    assert!(first.inserted == 2 && first.threads_inserted == 1);
    let counts = fx.store.table_counts().unwrap();
    let again = fx.store.ingest(&batch).unwrap();
    assert!(again.inserted == 0 && again.duplicates == 2 && again.conflicts == 0, "{again:?}");
    assert_eq!(fx.store.table_counts().unwrap(), counts);
    assert_eq!(again.event_ids, first.event_ids);
    assert_eq!(fx.store.conflict_count(None).unwrap(), 0);
}

#[test]
fn same_key_different_content_goes_to_conflicts_without_overwriting() {
    let fx = Fx::make("conflict");
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![text_event("k", 1, "original text")])).unwrap();
    let changed = IngestBatch::events(&fx.source, vec![text_event("k", 1, "different text")]);
    let r = fx.store.ingest(&changed).unwrap();
    assert!(r.conflicts == 1 && r.inserted == 0);
    assert_eq!(fx.store.conflict_count(None).unwrap(), 1);
    fx.store.ingest(&changed).unwrap(); // identical conflicting retry is not recorded twice
    assert_eq!(fx.store.conflict_count(None).unwrap(), 1);
    assert_eq!(fx.texts("SELECT c.text FROM event_text t JOIN text_chunks c USING(chunk_id)"), vec!["original text"]);
}

#[test]
fn failed_batch_rolls_back_everything_including_cursor() {
    let fx = Fx::make("rollback");
    let good = text_event("g", 1, "fine");
    let bad = IngestEvent::new("bad", "ai.turn", Some(2)); // missing role
    let batch = |events| IngestBatch { on_missing_parent: None, merge_text: None, removed_source_keys: vec![], payload_revisions: vec![], source_id: fx.source.clone(), cursor_after: Some(IngestCursor::new("s", "x")), threads: vec![], events };
    assert!(fx.store.ingest(&batch(vec![good.clone(), bad])).is_err());
    assert_eq!(fx.store.table_counts().unwrap()["events"], 0);
    // a parent that never arrives fails at ingest time and rolls back as well
    let mut orphan = IngestEvent::new("child", "screen.shot", Some(3));
    orphan.parent_key = Some("nope".into());
    assert!(matches!(fx.store.ingest(&batch(vec![good, orphan])), Err(TimelineError::MissingParent(_))));
    assert_eq!(fx.store.table_counts().unwrap()["events"], 0);
    assert_eq!(fx.store.cursor(&fx.source, "s").unwrap(), None);
}

#[test]
fn validation_rejects_bad_input() {
    let fx = Fx::make("validate");
    let rejects = |e: IngestEvent| fx.store.ingest(&IngestBatch::events(&fx.source, vec![e])).is_err();
    assert!(rejects(IngestEvent::new("1", "nope.kind", Some(1))));
    let mut e = note("2", Some(1));
    e.time_basis = "guess".into();
    assert!(rejects(e));
    assert!(rejects(note("3", None)));
    let mut e = note("4", Some(10));
    e.end_ms = Some(5);
    assert!(rejects(e));
    let mut e = note("5", Some(1));
    e.text = vec![IngestText::new("bogus", "x")];
    assert!(rejects(e));
    let mut e = note("6", Some(1));
    e.payload = json!([]);
    assert!(rejects(e));
    let mut e = note("7", Some(1));
    e.payload_version = 9;
    assert!(rejects(e));
    assert!(rejects(IngestEvent::new("8", "coverage.gap", Some(1))));
    assert!(fx.store.ingest(&IngestBatch::events("missing", vec![])).is_err());
    fx.store.set_source_status(&fx.source, "paused", None).unwrap();
    assert!(fx.store.ingest(&IngestBatch::events(&fx.source, vec![])).is_err());
    // unknown time is allowed only as `imported`
    fx.store.set_source_status(&fx.source, "active", None).unwrap();
    let mut e = note("u", None);
    e.time_basis = "imported".into();
    assert_eq!(fx.store.ingest(&IngestBatch::events(&fx.source, vec![e])).unwrap().inserted, 1);
}

#[test]
fn blobs_are_hashed_and_stored_once_across_events() {
    let fx = Fx::make("blobs");
    let bytes = b"synthetic image bytes";
    let (u1, sha) = write_staging(&fx.store, bytes, "jpg");
    let (u2, _) = write_staging(&fx.store, bytes, "jpg");
    let mut s1 = shot("s1", 1, &u1, &sha);
    s1.blobs[0].mime_type = Some("image/jpeg".into());
    s1.blobs[0].pixel_w = Some(4);
    s1.blobs[0].pixel_h = Some(3);
    let r = fx.store.ingest(&IngestBatch::events(&fx.source, vec![s1, shot("s2", 2, &u2, &sha)])).unwrap();
    assert!(r.inserted == 2 && r.blobs_stored == 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM blobs"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_blobs"), 2);
    assert!(!std::path::Path::new(&u1).exists() && !std::path::Path::new(&u2).exists());
    assert!(fx.store.integrity_check(true).unwrap().ok);
    // declared hash that does not match the bytes is rejected and nothing is stored
    let (bad, _) = write_staging(&fx.store, b"other", "jpg");
    assert!(fx.store.ingest(&IngestBatch::events(&fx.source, vec![shot("s3", 3, &bad, &sha)])).is_err());
    // paths outside staging are refused
    let outside = fx.root.join("outside.jpg");
    std::fs::write(&outside, bytes).unwrap();
    assert!(fx.store.ingest(&IngestBatch::events(&fx.source, vec![shot("s4", 4, &outside.to_string_lossy(), &sha)])).is_err());
}

#[test]
fn policy_and_protected_events_never_store_text_or_media() {
    let fx = Fx::with("policy", false, ClientScope::unbounded());
    let (u, sha) = write_staging(&fx.store, b"pixels", "jpg");
    let mut p = IngestEvent::new("p", "screen.text", Some(2));
    p.sensitivity = "protected".into();
    p.text = vec![IngestText::new("body", "secret-ish")];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![shot("m", 1, &u, &sha), p])).unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM blobs"), 0);
    assert!(!std::path::Path::new(&u).exists());
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_text"), 0);
    fx.store.update_policy(&fx.source, &SourcePolicy::new(false, true)).unwrap();
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![text_event("t", 3, "metadata only")])).unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_text"), 0);
    // exclusions
    let mut policy = SourcePolicy::default();
    policy.exclude.apps = vec!["com.example.vault".into()];
    policy.exclude.patterns = vec!["do-not-store".into()];
    fx.store.update_policy(&fx.source, &policy).unwrap();
    let r = fx
        .store
        .ingest(&IngestBatch::events(
            &fx.source,
            vec![
                screen_text("x1", 4, "com.example.vault", "Synthetic doc", "a", None, None),
                text_event("x2", 5, "contains DO-NOT-STORE marker"),
                text_event("x3", 6, "kept"),
            ],
        ))
        .unwrap();
    assert_eq!(r.inserted, 1);
}

#[test]
fn chunking_normalises_splits_long_paragraphs_and_dedupes() {
    let long = "这是一个合成的长句子用来测试切分。".repeat(60);
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
    assert_eq!(TextChunker::chunk(&"x".repeat(1300)).iter().map(|c| c.scalar_count).collect::<Vec<_>>(), vec![512, 512, 276]);
    assert_eq!(TextChunker::language(&"预算 budget".chars().collect::<Vec<_>>()), Some("mixed"));
}

#[test]
fn repeated_long_context_adds_references_not_text() {
    let fx = Fx::make("dedupe");
    let context = (0..20).map(|i| format!("Synthetic context paragraph number {i} repeated by a terminal window.")).collect::<Vec<_>>().join("\n\n");
    let events = (0..5)
        .map(|i| screen_text(&format!("e{i}"), 1000 + i, "com.example.notes", "Synthetic terminal", &format!("{context}\n\nunique line {i}"), None, None))
        .collect();
    fx.store.ingest(&IngestBatch::events(&fx.source, events)).unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM text_chunks"), 20 + 5);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_text"), 5 * 21);
    // is_new: first event all new; later events only their unique line
    let new_counts: Vec<(String, i64)> = fx
        .store
        .raw_db()
        .query(
            "SELECT e.source_key, SUM(t.is_new) FROM event_text t JOIN events e USING(event_id) GROUP BY e.source_key ORDER BY e.source_key",
            &[],
            |r| (r.text(0), r.int(1)),
        )
        .unwrap();
    assert_eq!(new_counts.iter().find(|c| c.0 == "e0").unwrap().1, 21);
    for i in 1..5 {
        assert_eq!(new_counts.iter().find(|c| c.0 == format!("e{i}")).unwrap().1, 1);
    }
    // other window of same app is compared separately
    let other = fx
        .store
        .ingest(&IngestBatch::events(&fx.source, vec![screen_text("o", 1010, "com.example.notes", "Another window", &context, None, None)]))
        .unwrap();
    assert_eq!(other.inserted, 1);
    assert_eq!(fx.scalar("SELECT SUM(is_new) FROM event_text t JOIN events e USING(event_id) WHERE e.source_key='o'"), 20);
    // non-screen text is always new
    let mut n = note("n", Some(2000));
    n.text = vec![IngestText::new("note", &context)];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![n])).unwrap();
    assert_eq!(fx.scalar("SELECT SUM(is_new) FROM event_text t JOIN events e USING(event_id) WHERE e.source_key='n'"), 20);
}

#[test]
fn attention_parts_keep_per_region_marks() {
    let fx = Fx::make("attention");
    let mut e = text_event("a", 1, "");
    e.text = vec![IngestText {
        field: "body".into(),
        content: String::new(),
        visible: None,
        focus_distance: None,
        parts: Some(vec![
            IngestTextPart { content: "focused line".into(), visible: Some(true), focus_distance: Some(0) },
            IngestTextPart { content: "far line".into(), visible: Some(false), focus_distance: Some(9) },
        ]),
    }];
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![e])).unwrap();
    let rows = fx
        .store
        .raw_db()
        .query("SELECT t.ord,t.visible,t.focus_distance,c.text,t.char_start FROM event_text t JOIN text_chunks c USING(chunk_id) ORDER BY t.ord", &[], |r| {
            (r.int(0), r.opt_int(1), r.opt_int(2), r.text(3), r.int(4))
        })
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].1 == Some(1) && rows[0].2 == Some(0) && rows[0].3 == "focused line");
    assert!(rows[1].1 == Some(0) && rows[1].2 == Some(9));
    assert_eq!(rows[1].4, "focused line".chars().count() as i64 + 2);
}

#[test]
fn changes_feed_and_monotonic_seq() {
    let fx = Fx::make("changes");
    let r1 = fx.store.ingest(&IngestBatch::events(&fx.source, vec![text_event("a", 1, "x"), text_event("b", 2, "y")])).unwrap();
    let r2 = fx.store.ingest(&IngestBatch::events(&fx.source, vec![text_event("c", 3, "z")])).unwrap();
    assert!(r1.first_seq == Some(1) && r1.last_seq == Some(2) && r2.first_seq == Some(3));
    let all = fx.store.changes(0, 500).unwrap();
    assert_eq!(all.iter().filter(|c| c.entity == "event" && c.op == "insert").count(), 3);
    let seqs: Vec<i64> = all.iter().map(|c| c.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort();
    assert_eq!(seqs, sorted);
    let tail = fx.store.changes(all[all.len() - 2].seq, 500).unwrap();
    assert_eq!(tail.len(), 1);
}

#[test]
fn deletion_is_tombstoned_first_and_blocks_late_arrivals() {
    let fx = Fx::make("delete");
    let (u, sha) = write_staging(&fx.store, b"img", "jpg");
    let mut focus = IngestEvent::new("focus", "window.focus", Some(1));
    focus.title = Some("Synthetic title".into());
    let mut shot_ev = shot("shot", 2, &u, &sha);
    shot_ev.parent_key = Some("focus".into());
    let mut msg = note("msg", Some(3));
    msg.thread_key = Some("th".into());
    msg.text = vec![IngestText::new("note", "unique deletable text")];
    let mut keep = note("keep", Some(4));
    keep.text = vec![IngestText::new("note", "shared text")];
    let mut keep2 = note("keep2", Some(5));
    keep2.text = vec![IngestText::new("note", "unique deletable text")];
    let mut batch = IngestBatch {
        on_missing_parent: None,
        merge_text: None, removed_source_keys: vec![], payload_revisions: vec![],
        source_id: fx.source.clone(),
        cursor_after: None,
        threads: vec![IngestThread { source_key: "th".into(), kind: "chat".into(), ..Default::default() }],
        events: vec![focus, shot_ev, msg, keep, keep2],
    };
    let inserted = fx.store.ingest(&batch).unwrap();
    let rel = fx.texts("SELECT rel_path FROM blobs")[0].clone();
    let focus_id = inserted.event_ids["focus"].clone();
    // deleting the parent removes the screenshot child, its blob file, and an emptied thread; shared chunk survives
    let report = fx.store.delete_events(&[focus_id.clone(), inserted.event_ids["msg"].clone()], "user").unwrap();
    assert!(report.events == 3 && report.blobs == 1 && report.threads == 1, "{report:?}");
    assert!(!fx.root.join(&rel).exists());
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM text_chunks WHERE text='unique deletable text'"), 1);
    assert!(fx.store.is_tombstoned("event", &focus_id).unwrap());
    let tombs = fx.texts("SELECT entity_id FROM tombstones");
    assert!(!tombs.iter().any(|t| t.contains("Synthetic title")));
    // late arrival of a deleted record is ignored, not resurrected
    let (u2, _) = write_staging(&fx.store, b"img", "jpg"); // a retried batch is re-staged by the connector
    batch.events[1].blobs[0].staging_path = u2;
    let again = fx.store.ingest(&batch).unwrap();
    assert!(again.tombstoned == 3 && again.inserted == 0 && again.duplicates == 2, "{again:?}");
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events"), 2);
    assert!(fx.store.integrity_check(false).unwrap().ok);
    // source-wide deletion leaves an empty, consistent store
    fx.store.delete_source_data(&fx.source, "source_revoked").unwrap();
    let counts = fx.store.table_counts().unwrap();
    assert!(counts["events"] == 0 && counts["text_chunks"] == 0);
    assert!(fx.store.integrity_check(false).unwrap().ok);
}

#[test]
fn ingest_batch_decodes_the_spec_example_shape() {
    let batch = r#"{"source_id":"s","cursor_after":{"stream":"sessions","cursor":"c"},"threads":[{"source_key":"t","kind":"ai_session","title":"x","started_ms":0}],"events":[{"source_key":"e","kind":"ai.turn","start_ms":0,"time_basis":"source","thread_key":"t","payload_version":1,"payload":{"role":"user"},"text":[{"field":"message","content":"hi"}],"blobs":[]}]}"#;
    let decoded: IngestBatch = serde_json::from_str(batch).unwrap();
    assert!(decoded.events[0].text[0].content == "hi" && decoded.threads[0].kind == "ai_session");
    assert_eq!(decoded.cursor_after.unwrap().stream, "sessions");
}

#[test]
fn source_removals_and_payload_revisions_commit_with_the_batch() {
    let fx = Fx::make("source-changes");
    let mail = |key: &str, boxes: &[&str]| {
        let mut e = IngestEvent::new(key, "mail.message", Some(1000));
        e.time_basis = "source".into();
        e.payload = json!({"direction": "in", "mailboxes": boxes});
        e.text = vec![IngestText::new("body", &format!("SYNTHETIC {key}")), IngestText::new("quoted", "> SYNTHETIC history")];
        e
    };
    let r = fx.store.ingest(&IngestBatch::events(&fx.source, vec![mail("m1", &["INBOX"]), mail("m2", &["INBOX"])])).unwrap();
    assert_eq!(r.inserted, 2);
    let id = fx.store.event_id(&fx.source, "m1").unwrap().unwrap();
    let moved = IngestBatch {
        source_id: fx.source.clone(),
        cursor_after: Some(IngestCursor::new("mail", "c2")),
        payload_revisions: vec![PayloadRevision { source_key: "m1".into(), payload: json!({"direction": "in", "mailboxes": ["Archive"]}) }],
        removed_source_keys: vec!["m2".into(), "never-stored".into()],
        ..Default::default()
    };
    let r = fx.store.ingest(&moved).unwrap();
    assert_eq!((r.revised, r.removed), (1, 1));
    assert_eq!(fx.store.event_id(&fx.source, "m1").unwrap().as_deref(), Some(id.as_str()));
    let payload: serde_json::Value = serde_json::from_str(&fx.texts(&format!("SELECT payload_json FROM events WHERE event_id='{id}'"))[0]).unwrap();
    assert_eq!((payload["mailboxes"].clone(), payload["revision"].clone()), (json!(["Archive"]), json!(1)));
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM agent_event_revisions"), 1);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM tombstones WHERE reason='source_deleted'"), 2); // event + key
    assert_eq!(fx.store.cursor(&fx.source, "mail").unwrap().as_deref(), Some("c2"));
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM changes WHERE op='update' AND entity='event'"), 1);
    // Idempotent: the same revision again changes nothing; the removed key cannot come back.
    let r = fx.store.ingest(&moved).unwrap();
    assert_eq!((r.revised, r.removed), (0, 0));
    let r = fx.store.ingest(&IngestBatch::events(&fx.source, vec![mail("m2", &["INBOX"])])).unwrap();
    assert_eq!((r.inserted, r.tombstoned), (0, 1));
    // A revision missing a required payload field rolls the whole batch back.
    let bad = IngestBatch {
        source_id: fx.source.clone(),
        cursor_after: Some(IngestCursor::new("mail", "c3")),
        payload_revisions: vec![PayloadRevision { source_key: "m1".into(), payload: json!({"mailboxes": []}) }],
        ..Default::default()
    };
    assert!(fx.store.ingest(&bad).is_err());
    assert_eq!(fx.store.cursor(&fx.source, "mail").unwrap().as_deref(), Some("c2"));
}
