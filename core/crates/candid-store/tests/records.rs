use candid_store::{records::*, *};
fn o(key: &str, at: i64, object: &str, input: Option<&str>) -> Observation {
    Observation {
        source: "synthetic".into(),
        key: key.into(),
        at_ms: at,
        sequence: at,
        app: "test".into(),
        evidence: Evidence {
            format: "candid-record-evidence/1".into(),
            object: ObjectIdentity {
                key: object.into(),
                basis: "url".into(),
            },
            field: Some("composer".into()),
            input_value: input.map(Into::into),
            input_status: "captured".into(),
            active: true,
            reading: vec![],
            confirmation: None,
        },
    }
}
fn confirmation(
    key: &str,
    at: i64,
    target: &str,
    message: &str,
    body: &str,
    basis: &str,
) -> Observation {
    let mut x = o(key, at, "chat", None);
    x.evidence.confirmation = Some(Confirmation {
        source_hash: None,
        observation: target.into(),
        message_id: message.into(),
        text: body.into(),
        basis: basis.into(),
    });
    x
}
#[test]
fn return_mouse_failure_newline_clear_and_two_identical_sends() {
    // Independent truth: two successful sends; one multiline failed draft and a cleared draft.
    let evidence = vec![
        o("a", 0, "chat", Some("hello")),
        o("b", 1, "chat", Some("")),
        confirmation("c", 2, "b", "message-1", "hello", "source_send"),
        o("d", 3, "chat", Some("hello")),
        confirmation("e", 4, "d", "message-2", "hello", "outgoing_message"),
        o("f", 5, "chat", Some("line\nnext")),
        o("g", 6, "chat", Some("")),
        o("h", 7, "chat", Some("cleared only")),
        o("i", 8, "chat", Some("")),
    ];
    let r = rebuild(&evidence, Rules::default()).unwrap();
    assert_eq!(r.inputs.len(), 4);
    assert_eq!(r.inputs.iter().filter(|i| i.sent.is_some()).count(), 2);
    assert_eq!(
        r.inputs
            .iter()
            .map(|i| i.final_text.as_str())
            .collect::<Vec<_>>(),
        vec!["hello", "hello", "line\nnext", "cleared only"]
    );
    assert_ne!(r.inputs[0].id, r.inputs[1].id);
    assert_eq!(
        rebuild(&[evidence.clone(), evidence].concat(), Rules::default()).unwrap(),
        r
    );
}
#[test]
fn visit_gap_excludes_away_and_exactly_ten_minutes_reuses() {
    let r = rebuild(
        &[
            o("a", 0, "doc", None),
            o("b", 100, "other", None),
            o("c", 600100, "doc", None),
            o("d", 600200, "other", None),
            o("e", 1200201, "doc", None),
        ],
        Rules::default(),
    )
    .unwrap();
    assert_eq!(r.visits.len(), 3);
    assert_eq!(r.visits[0].dwell_ms(), 200);
    assert_eq!(r.visits[0].observations.len(), 2);
}
#[test]
fn large_deletion_preserves_longest_and_empty_is_not_text() {
    let r = rebuild(
        &[
            o("a", 0, "chat", Some("a long original")),
            o("b", 1, "chat", Some("tiny")),
            o("c", 2, "chat", Some("final")),
            o("d", 3, "chat", Some("")),
        ],
        Rules::default(),
    )
    .unwrap();
    assert_eq!(
        r.inputs[0].longest_before_deletion.as_deref(),
        Some("a long original")
    );
    assert_eq!(r.inputs[0].final_text, "final");
    assert!(r.reading.is_empty());
}
#[test]
fn occurrence_identity_preserves_identical_messages_and_approximate_snapshots() {
    let mut a = o("a", 0, "chat", None);
    a.evidence.reading = vec![
        Reading {
            text: "yes".into(),
            occurrence: Some("msg1".into()),
        },
        Reading {
            text: "yes".into(),
            occurrence: Some("msg2".into()),
        },
        Reading {
            text: "A reasonably long paragraph with stable contents".into(),
            occurrence: None,
        },
    ];
    let mut b = a.clone();
    b.key = "b".into();
    b.at_ms = 1;
    b.evidence.reading[2].text.push('.');
    let r = rebuild(&[a, b], Rules::default()).unwrap();
    assert_eq!(r.reading.len(), 3);
    assert!(r.reading.iter().all(|p| p.observations.len() == 2));
}
#[test]
fn revoked_input_and_unknown_versions_fail_and_missing_signals_stay_unknown() {
    let mut a = o("a", 0, "chat", Some("secret"));
    a.evidence.input_status = "disabled".into();
    assert!(rebuild(&[a.clone()], Rules::default()).is_err());
    a.evidence.input_value = None;
    a.evidence.format = "candid-record-evidence/99".into();
    assert!(rebuild(&[a], Rules::default()).is_err());
    let mut e = IngestEvent::new("old", "screen.text", Some(1));
    e.title = Some("old".into());
    e.payload = serde_json::json!({"trigger":"return"});
    e.text.push(IngestText::new("body", "old visible text"));
    let r = rebuild(
        &[from_event("source", &e).unwrap().unwrap()],
        Rules::default(),
    )
    .unwrap();
    assert!(r.inputs.is_empty());
    assert_eq!(r.reading.len(), 1);
    assert_eq!(r.gaps.len(), 1);
}
#[test]
fn wrong_window_or_unproven_confirmation_does_not_send() {
    let a = o("a", 0, "chat", Some("hello"));
    let mut b = confirmation("b", 1, "a", "m", "hello", "source_send");
    b.evidence.object.key = "other".into();
    let r = rebuild(&[a.clone(), b], Rules::default()).unwrap();
    assert!(r.inputs[0].sent.is_none());
    assert!(rebuild(
        &[a, confirmation("c", 2, "a", "m", "hello", "click")],
        Rules::default()
    )
    .is_err());
}
#[test]
fn revisions_rebuild_deterministically_and_switch_back_after_validation() {
    let a = o("a", 0, "chat", Some("draft"));
    let b = o("b", 2, "other", None);
    let mut revisions = Revisions::default();
    let first = revisions
        .stage(rebuild(&[a.clone()], Rules::default()).unwrap())
        .unwrap();
    revisions.select(&first).unwrap();
    let second = revisions
        .stage(rebuild(&[b, a], Rules::default()).unwrap())
        .unwrap();
    revisions.select(&second).unwrap();
    revisions.select(&first).unwrap();
    assert_eq!(revisions.selected.as_deref(), Some(first.as_str()));
    assert_eq!(revisions.revisions.len(), 2);
    revisions.revisions.get_mut(&second).unwrap().inputs[0].final_text = "changed".into();
    assert!(revisions.select(&second).is_err());
    assert_eq!(revisions.selected.as_deref(), Some(first.as_str()));
}

