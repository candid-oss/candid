//! Fixed independent truth: two sends with distinct message IDs and identical content,
//! plus failed multiline and cleared drafts. No real capture or inferred success.
use candid_metrics::*;
use candid_store::records::{self, *};
#[test]
fn r2_output_uses_measurement_one_with_independent_truth() {
    let at = 1794268800000i64;
    let make = |key: &str, n: i64, value: Option<&str>| Observation {
        source: "synthetic".into(),
        key: key.into(),
        at_ms: at + n,
        sequence: n,
        app: "synthetic.chat".into(),
        evidence: Evidence {
            format: "candid-record-evidence/1".into(),
            object: ObjectIdentity {
                key: "https://synthetic.invalid/chat".into(),
                basis: "url".into(),
            },
            field: Some("composer".into()),
            input_value: value.map(Into::into),
            input_status: "captured".into(),
            active: true,
            reading: vec![],
            confirmation: None,
        },
    };
    let body = "A complete synthetic message retained exactly for independent comparison.";
    let mut first = make("c", 20, None);
    first.evidence.confirmation = Some(Confirmation {
        source_hash: None,
        observation: "b".into(),
        message_id: "message1".into(),
        text: body.into(),
        basis: "source_send".into(),
    });
    let mut second = make("e", 40, None);
    second.evidence.confirmation = Some(Confirmation {
        source_hash: None,
        observation: "d".into(),
        message_id: "message2".into(),
        text: body.into(),
        basis: "outgoing_message".into(),
    });
    let observations = vec![
        make("a", 0, Some(body)),
        make("b", 10, Some("")),
        first,
        make("d", 30, Some(body)),
        second,
        make("f", 50, Some("failed\nnewline")),
        make("g", 60, Some("")),
        make("h", 70, Some("cleared draft")),
        make("i", 80, Some("")),
    ];
    let r = records::rebuild(&observations, Rules::default()).unwrap();
    let truth = vec![
        ("a", body, true),
        ("d", body, true),
        ("f", "failed\nnewline", false),
        ("h", "cleared draft", false),
    ];
    let mut data = Dataset {
        format: "candid-measurement/1".into(),
        synthetic: true,
        observed: vec![Span {
            start_ms: at,
            end_ms: at + 100,
        }],
        effective: Some(vec![Span {
            start_ms: at,
            end_ms: at + 80,
        }]),
        visits: Some(vec![candid_metrics::Visit {
            id: r.visits[0].id.clone(),
            object_id: Some(r.visits[0].object.key.clone()),
            active: vec![Span {
                start_ms: at,
                end_ms: at + 80,
            }],
        }]),
        inputs: Some(vec![]),
        ..Default::default()
    };
    for (n, (key, text, sent)) in truth.iter().enumerate() {
        // Ground truth IDs are matched by source observation, never inferred from output count.
        let id = format!("9:synthetic{key}");
        data.inputs.as_mut().unwrap().push(InputSession {
            id: id.clone(),
            at_ms: at + n as i64,
            sent: *sent,
            expected_text: (*text).into(),
        });
        for input in r.inputs.iter().filter(|input| input.id == id) {
            data.input_records.push(InputRecord {
                at_ms: at + n as i64,
                session_id: id.clone(),
                visit_id: input.visit.clone(),
                text: input.final_text.clone(),
                truncated: false,
            });
            assert_eq!(input.sent.is_some(), *sent);
        }
    }
    assert_eq!(r.inputs.len(), 4); // zero missed sessions and zero wrongly merged sessions
    assert_eq!(r.inputs.iter().filter(|i| i.sent.is_some()).count(), 2); // zero false sends
    let report = measure(
        &data,
        chrono::NaiveDate::from_ymd_opt(2026, 11, 10).unwrap(),
        chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
        chrono_tz::Asia::Singapore,
    )
    .unwrap();
    let m = &report.days[0].metrics;
    assert_eq!(m["input_completeness"].numerator, Some(2.));
    assert_eq!(m["input_completeness"].denominator, Some(2.));
    assert_eq!(m["input_fragmentation"].value, Some(1.));
    assert_eq!(m["visit_fragmentation"].value, Some(0.));
    assert_eq!(m["object_identification"].value, Some(1.));
    println!("synthetic R2: completeness=2/2, input fragmentation=4/4, reading fragmentation=0/1, stable objects=1/1; missed=0, wrong merges=0, false sends=0");
}
