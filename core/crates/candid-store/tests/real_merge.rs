//! Counts only, on a copy of a real library (never the original): text events and paragraphs of the desktop source
//! when its `screen.text` events are ingested again without and with `merge_text`.
//! `PV_TIMELINE_COPY=<copied timeline root> cargo test -p candid-store --test real_merge -- --ignored --nocapture`
use candid_store::fixtures::temp_root;
use candid_store::testing::{Db, RowExt};
use candid_store::*;
use serde_json::Value;
use std::collections::BTreeMap;

#[test]
#[ignore]
fn merge_counts_on_a_copy() {
    let Ok(copy) = std::env::var("PV_TIMELINE_COPY") else { return };
    let db = Db::open(&std::path::Path::new(&copy).join("timeline.sqlite"), true, false).unwrap();
    let rows = db
        .query(
            "SELECT e.event_id,e.source_key,e.start_ms,e.app_bundle_id,e.app_name,e.title,e.sensitivity,e.payload_json FROM events e JOIN sources s USING(source_id)
             WHERE s.kind='mac.desktop' AND e.kind='screen.text' AND e.start_ms IS NOT NULL ORDER BY e.start_ms, e.source_key",
            &[],
            |r| (r.text(0), r.text(1), r.int(2), r.opt_text(3), r.opt_text(4), r.opt_text(5), r.text(6), r.text(7)),
        )
        .unwrap();
    let mut events = Vec::new();
    for (id, key, start, app, name, title, sens, payload) in rows {
        let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (f, t) in db
            .query("SELECT t.field,c.text FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id WHERE t.event_id=? ORDER BY t.field,t.ord", &[candid_store::testing::rusqlite::types::Value::Text(id)], |r| (r.text(0), r.text(1)))
            .unwrap()
        {
            fields.entry(f).or_default().push(t);
        }
        let mut e = IngestEvent::new(&key, "screen.text", Some(start));
        e.app_bundle_id = app;
        e.app_name = name;
        e.title = title;
        e.sensitivity = sens;
        e.payload = serde_json::from_str::<Value>(&payload).unwrap();
        e.text = fields.into_iter().map(|(f, p)| IngestText::new(&f, &p.join("\n\n"))).collect();
        events.push(e);
    }
    for merge in [false, true] {
        let root = temp_root("real-merge");
        let store = TimelineStore::open(&root, "candid/test", true, Clock::default()).unwrap();
        let sid = store.register_source("mac.desktop", "copy", &SourcePolicy::new(true, false)).unwrap();
        for chunk in events.chunks(200) {
            store.ingest(&IngestBatch { merge_text: Some(merge), ..IngestBatch::events(&sid, chunk.to_vec()) }).unwrap();
        }
        let n = |sql: &str| store.raw_db().scalar(sql, &[]).unwrap();
        println!(
            "merge={merge}: samples {} text events {} paragraph rows {} distinct paragraphs {}",
            events.len(),
            n("SELECT COUNT(*) FROM events WHERE kind='screen.text'"),
            n("SELECT COUNT(*) FROM event_text"),
            n("SELECT COUNT(*) FROM text_chunks")
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
