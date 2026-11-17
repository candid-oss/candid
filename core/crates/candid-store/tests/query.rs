//! Port of Swift `QueryTests`, on the shared world in `fixtures/world.json`.
mod common;
use common::*;
use candid_store::fixtures::{at, fixture_path, load_world, temp_root, test_clock, WorldIds, FIXED_NOW_MS, TODAY_START_MS};
use candid_store::testing::RowExt;
use candid_store::*;
use serde_json::json;
use std::collections::BTreeSet;

struct World {
    store: TimelineStore,
    root: std::path::PathBuf,
    ids: WorldIds,
}

impl World {
    fn new() -> World {
        let root = temp_root("query");
        let store = open(&root);
        let ids = load_world(&store, &fixture_path("world.json")).unwrap();
        World { store, root, ids }
    }
    fn c(&self, alias: &str) -> &str {
        &self.ids.clients[alias]
    }
    fn full(&self) -> &str {
        self.c("full")
    }
    fn id(&self, key: &str) -> String {
        self.ids.events[key].clone()
    }
    fn source(&self, alias: &str) -> String {
        self.ids.sources[alias].clone()
    }
    fn first_text(&self, sql: &str) -> Option<String> {
        self.store.raw_db().first(sql, &[], |r| r.text(0)).unwrap()
    }
    fn scalar(&self, sql: &str) -> i64 {
        self.store.raw_db().scalar(sql, &[]).unwrap()
    }
    fn exec(&self, sql: &str, params: &[testing::rusqlite::types::Value]) {
        self.store.raw_db().execute(sql, params).unwrap();
    }
    fn client(&self, name: &str, scope: ClientScope) -> String {
        self.store.register_client(name, "cli", &scope, None).unwrap().client_id
    }
    fn search(&self, req: SearchRequest, client: &str) -> SearchResponse {
        self.store.search(&req, client).unwrap()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn q(text: &str) -> SearchRequest {
    SearchRequest { q: Some(text.into()), ..Default::default() }
}
fn q_all(text: &str) -> SearchRequest {
    SearchRequest { q: Some(text.into()), attention: Some("all".into()), ..Default::default() }
}
fn s(v: &str) -> Option<String> {
    Some(v.into())
}
fn v(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|s| s.to_string()).collect())
}
fn unbounded() -> ClientScope {
    ClientScope::unbounded()
}
fn denial_codes(w: &World) -> Vec<String> {
    w.store.raw_db().query("SELECT COALESCE(denied,'') FROM access_log ORDER BY at_ms, rowid", &[], |r| r.text(0)).unwrap()
}
fn sub(text: &str, h: &[i64]) -> String {
    text.chars().skip(h[0] as usize).take((h[1] - h[0]) as usize).collect()
}

// MARK: search

#[test]
fn two_character_chinese_query_finds_text_titles_and_transcripts() {
    let w = World::new();
    let r = w.search(q_all("预算"), w.full());
    let kinds: BTreeSet<&str> = r.hits.iter().map(|h| h.kind.as_str()).collect();
    for k in ["screen.text", "calendar.event", "ai.turn", "speech.segment", "screen.shot"] {
        assert!(kinds.contains(k), "{k} in {kinds:?}");
    }
    let hit = r.hits.iter().find(|h| h.kind == "screen.text").unwrap();
    let snippet = hit.snippet.as_ref().unwrap();
    assert!(snippet.text.contains("预算") && !snippet.highlights.is_empty());
    for h in &snippet.highlights {
        assert_eq!(sub(&snippet.text, h), "预算");
    }
    let r_ = hit.r#ref.as_ref().unwrap();
    assert!(r_.field.as_deref() == Some("body") && r_.chunk_ord.is_some());
    assert_eq!(r.hits.iter().find(|h| h.kind == "calendar.event").unwrap().source_label, "calendar");
    let seg = r.hits.iter().find(|h| h.kind == "speech.segment").unwrap();
    assert!(seg.source_label == "audio·asr" && seg.layer == "perception");
    // the Keychain event matches the text but is denied by the client scope: never returned, not even by title
    assert!(!r.hits.iter().any(|h| h.app.as_ref().and_then(|a| a.bundle_id.as_deref()) == Some("com.apple.keychainaccess")));
    assert!(!r.scan_limited);
}

fn bundles(r: &SearchResponse) -> BTreeSet<String> {
    r.hits.iter().filter_map(|h| h.app.as_ref().and_then(|a| a.bundle_id.clone())).collect()
}

#[test]
fn english_and_mixed_multi_term_queries() {
    let w = World::new();
    let a = w.search(q_all("BUDGET"), w.full()); // trigram is case-insensitive
    assert_eq!(bundles(&a), ["com.example.notes".to_string(), "com.example.terminal".to_string()].into());
    let b = w.search(q_all("budget forecast"), w.full());
    assert!(b.hits.len() == 1 && b.hits[0].app.as_ref().unwrap().bundle_id.as_deref() == Some("com.example.notes"));
    let mixed = w.search(q_all("三段 forecast"), w.full()); // short + long term together
    assert_eq!(mixed.hits.len(), 1);
    assert_eq!(mixed.hits[0].snippet.as_ref().unwrap().highlights.len(), 2);
    let meta_only = w.search(q("standup"), w.full()); // title index
    assert!(meta_only.hits.iter().any(|h| h.title.as_deref() == Some("standup.m4a")));
}