mod common;
#[test]
fn persisted_raw_evidence_survives_merge_retry_and_reopen() {
    let fx = common::Fx::make("records-r2");
    let mut events = vec![];
    for x in [
        o("a", 0, "chat", Some("draft")),
        o("b", 1, "chat", Some("final")),
        confirmation("c", 2, "b", "message-1", "final", "source_send"),
    ] {
        let mut e = IngestEvent::new(&x.key, "screen.text", Some(x.at_ms));
        e.app_bundle_id = Some(x.app);
        e.payload = serde_json::json!({"record_evidence":x.evidence});
        events.push(e);
    }
    let mut batch = IngestBatch::events(&fx.source, events);
    batch.merge_text = Some(true);
    fx.store.ingest(&batch).unwrap();
    fx.store.ingest(&batch).unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events"), 3);
    let r = fx
        .store
        .rebuild_records(&fx.source, Rules::default())
        .unwrap();
    assert_eq!(r.inputs.len(), 1);
    assert!(r.inputs[0].sent.is_some());
    assert_eq!(r.evidence.len(), 3);
    let mut wrong = batch.clone();
    wrong.events[0].source_key = "bad".into();
    wrong.events[0].payload["record_evidence"]["format"] = serde_json::json!("invalid/2");
    assert!(fx.store.ingest(&wrong).is_err());
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events"), 3);
}

#[test]
fn disabled_storage_and_revoked_sources_cannot_expose_evidence() {
    let fx = common::Fx::make("records-policy");
    fx.store
        .update_policy(&fx.source, &SourcePolicy::new(false, false))
        .unwrap();
    let a = o("a", 0, "chat", Some("synthetic private input"));
    let mut e = IngestEvent::new("a", "screen.text", Some(0));
    e.payload = serde_json::json!({"record_evidence":a.evidence});
    fx.store
        .ingest(&IngestBatch::events(&fx.source, vec![e]))
        .unwrap();
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE json_extract(payload_json,'$.record_evidence') IS NOT NULL"),0);
    assert!(fx
        .store
        .rebuild_records(&fx.source, Rules::default())
        .is_err());
    fx.store
        .update_policy(&fx.source, &SourcePolicy::new(true, false))
        .unwrap();
    fx.store
        .set_source_status(&fx.source, "revoked", None)
        .unwrap();
    assert!(fx
        .store
        .rebuild_records(&fx.source, Rules::default())
        .is_err());
}
#[test]
fn empty_evidence_is_a_gap_without_invented_text() {
    let fx = common::Fx::make("records-empty");
    let x = o("a", 0, "chat", Some(""));
    let mut e = IngestEvent::new("a", "coverage.gap", Some(0));
    e.payload = serde_json::json!({"reason_code":"text_unavailable","record_evidence":x.evidence});
    fx.store
        .ingest(&IngestBatch::events(&fx.source, vec![e]))
        .unwrap();
    let r = fx
        .store
        .rebuild_records(&fx.source, Rules::default())
        .unwrap();
    assert!(r.inputs.is_empty());
    assert!(r.reading.is_empty());
    assert_eq!(r.gaps.len(), 1);
}

#[test]
fn same_millisecond_events_follow_source_sequence_and_conflicts_are_rejected() {
    let mut a = o("z", 0, "chat", Some("message"));
    a.sequence = 1;
    let mut b = confirmation("a", 0, "z", "m", "message", "source_send");
    b.sequence = 2;
    let r = rebuild(&[b, a.clone()], Rules::default()).unwrap();
    assert!(r.inputs[0].sent.is_some());
    let mut conflict = a.clone();
    conflict.evidence.input_value = Some("different".into());
    assert!(rebuild(&[a, conflict], Rules::default()).is_err());
}

#[test]
fn distinct_source_sends_survive_without_an_intervening_value_change() {
    let r = rebuild(
        &[
            o("draft", 0, "chat", Some("same")),
            confirmation("send1", 1, "draft", "m1", "same", "source_send"),
            confirmation("send2", 2, "draft", "m2", "same", "source_send"),
        ],
        Rules::default(),
    )
    .unwrap();
    assert_eq!(r.inputs.len(), 2);
    assert!(r.inputs.iter().all(|i| i.sent.is_some()));
    assert_ne!(r.inputs[0].id, r.inputs[1].id);
}
