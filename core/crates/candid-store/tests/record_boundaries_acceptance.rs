//! Independent R2 acceptance scenarios; all content is synthetic.
use candid_store::records::*;

fn observation(key: &str, at_ms: i64, field: &str, value: &str) -> Observation {
    Observation {
        source: "synthetic-boundaries".into(), key: key.into(), at_ms,
        sequence: at_ms, app: "synthetic-app".into(),
        evidence: Evidence {
            format: "candid-record-evidence/1".into(),
            object: ObjectIdentity { key: "synthetic-document".into(), basis: "path".into() },
            field: Some(field.into()), input_value: Some(value.into()),
            input_status: "captured".into(), active: true,
            reading: vec![], confirmation: None,
        },
    }
}

#[test]
fn a_pause_longer_than_two_minutes_starts_another_input_in_the_same_visit() {
    let result = rebuild(&[
        observation("first", 0, "composer", "first draft"),
        observation("second", 120_001, "composer", "second draft"),
    ], Rules::default()).unwrap();
    assert_eq!(result.visits.len(), 1);
    assert_eq!(result.inputs.len(), 2, "two input sessions must retain both final drafts");
    assert_eq!(result.inputs[0].final_text, "first draft");
}

#[test]
fn moving_between_input_fields_ends_the_previous_input() {
    let result = rebuild(&[
        observation("first", 0, "composer", "first draft"),
        observation("search", 1_000, "search-field", "search words"),
        observation("return", 2_000, "composer", "new draft"),
    ], Rules::default()).unwrap();
    assert_eq!(result.inputs.len(), 3, "returning to a field must not overwrite its previous session");
    assert_eq!(result.inputs[0].final_text, "first draft");
}

mod common;

#[test]
fn a_metadata_only_client_cannot_read_input_text_through_the_payload() {
    use candid_store::*;
    let fx = common::Fx::with("record-evidence-scope", false,
        ClientScope { text: false, ..ClientScope::unbounded() });
    let sample = observation("input", 1_000, "composer", "SYNTHETIC private draft");
    let mut event = IngestEvent::new("input", "screen.text", Some(1_000));
    event.payload = serde_json::json!({"record_evidence": sample.evidence});
    let result = fx.store.ingest(&IngestBatch::events(&fx.source, vec![event])).unwrap();
    let response = fx.store.get_event(&GetEventRequest {
        event_id: result.event_ids["input"].clone(), include: None,
    }, &fx.client).unwrap();
    assert!(!serde_json::to_string(&response).unwrap().contains("SYNTHETIC private draft"));
    let full = fx.store.register_client("Synthetic full reader", "http",
        &ClientScope::unbounded(), None).unwrap().client_id;
    let request = GetEventRequest { event_id: result.event_ids["input"].clone(), include: None };
    let response = fx.store.get_event(&request, &full).unwrap();
    assert!(serde_json::to_string(&response).unwrap().contains("SYNTHETIC private draft"));
    let response = fx.store.get_event(&GetEventRequest {
        include: Some(vec!["neighbors".into()]), ..request
    }, &full).unwrap();
    assert!(!serde_json::to_string(&response).unwrap().contains("SYNTHETIC private draft"));
}