#[test]
fn attention_focus_keeps_only_new_visible_near_focus_paragraphs() {
    let w = World::new();
    let focus = w.search(SearchRequest { kinds: v(&["screen.text"]), ..q("季度预算") }, w.full());
    assert_eq!(focus.attention, "focus");
    assert_eq!(focus.hits.len(), 1); // second occurrence is not new → background only
    let all = w.search(SearchRequest { kinds: v(&["screen.text"]), ..q_all("季度预算") }, w.full());
    assert_eq!(all.hits.len(), 2);
    assert!(w.search(q("running budget"), w.full()).hits.is_empty()); // visible=false, focus_distance=7
    assert_eq!(w.search(q_all("running budget"), w.full()).hits.len(), 1);
    assert!(w.store.search(&SearchRequest { attention: s("nope"), ..q("x") }, w.full()).is_err());
    // a visible/new/near paragraph outranks an equally relevant background one
    assert!(all.hits[0].attention.as_ref().unwrap().is_new);
}

#[test]
fn filters_are_applied_before_ranking_and_snippets() {
    let w = World::new();
    let by_app = w.search(SearchRequest { apps: v(&["com.example.terminal"]), ..q_all("budget") }, w.full());
    assert!(by_app.hits.len() == 1 && by_app.hits[0].app.as_ref().unwrap().bundle_id.as_deref() == Some("com.example.terminal"));
    let by_kind = w.search(SearchRequest { sources: v(&["ai.codex"]), ..q("预算") }, w.full());
    assert!(!by_kind.hits.is_empty() && by_kind.hits.iter().all(|h| h.kind == "ai.turn"));
    let by_time = w.search(SearchRequest { start: s("2027-01-15T00:00:00+08:00"), end: s("2027-01-15T10:00:00+08:00"), ..q_all("预算") }, w.full());
    assert!(!by_time.hits.is_empty());
    assert!(by_time
        .hits
        .iter()
        .all(|h| h.app.as_ref().and_then(|a| a.bundle_id.as_deref()) == Some("com.example.notes") || h.kind == "screen.shot"));
    assert_eq!(by_time.window.start.as_deref(), Some("2027-01-15T00:00:00.000+08:00"));
    let tid = w.first_text("SELECT thread_id FROM threads WHERE kind='recording'").unwrap();
    let thread = w.search(SearchRequest { thread_id: Some(tid), ..q_all("预算") }, w.full());
    assert!(thread.hits.len() == 1 && thread.hits[0].kind == "speech.segment");
    let fields = w.search(SearchRequest { fields: v(&["transcript"]), ..q_all("预算") }, w.full());
    assert!(fields.hits.iter().any(|h| h.kind == "speech.segment"));
    assert!(!fields.hits.iter().any(|h| h.kind == "screen.text" && h.r#ref.as_ref().and_then(|r| r.field.as_deref()) == Some("body")));
}

#[test]
fn metadata_only_scope_never_returns_text_but_still_finds_titles() {
    let w = World::new();
    assert!(w.search(q("预算"), w.c("narrow")).hits.is_empty()); // titles of ai.turn events don't contain it; message text is withheld
    let session = w.search(SearchRequest { sources: v(&["ai.codex"]), ..q("session") }, w.c("narrow"));
    assert!(session.hits.is_empty()); // thread titles are not event titles
    // text=false client on the full source set: title hit is metadata, paragraphs stay hidden
    let meta = w.store.register_client("Meta", "http", &ClientScope { text: false, ..unbounded() }, None).unwrap().client_id;
    let m = w.search(q("预算"), &meta);
    assert!(!m.hits.is_empty());
    assert!(m.hits.iter().all(|h| h.attention.is_none() && h.r#ref.as_ref().and_then(|r| r.field.as_deref()) != Some("body")));
    assert!(m.hits.iter().all(|h| h.snippet.is_none() || h.snippet.as_ref().unwrap().text.contains("预算") || h.title.as_deref().unwrap_or("").contains("预算")));
    assert!(w.search(q("forecast"), &meta).hits.is_empty());
}

#[test]
fn scope_time_window_and_max_days_back_limit_everything() {
    let w = World::new();
    let recent = w.client("Recent", ClientScope { time: ScopeTime { from: Some(at(11.0)), to: Some(at(13.0)), max_days_back: None }, ..Default::default() });
    let r = w.search(q_all("预算"), &recent);
    let kinds: BTreeSet<String> = r.hits.iter().map(|h| h.kind.clone()).collect();
    assert_eq!(kinds, ["ai.turn".to_string(), "calendar.event".to_string()].into());
    // fixed clock is 16:00 local: 1 day back reaches the start of "yesterday 16:00"
    let one_day = w.client("Day", ClientScope { time: ScopeTime { max_days_back: Some(1), ..Default::default() }, ..Default::default() });
    assert!(!w.search(q_all("预算"), &one_day).hits.is_empty());
    let none = w.client("Zero", ClientScope { time: ScopeTime { from: Some(at(23.0)), ..Default::default() }, ..Default::default() });
    assert!(w.search(q_all("预算"), &none).hits.is_empty());
}

#[test]
fn empty_results_explain_coverage_instead_of_silence() {
    let w = World::new();
    let r = w.search(q("zzzzqqqq不存在"), w.full());
    assert!(r.hits.is_empty() && r.next_cursor.is_none());
    let nothing = w.search(SearchRequest { start: s("2026-01-01"), end: s("2026-01-02"), ..q("预算") }, w.full());
    assert!(nothing.hits.is_empty() && !nothing.coverage.has_events);
    assert!(nothing.coverage.notes.iter().any(|n| n.contains("没有事件")));
    // gaps inside the window are reported with a human sentence
    let gap = w.search(SearchRequest { start: s("2027-01-15T12:00:00+08:00"), end: s("2027-01-15T13:30:00+08:00"), ..q("预算") }, w.full());
    assert_eq!(gap.coverage.gaps.len(), 1);
    assert_eq!(gap.coverage.gaps[0].note, "12:30–13:00 已暂停记录");
}

#[test]
fn paging_is_pinned_to_a_snapshot_and_rejects_mismatched_cursors() {
    let w = World::new();
    let desktop = w.source("desktop");
    let batch = (0..25)
        .map(|i| screen_text(&format!("p{i}"), at(15.0) + i * 1000, "com.example.pager", &format!("Pager {i}"), &format!("分页测试 marker {i}"), Some(true), Some(0)))
        .collect();
    w.store.ingest(&IngestBatch::events(&desktop, batch)).unwrap();
    let mut seen = Vec::new();
    let mut req = SearchRequest { apps: v(&["com.example.pager"]), limit: Some(10), ..q("marker") };
    let mut pages = 0;
    loop {
        let r = w.search(req.clone(), w.full());
        seen.extend(r.hits.iter().map(|h| h.event_id.clone()));
        pages += 1;
        if pages == 1 {
            // new data arriving mid-pagination must not appear in later pages, but is flagged
            w.store
                .ingest(&IngestBatch::events(&desktop, vec![screen_text("late", at(15.5), "com.example.pager", "Late", "late marker", Some(true), Some(0))]))
                .unwrap();
        } else {
            assert!(r.has_new_records);
        }
        match r.next_cursor {
            Some(n) => req.cursor = Some(n),
            None => break,
        }
    }
    assert!(pages == 3 && seen.len() == 25 && seen.iter().collect::<BTreeSet<_>>().len() == 25);
    let late = w.first_text("SELECT event_id FROM events WHERE source_key='late'").unwrap();
    assert!(!seen.contains(&late));
    // a cursor from one query cannot be replayed on another
    let first = w.search(SearchRequest { apps: v(&["com.example.pager"]), limit: Some(5), ..q("marker") }, w.full());
    let replay = SearchRequest { apps: v(&["com.example.pager"]), limit: Some(5), cursor: first.next_cursor, ..q("other") };
    assert_eq!(w.store.search(&replay, w.full()).err(), Some(TimelineError::InvalidCursor));
    assert_eq!(w.store.search(&SearchRequest { cursor: s("not-a-cursor"), ..q("marker") }, w.full()).err(), Some(TimelineError::InvalidCursor));
    // limit above 50 is clamped
    assert_eq!(w.search(SearchRequest { apps: v(&["com.example.pager"]), limit: Some(500), ..q("marker") }, w.full()).hits.len(), 26);
}

#[test]
fn oversize_responses_are_trimmed_with_a_resumable_cursor() {
    let w = World::new();
    let body = |i: i64| format!("{}needle {i}", "long synthetic oversize sentence. ".repeat(40));
    let batch = (0..12)
        .map(|i| screen_text(&format!("big{i}"), at(16.0) + i * 1000, "com.example.big", &format!("Big document {i}"), &body(i), Some(true), Some(0)))
        .collect();
    w.store.ingest(&IngestBatch::events(&w.source("desktop"), batch)).unwrap();
    let mut req = SearchRequest { apps: v(&["com.example.big"]), limit: Some(12), max_chars: Some(600), ..q("needle") };
    let mut seen = BTreeSet::new();
    let mut trimmed_pages = 0;
    loop {
        let r = w.search(req.clone(), w.c("tiny"));
        assert!(json::canonical_of(&r).len() <= 1400);
        if r.truncated_by_size {
            trimmed_pages += 1;
            assert!(r.next_cursor.is_some());
        }
        for h in &r.hits {
            assert!(seen.insert(h.event_id.clone()));
        }
        match r.next_cursor {
            Some(n) => req.cursor = Some(n),
            None => break,
        }
    }
    assert!(seen.len() == 12 && trimmed_pages >= 1);
    // snippets obey max_chars and report truncation
    let s_ = w.search(SearchRequest { apps: v(&["com.example.big"]), limit: Some(1), max_chars: Some(80), ..q("needle") }, w.full());
    let snippet = s_.hits[0].snippet.as_ref().unwrap();
    assert!(snippet.truncated && snippet.text.chars().count() <= 82 && snippet.text.contains("needle"));
    // a budget that cannot even hold the envelope is a denial, not a silent empty answer
    let micro = w.client("Micro", ClientScope { max_result_bytes: 20, ..unbounded() });
    assert_eq!(w.store.search(&q("needle"), &micro).err(), Some(TimelineError::Denied("max_result_bytes_too_small".into())));
}

#[test]
fn short_query_scan_is_bounded_and_reports_the_limit() {
    let w = World::new();
    let batch = (0..60)
        .map(|i| {
            let paras = (0..99).map(|j| format!("段落 {j} of event {}", j + 1000 * j)).collect::<Vec<_>>().join("\n\n");
            screen_text(&format!("scan{i}"), at(17.0) + i, "com.example.scan", "Scan", &format!("扫描 filler {i}\n\n{paras}"), None, None)
        })
        .collect();
    w.store.ingest(&IngestBatch::events(&w.source("desktop"), batch)).unwrap();
    assert!(w.search(q_all("扫描"), w.full()).scan_limited); // > 5000 paragraphs match the structural filter
    let narrow = w.search(
        SearchRequest { start: s("2027-01-15T00:00:00+08:00"), apps: v(&["com.example.scan"]), fields: v(&["body"]), ..q_all("扫描") },
        w.full(),
    );
    assert!(narrow.scan_limited);
}

// MARK: other tools

fn day() -> OverviewRequest {
    OverviewRequest { start: s("2027-01-15"), end: s("2027-01-15"), group_by: None }
}

#[test]
fn overview_computes_focus_time_by_app_and_lists_agenda() {
    let w = World::new();
    let r = w.store.overview(&day(), w.full()).unwrap();
    assert_eq!(r.window.start.as_deref(), Some("2027-01-15T00:00:00.000+08:00"));
    assert_eq!(r.window.end.as_deref(), Some("2027-01-16T00:00:00.000+08:00"));
    let notes = r.groups.iter().find(|g| g.key == "com.example.notes").unwrap();
    assert_eq!(notes.seconds, Some(840.0)); // 09:00→ last observation 09:12 + 1 min tail; then 10:30 with nothing after it → 1 min
    assert_eq!(r.groups.iter().find(|g| g.key == "com.example.terminal").unwrap().seconds, Some(780.0));
    // Visits, not observations: Notes came to the front twice (09:00, 10:30), Terminal once.
    assert_eq!((notes.switches, r.groups.iter().find(|g| g.key == "com.example.terminal").unwrap().switches), (Some(2), Some(1)));
    assert!(notes.events > notes.switches.unwrap());
    assert_eq!(r.groups[0].key, "com.example.notes");
    assert_eq!(r.calendar.iter().map(|c| c.title.clone().unwrap()).collect::<Vec<_>>(), vec!["预算评审会"]);
    assert!(r.recordings.len() == 1 && r.gaps.len() == 1);
    assert_eq!(r.episodes.iter().map(|e| e.title.clone().unwrap()).collect::<Vec<_>>(), vec!["写预算草稿"]);
    assert!(!r.groups.iter().any(|g| g.key == "com.apple.keychainaccess"));
    let hours = w.store.overview(&OverviewRequest { group_by: s("hour"), ..day() }, w.full()).unwrap();
    assert_eq!(hours.groups.iter().find(|g| g.key == "09:00").unwrap().seconds, Some(780.0));
    let by_source = w.store.overview(&OverviewRequest { group_by: s("source"), ..day() }, w.full()).unwrap();
    assert_eq!(by_source.groups.len(), 3);
    let by_episode = w.store.overview(&OverviewRequest { group_by: s("episode"), ..day() }, w.full()).unwrap();
    assert_eq!(by_episode.groups.iter().map(|g| g.label.clone()).collect::<Vec<_>>(), vec!["写预算草稿"]);
    assert_eq!(by_episode.groups[0].events, 2); // t4 (Keychain) is out of scope
    assert!(w.store.overview(&day(), w.c("noDerived")).unwrap().episodes.is_empty());
    let empty = w.store.overview(&OverviewRequest { start: s("2020-01-01"), end: s("2020-01-01"), group_by: None }, w.full()).unwrap();
    assert!(empty.total_events == 0 && empty.groups.is_empty() && empty.coverage.notes.iter().any(|n| n.contains("没有事件")));
    assert!(w.store.overview(&OverviewRequest { group_by: s("weekday"), ..Default::default() }, w.full()).is_err());
    // default window is "today" up to now
    assert!(w.store.overview(&OverviewRequest::default(), w.full()).unwrap().total_events > 0);
}

#[test]
fn timeline_pages_events_and_episodes_in_order() {
    let w = World::new();
    let mut req = TimelineRequest { start: s("2027-01-15"), end: s("2027-01-15"), kinds: v(&["screen.text", "app.focus", "ai.turn"]), limit: Some(4), ..Default::default() };
    let mut keys = Vec::new();
    let mut pages = 0;
    loop {
        let r = w.store.timeline(&req, w.full()).unwrap();
        keys.extend(r.entries.iter().filter_map(|e| e.event.as_ref().map(|ev| ev.event_id.clone())));
        pages += 1;
        let starts: Vec<String> = r.entries.iter().filter_map(|e| e.event.as_ref().and_then(|ev| ev.start.clone())).collect();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(starts, sorted);
        match r.next_cursor {
            Some(n) => req.cursor = Some(n),
            None => break,
        }
    }
    assert!(keys.len() == 3 + 3 + 2 && keys.iter().collect::<BTreeSet<_>>().len() == keys.len() && pages == 2); // keychain excluded by scope
    let with_preview = w
        .store
        .timeline(&TimelineRequest { start: s("2027-01-15"), end: s("2027-01-15"), kinds: v(&["screen.text"]), limit: Some(50), ..Default::default() }, w.full())
        .unwrap();
    assert!(with_preview.entries.iter().all(|e| e.preview.is_some()));
    let episodes = w
        .store
        .timeline(&TimelineRequest { start: s("2027-01-15"), end: s("2027-01-15"), level: s("episode"), ..Default::default() }, w.full())
        .unwrap();
    assert_eq!(episodes.entries.iter().map(|e| e.episode.as_ref().unwrap().title.clone().unwrap()).collect::<Vec<_>>(), vec!["写预算草稿"]);
    assert_eq!(episodes.entries[0].episode.as_ref().unwrap().evidence_count, 2);
    assert_eq!(
        w.store.timeline(&TimelineRequest { level: s("episode"), ..Default::default() }, w.c("noDerived")).err(),
        Some(TimelineError::Denied("derived_not_permitted".into()))
    );
    let empty = w.store.timeline(&TimelineRequest { start: s("2020-01-01"), end: s("2020-01-02"), ..Default::default() }, w.full()).unwrap();
    assert!(empty.entries.is_empty() && empty.next_cursor.is_none());
}

#[test]
fn context_shows_what_was_happening_around_a_moment() {
    let w = World::new();
    let r = w
        .store
        .context(&ContextRequest { at: "2027-01-15T11:00:00+08:00".into(), before: Some(120), after: Some(5), include: v(&["text", "media_refs"]) }, w.full())
        .unwrap();
    assert_eq!(r.calendar.iter().map(|c| c.title.clone().unwrap()).collect::<Vec<_>>(), vec!["预算评审会"]);
    assert!(r.events.iter().any(|e| e.kind == "screen.text"));
    assert!(!r.events.iter().any(|e| e.app.as_ref().and_then(|a| a.bundle_id.as_deref()) == Some("com.apple.keychainaccess")));
    assert!(r.focus_paragraphs.iter().all(|p| p.attention.is_new));
    assert!(r.media_refs.is_empty()); // `full` has no media scope
    let with_media = w
        .store
        .context(&ContextRequest { at: "2027-01-15T09:10:00+08:00".into(), before: Some(5), after: Some(5), include: v(&["media_refs"]) }, w.c("media"))
        .unwrap();
    assert!(with_media.media_refs.len() == 1 && with_media.media_refs[0].mime_type == "image/jpeg");
    let rec = w.store.context(&ContextRequest { at: "2027-01-15T14:10:00+08:00".into(), ..Default::default() }, w.full()).unwrap();
    assert_eq!(rec.recordings.len(), 1);
    let quiet = w.store.context(&ContextRequest { at: "2020-01-01".into(), ..Default::default() }, w.full()).unwrap();
    assert!(quiet.events.is_empty() && quiet.coverage.notes.iter().any(|n| n.contains("没有事件")));
    assert!(w.store.context(&ContextRequest { at: "sometime".into(), ..Default::default() }, w.full()).is_err());
}

#[test]
fn get_event_returns_text_by_attention_neighbors_thread_and_derived() {
    let w = World::new();
    let r = w.store.get_event(&GetEventRequest { event_id: w.id("t2"), include: v(&["text", "neighbors", "derived"]) }, w.full()).unwrap();
    assert!(r.paragraphs[0].attention.is_new && r.paragraphs[0].text.contains("新增")); // new paragraph before repeated one
    assert!(r.paragraphs.len() == 2 && r.episodes.iter().map(|e| e.episode_id.as_str()).collect::<Vec<_>>() == vec!["ep1"]);
    assert!(r.neighbors.len() >= 2 && !r.neighbors.iter().any(|n| n.event_id == w.id("t2")));
    let seg = w.store.get_event(&GetEventRequest { event_id: w.id("seg1"), include: None }, w.full()).unwrap();
    assert!(seg.thread.unwrap().kind == "recording" && seg.event.layer == "perception");
    let no_text = w.store.get_event(&GetEventRequest { event_id: w.id("u1"), include: None }, w.c("narrow")).unwrap();
    assert!(no_text.paragraphs.is_empty() && no_text.event.title.is_none());
    // exists but hidden by scope ⇒ same answer as a missing id, audited as out_of_scope
    assert_eq!(w.store.get_event(&GetEventRequest { event_id: w.id("t4"), include: None }, w.full()).err(), Some(TimelineError::NotFound(w.id("t4"))));
    assert_eq!(w.store.get_event(&GetEventRequest { event_id: "nope".into(), include: None }, w.full()).err(), Some(TimelineError::NotFound("nope".into())));
    assert!(denial_codes(&w).contains(&"out_of_scope".to_string()));
    assert!(w.store.get_event(&GetEventRequest { event_id: w.id("t1"), include: None }, w.c("narrow")).is_err());
}

#[test]
fn episodes_and_episode_respect_scope_and_revisions() {
    let w = World::new();
    let list = w.store.episodes(&EpisodesRequest { start: s("2027-01-15"), end: s("2027-01-15"), ..Default::default() }, w.full()).unwrap();
    assert_eq!(list.episodes.iter().map(|e| e.episode_id.as_str()).collect::<Vec<_>>(), vec!["ep1"]); // rejected hidden by default
    let rejected = w.store.episodes(&EpisodesRequest { status: v(&["rejected"]), ..Default::default() }, w.full()).unwrap();
    assert_eq!(rejected.episodes.iter().map(|e| e.episode_id.as_str()).collect::<Vec<_>>(), vec!["ep2"]);
    use testing::rusqlite::types::Value::Integer;
    w.exec(
        "INSERT INTO episodes(episode_id,rev,start_ms,end_ms,title,kind,status,producer,created_ms) VALUES('ep1',2,?,?,'修订后的标题','activity','edited','rules/1',2)",
        &[Integer(at(9.0)), Integer(at(10.0))],
    );
    assert_eq!(w.store.episodes(&EpisodesRequest::default(), w.full()).unwrap().episodes[0].title.as_deref(), Some("修订后的标题")); // latest revision wins
    let detail = w.store.episode(&EpisodeRequest { episode_id: "ep1".into(), rev: Some(1) }, w.full()).unwrap();
    assert!(detail.episode.rev == 1 && detail.evidence.len() == 2 && detail.hidden_evidence == 1); // keychain evidence hidden
    let primary = detail.evidence.iter().find(|e| e.role == "primary").unwrap();
    assert_eq!(primary.selector.as_ref().unwrap()["type"], json!("text"));
    assert_eq!(w.store.episode(&EpisodeRequest { episode_id: "ep1".into(), rev: None }, w.full()).unwrap().episode.rev, 2);
    let denied = Some(TimelineError::Denied("derived_not_permitted".into()));
    assert_eq!(w.store.episodes(&EpisodesRequest::default(), w.c("noDerived")).err(), denied);
    assert_eq!(w.store.episode(&EpisodeRequest { episode_id: "ep1".into(), rev: None }, w.c("noDerived")).err(), denied);
    assert_eq!(
        w.store.episode(&EpisodeRequest { episode_id: "missing".into(), rev: None }, w.full()).err(),
        Some(TimelineError::NotFound("missing".into()))
    );
    // paging over many episodes
    for i in 0..7 {
        w.exec(
            "INSERT INTO episodes(episode_id,rev,start_ms,end_ms,title,kind,status,producer,created_ms) VALUES(?,1,?,?,?, 'focus','draft','rules/1',1)",
            &[
                testing::rusqlite::types::Value::Text(format!("pg{i}")),
                Integer(at(18.0) + i),
                Integer(at(19.0)),
                testing::rusqlite::types::Value::Text(format!("P{i}")),
            ],
        );
    }
    let mut req = EpisodesRequest { start: s("2027-01-15T18:00:00+08:00"), limit: Some(3), ..Default::default() };
    let mut got = Vec::new();
    loop {
        let r = w.store.episodes(&req, w.full()).unwrap();
        got.extend(r.episodes.iter().map(|e| e.episode_id.clone()));
        match r.next_cursor {
            Some(n) => req.cursor = Some(n),
            None => break,
        }
    }
    assert_eq!(got, (0..7).map(|i| format!("pg{i}")).collect::<Vec<_>>());
}

#[test]
fn thread_returns_ordered_messages_with_paging() {
    let w = World::new();
    let tid = w.first_text("SELECT thread_id FROM threads WHERE kind='ai_session'").unwrap();
    let r = w.store.thread(&ThreadRequest { thread_id: tid.clone(), ..Default::default() }, w.full()).unwrap();
    assert_eq!(r.messages.iter().map(|m| m.role.clone().unwrap()).collect::<Vec<_>>(), vec!["user", "assistant"]);
    assert!(r.messages[1].text.as_deref().unwrap().contains("synthetic result"));
    assert_eq!(r.messages[0].source_label, "ai·codex");
    let p1 = w.store.thread(&ThreadRequest { thread_id: tid.clone(), cursor: None, limit: Some(1) }, w.full()).unwrap();
    let p2 = w.store.thread(&ThreadRequest { thread_id: tid.clone(), cursor: p1.next_cursor.clone(), limit: Some(1) }, w.full()).unwrap();
    assert!(p1.messages.len() == 1 && p2.messages.len() == 1 && p2.next_cursor.is_none() && p1.messages[0].event_id != p2.messages[0].event_id);
    let hidden = w.store.thread(&ThreadRequest { thread_id: tid, ..Default::default() }, w.c("narrow")).unwrap(); // text=false ⇒ structure only
    assert!(hidden.messages.len() == 2 && hidden.messages.iter().all(|m| m.text.is_none()));
    let recording = w.first_text("SELECT thread_id FROM threads WHERE kind='recording'").unwrap();
    assert_eq!(
        w.store.thread(&ThreadRequest { thread_id: recording.clone(), ..Default::default() }, w.c("narrow")).err(),
        Some(TimelineError::NotFound(recording))
    );
    assert_eq!(
        w.store.thread(&ThreadRequest { thread_id: "nope".into(), ..Default::default() }, w.full()).err(),
        Some(TimelineError::NotFound("nope".into()))
    );
}

#[test]
fn media_requires_scope_and_serves_verified_bytes() {
    let w = World::new();
    let shot = w.id("shot1");
    assert_eq!(
        w.store.media(&MediaRequest { event_id: shot.clone(), ..Default::default() }, w.full()).err(),
        Some(TimelineError::Denied("media_not_permitted".into()))
    );
    let m = w.store.media(&MediaRequest { event_id: shot.clone(), variant: s("preview"), range: None }, w.c("media")).unwrap();
    assert!(m.variant_served == "original" && !m.too_large); // no preview stored: original is served
    use base64::Engine;
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(m.base64.unwrap()).unwrap(), b"synthetic screenshot");
    let small = w.client("SmallMedia", ClientScope { media: true, max_result_bytes: 16, ..unbounded() });
    let big = w.store.media(&MediaRequest { event_id: shot.clone(), variant: s("original"), range: None }, &small).unwrap();
    assert!(big.too_large && big.base64.is_none() && big.byte_count == 20);
    assert_eq!(
        w.store.media(&MediaRequest { event_id: w.id("t1"), ..Default::default() }, w.c("media")).err(),
        Some(TimelineError::NotFound("该事件没有媒体".into()))
    );
    assert_eq!(
        w.store.media(&MediaRequest { event_id: w.id("t4"), ..Default::default() }, w.c("media")).err(),
        Some(TimelineError::NotFound(w.id("t4")))
    );
    // tampered file is refused rather than served
    let rel = w.first_text("SELECT rel_path FROM blobs").unwrap();
    std::fs::write(w.root.join(rel), b"changed").unwrap();
    assert!(w.store.media(&MediaRequest { event_id: shot, ..Default::default() }, w.c("media")).is_err());
}

#[test]
fn usage_separates_actual_from_estimated_cost_and_external_tokens() {
    let w = World::new();
    let by_feature = w.store.usage(&UsageRequest { group_by: s("feature"), ..Default::default() }, w.full()).unwrap();
    let episode = by_feature.rows.iter().find(|r| r.key == "episode").unwrap();
    assert!(episode.calls == 2 && episode.input_tokens == 1500 && episode.cost_actual == 0.5 && episode.cost_estimated == 0.25);
    assert_eq!(episode.currency.as_deref(), Some("USD"));
    let review = by_feature.rows.iter().find(|r| r.key == "daily_review").unwrap();
    assert!(review.cost_unknown_calls == 1 && review.cost_actual == 0.0);
    assert!(by_feature.totals.calls == 3 && by_feature.rows.iter().all(|r| r.origin == "app"));
    let keys = |g: &str| w.store.usage(&UsageRequest { group_by: s(g), ..Default::default() }, w.full()).unwrap().rows.into_iter().map(|r| r.key).collect::<Vec<_>>();
    assert_eq!(keys("model"), vec!["prov/m1", "prov/m2"]);
    assert_eq!(keys("provider"), vec!["prov"]);
    assert_eq!(keys("day"), vec!["2027-01-14", "2027-01-15"]);
    let windowed = w.store.usage(&UsageRequest { start: s("2027-01-15"), end: s("2027-01-15"), ..Default::default() }, w.full()).unwrap();
    assert_eq!(windowed.totals.calls, 2);
    let with_external = w.store.usage(&UsageRequest { group_by: s("model"), include_external: Some(true), ..Default::default() }, w.full()).unwrap();
    let ext = with_external.rows.iter().find(|r| r.origin == "external").unwrap();
    assert!(ext.key == "synthetic-model" && ext.input_tokens == 100 && ext.output_tokens == 40 && ext.cache_read_tokens == 10 && ext.cost_unknown_calls == 1);
    // external usage honours the client's scope (Codex source is outside this client)
    let screen_only = w.client("ScreenOnly", ClientScope { sources: vec![w.source("desktop")], ..unbounded() });
    assert!(w.store.usage(&UsageRequest { include_external: Some(true), ..Default::default() }, &screen_only).unwrap().rows.iter().all(|r| r.origin == "app"));
    assert!(w.store.usage(&UsageRequest { group_by: s("colour"), ..Default::default() }, w.full()).is_err());
    assert!(w.store.usage(&UsageRequest { start: s("2030-01-01"), end: s("2030-01-02"), ..Default::default() }, w.full()).unwrap().rows.is_empty());
}

#[test]
fn propose_stores_pending_proposals_only_for_permitted_clients_and_visible_evidence() {
    let w = World::new();
    let c1 = w.id("c1");
    let ok = w
        .store
        .propose(&ProposeRequest { kind: "create_reminder".into(), payload: json!({"title": "synthetic reminder"}), evidence: Some(vec![c1.clone(), c1.clone()]) }, w.full())
        .unwrap();
    assert_eq!(ok.status, "pending");
    let row = w
        .store
        .raw_db()
        .first("SELECT status, evidence_json, client_id FROM proposals WHERE proposal_id=?", &[testing::rusqlite::types::Value::Text(ok.proposal_id)], |r| {
            (r.text(0), r.text(1), r.text(2))
        })
        .unwrap()
        .unwrap();
    assert!(row.0 == "pending" && row.1 == format!("[\"{c1}\"]") && row.2 == w.full());
    let p = |kind: &str, payload: serde_json::Value, evidence: Option<Vec<String>>| ProposeRequest { kind: kind.into(), payload, evidence };
    assert_eq!(w.store.propose(&p("x", json!({}), None), w.c("narrow")).err(), Some(TimelineError::Denied("propose_not_permitted".into())));
    assert_eq!(w.store.propose(&p("x", json!({}), Some(vec![w.id("t4")])), w.full()).err(), Some(TimelineError::Denied("evidence_out_of_scope".into())));
    assert!(w.store.propose(&p("", json!({}), None), w.full()).is_err());
    assert!(w.store.propose(&p("x", json!("not an object"), None), w.full()).is_err());
    assert_eq!(w.scalar("SELECT COUNT(*) FROM proposals"), 1);
}

#[test]
fn status_reports_store_sources_capture_and_agent() {
    let mut w = World::new();
    let st = w.store.status(w.full()).unwrap();
    assert!(st.store.schema_version == SCHEMA_VERSION && st.store.events >= 15 && st.store.db_bytes > 0 && st.store.blob_bytes == 20);
    assert!(st.sources.len() == 3 && st.sources.iter().all(|s| s.last_sync.is_some()));
    assert!(st.capture.recording); // the desktop source synced at the fixed "now"
    assert!(!st.agent.available && st.agent.detail.as_deref() == Some("not_configured"));
    w.store.set_agent_probe(|| (true, Some("pi 0.0.0-synthetic".into())));
    assert_eq!(w.store.status(w.full()).unwrap().agent, StatusAgent { available: true, detail: Some("pi 0.0.0-synthetic".into()) });
    let desktop = w.source("desktop");
    w.store.set_source_status(&desktop, "paused", None).unwrap();
    let paused = w.store.status(w.full()).unwrap();
    assert!(!paused.capture.recording);
    assert_eq!(paused.sources.iter().find(|s| s.source_id == desktop).unwrap().status, "paused");
    let narrow = w.store.status(w.c("narrow")).unwrap();
    assert_eq!(narrow.sources.iter().map(|s| s.source_id.clone()).collect::<Vec<_>>(), vec![w.source("codex")]); // other sources are not even listed
}

// MARK: audit, clients, time

#[test]
fn every_call_is_audited_including_denials() {
    let w = World::new();
    w.search(q("预算"), w.full());
    let _ = w.store.media(&MediaRequest { event_id: w.id("shot1"), ..Default::default() }, w.full());
    let _ = w.store.get_event(&GetEventRequest { event_id: w.id("t4"), include: None }, w.full());
    let _ = w.store.search(&SearchRequest { cursor: s("bad"), ..q("x") }, w.full());
    let log = w
        .store
        .raw_db()
        .query("SELECT tool, denied, result_count, result_bytes FROM access_log ORDER BY rowid", &[], |r| (r.text(0), r.opt_text(1), r.opt_int(2), r.opt_int(3)))
        .unwrap();
    assert_eq!(log.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(), vec!["search", "media", "get_event", "search"]);
    assert_eq!(
        log.iter().map(|l| l.1.clone().unwrap_or_else(|| "ok".into())).collect::<Vec<_>>(),
        vec!["ok", "media_not_permitted", "out_of_scope", "invalid_cursor"]
    );
    assert!(log[0].2.unwrap_or(0) > 0 && log[0].3.unwrap_or(0) > 0);
    let params = w.first_text("SELECT params_json FROM access_log ORDER BY rowid LIMIT 1").unwrap();
    assert!(params.contains("预算"));
    w.exec("UPDATE store_meta SET value='redacted_q' WHERE key='access_log_params'", &[]);
    w.search(q("secret keyword"), w.full());
    let redacted = w.first_text("SELECT params_json FROM access_log ORDER BY rowid DESC LIMIT 1").unwrap();
    assert!(!redacted.contains("secret keyword") && redacted.contains("<redacted>"));
}

#[test]
fn revoked_and_unknown_clients_are_refused() {
    let w = World::new();
    w.store.revoke_client(w.full()).unwrap();
    assert_eq!(w.store.status(w.full()).err(), Some(TimelineError::Denied("client_revoked".into())));
    assert_eq!(denial_codes(&w), vec!["client_revoked"]);
    assert_eq!(w.store.status("ghost").err(), Some(TimelineError::UnknownClient("ghost".into())));
    // bearer token lookup: only the hash is stored
    let reg = w.store.register_client("Tokened", "http", &ClientScope::default(), None).unwrap();
    let token = reg.token.clone().unwrap();
    assert_eq!(w.store.client_id_for_token(&token).unwrap(), Some(reg.client_id.clone()));
    assert_eq!(w.store.client_id_for_token("wrong").unwrap(), None);
    assert_eq!(
        w.store.raw_db().scalar("SELECT COUNT(*) FROM clients WHERE token_hash=?", &[testing::rusqlite::types::Value::Text(token.clone())]).unwrap(),
        0
    );
    w.store.revoke_client(&reg.client_id).unwrap();
    assert_eq!(w.store.client_id_for_token(&token).unwrap(), None);
    // a scope change takes effect on the next call
    let c = w.client("Changing", unbounded());
    assert!(!w.search(q_all("预算"), &c).hits.is_empty());
    w.store.update_scope(&c, &ClientScope { kinds: vec!["note".into()], ..unbounded() }).unwrap();
    assert!(w.search(q_all("预算"), &c).hits.is_empty());
}

#[test]
fn sources_hidden_from_agents_stay_hidden_unless_listed_explicitly() {
    let w = World::new();
    let codex = w.source("codex");
    w.store.update_policy(&codex, &SourcePolicy { agent_visible: false, ..Default::default() }).unwrap();
    assert!(w.search(q("汇总"), w.full()).hits.is_empty());
    let explicit = w.client("Explicit", ClientScope { sources: vec![codex], ..unbounded() });
    assert_eq!(w.search(q("汇总"), &explicit).hits.len(), 2);
}

#[test]
fn relative_and_absolute_time_parsing() {
    let clock = test_clock();
    assert_eq!(clock.parse("now", false).unwrap(), FIXED_NOW_MS);
    assert_eq!(clock.parse("today", false).unwrap(), TODAY_START_MS);
    assert_eq!(clock.parse("2027-01-14T16:00:00.500Z", false).unwrap(), TODAY_START_MS + 500);
    assert_eq!(clock.iso(TODAY_START_MS), "2027-01-15T00:00:00.000+08:00");
}

#[test]
fn request_and_response_types_round_trip_through_json() {
    let w = World::new();
    let req: SearchRequest = serde_json::from_str(r#"{"q":"预算","kinds":["screen.text"],"limit":3,"attention":"all","max_chars":100}"#).unwrap();
    assert!(req.q.as_deref() == Some("预算") && req.limit == Some(3) && req.max_chars == Some(100));
    let res = w.search(req, w.full());
    let data = json::canonical_of(&res);
    assert_eq!(serde_json::from_str::<SearchResponse>(&data).unwrap(), res);
    let object: serde_json::Value = serde_json::from_str(&data).unwrap();
    for key in ["hits", "has_new_records", "coverage", "window"] {
        assert!(object.get(key).is_some(), "missing {key}");
    }
    let hit = &object["hits"][0];
    for key in ["event_id", "kind", "start", "app", "title", "source_label", "layer", "snippet", "attention", "ref", "score"] {
        assert!(hit.get(key).is_some(), "missing {key}");
    }
}

#[test]
fn empty_page_cursors_are_first_page_but_nonempty_invalid_cursors_still_fail() {
    let w = World::new();
    let base = w.store.search(&q("预算"), w.full()).unwrap();
    for empty in ["", "   "] {
        let paged = w.store.search(&SearchRequest { cursor: s(empty), ..q("预算") }, w.full()).unwrap();
        assert_eq!(base.hits.iter().map(|h| &h.event_id).collect::<Vec<_>>(), paged.hits.iter().map(|h| &h.event_id).collect::<Vec<_>>());
        let base = w.store.timeline(&TimelineRequest::default(), w.full()).unwrap();
        let page = w.store.timeline(&TimelineRequest { cursor: s(empty), ..Default::default() }, w.full()).unwrap();
        assert_eq!(serde_json::to_value(base).unwrap(), serde_json::to_value(page).unwrap());
        assert!(w.store.episodes(&EpisodesRequest { cursor: s(empty), ..Default::default() }, w.full()).is_ok());
    }
    assert_eq!(w.store.timeline(&TimelineRequest { cursor: s("not-a-cursor"), ..Default::default() }, w.full()).err(), Some(TimelineError::InvalidCursor));
}
