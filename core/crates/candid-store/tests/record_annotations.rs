mod common;
use common::*;
use candid_store::*;
use serde_json::{json, Value};
fn command(s: &TimelineStore, c: &str, v: Value) -> Value {
    s.record_management(&v, c).unwrap()
}
fn event(s: &TimelineStore, c: &str, id: &str) -> Value {
    serde_json::to_value(
        s.get_event(&serde_json::from_value(json!({"event_id":id})).unwrap(), c)
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn annotations_restart_retry_clear_reset_rebuild_and_read_projections() {
    let mut fx = Fx::make("annotations");
    let app = fx
        .store
        .register_client("App", "internal", &ClientScope::unbounded(), None)
        .unwrap()
        .client_id;
    fx.store
        .ingest(&IngestBatch::events(
            &fx.source,
            vec![text_event("a", 1000, "SYNTHETIC original words")],
        ))
        .unwrap();
    let id = fx.store.event_id(&fx.source, "a").unwrap().unwrap();
    let save = json!({"action":"annotation_save","event_ids":[id],"request_id":"first","title":"人工标题","notes":"SYNTHETIC manualnote"});
    command(&fx.store, &app, save.clone());
    command(&fx.store, &app, save.clone());
    assert_eq!(fx.scalar("SELECT COUNT(*) FROM corrections"), 1);
    assert_eq!(
        event(&fx.store, &fx.client, &id)["event"]["title"],
        "人工标题"
    );
    assert_eq!(
        event(&fx.store, &fx.client, &id)["paragraphs"][0]["text"],
        "SYNTHETIC original words"
    );
    assert_eq!(
        event(&fx.store, &fx.client, &id)["payload"]["manual_metadata"]["notes"],
        "SYNTHETIC manualnote"
    );
    let list = fx
        .store
        .app_activities(
            &AppActivitiesRequest {
                start_ms: 0,
                end_ms: 10000,
                ..Default::default()
            },
            &app,
        )
        .unwrap();
    assert_eq!(
        list.activities[0].latest.manual_title.as_deref(),
        Some("人工标题")
    );
    let search = fx
        .store
        .search(
            &serde_json::from_value(json!({"q":"manualnote"})).unwrap(),
            &fx.client,
        )
        .unwrap();
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].title.as_deref(), Some("人工标题"));
    let revision = fx
        .store
        .stage_record_revision(&fx.source, Default::default())
        .unwrap();
    fx.store.select_record_revision(&revision, &app).unwrap();
    fx.store.close();
    let reopened = open(&fx.root);
    assert_eq!(
        event(&reopened, &fx.client, &id)["event"]["title"],
        "人工标题"
    );
    let mut clear = save.clone();
    clear["request_id"] = json!("clear");
    clear["title"] = json!("");
    clear["notes"] = json!("");
    command(&reopened, &app, clear);
    assert_eq!(event(&reopened, &fx.client, &id)["event"]["title"], "");
    let mut reset = save.clone();
    reset["request_id"] = json!("reset");
    reset["title"] = Value::Null;
    reset["notes"] = Value::Null;
    command(&reopened, &app, reset);
    assert_eq!(
        event(&reopened, &fx.client, &id)["event"]["title"],
        "Synthetic doc"
    );
    command(&reopened, &app, save.clone()); // delayed old retry cannot restore an old edit
    assert_eq!(
        event(&reopened, &fx.client, &id)["event"]["title"],
        "Synthetic doc"
    );
    assert!(reopened.record_management(&save, &fx.client).is_err());
    reopened.delete_events(&[id.clone()], "user").unwrap();
    assert!(reopened.record_management(&save, &app).is_err());
    assert_eq!(
        reopened
            .search(
                &serde_json::from_value(json!({"q":"manualnote"})).unwrap(),
                &fx.client
            )
            .unwrap()
            .hits
            .len(),
        0
    );
}
#[test]
fn attached_edit_requires_every_source_and_scope() {
    let fx = Fx::make("annotation-permission");
    let app = fx
        .store
        .register_client("App", "internal", &ClientScope::unbounded(), None)
        .unwrap()
        .client_id;
    let other = fx
        .store
        .register_source(
            "mac.desktop",
            "Synthetic other",
            &SourcePolicy::new(true, true),
        )
        .unwrap();
    for source in [&fx.source, &other] {
        fx.store
            .ingest(&IngestBatch::events(
                source,
                vec![text_event("a", 1000, "SYNTHETIC")],
            ))
            .unwrap();
    }
    let a = fx.store.event_id(&fx.source, "a").unwrap().unwrap();
    let b = fx.store.event_id(&other, "a").unwrap().unwrap();
    command(
        &fx.store,
        &app,
        json!({"action":"annotation_save","event_ids":[a,b],"request_id":"joined","title":"joinedtitle","notes":"joinednote"}),
    );
    let mut scope = ClientScope::unbounded();
    scope.sources = vec![fx.source.clone()];
    let limited = fx
        .store
        .register_client("Limited", "http", &scope, None)
        .unwrap()
        .client_id;
    assert!(event(&fx.store, &limited, &a)["payload"]
        .get("manual_metadata")
        .is_none());
    fx.store.delete_events(&[b], "user").unwrap();
    assert_eq!(
        event(&fx.store, &app, &a)["event"]["title"],
        "Synthetic doc"
    );
    assert!(event(&fx.store, &app, &a)["payload"]
        .get("manual_metadata")
        .is_none());
}
#[test]
fn revoke_narrow_policy_and_restore_never_restore_deleted_evidence() {
    let fx = Fx::make("annotation-policy");
    let app = fx
        .store
        .register_client("App", "internal", &ClientScope::unbounded(), None)
        .unwrap()
        .client_id;
    fx.store
        .ingest(&IngestBatch::events(
            &fx.source,
            vec![text_event("a", 1000, "SYNTHETIC original")],
        ))
        .unwrap();
    let id = fx.store.event_id(&fx.source, "a").unwrap().unwrap();
    let edit = json!({"action":"annotation_save","event_ids":[id],"request_id":"edit","title":"label","notes":"privateannotation"});
    command(&fx.store, &app, edit.clone());
    let mut policy = SourcePolicy::new(true, true);
    policy.exclude.apps.push("com.example.notes".into());
    fx.store.update_policy(&fx.source, &policy).unwrap();
    assert!(event(&fx.store, &app, &id)["payload"]
        .get("manual_metadata")
        .is_none());
    assert!(fx.store.record_management(&edit, &app).is_err());
    fx.store
        .update_policy(&fx.source, &SourcePolicy::new(true, true))
        .unwrap();
    assert_eq!(event(&fx.store, &app, &id)["event"]["title"], "label");
    fx.store
        .set_source_status(&fx.source, "revoked", None)
        .unwrap();
    assert!(fx
        .store
        .record_management(&json!({"action":"annotation_get","event_ids":[id]}), &app)
        .is_err());
    assert_eq!(
        fx.store
            .search(
                &serde_json::from_value(json!({"q":"privateannotation"})).unwrap(),
                &app
            )
            .unwrap()
            .hits
            .len(),
        0
    );
    fx.store
        .set_source_status(&fx.source, "active", None)
        .unwrap();
    fx.store.delete_events(&[id.clone()], "user").unwrap();
    assert!(fx.store.record_management(&edit, &app).is_err());
}
