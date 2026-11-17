use super::*;
use crate::{
    db::{text, RowExt},
    TimelineStore,
};
use base64::Engine;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::time::{Instant,Duration};
use unicode_normalization::UnicodeNormalization;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentRequest {
    pub agent: String,
    pub action: String,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub start_ms: Option<i64>,
    #[serde(default)]
    pub end_ms: Option<i64>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub deadline_ms: Option<u64>,
}
fn in_range(ts: i64, a: Option<i64>, b: Option<i64>) -> bool {
    a.is_none_or(|a| ts >= a) && b.is_none_or(|b| ts < b)
}
fn project_matches(s: &Session, r: &AgentRequest) -> bool {
    r.project
        .as_deref()
        .is_none_or(|p| s.project.as_deref().is_some_and(|s| below(s, p)))
}
fn visible(e: &Entry, c: &AgentConsent) -> bool {
    match e.kind.as_str() {
        "message" | "candidateUser" => {
            (e.role == "user" && c.content.user) || (e.role == "assistant" && c.content.assistant)
        }
        "reasoning" => c.content.reasoning,
        "toolCall" => c.content.tool_calls == "name_only",
        "toolOutput" => c.content.tool_output,
        _ => false,
    }
}
fn message(s: &Session, e: &Entry, c: &AgentConsent) -> Value {
    let max = if e.kind == "toolOutput" { 4000 } else { 131_072 };
    let t = e.text.as_deref().map(redact);
    let truncated = t.as_ref().is_some_and(|t| t.chars().count() > max);
    json!({"ref":format!("{}#{}",s.reference(&c.agent),e.id),"tsMs":e.ts,"tzOffsetMin":e.offset,"role":e.role,"text":t.map(|t|t.chars().take(max).collect::<String>()),"toolName":e.tool,"reasoning":e.kind=="reasoning","truncated":truncated,"layer":"external","stored":false,"source_hash":e.source_hash})
}
fn totals(s: &Session, start: Option<i64>, end: Option<i64>) -> Usage {
    let mut u = Usage::default();
    let mut keyed = std::collections::BTreeMap::new();
    for e in &s.entries {
        if let Some(v) = &e.usage {
            if let Some(k) = &e.usage_key {
                keyed.insert(k, (e.ts, v));
            } else if in_range(e.ts, start, end) {
                u.add(v);
            }
        }
    }
    for (ts, v) in keyed.values() {
        if in_range(*ts, start, end) {
            u.add(v);
        }
    }
    u
}
impl TimelineStore {
    /// The App's source administration entry; external transports never route here.
    pub fn agent_settings(&self) -> Result<Value> {
        let mut counts = serde_json::Map::new();
        for agent in AGENTS {
            counts.insert(
                agent.into(),
                json!(self.db.scalar(
                    "SELECT COUNT(*) FROM events WHERE source_id=?",
                    &[text(self.agent_source_id(agent)?.unwrap_or_default())]
                )?),
            );
        }
        Ok(
            json!({"consents":AGENTS.iter().map(|a|self.agent_consent(a)).collect::<Result<Vec<_>>>()?,"stored_counts":counts}),
        )
    }
    pub fn agent_call(&self, req: &AgentRequest, client: &str) -> Result<Value> {
        self.audited("agent",client,req,|_|None,|ctx|{
            let internal=self.db.first("SELECT transport FROM clients WHERE client_id=?",&[text(client)],|r|r.text(0))?.as_deref()==Some("internal");
            if !AGENTS.contains(&req.agent.as_str()){return Err(failure("unsupported_agent").into());}
            if !internal && !ctx.scope.agents.contains(&req.agent) {return Err(TimelineError::Denied("agent_scope".into()).into());}
            let c=self.agent_consent(&req.agent)?;
            c.validate()?;
            if req.action=="describe"{return Ok(description(&req.agent));}
            if req.action=="detect"{
                let exists=c.roots().iter().any(|p|p.is_dir());
                // No listing or file open without read consent.
                let mut state=if exists{"found"}else{"notFound"};let mut inspected=false;let mut version=None;
                if exists&&c.read {let fs=files(&c)?;for f in fs.iter().take(3){inspected=true;if formats::detect(f,&c).is_ok(){version=Some(description(&req.agent)["formatVersions"][0].clone());}else{state="unknownFormat";}}}
                return Ok(json!({"agent":req.agent,"state":state,"formatVersion":version,"headInspected":inspected}));
            }
            c.require_read()?;
            if !ctx.scope.text{return Err(TimelineError::Denied("text_scope".into()).into());}
            let source_id=self.agent_source_id(&req.agent)?;
            if self.db.scalar("SELECT COUNT(*) FROM sources WHERE source_id=? AND status='revoked'",&[text(source_id.clone().unwrap_or_default())])?>0 {return Err(TimelineError::Denied("source_revoked".into()).into());}
            if !internal && !ctx.scope.sources.iter().any(|x| x=="*"||Some(x)==source_id.as_ref()){return Err(TimelineError::Denied("source_scope".into()).into());}
            if !ctx.scope.kinds.iter().any(|x|x=="*"||x=="ai.prompt"||x=="ai.work"){return Err(TimelineError::Denied("kind_scope".into()).into());}
            if !ctx.scope.apps.allow.contains(&"*".to_string()){return Err(TimelineError::Denied("app_scope".into()).into());}
            if req.action=="search"&&!c.search{return Err(TimelineError::Denied("search_required".into()).into());}
            let mut r=req.clone();
            if let Some(lower)=self.scope_lower_bound(&ctx.scope){r.start_ms=Some(r.start_ms.unwrap_or(lower).max(lower));}
            if let Some(upper)=ctx.scope.time.to{r.end_ms=Some(r.end_ms.unwrap_or(upper).min(upper));}
            let limit=r.limit.unwrap_or(100).clamp(1,500);
            let deadline=r.deadline_ms.unwrap_or(1000).min(10_000);let started=Instant::now();
            let prefix=format!("agent:{}:",c.agent);
            let ref_parts=if let Some(reference)=&r.reference {
                let rest=reference.strip_prefix(&prefix).ok_or_else(||failure("invalid_ref"))?;
                if rest.is_empty()||rest.contains('/')||rest.contains("..") {return Err(failure("invalid_ref").into());}
                Some(rest.split_once('#').unwrap_or((rest,"")))
            }else{None};
            if r.action=="read" {
                if let Some((key,anchor))=ref_parts.filter(|(_,a)|!a.is_empty()) {
                    if let Some(found)=self.agent_read_at(&c,&r,key,anchor,limit,started+Duration::from_millis(deadline),ctx.scope.max_result_bytes)? {return Ok(found);}
                }
            }
            let mut sessions=vec![];let mut hits=vec![];let mut issues=vec![];let mut loaded=std::collections::BTreeMap::<String,Session>::new();let mut scanned=0;let mut limited=false;let mut usage=Usage::default();let mut usage_sessions=0;
            let (paths, enumeration_limited) = file_page(&c)?;
            limited |= enumeration_limited;
            let mut source_bytes = 0u64;
            let sid=source_id.clone().unwrap_or_default();let mut file_index=0;
            for path in paths {
                // A read names its session: a file fully read in earlier that belongs to another session is not
                // opened (nor counted against the budget). New or changed files still are.
                if let Some((key,_))=ref_parts {
                    let len=fs::metadata(&path).map(|m|m.len()).ok();
                    let other=self.cursor(&sid,&format!("file:{}",path.to_string_lossy()))?.and_then(|s|serde_json::from_str::<Session>(&s).ok()).is_some_and(|f|!f.key.is_empty()&&f.key!=key&&Some(f.offset)==len);
                    if other {continue;}
                }
                file_index+=1;
                source_bytes += fs::metadata(&path).map(|m|m.len()).unwrap_or(0);
                if file_index > 64 || source_bytes > 16*1024*1024 { limited=true; break; }
                if started.elapsed().as_millis()>=deadline as u128 {limited=true;break;}
                let session=match formats::load_until(&path,&c,None,false,Some(started+Duration::from_millis(deadline))){Ok(s)=>s,Err(_)=>{limited=true; issues.push(json!({"sessionKey":path.file_stem().unwrap_or_default().to_string_lossy(),"code":"invalid_session","line":null}));continue;}};
                if session.limited {limited=true;}
                if session.key.is_empty()||c.excluded(&session.key,session.project.as_deref())||!project_matches(&session,&r){continue;}
                if let Some((key,_))=ref_parts{if session.key!=key{continue;}}
                loaded.entry(session.key.clone()).and_modify(|s| s.merge_fragment(&session)).or_insert(session);
            }
            let mut ordered:Vec<_> = loaded.into_values().collect();
            ordered.sort_by_key(|s| std::cmp::Reverse(s.entries.first().map(|e| e.ts)));
            for session in ordered {
                limited |= session.limited;
                let a=session.entries.iter().map(|e|e.ts).min();let b=session.entries.iter().map(|e|e.ts).max();
                if a.is_some_and(|a|r.end_ms.is_some_and(|end|a>=end))||b.is_some_and(|b|r.start_ms.is_some_and(|start|b<start)){continue;}
                scanned+=1;
                match req.action.as_str(){
                    "read"=>{
                        let Some((_,anchor))=ref_parts else{return Err(failure("invalid_ref").into());};
                        let fingerprint=crate::ids::sha256_hex(format!("{}:{}:{}",session.reference(&c.agent),session.prefix_hash,crate::json::canonical_of(&c)));
                        let mut index=0;
                        if let Some(cursor)=&r.cursor {
                            let bytes=base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cursor).map_err(|_|failure("invalid_cursor"))?;
                            let v:Value=serde_json::from_slice(&bytes).map_err(|_|failure("invalid_cursor"))?;
                            if v["fingerprint"]!=fingerprint{return Err(failure("stale_cursor").into());}index=v["index"].as_u64().ok_or_else(||failure("invalid_cursor"))? as usize;
                        }else if !anchor.is_empty(){index=session.entries.iter().position(|e|e.id==anchor).ok_or_else(||failure("message_not_found"))?;}
                        let mut messages=vec![];let mut next=None;
                        for (i,e) in session.entries.iter().enumerate().skip(index){if visible(e,&c)&&in_range(e.ts,r.start_ms,r.end_ms){if messages.len()==limit{next=Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::json::canonical(&json!({"fingerprint":fingerprint,"index":i}))));break;}messages.push(message(&session,e,&c));}}
                        if limited { return Err(failure("source_scan_incomplete").into()); }
                        let response=json!({"ref":session.reference(&c.agent),"messages":messages,"nextCursor":next,"layer":"external","stored":false});
                        if crate::json::canonical(&response).len() as i64>ctx.scope.max_result_bytes{return Err(TimelineError::Denied("result_too_large".into()).into());}
                        return Ok(response);
                    },
                    "sessions"=>{
                        if sessions.len()>=limit{limited=true;break;}
                        sessions.push(json!({"ref":session.reference(&c.agent),"agent":c.agent,"key":session.key,"startMs":a,"endMs":b,"project":session.project,"title":session.title.as_deref().map(redact),"model":session.model,"turns":session.entries.iter().filter(|e|["message","candidateUser"].contains(&e.kind.as_str())).count(),"usage":totals(&session,r.start_ms,r.end_ms),"layer":"external","stored":false}));
                    },
                    "usage"=>{usage.add(&totals(&session,r.start_ms,r.end_ms));usage_sessions+=1;},
                    "search"=>{
                        let q=fold(r.query.as_deref().unwrap_or("").trim());
                        if !q.is_empty(){for e in &session.entries{if !in_range(e.ts,r.start_ms,r.end_ms)||!["message","candidateUser"].contains(&e.kind.as_str())||!visible(e,&c){continue;}
                            let m=message(&session,e,&c);let t=m["text"].as_str().unwrap_or("");
                            if fold(t).contains(&q){hits.push(json!({"ref":m["ref"],"tsMs":e.ts,"role":e.role,"snippet":snippet(t,&q),"project":session.project,"layer":"external","stored":false}));if hits.len()>=limit{limited=true;break;}}
                        }}
                    },
                    _=>return Err(failure("unknown_agent_action").into()),
                }
                if limited{break;}
            }
            sessions.sort_by_key(|s|std::cmp::Reverse(s["startMs"].as_i64().unwrap_or(0)));hits.sort_by_key(|s|std::cmp::Reverse(s["tsMs"].as_i64().unwrap_or(0)));
            let result=match req.action.as_str(){"sessions"=>json!({"sessions":sessions,"issues":issues,"limited":limited,"layer":"external","stored":false}),"usage"=>json!({"agent":c.agent,"totals":usage,"sessions":usage_sessions,"issues":issues,"layer":"external","stored":false}),"search"=>json!({"hits":hits,"limited":limited,"scannedSessions":scanned,"issues":issues,"layer":"external","stored":false}),"read"=>return Err(TimelineError::NotFound("reply_source_missing".into()).into()),_=>return Err(failure("unknown_agent_action").into())};
            if crate::json::canonical(&result).len() as i64>ctx.scope.max_result_bytes{return Err(TimelineError::Denied("result_too_large".into()).into());}Ok(result)
        })
    }
}

