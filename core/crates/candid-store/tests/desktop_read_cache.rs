//! Desktop cache correctness: payload edits, source policies, annotations, paging, deletion and per-call audit.
mod common;
use common::*;
use candid_store::*;
use serde_json::json;

const END: i64 = 10000;
fn request(q: Option<&str>) -> AppActivitiesRequest {
    AppActivitiesRequest { start_ms: 0, end_ms: END, q: q.map(String::from), ..Default::default() }
}

#[test]
fn unrelated_source_changes_reuse_results_but_desktop_edits_and_deletes_are_fresh() {
    let fx = Fx::make("desktop-cache");
    let s = &fx.store;
    let app = s.register_client("App", "internal", &ClientScope::unbounded(), None).unwrap().client_id;
    let other = s.register_source("manual", "SYNTHETIC unrelated", &SourcePolicy::default()).unwrap();
    let mut row = text_event("a", 1000, "SYNTHETIC initial");
    row.title = Some("SYNTHETIC initial".into());
    s.ingest(&IngestBatch::events(&fx.source, vec![row.clone()])).unwrap();
    let first = s.app_activities(&request(None), &app).unwrap();
    assert_eq!(first.stats.record_count, 1);
    let detail = AppActivityRequest { id: "a".into(), ..Default::default() };
    let first_detail = s.app_activity(&detail, &app).unwrap();
    s.ingest(&IngestBatch::events(&other, vec![note("note", Some(1000))])).unwrap();
    s.set_source_status(&other, "error", Some("SYNTHETIC offline")).unwrap();
    s.set_source_status(&other, "active", None).unwrap();
    assert_eq!(s.app_activities(&request(None), &app).unwrap(), first);
    assert_eq!(s.app_activity(&detail, &app).unwrap(), first_detail);
    assert!(s.app_activities(&request(Some("missing")), &app).unwrap().activities.is_empty());
    assert_eq!(s.app_activities(&request(None), &app).unwrap(), first);
    row.payload["trigger"] = json!("focus");
    let mut revision = IngestBatch::events(&fx.source, vec![]);
    revision.payload_revisions.push(PayloadRevision { source_key: "a".into(), payload: row.payload });
    assert_eq!(s.ingest(&revision).unwrap().revised, 1);
    let updated = s.app_activities(&request(None), &app).unwrap();
    assert_ne!(updated, first);
    assert_eq!(updated.stats.record_count, 1);
    assert_ne!(s.app_activity(&detail, &app).unwrap(), first_detail);
    let id = s.event_id(&fx.source, "a").unwrap().unwrap();
    s.delete_events(&[id], "user").unwrap();
    assert_eq!(s.app_stats().unwrap().record_count, 0);
    assert!(s.app_activities(&request(None), &app).unwrap().activities.is_empty());
    assert!(s.app_activity(&detail, &app).unwrap().activity.is_none());
    assert!(fx.scalar("SELECT COUNT(*) FROM access_log WHERE tool='app_activities'") >= 6);
}

#[test]
fn annotations_policy_changes_and_client_revocation_apply_to_previously_cached_pages() {
    let fx = Fx::make("desktop-cache-permissions");
    let s = &fx.store;
    let app = s.register_client("App", "internal", &ClientScope::unbounded(), None).unwrap().client_id;
    s.ingest(&IngestBatch::events(&fx.source, vec![text_event("a", 1000, "SYNTHETIC words")])).unwrap();
    let id = s.event_id(&fx.source, "a").unwrap().unwrap();
    s.app_activities(&request(None), &app).unwrap();
    let detail = AppActivityRequest { id: "a".into(), ..Default::default() };
    s.app_activity(&detail, &app).unwrap();
    s.record_management(&json!({"action":"annotation_save","event_ids":[id],"request_id":"SYNTHETIC-edit","title":"SYNTHETIC manual title","notes":"SYNTHETIC note"}), &app).unwrap();
    assert_eq!(s.app_activities(&request(None), &app).unwrap().activities[0].latest.manual_title.as_deref(), Some("SYNTHETIC manual title"));
    assert_eq!(s.app_activity(&detail, &app).unwrap().activity.unwrap().latest.manual_title.as_deref(), Some("SYNTHETIC manual title"));
    s.set_source_status(&fx.source, "revoked", None).unwrap();
    assert!(s.app_activities(&request(None), &app).unwrap().activities.is_empty());
    assert!(s.app_activity(&detail, &app).unwrap().activity.is_none());
    assert_eq!(s.app_stats().unwrap().record_count, 0);
    s.set_source_status(&fx.source, "active", None).unwrap();
    assert_eq!(s.app_activities(&request(None), &app).unwrap().activities.len(), 1);
    s.revoke_client(&app).unwrap();
    assert!(s.app_activities(&request(None), &app).is_err());
    assert!(s.app_activity(&detail, &app).is_err());
}
