//! The golden file for the App's capture mapping (`fixtures/desktop-mapping.json`): synthetic ledger records and the
//! timeline events the ledger import makes of them. The Swift capture writes new records through its own mapping and
//! its test checks it against this file, so imported and directly captured records read back the same way.
//! `UPDATE_FIXTURES=1 cargo test -p candid-store --test desktop_mapping` rewrites the file.
use candid_store::fixtures::{ledger_db, make_synthetic_ledger, map_ledger_record, temp_root};
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::{json, Value};
use std::path::PathBuf;

#[test]
fn ledger_mapping_matches_the_golden_file() {
    let ledger = make_synthetic_ledger().unwrap();
    let root = temp_root("mapping");
    let store = TimelineStore::open(&root, "candid/test", true, Clock::default()).unwrap();
    let db = ledger_db(&ledger.root).unwrap();
    let rows = db.query("SELECT id, payload FROM records ORDER BY occurred, id", &[], |r| (r.text(0), r.blob(1))).unwrap();
    let mut cases = Vec::new();
    // Shapes the synthetic ledger lacks: system state on a sampled context, a composing context, gap and system wording.
    let t = 811_692_900.5;
    let extra = [
        json!({"id": "rec-state", "timestamp": t, "kind": "context", "appName": "Notes", "bundleID": "com.example.notes", "windowTitle": "合成",
            "detail": "", "runID": "synthetic-run", "trigger": "content", "activityID": "act-9",
            "context": {"textStatus": "captured", "sessionActive": true, "onBattery": false, "lowPower": true, "thermalState": "nominal",
                "idleSeconds": 2.5, "isComposing": false, "readableText": "合成正文\n\n第二段", "selectedText": "选中"}}),
        json!({"id": "rec-compose", "timestamp": t + 1.0, "kind": "context", "appName": "Notes", "bundleID": "com.example.notes", "windowTitle": "",
            "detail": "", "runID": "synthetic-run", "context": {"textStatus": "composing", "isComposing": true}}),
        json!({"id": "rec-gap2", "timestamp": t + 2.0, "kind": "gap", "appName": "", "bundleID": "", "windowTitle": "", "parentID": "rec-state",
            "detail": "截图未保存：没有屏幕录制权限", "runID": "synthetic-run", "activityID": "act-9"}),
        json!({"id": "rec-sys2", "timestamp": t + 3.0, "kind": "system", "appName": "", "bundleID": "", "windowTitle": "",
            "detail": "系统进入睡眠", "runID": "synthetic-run", "trigger": "system"}),
    ];
    let mut rows = rows;
    for e in extra {
        rows.push((e["id"].as_str().unwrap().to_string(), serde_json::to_vec(&e).unwrap()));
    }
    let ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    for (_, payload) in &rows {
        let record: Value = serde_json::from_slice(payload).unwrap();
        let parent_exists = record["parentID"].as_str().map(|p| ids.iter().any(|i| i == p)).unwrap_or(false);
        let event = map_ledger_record(&store, payload, parent_exists, &ledger.root.join("blobs")).unwrap();
        cases.push(json!({"record": record, "event": serde_json::to_value(&event).unwrap()}));
    }
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/desktop-mapping.json");
    let text = serde_json::to_string_pretty(&json!({"cases": cases})).unwrap() + "\n";
    if std::env::var("UPDATE_FIXTURES").is_ok() {
        std::fs::write(&file, &text).unwrap();
    }
    let golden = std::fs::read_to_string(&file).expect("fixtures/desktop-mapping.json (run with UPDATE_FIXTURES=1)");
    let a: Value = serde_json::from_str(&golden).unwrap();
    let b: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(a, b, "the ledger mapping changed; the Swift capture mapping must change with it");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&ledger.root);
}