/// Bytes of source read per page before handing back a cursor; the next page resumes at the same byte.
const READ_PAGE_BYTES: u64 = 8 * 1024 * 1024;
impl TimelineStore {
    /// Reads a session forward from one message instead of parsing the whole file: the stored session history
    /// says which file and line each message came from, so the reader skips to that line (no parsing), checks it
    /// is still the same line, and parses only what follows. Pages resume at a byte offset, so a long session costs
    /// the same as a short one. None means the history doesn't place this message; the caller then falls back to
    /// the whole-file read.
    fn agent_read_at(&self, c: &AgentConsent, r: &AgentRequest, key: &str, anchor: &str, limit: usize, until: Instant, max_bytes: i64) -> Result<Option<Value>> {
        let Some(sid) = self.agent_source_id(&c.agent)? else { return Ok(None) };
        let Some(session) = self.load_agent_session(&sid, &format!("session:{key}"))? else { return Ok(None) };
        if c.excluded(&session.key, session.project.as_deref()) || !project_matches(&session, r) {
            return Err(TimelineError::NotFound("reply_source_missing".into()));
        }
        // A session split across files (Codex rotates long ones) is merged by the whole read instead.
        if session.entries.iter().any(|e| e.source_file != session.entries[0].source_file) { return Ok(None); }
        let Some(at) = session.entries.iter().find(|e| e.id == anchor) else { return Ok(None) };
        let path = PathBuf::from(&at.source_file);
        // Only a file still under a consented folder is opened.
        if at.source_file.is_empty() || at.source_file.contains("/../") || at.source_line == 0
            || !c.roots().iter().any(|root| below(&at.source_file, &root.to_string_lossy())) {
            return Ok(None);
        }
        let fingerprint = crate::ids::sha256_hex(format!("{}#{anchor}:{}:{}", session.reference(&c.agent), at.source_file, crate::json::canonical_of(c)));
        let encode = |offset: u64, line: usize| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::json::canonical(&json!({"fingerprint":fingerprint,"offset":offset,"line":line})));
        let mut file = fs::File::open(&path).map_err(|_| failure("unreadable"))?;
        let size = file.metadata()?.len();
        let continuing = r.cursor.is_some();
        let (mut offset, mut line_no) = if let Some(cursor) = &r.cursor {
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cursor).map_err(|_| failure("invalid_cursor"))?;
            let v: Value = serde_json::from_slice(&bytes).map_err(|_| failure("invalid_cursor"))?;
            // A cursor from the whole-file read keeps going that way.
            let (Some(offset), Some(line)) = (v["offset"].as_u64(), v["line"].as_u64()) else { return Ok(None) };
            if v["fingerprint"] != fingerprint || offset > size { return Err(failure("stale_cursor").into()); }
            (offset, line as usize)
        } else {
            // Counting newlines is cheap even for a long file; nothing before the message is parsed.
            let mut reader = BufReader::with_capacity(1 << 20, &mut file);
            let (mut pos, mut n) = (0u64, 0usize);
            while n + 1 < at.source_line {
                let skipped = reader.skip_until(b'\n')?;
                if skipped == 0 { return Ok(None); }
                pos += skipped as u64; n += 1;
            }
            (pos, n)
        };
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file);
        let mut st = Session { key: session.key.clone(), project: session.project.clone(), agent_thread: session.agent_thread, ..Default::default() };
        let (mut messages, mut next, mut read, mut size_so_far) = (vec![], None, 0u64, 0usize);
        let mut first = !continuing;
        loop {
            if (!messages.is_empty() && (messages.len() >= limit || size_so_far as i64 >= max_bytes / 2))
                || (!first && (read >= READ_PAGE_BYTES || Instant::now() >= until)) {
                next = Some(encode(offset, line_no));
                break;
            }
            let mut line = Vec::new();
            let n = reader.by_ref().take(16 * 1024 * 1024 + 1).read_until(b'\n', &mut line)?;
            if n > 16 * 1024 * 1024 { return Err(failure("line_too_large").into()); }
            if n == 0 || !line.ends_with(b"\n") { break; }
            offset += n as u64; line_no += 1; read += n as u64;
            if line.iter().all(u8::is_ascii_whitespace) { continue; }
            let v: Value = serde_json::from_slice(&line).map_err(|_| failure("malformed_json"))?;
            if first {
                first = false;
                // The file was rewritten since it was read in: fall back rather than show the wrong message.
                if crate::ids::sha256_hex(crate::json::canonical(&v)) != at.source_hash { return Ok(None); }
            }
            let mut entries = formats::parse(&c.agent, &v, &mut st)?;
            if c.agent == "codex" {
                entries.retain(|e| e.kind != "candidateUser");
                for e in &mut entries { if e.kind == "humanMessage" { e.kind = "message".into(); } }
            }
            for mut e in entries {
                if line_no == at.source_line { e.id = anchor.into(); }
                if visible(&e, c) && in_range(e.ts, r.start_ms, r.end_ms) {
                    let m = message(&session, &e, c);
                    size_so_far += m["text"].as_str().map_or(0, str::len) + 256;
                    messages.push(m);
                }
            }
        }
        let response = json!({"ref":session.reference(&c.agent),"messages":messages,"nextCursor":next,"layer":"external","stored":false});
        if crate::json::canonical(&response).len() as i64 > max_bytes { return Err(TimelineError::Denied("result_too_large".into()).into()); }
        Ok(Some(response))
    }
}

