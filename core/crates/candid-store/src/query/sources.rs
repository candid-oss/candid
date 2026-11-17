//! Bounded message-index projection and structured, live permission dependencies.
use super::core::*;
use crate::{TimelineStore, Result, TimelineError, agents::AgentRequest, db::{text,int,RowExt,placeholders}};
use serde_json::{Value,json};
impl TimelineStore {
    pub(crate) fn source_query(&self, req: &Value, client: &str) -> Result<Value> {
        self.audited("source_query",client,req, |_|None, |ctx| {
            if !ctx.scope.text { return Err(TimelineError::Denied("text_scope".into()).into()); }
            let action = req["action"].as_str().unwrap_or("candidates");
            if action == "reference" {
                let reference=req["reference"].as_str().unwrap_or("");
                let id=reference.strip_prefix("candid://event/").or_else(||reference.split("event_id: ").nth(1)).map(str::trim);
                let media=reference.split("media_id: ").nth(1).map(str::trim);
                let mut f=self.scope_filter(ctx,"e");
                if let Some(id)=id { f.add("e.event_id=?",vec![text(id)]); }
                else if let Some(id)=media { f.add("(json_extract(e.payload_json,'$.media_id')=? OR json_extract(e.payload_json,'$.hub_media_id')=? OR e.source_key=?)",vec![text(id),text(id),text(format!("media:{id}"))]); }
                else { return Err(TimelineError::InvalidInput("来源引用无效".into()).into()); }
                let rows=self.db.query(&format!("SELECT e.event_id,e.source_id,e.payload_json FROM events e WHERE {} LIMIT 2",f.sql()),&f.params,|r|(r.text(0),r.text(1),r.text(2)))?;
                if rows.len()!=1 {return Err(TimelineError::NotFound("来源".into()).into());}
                let (id,source,payload)=&rows[0]; let payload:Value=serde_json::from_str(payload).map_err(TimelineError::from)?;
                let mut refs=payload["source_links"]["refs"].as_array().cloned().unwrap_or_default();
                if payload["record_evidence"]["confirmation"]["basis"]=="source_send" {
                    let mut dep=self.scope_filter(ctx,"e");
                    dep.add("e.kind='ai.prompt' AND EXISTS (SELECT 1 FROM json_each(e.payload_json,'$.messages') m WHERE json_extract(m.value,'$.ref')=? AND json_extract(m.value,'$.source_hash')=?)",vec![text(payload["record_evidence"]["confirmation"]["message_id"].as_str().unwrap_or("")),text(payload["record_evidence"]["confirmation"]["source_hash"].as_str().unwrap_or(""))]);
                    let targets=self.db.query(&format!("SELECT e.event_id,e.source_id FROM events e WHERE {} LIMIT 2",dep.sql()),&dep.params,|r|json!({"event_id":r.text(0),"source_id":r.text(1)}))?;
                    if targets.len()!=1 {return Err(TimelineError::Denied("来源确认已失效".into()).into());}
                    refs.extend(targets);
                }
                refs.push(json!({"event_id":id,"source_id":source}));
                if refs.len()>32 {return Err(TimelineError::InvalidInput("来源引用过多".into()).into());}
                return Ok(json!({"format":"candid-source-links/1","refs":refs}));
            }
            if action!="candidates" {return Err(TimelineError::InvalidInput("来源查询无效".into()).into());}
            if req["sessions"].as_array().is_some_and(|s|s.len()>16 || s.iter().any(|v|v.as_str().is_none_or(|r|r.len()>256))) {return Err(TimelineError::InvalidInput("会话查询范围过大".into()).into());}
            let from=req["start_ms"].as_i64().ok_or_else(||TimelineError::InvalidInput("缺少消息时间".into()))?;
            let to=req["end_ms"].as_i64().ok_or_else(||TimelineError::InvalidInput("缺少消息时间".into()))?;
            if to<from || to-from>120_000 {return Err(TimelineError::InvalidInput("消息查询范围过大".into()).into());}
            let mut base=req.clone(); base.as_object_mut().unwrap().remove("cursor");
            let revisions=self.db.scalar("SELECT COALESCE(MAX(rowid),0) FROM agent_event_revisions",&[])?;
            let fp=request_fingerprint(&json!({"request":base,"revisions":revisions}));
            let cursor=req["cursor"].as_str().map(|c|decode_cursor(c,"source_query",&fp)).transpose()?;
            let snap=cursor.as_ref().map(|c|c.snap).unwrap_or(self.current_seq()?);
            let before=cursor.as_ref().and_then(|c|c.off).unwrap_or(snap+1);
            if self.db.scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='agent_message_time_scan'",&[])?!=1 {return Ok(json!({"messages":[],"next_cursor":null,"incomplete":true,"scanned":0}));}
            let last_time=cursor.as_ref().and_then(|c|c.t).unwrap_or(i64::MAX);
            let scan=self.db.query("SELECT ingest_seq,event_id,json_extract(payload_json,'$.last_message_ms') FROM events INDEXED BY agent_message_time_scan WHERE kind='ai.prompt' AND json_extract(payload_json,'$.last_message_ms')>=? AND ingest_seq<=? AND (json_extract(payload_json,'$.last_message_ms')<? OR (json_extract(payload_json,'$.last_message_ms')=? AND ingest_seq<?)) ORDER BY json_extract(payload_json,'$.last_message_ms') DESC,ingest_seq DESC LIMIT 129",&[int(from),int(snap),int(last_time),int(last_time),int(before)],|r|(r.int(0),r.text(1),r.int(2)))?;
            let mut f=self.scope_filter(ctx,"e");
            f.add("e.kind='ai.prompt' AND e.ingest_seq<=?",vec![int(snap)]);
            if scan.is_empty() {return Ok(json!({"messages":[],"next_cursor":null,"incomplete":false,"scanned":0}));}
            f.add(format!("e.event_id IN ({})",placeholders(scan.len().min(128))),scan.iter().take(128).map(|(_,id,_)|text(id)).collect());
            // Scan bounded indexed rows, not event start time: followups can revise old prompts.
            let rows=self.db.query(&format!("SELECT e.ingest_seq,substr(e.payload_json,1,1048576),length(e.payload_json) FROM events e WHERE {} ORDER BY e.ingest_seq DESC LIMIT 129",f.sql()),&f.params,|r|(r.int(0),r.text(1),r.int(2)))?;
            let started=std::time::Instant::now();
            let mut messages=vec![]; let mut incomplete=false;
            for source in self.sources()?.into_iter().filter(|s|ctx.allowed_sources.contains(&s.source_id) && s.kind=="agent.transcript") {
                incomplete |= source.status=="error";
            }
 let mut seen_sessions=std::collections::BTreeSet::new();
            for (_,body,size) in rows.iter().take(128) {
                if started.elapsed().as_millis()>750 {incomplete=true;break;}
                if *size>1048576 { incomplete=true; continue; }
                let p:Value=serde_json::from_str(body).map_err(TimelineError::from)?;
                let session=p["session_ref"].as_str().unwrap_or("");
                if req["sessions"].as_array().is_some_and(|s|!s.is_empty() && !s.iter().any(|v|v.as_str()==Some(session))) {continue;}
                let Some(refs)=p["messages"].as_array() else {continue;};
                if refs.len()>256 {incomplete=true;continue;}
                for m in refs {
                    let Some(at)=m["at_ms"].as_i64() else {continue;};
                    if at<from || at>=to || m["truncated"]!=false {continue;}
                    let Some(_reference)=m["ref"].as_str() else {continue;};
                    let agent=session.split(':').nth(1).unwrap_or("");
                    let consent=self.agent_consent(agent)?;
                    if !consent.activity {continue;}
                    if seen_sessions.contains(session) {continue;}
                    if seen_sessions.len()>=8 || started.elapsed().as_millis()>750 {incomplete=true;break;}
                    seen_sessions.insert(session.to_owned());
                    // Re-read the whole bounded session page so an unsynced same-text
                    // send cannot make an indexed message look unique.
                    let read=self.agent_call(&AgentRequest{agent:agent.into(),action:"read".into(),reference:Some(session.into()),start_ms:Some(from),end_ms:Some(to),limit:Some(500),deadline_ms:Some(200),..Default::default()},client);
                    let Ok(read)=read else {incomplete=true;continue;};
                    if !read["nextCursor"].is_null() {incomplete=true;continue;}
                    let sid=self.agent_source_id(agent)?.unwrap_or_default();
                    let mut indexed=self.scope_filter(ctx,"e");
                    indexed.add("e.kind='ai.prompt' AND e.thread_id=(SELECT thread_id FROM threads WHERE source_id=? AND source_key=?)",vec![text(&sid),text(session)]);
                    let index_rows=self.db.query(&format!("SELECT substr(e.payload_json,1,1048576),length(e.payload_json) FROM events e WHERE {} LIMIT 513",indexed.sql()),&indexed.params,|r|(r.text(0),r.int(1)))?;
                    if index_rows.len()>512 {incomplete=true;continue;}
                    let mut indexed_refs=std::collections::BTreeMap::new();
                    for (body,bytes) in index_rows {
                        if started.elapsed().as_millis()>750 {incomplete=true;break;}
                        if bytes>1048576 {incomplete=true;continue;}
                        let payload:Value=serde_json::from_str(&body).map_err(TimelineError::from)?;
                        for m in payload["messages"].as_array().into_iter().flatten().take(512) {
                            if let (Some(r),Some(h))=(m["ref"].as_str(),m["source_hash"].as_str()) {indexed_refs.insert(r.to_owned(),h.to_owned());}
                        }
                    }
                    for msg in read["messages"].as_array().into_iter().flatten() {
                        let Some(actual_at)=msg["tsMs"].as_i64() else {continue;};
                        if actual_at<from || actual_at>=to || msg["role"]!="user" {continue;}
                        if msg["truncated"]!=false || messages.len()>=32 {incomplete=true;continue;}
                        let actual_ref=msg["ref"].as_str().unwrap_or("");
                        if indexed_refs.get(actual_ref).map(String::as_str)!=msg["source_hash"].as_str() {incomplete=true;continue;}
                        messages.push(json!({"session":session,"reference":actual_ref,"at_ms":actual_at,"text":msg["text"],"source_hash":msg["source_hash"]}));
                    }
                }
            }
            incomplete |= self.db.scalar("SELECT COALESCE(MAX(rowid),0) FROM agent_event_revisions",&[])?!=revisions;
            let next=if scan.len()>128 {let mut c=PageCursor::new("source_query",snap,&fp);c.off=Some(scan[127].0);c.t=Some(scan[127].2);Some(encode_cursor(&c))}else{None};
            let mut result=json!({"messages":messages,"next_cursor":next,"incomplete":incomplete,"scanned":scan.len().min(128)});
            if crate::json::canonical(&result).len() as i64>ctx.scope.max_result_bytes {result["messages"]=json!([]);result["incomplete"]=json!(true);}
            Ok(result)
        })
    }
}
