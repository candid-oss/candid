use super::*;
use crate::db::{int, text, RowExt};
use crate::{IngestBatch, IngestCursor, IngestEvent, IngestText, IngestThread, TimelineStore};
use std::collections::{BTreeMap, BTreeSet};

impl TimelineStore {
    pub fn agent_source_id(&self,agent:&str)->Result<Option<String>> {
        self.db.first("SELECT source_id FROM sources WHERE kind='agent.transcript' AND json_extract(policy_json,'$.agent')=?",&[text(agent)],|r|r.text(0))
    }
    pub(crate) fn source_agent(&self,source:&str)->Result<Option<String>> {
        self.db.first("SELECT json_extract(policy_json,'$.agent') FROM sources WHERE source_id=? AND kind='agent.transcript'",&[text(source)],|r|r.opt_text(0)).map(Option::flatten)
    }
    pub fn agent_consent(&self, agent: &str) -> Result<AgentConsent> {
        if !AGENTS.contains(&agent) {
            return Err(failure("unsupported_agent"));
        }
        let policy = self.db.first(
            "SELECT policy_json FROM sources WHERE kind='agent.transcript' AND json_extract(policy_json,'$.agent')=?",
            &[text(agent)],
            |r| r.text(0),
        )?;
        policy
            .map(|p| serde_json::from_str(&p).map_err(|_| failure("invalid_consent")))
            .unwrap_or_else(|| Ok(AgentConsent::new(agent)))
    }
    pub fn set_agent_consent(&self, c: &AgentConsent, delete_stored: bool) -> Result<()> {
        self.agent_file_stamps.borrow_mut().clear();
        c.validate()?;
        // New folders or exclusions: everything is looked at again on the next pass.
        let mut normalized=c.clone();
        normalized.exclude.projects=c.exclude.projects.iter().map(|p|crate::store::standardize(&expand(p)).to_string_lossy().into_owned()).collect();
        normalized.paths=c.roots().iter().map(|p|crate::store::standardize(p).to_string_lossy().into_owned()).collect();
        let c=&normalized;
        let sid = self.agent_source_id(&c.agent)?.unwrap_or_else(||crate::ids::uuid_v7(Some(self.clock.now_ms())));
        self.db.transaction(||{
            self.db.execute("INSERT INTO sources(source_id,kind,display_name,config_json,policy_json,status,created_ms,updated_ms) VALUES(?,'agent.transcript',?,'{\"version\":1}',?,'active',?,?) ON CONFLICT(source_id) DO UPDATE SET policy_json=excluded.policy_json,updated_ms=excluded.updated_ms",&[text(&sid),text(&c.agent),text(crate::json::canonical_of(c)),int(self.clock.now_ms()),int(self.clock.now_ms())])?;
            // Replay files under changed settings; keep admitted session history across source rotation.
            self.db.execute("DELETE FROM source_cursors WHERE source_id=? AND stream LIKE 'file:%'",&[text(&sid)])?;
            self.record_change("source",&sid,"update")
        })?;
        if delete_stored {
            self.delete_source_data(&sid, "source_revoked")?;
        }
        Ok(())
    }
    pub fn sync_agents(&self) -> Result<Value> {
        // Rebuildable query accelerator, independent of the database contract version.
        self.db.execute("CREATE INDEX IF NOT EXISTS agent_message_time_scan ON events(kind,json_extract(payload_json,'$.last_message_ms'),ingest_seq) WHERE kind='ai.prompt'",&[])?;
        let mut inserted = 0;
        let mut revised = 0;
        let mut issues = vec![];
        for agent in AGENTS {
            let c = self.agent_consent(agent)?;
            if !c.read || !c.activity {
                continue;
            }
            let sid=self.agent_source_id(agent)?.ok_or_else(||failure("missing_agent_source"))?;
            let before=self.db.first("SELECT last_sync_ms FROM sources WHERE source_id=?",&[text(&sid)],|r|r.opt_int(0))?.flatten();
            let successful=match self.sync_agent(&c) {
                Ok((i, r, errors)) => {
                    inserted += i;
                    revised += r;
                    let successful=errors.is_empty();
                    issues.extend(errors);
                    successful
                }
                Err(e) => {issues.push(json!({"agent":agent,"code":e.code()}));false},
            };
            if successful {self.touch_source_sync(&sid)?;}
            else {self.db.execute("UPDATE sources SET status='error',last_error='invalid_session',last_sync_ms=? WHERE source_id=? AND (status IS NOT 'error' OR last_error IS NOT 'invalid_session' OR last_sync_ms IS NOT ?)",&[crate::db::opt_int(before),text(&sid),crate::db::opt_int(before)])?;}
        }
        Ok(json!({"inserted":inserted,"revised":revised,"issues":issues}))
    }
    fn sync_agent(&self, c: &AgentConsent) -> Result<(usize, usize, Vec<Value>)> {
        c.require_read()?;
        if !c.activity {
            return Err(TimelineError::Denied("activity_required".into()));
        }
        let sid = self.agent_source_id(&c.agent)?.ok_or_else(||failure("missing_agent_source"))?;
        let mut inserted = 0;
        let mut revised = 0;
        let mut issues = vec![];
        // Files are ordered by modification time, not by conversation time.
        // Stage every fragment of a session before constructing prompts/work.
        let mut sessions: BTreeMap<String, (Session, Vec<IngestCursor>, CheckpointDelta)> = BTreeMap::new();
        let (paths, limited) = file_page(c)?;
        if limited { return Err(failure("source_enumeration_incomplete")); }
        // Files unchanged since their last successful pass are skipped before any read: a pass over a large
        // history with nothing new stays cheap (the store is locked while it runs).
        let mut processed: Vec<(String, FileStamp)> = vec![];
        for path in paths {
            let stamp = std::fs::metadata(&path).ok().map(|m| (m.len(), m.modified().ok()));
            let key = path.to_string_lossy().to_string();
            let done_key = format!("{}\u{1f}{}", c.agent, key);
            let stream = format!("file:{}", path.to_string_lossy());
            if stamp.is_some() && self.agent_file_stamps.borrow().get(&done_key) == stamp.as_ref() {
                continue;
            }
            let previous = self.load_agent_file(&sid, &stream)?;
            // A file that failed to parse is not parsed again until it changes (size or modification time).
            if stamp.is_some() && unreadable().lock().unwrap().get(&key) == stamp.as_ref() {
                issues.push(json!({"agent":c.agent,"code":"invalid_session"}));
                continue;
            }
            let file_state = match formats::load(&path, c, previous, true) {
                Ok(s) => {
                    unreadable().lock().unwrap().remove(&key);
                    s
                }
                Err(_) => {
                    if let Some(stamp) = stamp { unreadable().lock().unwrap().insert(key, stamp); }
                    issues.push(json!({"agent":c.agent,"code":"invalid_session"}));
                    continue;
                }
            };
            if let Some(stamp) = stamp.filter(|s| s.0 == file_state.offset && file_state.checkpoint_format == super::CHECKPOINT_FORMAT) {
                processed.push((done_key, stamp));
            }
            if file_state.key.is_empty()
                || c.excluded(&file_state.key, file_state.project.as_deref())
            {
                continue;
            }
            let session_stream = format!("session:{}", file_state.key);
            if !sessions.contains_key(&session_stream) {
                let st = self.load_agent_session(&sid, &session_stream)?.unwrap_or_else(|| file_state.clone());
                sessions.insert(session_stream.clone(), (st, vec![], CheckpointDelta::default()));
            }
            let (st, cursors, delta) = sessions.get_mut(&session_stream).unwrap();
            st.merge_fragment(&file_state);
            cursors.push(super::checkpoints::file_checkpoint(&stream, file_state, delta));
        }
        for (session_stream, (mut st, mut cursors, mut delta)) in sessions {
            if st.limited {
                for cursor in &cursors {
                    if let Some(path) = cursor.stream.strip_prefix("file:") {
                        processed.retain(|(key,_)| key != &format!("{}\u{1f}{}",c.agent,path));
                    }
                }
                issues.push(json!({"agent":c.agent,"code":"ambiguous_source"}));continue;
            }
            apply_prompt_limit(&mut st, c);
            let mut batch = build(c, &st, self.clock.now_ms());
            batch.source_id = sid.clone();
            cursors.push(super::checkpoints::session_checkpoint(&session_stream, &st, &mut delta));
            let (i, r) = self.ingest_agent_batch(&batch, &cursors, &delta)?;
            inserted += i;
            revised += r;
        }
        // Only once everything above committed: a failed pass reads these files again next time.
        self.agent_file_stamps.borrow_mut().extend(processed);
        Ok((inserted, revised, issues))
    }
}
struct Group {
    messages: Vec<Entry>,
    work: Vec<Entry>,
    started: bool,
    boundary: Option<i64>,
}
fn build(c: &AgentConsent, s: &Session, now: i64) -> IngestBatch {
    let mut groups: Vec<Group> = vec![];
    for e in &s.entries {
        if e.kind == "message" && e.role == "user" {
            if let Some(g) = groups.last_mut().filter(|g| !g.started && g.messages.last().is_none_or(|m| m.text != e.text)) {
                g.messages.push(e.clone());
            } else {
                if let Some(g) = groups.last_mut() {
                    g.boundary = Some(e.ts);
                }
                groups.push(Group {
                    messages: vec![e.clone()],
                    work: vec![],
                    started: false,
                    boundary: None,
                });
            }
        } else if let Some(g) = groups.last_mut() {
            if e.role == "assistant" || e.kind == "toolCall" || e.kind == "status" {
                g.started = true;
            }
            g.work.push(e.clone());
        }
    }
    let session_ref = s.reference(&c.agent);
    let mut events = vec![];
    for g in groups {
        let first = &g.messages[0];
        let prompt_ref = format!("{session_ref}#{}", first.id);
        let mut text = String::new();
        let mut refs = vec![];
        for m in &g.messages {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            let begin = text.chars().count();
            if let Some(t) = &m.text {
                text.push_str(t);
            }
            refs.push(json!({"ref":format!("{session_ref}#{}",m.id),"stored_char_range":[begin,text.chars().count()],"source_char_range":[0,m.original_chars],"source_hash":m.source_hash,"source_file":m.source_file,"truncated":m.truncated,"at_ms":m.ts}));
        }
        let original_chars =
            g.messages.iter().map(|m| m.original_chars).sum::<usize>() + 2 * (g.messages.len() - 1);
        let text = redact(&text);
        let max = c.prompt_max_chars.unwrap_or(usize::MAX);
        let truncated = text.chars().count() > max || g.messages.iter().any(|m| m.truncated);
        let text = text.chars().take(max).collect::<String>();
        let kept = text.chars().count();
        for r in &mut refs {
            let a = r["stored_char_range"][0].as_u64().unwrap_or(0) as usize;
            let b = r["stored_char_range"][1].as_u64().unwrap_or(0) as usize;
            r["stored_char_range"] = json!([a.min(kept), b.min(kept)]);
            if b > kept {
                r["truncated"] = json!(true);
            }
        }
        if !c.content.user {
            continue;
        }
        let mut p = IngestEvent::new(&prompt_ref, "ai.prompt", Some(first.ts));
        p.time_basis = "source".into();
        p.thread_key = Some(session_ref.clone());
        p.tz_offset_min = first.offset;
        p.payload = json!({"agent":c.agent,"session_ref":session_ref,"message_ref":prompt_ref,"project":s.project,"last_message_ms":g.messages.iter().map(|m|m.ts).max(),"followups":g.messages.len()-1,"truncated":truncated,"original_chars":original_chars,"origin":"transcript","messages":refs,"builder":"agent-records/2"});
        p.text = vec![IngestText::new("prompt", &text)];
        events.push(p);
        let mut tools: BTreeMap<String, usize> = BTreeMap::new();
        let mut calls_seen=BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut write_calls:BTreeMap<&str,Vec<&String>>=BTreeMap::new();
        let mut outcomes=BTreeMap::new();
        let mut confirmed=BTreeSet::new();
        let mut assistants = BTreeSet::new();
        let mut usage = Usage::default();
        let mut keyed = BTreeMap::new();
        let mut status = "running";
        for e in &g.work {
            if e.kind == "message" && e.role == "assistant" {
                assistants.insert(e.usage_key.as_ref().unwrap_or(&e.id));
            }
            if e.kind == "toolCall" && c.content.tool_calls != "none" {
                if let Some(t) = &e.tool {
                    if calls_seen.insert((t,e.call_id.as_ref().unwrap_or(&e.id))){*tools.entry(t.clone()).or_default() += 1;}
                }
            }
            if let Some(p) = &e.path {
                paths.insert(p);
            }
            for p in &e.paths {
                paths.insert(p);
            }
            if e.kind=="toolCall" && (e.path.is_some()||!e.paths.is_empty()) {let targets=e.path.iter().chain(e.paths.iter()).collect::<Vec<_>>();write_calls.insert(e.call_id.as_deref().unwrap_or(&e.id),targets);}
            if e.kind=="toolOutput" {if let (Some(id),Some(ok))=(&e.call_id,e.succeeded){outcomes.insert(id.as_str(),ok);}}
            if let Some(u) = &e.usage {
                if let Some(k) = &e.usage_key {
                    keyed.insert(k, u);
                } else {
                    usage.add(u);
                }
            }
            if let Some(st) = &e.status {
                status = st;
            } else if e.role == "assistant" || e.kind == "toolCall" {
                status = "running";
            }
        }
        for (id,targets) in &write_calls {if outcomes.get(id)==Some(&true){confirmed.extend(targets.iter().copied());}}
        let files_complete=write_calls.keys().all(|id|outcomes.contains_key(id));
        for u in keyed.values() {
            usage.add(u);
        }
        let last = g.work.last().unwrap_or(g.messages.last().unwrap());
        if status == "running" && (g.boundary.is_some() || now - last.ts > 120_000) {
            status = "interrupted";
        }
        let mut w = IngestEvent::new(&format!("{prompt_ref}:work"), "ai.work", Some(first.ts));
        w.end_ms = Some(g.boundary.unwrap_or(last.ts).max(first.ts));
        w.time_basis = "source".into();
        w.thread_key = Some(session_ref.clone());
        w.actor = "agent".into();
        w.parent_key = Some(prompt_ref.clone());
        w.payload = json!({"agent":c.agent,"session_ref":session_ref,"prompt_event_id":prompt_ref,"project":s.project,"status":status,"assistant_messages":assistants.len(),"tool_calls":tools,"files_touched":confirmed.len(),"file_targets":paths.len(),"files_touched_complete":files_complete,"files_touched_basis":"successful_write_tool_targets","tokens":usage,"builder":"agent-records/2"});
        events.push(w);
    }
    IngestBatch {
        threads: vec![IngestThread {
            source_key: session_ref,
            kind: "agent_session".into(),
            title: s.title.as_deref().map(redact),
            started_ms: s.entries.first().map(|e| e.ts),
            ended_ms: s.entries.last().map(|e| e.ts),
            payload: Some(json!({"agent":c.agent,"project":s.project})),
            ..Default::default()
        }],
        events,
        ..Default::default()
    }
}

fn apply_prompt_limit(session:&mut Session,c:&AgentConsent){
    let Some(max)=c.prompt_max_chars else{return;};
    let mut has_prompt=false;let mut started=false;let mut kept=0usize;
    for e in &mut session.entries {
        if e.kind=="message"&&e.role=="user" {
            if !has_prompt||started {kept=0;started=false;has_prompt=true;}
            let separator=if kept>0{2}else{0};
            let available=max.saturating_sub(kept.saturating_add(separator));
            if let Some(text)=&mut e.text {if text.chars().count()>available{*text=text.chars().take(available).collect();e.truncated=true;}kept=max.min(kept.saturating_add(separator).saturating_add(text.chars().count()));}
        } else if e.role=="assistant"||e.kind=="toolCall"||e.kind=="status" {started=true;}
    }
}

type FileStamp = (u64, Option<std::time::SystemTime>);

/// Session files that failed to parse, with the size and modification time they had then (process memory).
fn unreadable() -> &'static std::sync::Mutex<std::collections::HashMap<String, FileStamp>> {
    static UNREADABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, FileStamp>>> = std::sync::OnceLock::new();
    UNREADABLE.get_or_init(Default::default)
}
