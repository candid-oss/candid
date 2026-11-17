use super::*;
use crate::{fixtures, IngestBatch, IngestCursor, TimelineStore};

#[test]
fn compact_entries_read_old_nulls_and_preserve_all_values() {
    let legacy = json!({"id":"SYNTHETIC-id","source_file":"/SYNTHETIC/session.jsonl","source_line":7,
        "source_hash":"SYNTHETIC-hash","order_ms":123,"ts":123,"offset":null,"kind":"message","role":"user",
        "text":"SYNTHETIC prompt","tool":null,"original_chars":16,"truncated":false,"call_id":null,
        "succeeded":null,"path":null,"paths":[],"usage":null,"usage_key":null,"status":null});
    let entry: Entry = serde_json::from_value(legacy.clone()).unwrap();
    let compact = serde_json::to_value(&entry).unwrap();
    assert!(compact.get("tool").is_none());
    assert!(compact.get("paths").is_none());
    assert!(compact.get("truncated").is_none());
    let roundtrip: Entry = serde_json::from_value(compact.clone()).unwrap();
    assert_eq!(roundtrip.source_file, "/SYNTHETIC/session.jsonl");
    assert_eq!(roundtrip.source_line, 7);
    assert_eq!(roundtrip.text.as_deref(), Some("SYNTHETIC prompt"));
    assert_eq!(roundtrip.order_ms, Some(123));
    assert!(!roundtrip.truncated && roundtrip.paths.is_empty());
    assert_eq!(serde_json::to_value(roundtrip).unwrap(), compact);
    assert!(serde_json::to_vec(&compact).unwrap().len() < serde_json::to_vec(&legacy).unwrap().len());
}

