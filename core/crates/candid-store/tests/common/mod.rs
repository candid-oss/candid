//! Test support mirroring the Swift `TimelineStoreTests/Support.swift`. Synthetic fixtures only.
#![allow(dead_code)]

use candid_store::fixtures::{temp_root, test_clock};
use candid_store::testing::RowExt;
use candid_store::*;
use std::path::PathBuf;

pub struct Fx {
    pub store: TimelineStore,
    pub root: PathBuf,
    pub source: String,
    pub client: String,
}

impl Fx {
    pub fn make(label: &str) -> Fx {
        Fx::with(label, true, ClientScope::unbounded())
    }

    pub fn with(label: &str, media_source: bool, scope: ClientScope) -> Fx {
        let root = temp_root(label);
        let store = open(&root);
        let source = store
            .register_source(
                "mac.desktop",
                "Synthetic Mac",
                &SourcePolicy::new(true, media_source),
            )
            .unwrap();
        let client = store
            .register_client("Synthetic AI", "http", &scope, None)
            .unwrap()
            .client_id;
        Fx {
            store,
            root,
            source,
            client,
        }
    }

    pub fn scalar(&self, sql: &str) -> i64 {
        self.store.raw_db().scalar(sql, &[]).unwrap()
    }

    pub fn texts(&self, sql: &str) -> Vec<String> {
        self.store.raw_db().query(sql, &[], |r| r.text(0)).unwrap()
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn open(root: &std::path::Path) -> TimelineStore {
    TimelineStore::open(root, "candid/test", true, test_clock()).unwrap()
}

pub fn try_open(root: &std::path::Path) -> Result<TimelineStore> {
    TimelineStore::open(root, "candid/test", true, test_clock())
}

pub fn screen_text(
    key: &str,
    ms: i64,
    app: &str,
    title: &str,
    body: &str,
    visible: Option<bool>,
    focus: Option<i64>,
) -> IngestEvent {
    let mut e = IngestEvent::new(key, "screen.text", Some(ms));
    e.app_bundle_id = Some(app.into());
    e.app_name = app.split('.').next_back().map(String::from);
    e.title = Some(title.into());
    e.payload = serde_json::json!({"trigger": "timer"});
    e.text = vec![IngestText {
        field: "body".into(),
        content: body.into(),
        visible,
        focus_distance: focus,
        parts: None,
    }];
    e
}

pub fn text_event(key: &str, ms: i64, body: &str) -> IngestEvent {
    screen_text(
        key,
        ms,
        "com.example.notes",
        "Synthetic doc",
        body,
        None,
        None,
    )
}

pub fn write_staging(store: &TimelineStore, bytes: &[u8], ext: &str) -> (String, String) {
    let path = store
        .staging_directory()
        .join(format!("{}.{ext}", uuid_v4()));
    std::fs::write(&path, bytes).unwrap();
    (path.to_string_lossy().into_owned(), sha256_hex(bytes))
}

pub fn shot(key: &str, ms: i64, path: &str, sha: &str) -> IngestEvent {
    let mut e = IngestEvent::new(key, "screen.shot", Some(ms));
    e.blobs = vec![IngestBlob::new(path, sha)];
    e
}

pub fn note(key: &str, ms: Option<i64>) -> IngestEvent {
    let mut e = IngestEvent::new(key, "note", ms);
    e.payload = serde_json::json!({"format": "text"});
    e
}

pub fn blob_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("blobs/sha256")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
