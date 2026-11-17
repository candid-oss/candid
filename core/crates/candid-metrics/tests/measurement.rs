use chrono::NaiveDate;
use chrono_tz::Asia::Singapore;
use candid_metrics::*;
use serde_json::Value;
fn fixture() -> Dataset {
    serde_json::from_str(include_str!("../../../fixtures/metrics/ideal-day.json")).unwrap()
}
fn report(d: &Dataset) -> Report {
    measure(
        d,
        NaiveDate::from_ymd_opt(2026, 11, 10).unwrap(),
        NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
        Singapore,
    )
    .unwrap()
}
fn val(r: &Report, key: &str) -> Option<f64> {
    r.days[0].metrics[key].value
}

#[test]
fn ideal_day_matches_independent_fixed_expectations() {
    let r = report(&fixture());
    let expected: Value =
        serde_json::from_str(include_str!("../../../fixtures/metrics/expected.json")).unwrap();
    assert!(r.synthetic);
    assert_eq!(serde_json::to_value(&r.counts).unwrap(), expected["counts"]);
    for (key, value) in expected["counts"].as_object().unwrap() {
        assert_eq!(r.counts[key], value.as_u64().unwrap(), "{key}");
    }
    for (key, value) in expected["metrics"].as_object().unwrap() {
        let m = &r.days[0].metrics[key];
        assert_eq!(m.numerator, value["numerator"].as_f64(), "{key}");
        assert_eq!(m.denominator, value["denominator"].as_f64(), "{key}");
        assert!(
            (m.value.unwrap() - value["value"].as_f64().unwrap()).abs() < 1e-12,
            "{key}"
        );
        assert!(m.unavailable.is_none());
    }
}
#[test]
fn no_content_is_removed_to_improve_redundancy() {
    let mut d = fixture();
    let original_chars = report(&d).days[0].metrics["redundancy"]
        .denominator
        .unwrap();
    let mut repeat = d.text_records[0].clone();
    repeat.at_ms += 500;
    // Repeated UI labels count; OCR jitter and changing counts remain separate exact lines.
    let len = repeat
        .text
        .lines()
        .map(|s| s.chars().count())
        .sum::<usize>() as f64;
    d.text_records.push(repeat);
    let r = report(&d);
    assert_eq!(r.days[0].metrics["redundancy"].numerator, Some(len));
    assert_eq!(
        r.days[0].metrics["redundancy"].denominator,
        Some(original_chars + len)
    );
    assert_eq!(val(&r, "visit_fragmentation"), Some(1.));
}
#[test]
fn lost_partial_and_multiple_input_records_fail_completeness_without_dropping_denominator() {
    let mut d = fixture();
    d.input_records.remove(0); // lost
    d.input_records[0].truncated = true;
    d.input_records[1].text = "合成输入".into(); // incomplete, even without truncation flag
    d.input_records.push(d.input_records[2].clone()); // duplicate
    let r = report(&d);
    assert_eq!(r.days[0].metrics["input_completeness"].numerator, Some(21.));
    assert_eq!(
        r.days[0].metrics["input_completeness"].denominator,
        Some(25.)
    );
    assert_eq!(
        r.days[0].metrics["input_fragmentation"].denominator,
        Some(40.)
    );
    assert_eq!(val(&r, "input_fragmentation"), Some(1.)); // illustrates why both metrics are needed
}
#[test]
fn coverage_unions_overlaps_and_preserves_permission_failure_in_denominator() {
    let mut d = fixture();
    let r = report(&d);
    let duplicate = d.covered.as_ref().unwrap()[0].clone();
    d.covered.as_mut().unwrap().push(duplicate);
    // A text interval over the locked lunch hour cannot increase effective coverage.
    d.covered.as_mut().unwrap().push(Span {
        start_ms: d.effective.as_ref().unwrap()[0].end_ms,
        end_ms: d.effective.as_ref().unwrap()[1].start_ms,
    });
    assert_eq!(val(&report(&d), "text_coverage"), val(&r, "text_coverage"));
    d.visits.as_mut().unwrap()[0].object_id = None;
    assert_eq!(val(&report(&d), "object_identification"), Some(119. / 120.));
}
#[test]
fn unknown_and_known_zero_are_distinct() {
    let mut d = fixture();
    d.inputs = None;
    d.input_records.clear();
    d.covered = None;
    d.resources = None;
    d.searchable = None;
    let r = report(&d);
    for k in [
        "input_completeness",
        "input_fragmentation",
        "text_coverage",
        "capture_cpu",
        "capture_wakeups",
        "searchable_latency_mean",
    ] {
        assert!(r.days[0].metrics[k].value.is_none());
        assert!(r.days[0].metrics[k].collect_next.is_some());
    }
    d.searchable = Some(vec![]);
    let zero = report(&d);
    assert_eq!(val(&zero, "search_probe_poll_interval_max"), None);
    d.inputs = Some(vec![]);
    let r = report(&d);
    assert_eq!(
        r.days[0].metrics["input_completeness"].denominator,
        Some(0.)
    );
    assert_eq!(val(&r, "input_completeness"), None);
}
#[test]
fn identity_is_visit_scoped_and_cross_day_seen_text_is_preserved() {
    let mut d = fixture();
    let midnight = 1794326400000;
    d.observed[0].end_ms = midnight + 60_000;
    d.visits.as_mut().unwrap()[0].active.push(Span {
        start_ms: midnight,
        end_ms: midnight + 60_000,
    });
    let mut repeat = d.text_records[0].clone();
    repeat.at_ms = midnight;
    d.text_records.push(repeat);
    let r = measure(
        &d,
        NaiveDate::from_ymd_opt(2026, 11, 10).unwrap(),
        NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        Singapore,
    )
    .unwrap();
    assert_eq!(r.days[1].metrics["redundancy"].value, Some(1.));
    assert_eq!(r.days[1].metrics["visit_fragmentation"].value, Some(1.));
    // Same object and same content in a distinct visit does not count as intra-visit redundancy.
    assert_eq!(r.days[0].metrics["redundancy"].value, Some(0.));
    assert_eq!(
        r.days[1].metrics["input_completeness"].denominator,
        Some(0.)
    );
}
#[test]
fn input_fragments_across_midnight_follow_session_day() {
    let mut d = fixture();
    let midnight = 1794326400000;
    d.observed[0].end_ms = midnight + 1000;
    d.input_records[0].at_ms = midnight;
    let r = report(&d);
    assert_eq!(val(&r, "input_completeness"), Some(1.));
    assert_eq!(val(&r, "input_fragmentation"), Some(1.));
}
#[test]
fn screenshot_references_and_unique_storage_are_separate() {
    let mut d = fixture();
    let mut repeat = d.shots.as_ref().unwrap()[0].clone();
    repeat.at_ms += 1;
    d.shots.as_mut().unwrap().push(repeat);
    let r = report(&d);
    assert_eq!(val(&r, "screenshot_bytes"), Some(15_100_000.));
    assert_eq!(val(&r, "screenshot_unique_bytes"), Some(15_000_000.));
}
#[test]
fn unresolved_probes_stay_visible_and_p95_uses_nearest_rank() {
    let mut d = fixture();
    let t = d.observed[0].start_ms;
    d.searchable = Some(
        (1..=20)
            .map(|i| Searchable {
                occurred_ms: t,
                searchable_ms: Some(t + i * 1000),
                poll_interval_ms: 100,
            })
            .chain(std::iter::once(Searchable {
                occurred_ms: t,
                searchable_ms: None,
                poll_interval_ms: 100,
            }))
            .collect(),
    );
    let r = report(&d);
    assert_eq!(val(&r, "searchable_latency_p95"), Some(19000.));
    assert_eq!(val(&r, "search_probe_unresolved"), Some(1. / 21.));
    assert_eq!(
        r.days[0].metrics["searchable_latency_mean"].denominator,
        Some(20.)
    );
}
#[test]
fn invalid_units_and_ambiguous_signals_are_rejected() {
    let mut d = fixture();
    d.visits.as_mut().unwrap()[1].id = "visit-0".into();
    assert!(d.validate().is_err());
    let mut d = fixture();
    d.text_records[0].visit_id = Some("unknown".into());
    assert!(d.validate().is_err());
    let mut d = fixture();
    d.effective.as_mut().unwrap()[0].end_ms = d.effective.as_ref().unwrap()[0].start_ms;
    assert!(d.validate().is_err());
    let mut d = fixture();
    let duplicate = d.resources.as_ref().unwrap()[0].clone();
    d.resources.as_mut().unwrap().push(duplicate);
    assert!(d.validate().is_err());
    let mut d = fixture();
    d.searchable.as_mut().unwrap()[0].searchable_ms = Some(0);
    assert!(d.validate().is_err());
    let mut d = fixture();
    let mut inconsistent = d.shots.as_ref().unwrap()[0].clone();
    inconsistent.bytes += 1;
    d.shots.as_mut().unwrap().push(inconsistent);
    assert!(d.validate().is_err());
    let mut d = fixture();
    let segment = d
        .context_items
        .iter_mut()
        .find(|item| item.kind == "audio_segment")
        .unwrap();
    segment.parent_id = Some("missing-recording".into());
    assert!(d.validate().is_err());
    let mut d = fixture();
    d.counts.insert("private app name".into(), 1);
    assert!(d.validate().is_err());
}
#[test]
fn no_observation_day_and_dst_are_explicit() {
    let d = fixture();
    let r = measure(
        &d,
        NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
        NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        Singapore,
    )
    .unwrap();
    assert!(r.days[0].metrics.values().all(|m| m.value.is_none()));
    let r = measure(
        &d,
        NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
        NaiveDate::from_ymd_opt(2026, 3, 9).unwrap(),
        chrono_tz::America::New_York,
    )
    .unwrap();
    assert_eq!(r.days[0].end_ms - r.days[0].start_ms, 23 * 3_600_000);
}
#[test]
fn resource_counters_crossing_midnight_are_not_prorated() {
    let mut d = fixture();
    let midnight = 1794326400000;
    d.observed[0].end_ms = midnight + 60_000;
    d.resources.as_mut().unwrap()[0].span.end_ms = midnight + 60_000;
    let r = report(&d);
    assert_eq!(val(&r, "capture_cpu"), None);
    assert!(r.days[0].metrics["capture_cpu"]
        .unavailable
        .as_ref()
        .unwrap()
        .contains("跨日"));
}
