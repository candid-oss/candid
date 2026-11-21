//! Screenshot budget: over the limit, old originals step down to text only (synthetic library).
mod common;
use candid_store::*;
use common::*;

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;

fn ocr(s: &TimelineStore, source: &str, key: &str, parent: &str, ms: i64, body: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "image.text", Some(ms));
    e.parent_key = Some(parent.into());
    e.payload = serde_json::json!({"method": "ocr", "image_event_id": s.event_id(source, parent).unwrap().unwrap()});
    e.text = vec![IngestText::new("ocr", body)];
    e
}

#[test]
fn oldest_originals_with_text_go_first_and_recent_ones_stay() {
    let fx = Fx::make("screenshot-budget");
    let s = &fx.store;
    let (mut events, mut texts) = (Vec::new(), Vec::new());
    for (key, days, with_text) in [("old-ocr", 30, true), ("old-bare", 20, false), ("mid-ocr", 10, true), ("recent", 1, true)] {
        let (path, sha) = write_staging(s, &vec![days as u8; 1000], "jpg");
        events.push(shot(key, NOW - days * DAY, &path, &sha));
        if with_text {
            texts.push((key, days));
        }
    }
    s.ingest(&IngestBatch::events(&fx.source, events)).unwrap();
    let texts = texts.into_iter().map(|(key, days)| ocr(s, &fx.source, &format!("{key}-ocr"), key, NOW - days * DAY + 1, &format!("SYNTHETIC words {key}"))).collect();
    s.ingest(&IngestBatch::events(&fx.source, texts)).unwrap();
    assert_eq!(s.app_stats().unwrap().storage_bytes, 4000);
    assert_eq!(blob_files(&fx.root).len(), 4);

    // Under the target: nothing happens.
    let none = s.free_screenshot_space(&ScreenshotSpaceRequest { target_bytes: 4000 }).unwrap();
    assert_eq!((none.freed_bytes, none.screenshots, none.reached_target), (0, 0, true));

    // Words-kept screenshots go first, oldest first, and only as many as needed.
    let first = s.free_screenshot_space(&ScreenshotSpaceRequest { target_bytes: 2500 }).unwrap();
    assert_eq!((first.freed_bytes, first.screenshots, first.storage_bytes, first.reached_target), (2000, 2, 2000, true));
    let originals = |key: &str| {
        fx.scalar(&format!(
            "SELECT COUNT(*) FROM event_blobs eb JOIN events e USING(event_id) WHERE e.source_key='{key}' AND eb.role='original'"
        ))
    };
    assert_eq!((originals("old-ocr"), originals("old-bare"), originals("mid-ocr"), originals("recent")), (0, 1, 0, 1));
    assert_eq!(blob_files(&fx.root).len(), 2);
    // The events and their OCR text stay.
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM events WHERE kind='screen.shot'"), 4);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM text_chunks WHERE text LIKE 'SYNTHETIC words old-ocr%'"), 1);

    // Asked for more than is allowed: the bare one goes too, the last 7 days never.
    let second = s.free_screenshot_space(&ScreenshotSpaceRequest { target_bytes: 0 }).unwrap();
    assert_eq!((second.freed_bytes, second.screenshots, second.storage_bytes, second.reached_target), (1000, 1, 1000, false));
    assert_eq!((second.total_freed_bytes, second.total_screenshots), (3000, 3));
    assert_eq!(originals("recent"), 1);
    let stats = s.app_stats().unwrap();
    assert_eq!((stats.storage_bytes, stats.freed_bytes, stats.freed_screenshots), (1000, 3000, 3));
    assert!(s.free_screenshot_space(&ScreenshotSpaceRequest { target_bytes: -1 }).is_err());
}

#[test]
fn a_blob_owned_elsewhere_stops_counting_without_evicting_more_screenshots() {
    let fx = Fx::make("screenshot-budget-shared");
    let s = &fx.store;
    let other = s.register_source("media.import", "Synthetic import", &SourcePolicy::new(true, true)).unwrap();
    let shared = vec![1; 1000];
    let (path, sha) = write_staging(s, &shared, "jpg");
    let (newer_path, newer_sha) = write_staging(s, &vec![2; 1000], "jpg");
    s.ingest(&IngestBatch::events(&fx.source, vec![
        shot("old", NOW - 30 * DAY, &path, &sha),
        shot("newer", NOW - 20 * DAY, &newer_path, &newer_sha),
    ])).unwrap();
    let (path, sha) = write_staging(s, &shared, "jpg");
    s.ingest(&IngestBatch::events(&other, vec![shot("import", NOW - 30 * DAY, &path, &sha)])).unwrap();

    let report = s.free_screenshot_space(&ScreenshotSpaceRequest { target_bytes: 1000 }).unwrap();
    assert_eq!((report.screenshots, report.freed_bytes, report.storage_bytes, report.reached_target), (1, 0, 1000, true));
    assert_eq!(blob_files(&fx.root).len(), 2);
}
