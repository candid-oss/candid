//! `status` sources: `last_sync` is the last successful check, `last_ingest` the last time new data arrived.
mod common;
use candid_store::fixtures::{make_synthetic_hub, temp_root};
use candid_store::*;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const START: i64 = 1_800_000_000_000;

fn media_source(store: &TimelineStore, client: &str) -> StatusSource {
    store.status(client).unwrap().sources.into_iter().find(|s| s.kind == "media.import").expect("media source")
}

#[test]
fn last_sync_follows_successful_checks_and_last_ingest_follows_new_data() {
    let hub = make_synthetic_hub().unwrap();
    let _c = Cleanup(hub.root.clone());
    let root = temp_root("sync-status");
    let _r = Cleanup(root.clone());
    let now = Arc::new(AtomicI64::new(START));
    let n = now.clone();
    let clock = Clock::with(move || n.load(Ordering::SeqCst), Zone::Named(chrono_tz::Asia::Shanghai));
    let store = TimelineStore::open(&root, "candid/test", true, clock.clone()).unwrap();
    let client = store.register_client("Synthetic AI", "http", &ClientScope::unbounded(), None).unwrap().client_id;

    // New data: both times are the moment of the pass.
    assert_eq!(store.sync_hub_media(&hub.db, &hub.vault, None).unwrap().inserted, 4);
    let first = media_source(&store, &client);
    assert_eq!(first.last_sync, Some(clock.iso(START)));
    assert_eq!(first.last_ingest, Some(clock.iso(START)));
    assert_eq!(first.lag_seconds, Some(0));

    // Nothing new: the check still counts, the data time stays.
    now.store(START + 3_600_000, Ordering::SeqCst);
    assert!(!store.sync_hub_media(&hub.db, &hub.vault, None).unwrap().changed());
    now.store(START + 3_600_000 + 20_000, Ordering::SeqCst);
    let quiet = media_source(&store, &client);
    assert_eq!(quiet.last_sync, Some(clock.iso(START + 3_600_000)));
    assert_eq!(quiet.last_ingest, Some(clock.iso(START)));
    assert_eq!(quiet.lag_seconds, Some(20));
    assert_eq!(quiet.status, "active");

    // A failed pass moves neither time and shows the error; the next good pass clears it.
    now.store(START + 7_200_000, Ordering::SeqCst);
    assert!(store.sync_hub_media(&hub.root.join("missing.sqlite"), &hub.vault, Some(&first.source_id)).is_err());
    let failed = media_source(&store, &client);
    assert_eq!(failed.last_sync, Some(clock.iso(START + 3_600_000)));
    assert_eq!(failed.last_ingest, Some(clock.iso(START)));
    assert_eq!((failed.status.as_str(), failed.last_error.as_deref()), ("error", Some("sync_failed")));
    now.store(START + 7_300_000, Ordering::SeqCst);
    store.sync_hub_media(&hub.db, &hub.vault, None).unwrap();
    let healed = media_source(&store, &client);
    assert_eq!((healed.status.as_str(), healed.last_error), ("active", None));
    assert_eq!(healed.last_sync, Some(clock.iso(START + 7_300_000)));
    assert_eq!(healed.last_ingest, Some(clock.iso(START)));
}