#[test]
fn identical_agent_checkpoints_do_not_rewrite_but_changes_and_retries_remain_durable() {
    let root = fixtures::temp_root("SYNTHETIC-checkpoint-writes");
    let store = TimelineStore::open(&root, "SYNTHETIC-test", true, fixtures::test_clock()).unwrap();
    let sid = store.register_source("manual", "SYNTHETIC checkpoint", &crate::SourcePolicy::default()).unwrap();
    store.db.script("CREATE TEMP TABLE checkpoint_writes(n INTEGER); INSERT INTO checkpoint_writes VALUES(0); CREATE TEMP TRIGGER count_checkpoint_writes AFTER UPDATE ON source_cursors BEGIN UPDATE checkpoint_writes SET n=n+1; END;").unwrap();
    let batch = IngestBatch::events(&sid, vec![]);
    let checkpoint = IngestCursor::new("session:SYNTHETIC", &"SYNTHETIC repeated data ".repeat(4096));
    store.ingest_agent_batch(&batch, &[checkpoint.clone()], &CheckpointDelta::default()).unwrap();
    store.ingest_agent_batch(&batch, &[checkpoint.clone()], &CheckpointDelta::default()).unwrap();
    assert_eq!(store.db.scalar("SELECT n FROM checkpoint_writes", &[]).unwrap(), 0);
    let changed = IngestCursor::new(&checkpoint.stream, &(checkpoint.cursor.clone() + "SYNTHETIC changed"));
    store.ingest_agent_batch(&batch, &[changed.clone()], &CheckpointDelta::default()).unwrap();
    store.ingest_agent_batch(&batch, &[changed.clone()], &CheckpointDelta::default()).unwrap();
    assert_eq!(store.db.scalar("SELECT n FROM checkpoint_writes", &[]).unwrap(), 1);
    assert_eq!(store.cursor(&sid, &checkpoint.stream).unwrap().as_deref(), Some(changed.cursor.as_str()));
    drop(store);
    let reopened = TimelineStore::open(&root, "SYNTHETIC-test", false, fixtures::test_clock()).unwrap();
    assert_eq!(reopened.cursor(&sid, &checkpoint.stream).unwrap().as_deref(), Some(changed.cursor.as_str()));
    drop(reopened); let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_checkpoint_converts_atomically_then_only_appended_entries_are_written() {
    let root=fixtures::temp_root("SYNTHETIC-delta-checkpoint");
    let store=TimelineStore::open(&root,"SYNTHETIC",true,fixtures::test_clock()).unwrap();
    let sid=store.register_source("manual","SYNTHETIC",&crate::SourcePolicy::default()).unwrap();
    let batch=IngestBatch::events(&sid,vec![]);
    let stream="session:SYNTHETIC";
    let legacy=Session {key:"SYNTHETIC".into(),entries:(0..100).map(|i|Entry{id:format!("SYNTHETIC-{i}"),kind:"message".into(),role:"user".into(),ts:i,text:Some("SYNTHETIC user text ".repeat(100)),..Default::default()}).collect(),..Default::default()};
    store.ingest_agent_batch(&batch,&[IngestCursor::new(stream,&crate::json::canonical_of(&legacy))],&CheckpointDelta::default()).unwrap();
    let mut state=store.load_agent_session(&sid,stream).unwrap().unwrap();
    let mut delta=CheckpointDelta::default();
    let cursor=super::checkpoints::session_checkpoint(stream,&state,&mut delta);
    assert_eq!(delta.entries.len(),100);
    assert!(cursor.cursor.len()<1000);
    store.db.script("CREATE TEMP TRIGGER fail_checkpoint BEFORE UPDATE ON source_cursors BEGIN SELECT RAISE(ABORT,'SYNTHETIC interrupted'); END;").unwrap();
    assert!(store.ingest_agent_batch(&batch,&[cursor.clone()],&delta).is_err());
    assert_eq!(store.db.scalar("SELECT COUNT(*) FROM agent_session_entries",&[]).unwrap(),0);
    assert_eq!(store.load_agent_session(&sid,stream).unwrap().unwrap().entries,legacy.entries);
    store.db.script("DROP TRIGGER fail_checkpoint").unwrap();
    store.ingest_agent_batch(&batch,&[cursor],&delta).unwrap();
    drop(store);
    let store=TimelineStore::open(&root,"SYNTHETIC",false,fixtures::test_clock()).unwrap();
    state=store.load_agent_session(&sid,stream).unwrap().unwrap();
    assert_eq!(state.entries,legacy.entries);
    state.entries.push(Entry{id:"SYNTHETIC-new".into(),kind:"message".into(),ts:101,..Default::default()});
    let mut delta=CheckpointDelta::default();
    let cursor=super::checkpoints::session_checkpoint(stream,&state,&mut delta);
    assert_eq!(delta.entries.len(),1);
    store.ingest_agent_batch(&batch,&[cursor.clone()],&delta).unwrap();
    let before=store.db.scalar("SELECT total_changes()",&[]).unwrap();
    store.ingest_agent_batch(&batch,&[cursor],&delta).unwrap();
    assert_eq!(store.db.scalar("SELECT total_changes()",&[]).unwrap(),before);
    assert_eq!(store.load_agent_session(&sid,stream).unwrap().unwrap().entries.len(),101);
    store.delete_source_data(&sid,"source_revoked").unwrap();
    assert_eq!(store.db.scalar("SELECT COUNT(*) FROM agent_session_entries",&[]).unwrap(),0);
    drop(store);let _=fs::remove_dir_all(root);
}

#[test]
fn file_occurrence_counters_append_and_reset_without_rewriting_old_counts() {
    let root=fixtures::temp_root("SYNTHETIC-delta-counts");
    let store=TimelineStore::open(&root,"SYNTHETIC",true,fixtures::test_clock()).unwrap();
    let sid=store.register_source("manual","SYNTHETIC",&crate::SourcePolicy::default()).unwrap();
    let batch=IngestBatch::events(&sid,vec![]);let stream="file:SYNTHETIC";
    let mut state=Session::default();state.fallback_counts.insert("SYNTHETIC-a".into(),1);
    let mut delta=CheckpointDelta::default();
    let cursor=super::checkpoints::file_checkpoint(stream,state,&mut delta);
    store.ingest_agent_batch(&batch,&[cursor],&delta).unwrap();
    let mut state=store.load_agent_file(&sid,stream).unwrap().unwrap();
    state.fallback_counts.insert("SYNTHETIC-b".into(),1);
    let mut delta=CheckpointDelta::default();let cursor=super::checkpoints::file_checkpoint(stream,state,&mut delta);
    assert_eq!(delta.counts.len(),1);
    store.ingest_agent_batch(&batch,&[cursor],&delta).unwrap();
    let mut reset=Session{reset_counts:true,..Default::default()};reset.fallback_counts.insert("SYNTHETIC-c".into(),1);
    let mut delta=CheckpointDelta::default();let cursor=super::checkpoints::file_checkpoint(stream,reset,&mut delta);
    store.ingest_agent_batch(&batch,&[cursor],&delta).unwrap();
    assert_eq!(store.load_agent_file(&sid,stream).unwrap().unwrap().fallback_counts.len(),1);
    store.delete_source_data(&sid,"source_revoked").unwrap();
    assert_eq!(store.db.scalar("SELECT COUNT(*) FROM agent_file_counts",&[]).unwrap(),0);
    drop(store);let _=fs::remove_dir_all(root);
}

#[test]
fn good_fragments_do_not_clear_an_unresolved_source_error() {
    let root=fixtures::temp_root("SYNTHETIC-fragment-health");
    let store=TimelineStore::open(&root,"SYNTHETIC",true,fixtures::test_clock()).unwrap();
    let sid=store.register_source("manual","SYNTHETIC",&crate::SourcePolicy::default()).unwrap();
    store.db.execute("UPDATE sources SET status='error',last_error='invalid_session' WHERE source_id=?",&[crate::db::text(&sid)]).unwrap();
    let revision=store.background_revision("workflows").unwrap();
    store.ingest_agent_batch(&IngestBatch::events(&sid,vec![]),&[],&CheckpointDelta::default()).unwrap();
    assert_eq!(store.background_revision("workflows").unwrap(),revision);
    assert_eq!(store.sources().unwrap().iter().find(|s|s.source_id==sid).unwrap().status,"error");
    drop(store);let _=fs::remove_dir_all(root);
}

#[test]
fn same_line_entries_keep_the_original_order_after_restart() {
    let root=fixtures::temp_root("SYNTHETIC-entry-order");
    let store=TimelineStore::open(&root,"SYNTHETIC",true,fixtures::test_clock()).unwrap();
    let sid=store.register_source("manual","SYNTHETIC",&crate::SourcePolicy::default()).unwrap();
    let entries=["z","a","m"].iter().map(|id|Entry{id:id.to_string(),kind:"toolCall".into(),ts:1,source_file:"/SYNTHETIC/log".into(),source_line:3,..Default::default()}).collect::<Vec<_>>();
    let state=Session{entries:entries.clone(),..Default::default()};let mut delta=CheckpointDelta::default();
    let cursor=super::checkpoints::session_checkpoint("session:SYNTHETIC",&state,&mut delta);
    store.ingest_agent_batch(&IngestBatch::events(&sid,vec![]),&[cursor],&delta).unwrap();drop(store);
    let store=TimelineStore::open(&root,"SYNTHETIC",false,fixtures::test_clock()).unwrap();
    assert_eq!(store.load_agent_session(&sid,"session:SYNTHETIC").unwrap().unwrap().entries,entries);
    drop(store);let _=fs::remove_dir_all(root);
}
