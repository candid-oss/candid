//! One-time conversion of pre-v1 originals: JPEG screenshots → WebP, WAV parts queued for AAC. Synthetic data only.
mod common;
use candid_store::library_conversion::jpeg_to_webp;
use candid_store::*;
use common::*;

const JPEG: &[u8] = include_bytes!("fixtures/synthetic-gradient.jpg");

#[test]
fn jpeg_screenshots_become_webp_for_every_event_that_used_them() {
    let fx = Fx::make("jpeg-webp");
    fx.store.mark_desktop_direct(&serde_json::json!({"synthetic": true})).unwrap();
    let (u1, sha) = write_staging(&fx.store, JPEG, "jpg");
    let (u2, _) = write_staging(&fx.store, JPEG, "jpg");
    let mut s1 = shot("s1", 1, &u1, &sha);
    s1.blobs[0].mime_type = Some("image/jpeg".into());
    let mut s2 = shot("s2", 2, &u2, &sha);
    s2.blobs[0].mime_type = Some("image/jpeg".into());
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s1, s2])).unwrap();
    let old_file = fx.root.join(fx.texts("SELECT rel_path FROM blobs")[0].clone());
    assert_eq!(fx.store.jpeg_screenshot_backlog().unwrap().0, 1);

    let candidates = fx.store.jpeg_screenshot_candidates(10).unwrap();
    assert_eq!(candidates.len(), 1);
    let (webp, w, h) = jpeg_to_webp(JPEG).unwrap();
    assert_eq!((w, h), (96, 64));
    assert!(fx.store.replace_screenshot_with_webp(&candidates[0], &webp, w, h).unwrap());

    assert_eq!(fx.texts("SELECT mime_type FROM blobs"), ["image/webp"]);
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM event_blobs WHERE sha256=(SELECT sha256 FROM blobs)"), 2);
    assert!(!old_file.exists(), "the JPEG file is removed");
    assert_eq!(fx.store.jpeg_screenshot_backlog().unwrap().0, 0);
    assert!(fx.store.integrity_check(true).unwrap().ok);
    let status = fx.store.conversion_status().unwrap();
    assert_eq!(status["screenshots"]["count"], 1);
    // Running again finds nothing, and a stale candidate changes nothing.
    assert!(fx.store.jpeg_screenshot_candidates(10).unwrap().is_empty());
    assert!(!fx.store.replace_screenshot_with_webp(&candidates[0], &webp, w, h).unwrap());
    assert!(fx.store.integrity_check(true).unwrap().ok);
}

#[test]
fn nothing_is_converted_before_the_cutover_to_direct_capture() {
    let fx = Fx::make("jpeg-ledger");
    let (u1, sha) = write_staging(&fx.store, JPEG, "jpg");
    let mut s1 = shot("s1", 1, &u1, &sha);
    s1.blobs[0].mime_type = Some("image/jpeg".into());
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s1])).unwrap();
    assert!(fx.store.jpeg_screenshot_candidates(10).unwrap().is_empty());
}

#[test]
fn bad_input_is_refused_and_leaves_the_jpeg() {
    let fx = Fx::make("jpeg-bad");
    fx.store.mark_desktop_direct(&serde_json::json!({"synthetic": true})).unwrap();
    let (u1, sha) = write_staging(&fx.store, JPEG, "jpg");
    let mut s1 = shot("s1", 1, &u1, &sha);
    s1.blobs[0].mime_type = Some("image/jpeg".into());
    fx.store.ingest(&IngestBatch::events(&fx.source, vec![s1])).unwrap();
    let c = fx.store.jpeg_screenshot_candidates(10).unwrap();
    assert!(fx.store.replace_screenshot_with_webp(&c[0], b"not a webp", 1, 1).is_err());
    assert!(jpeg_to_webp(b"not a jpeg").is_err());
    assert_eq!(fx.texts("SELECT mime_type FROM blobs"), ["image/jpeg"]);
    assert!(fx.store.integrity_check(true).unwrap().ok);
}