impl TimelineStore {
    pub(crate) fn search_with_agents(
        &self,
        body: &[u8],
        client: &str,
    ) -> std::result::Result<Value, crate::ToolCallFailure> {
        let req: crate::SearchRequest =
            serde_json::from_slice(body).map_err(crate::ToolCallFailure::Decode)?;
        let mut result = serde_json::to_value(self.search(&req, client)?)
            .map_err(|e| failure(&e.to_string()))?;
        let Some((ctx, _)) = self.load_client(client)? else {
            return Err(TimelineError::UnknownClient(client.into()).into());
        };
        if req.cursor.is_some()
            || !ctx.scope.text
            || req.q.as_deref().unwrap_or("").trim().is_empty()
            || req.apps.as_ref().is_some_and(|a| !a.is_empty())
        {
            return Ok(result);
        }
        let (a, b) = self.window(req.start.as_deref(), req.end.as_deref(), None, false)?;
        let internal = self
            .db
            .first(
                "SELECT transport FROM clients WHERE client_id=?",
                &[text(client)],
                |r| r.text(0),
            )?
            .as_deref()
            == Some("internal");
        for agent in AGENTS {
            if !internal && !ctx.scope.agents.contains(&agent.to_string()) {
                continue;
            }
            let c = self.agent_consent(agent)?;
            if !c.read || !c.search {
                continue;
            }
            let source_id=self.agent_source_id(agent)?;
            if req.sources.as_ref().is_some_and(|s| {
                !s.is_empty()
                    && !s
                        .iter()
                        .any(|x| x == "agent.transcript" || Some(x) == source_id.as_ref())
            }) {
                continue;
            }
            if req
                .kinds
                .as_ref()
                .is_some_and(|k| !k.is_empty() && !k.iter().any(|x| x == "ai.prompt"))
            {
                continue;
            }
            let request = AgentRequest {
                agent: agent.into(),
                action: "search".into(),
                query: req.q.clone(),
                start_ms: a,
                end_ms: b,
                limit: Some(req.limit.unwrap_or(10).clamp(1, 50) as usize),
                ..Default::default()
            };
            let external = match self.agent_call(&request, client) {
                Ok(v) => v,
                Err(TimelineError::Denied(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            if let Some(hits) = external["hits"].as_array() {
                for h in hits {
                    if h["role"] != "user" && (req.kinds.as_ref().is_some_and(|k|k.contains(&"ai.prompt".to_string())) || req.fields.as_ref().is_some_and(|f|!f.is_empty()&&f.iter().all(|f|f=="prompt"))) {continue;}
                    let reference = h["ref"].as_str().unwrap_or("");
                    if h["role"]=="user"&&self.db.scalar("SELECT COUNT(*) FROM events e,json_each(e.payload_json,'$.messages') m WHERE e.source_id=? AND json_extract(m.value,'$.ref')=?",&[text(self.agent_source_id(agent)?.unwrap_or_default()),text(reference)])?>0 {continue;}
                    let hit = json!({"event_id":reference,"kind":"agent.message","start":self.iso(h["tsMs"].as_i64()),"source_label":format!("ai·{agent}"),"layer":"external","stored":false,"snippet":{"text":h["snippet"],"highlights":[],"truncated":true},"score":1.0,"ref":{"event_id":reference},"role":h["role"]});
                    if let Some(hits) = result["hits"].as_array_mut() {
                        hits.push(hit);
                    }
                }
            }
            if external["limited"] == true {
                result["scan_limited"] = json!(true);
            }
        }
        if let Some(hits)=result["hits"].as_array_mut(){let limit=req.limit.unwrap_or(10).clamp(1,50) as usize;if hits.len()>limit{hits.truncate(limit);result["scan_limited"]=json!(true);}}
        while crate::json::canonical(&result).len() as i64 > ctx.scope.max_result_bytes {
            let hits = result["hits"].as_array_mut().unwrap();
            if hits.is_empty() {
                break;
            }
            hits.pop();
            result["truncated_by_size"] = json!(true);
        }
        Ok(result)
    }
}

fn fold(s:&str)->String {s.nfkd().filter(|c|!unicode_normalization::char::is_combining_mark(*c)).flat_map(char::to_lowercase).collect()}
fn snippet(text:&str,query:&str)->String {
    let mut folded=String::new();let mut positions=vec![];
    for (i,c) in text.chars().enumerate(){for c in fold(&c.to_string()).chars(){folded.push(c);positions.push(i);}}
    let at=folded.find(query).map(|byte|folded[..byte].chars().count()).and_then(|i|positions.get(i)).copied().unwrap_or(0);
    text.chars().skip(at.saturating_sub(80)).take(query.chars().count()+160).collect()
}
