//! `app_day_counts`: per-day amounts for the App's date picker (screen minutes, media, AI), counted with the App's
//! own day boundaries; revoked sources do not count; bad requests and non-App clients are refused. Synthetic data.
mod common;
use common::*;
use candid_store::*;
use candid_store::testing::RowExt;

const DAY: i64 = 86_400_000;
/// Synthetic local midnight (any fixed instant works: the App sends the boundaries).
const D0: i64 = 1_800_000_000_000;

fn request(days: &[[i64; 2]]) -> AppDayCountsRequest {
    AppDayCountsRequest { days: days.to_vec() }
}

#[test]
fn counts_screen_minutes_media_and_ai_per_requested_day() {
    let fx = Fx::make("day-counts");
    let app = fx.store.register_client("App", "internal", &ClientScope::unbounded(), None).unwrap().client_id;
    let mut events = vec![
        // Day 0: three observations in one minute and one two minutes later = 2 distinct minutes.
        text_event("a", D0 + 10_000, "SYNTHETIC one"),
        text_event("b", D0 + 20_000, "SYNTHETIC two"),
        text_event("c", D0 + 50_000, "SYNTHETIC three"),
        text_event("d", D0 + 3 * 60_000, "SYNTHETIC four"),
        note("n1", Some(D0 + 5 * 60_000)),
        // Last millisecond of day 0 and first of day 2 (day 1 stays empty).
        text_event("e", D0 + DAY - 1, "SYNTHETIC late"),
        text_event("f", D0 + 2 * DAY, "SYNTHETIC early"),
    ];
    let mut prompt = IngestEvent::new("p1", "ai.prompt", Some(D0 + 2 * DAY + 60_000));
    prompt.payload = serde_json::json!({"agent": "codex", "session_ref": "SYNTHETIC", "message_ref": "m1", "origin": "user"});
    events.push(prompt);
    events.push(IngestEvent::new("i1", "image.capture", Some(D0 + 2 * DAY + 120_000)));
    let r = fx.store.ingest(&IngestBatch::events(&fx.source, events)).unwrap();
    assert_eq!(r.inserted, 9, "{r:?}");

    let days = [[D0, D0 + DAY], [D0 + DAY, D0 + 2 * DAY], [D0 + 2 * DAY, D0 + 3 * DAY]];
    let counts = fx.store.app_day_counts(&request(&days), &app).unwrap();
    assert_eq!(counts.days.len(), 2, "the empty day is left out: {counts:?}");
    assert_eq!(counts.days[0], AppDayCount { start_ms: D0, screen_minutes: 3, media: 0, ai: 1 });
    assert_eq!(counts.days[1], AppDayCount { start_ms: D0 + 2 * DAY, screen_minutes: 1, media: 1, ai: 1 });

    // A 23-hour day (daylight saving) is whatever the App says it is.
    let short = fx.store.app_day_counts(&request(&[[D0 + 3_600_000, D0 + DAY]]), &app).unwrap();
    assert_eq!(short.days[0].screen_minutes, 1, "only the late record falls inside");

    // Each day is an index range on start time, never a scan of all events.
    let plan = fx.store.raw_db().query(
        "EXPLAIN QUERY PLAN WITH d(i,a,b) AS (VALUES (0,1,2),(1,2,3)) SELECT d.i, COUNT(*) FROM d JOIN events e ON e.start_ms>=d.a AND e.start_ms<d.b
         JOIN sources s ON s.source_id=e.source_id AND s.status<>'revoked' WHERE e.kind IN ('screen.text','note') GROUP BY d.i",
        &[], |r| r.text(3)).unwrap().join(" | ");
    assert!(plan.contains("SEARCH e USING INDEX") && plan.contains("start_ms>? AND start_ms<?"), "{plan}");

    // A revoked source no longer counts.
    fx.store.raw_db().execute("UPDATE sources SET status='revoked' WHERE source_id=?", &[testing::rusqlite::types::Value::Text(fx.source.clone())]).unwrap();
    assert!(fx.store.app_day_counts(&request(&days), &app).unwrap().days.is_empty());
}

#[test]
fn refuses_bad_requests_and_other_clients() {
    let fx = Fx::make("day-counts-refused");
    let app = fx.store.register_client("App", "internal", &ClientScope::unbounded(), None).unwrap().client_id;
    for bad in [vec![], vec![[D0, D0]], vec![[D0 + DAY, D0]], vec![[D0, D0 + 3 * DAY]], vec![[D0, D0 + DAY]; MAX_COUNT_DAYS + 1]] {
        assert!(fx.store.app_day_counts(&request(&bad), &app).is_err(), "{} days accepted", bad.len());
    }
    // The AI client registered by the fixture is not the App.
    assert!(fx.store.app_day_counts(&request(&[[D0, D0 + DAY]]), &fx.client).is_err());
    // Through the shared tool entry, with JSON arguments.
    let value = fx.store.call_tool("app_day_counts", br#"{"days":[[0,86400000]]}"#, &app).ok().unwrap();
    assert_eq!(value["days"].as_array().unwrap().len(), 0);
    assert!(APP_TOOL_NAMES.contains(&"app_day_counts"));
}
