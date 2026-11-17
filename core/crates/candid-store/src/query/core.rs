use super::types::*;
use crate::db::{int, opt_int, opt_text, placeholders, text, RowExt, P};
use crate::ids::{sha256_hex, uuid_v7};
use crate::json;
use crate::store::TimelineStore;
use crate::types::ClientScope;
use crate::{Result, TimelineError};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Instant;

/// Raised inside a tool body when the target exists but the client's scope hides it. The caller only ever sees
/// `NotFound`, so existence does not leak; the audit log records `out_of_scope`.
pub(crate) enum QErr {
    ScopeMiss(String),
    Err(TimelineError),
}

impl From<TimelineError> for QErr {
    fn from(e: TimelineError) -> Self {
        QErr::Err(e)
    }
}

pub(crate) type QResult<T> = std::result::Result<T, QErr>;

pub(crate) struct ClientContext {
    pub id: String,
    pub scope: ClientScope,
    /// Source IDs this client may read, after scope and the per-source `agent_visible` default.
    pub allowed_sources: Vec<String>,
}

#[derive(Clone, Default)]
pub(crate) struct EventFilter {
    pub clauses: Vec<String>,
    pub params: Vec<P>,
}

impl EventFilter {
    pub fn add(&mut self, clause: impl Into<String>, values: Vec<P>) {
        self.clauses.push(clause.into());
        self.params.extend(values);
    }
    pub fn sql(&self) -> String {
        if self.clauses.is_empty() {
            "1=1".into()
        } else {
            self.clauses.join(" AND ")
        }
    }
    pub fn with(&self, extra: Vec<P>) -> Vec<P> {
        let mut p = self.params.clone();
        p.extend(extra);
        p
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PageCursor {
    pub v: i64,
    pub tool: String,
    pub snap: i64,
    pub fp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub we: Option<i64>,
}

impl PageCursor {
    pub fn new(tool: &str, snap: i64, fp: &str) -> Self {
        PageCursor { v: 1, tool: tool.into(), snap, fp: fp.into(), off: None, t: None, id: None, ws: None, we: None }
    }
}

pub(crate) const SORT_KEY: &str = "COALESCE(e.start_ms, e.ingested_ms)";

pub(crate) fn clamp(v: i64, lo: i64, hi: i64) -> i64 {
    v.max(lo).min(hi)
}

/// Lower-cases ASCII only, matching SQLite's built-in `lower()`.
pub(crate) fn ascii_lower(s: &str) -> String {
    s.chars().map(|c| c.to_ascii_lowercase()).collect()
}

pub(crate) fn truncate(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        None => (text.to_string(), false),
        Some((i, _)) => (text[..i].to_string(), true),
    }
}

pub(crate) struct TextRow {
    pub event_id: String,
    pub field: String,
    pub ord: i64,
    pub text: String,
    pub visible: Option<bool>,
    pub focus_distance: Option<i64>,
    pub is_new: bool,
    pub char_start: Option<i64>,
}

pub(crate) const SUMMARY_COLUMNS: &str =
    "e.event_id,e.kind,e.start_ms,e.end_ms,e.time_basis,e.app_bundle_id,e.app_name,e.title,e.source_id,s.kind,e.thread_id,e.sensitivity,k.layer,e.payload_json";
pub(crate) const SUMMARY_FROM: &str = "FROM events e JOIN sources s ON s.source_id=e.source_id JOIN event_kinds k ON k.kind=e.kind";

pub(crate) const CAPTURE_KINDS: &[&str] = &["mac.desktop", "mac.microphone", "mac.system_audio", "mac.region", "device.pendant", "phone.android"];

pub(crate) fn source_label(kind: &str, source_kind: &str, payload: Option<&Value>) -> String {
    match kind {
        "screen.text" => "screen·ax".into(),
        "screen.shot" => "screen·shot".into(),
        "app.focus" | "window.focus" => "screen·focus".into(),
        "image.text" => match payload.and_then(|p| json::str_at(p, "method")) {
            Some("ocr") => "screen·ocr".into(),
            Some("vlm") => "screen·vlm".into(),
            _ => "screen·ax".into(),
        },
        "speech.segment" => "audio·asr".into(),
        "audio.recording" => "audio".into(),
        "image.capture" => "image".into(),
        "calendar.event" => "calendar".into(),
        "reminder.item" => "reminders".into(),
        "mail.message" => "mail".into(),
        "ai.prompt" | "ai.work" => if payload.is_some_and(|p|p["record_evidence"]["confirmation"].is_object()) { "ai·screen".into() } else { format!("ai·{}", payload.and_then(|p| p.get("agent")).and_then(Value::as_str).unwrap_or("unknown")) },
        "ai.turn" => match source_kind {
            "ai.codex" => "ai·codex".into(),
            "ai.claude_code" => "ai·claude".into(),
            _ => "ai".into(),
        },
        "system.state" => "system".into(),
        "coverage.gap" => "coverage".into(),
        other => other.into(),
    }
}

pub(crate) fn encode_cursor(c: &PageCursor) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json::canonical_of(c))
}

pub(crate) fn decode_cursor(text_: &str, tool: &str, fp: &str) -> Result<PageCursor> {
    let mut s = text_.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    let data = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).map_err(|_| TimelineError::InvalidCursor)?;
    let c: PageCursor = serde_json::from_slice(&data).map_err(|_| TimelineError::InvalidCursor)?;
    if c.v != 1 || c.tool != tool || c.fp != fp {
        return Err(TimelineError::InvalidCursor);
    }
    Ok(c)
}

pub(crate) fn request_fingerprint<R: Serialize>(request: &R) -> String {
    sha256_hex(json::canonical_of(request))[..16].to_string()
}

pub(crate) fn encoded_len<R: Serialize>(r: &R) -> usize {
    json::canonical_of(r).len()
}

impl TimelineStore {
    // MARK: Client + scope

    pub(crate) fn load_client(&self, id: &str) -> Result<Option<(ClientContext, bool)>> {
        let Some((scope_json, revoked, transport)) =
            self.db.first("SELECT scope_json, revoked_ms, transport FROM clients WHERE client_id=?", &[text(id)], |r| (r.text(0), r.opt_int(1), r.text(2)))?
        else {
            return Ok(None);
        };
        let scope: ClientScope =
            serde_json::from_str(&scope_json).map_err(|e| TimelineError::Database(format!("scope_json: {e}")))?;
        let star = scope.sources.iter().any(|s| s == "*");
        let mut allowed = Vec::new();
        for s in self.sources()? {
            if s.status=="revoked" {continue;}
            if s.kind == "agent.transcript" {
                let Some(agent) = self.source_agent(&s.source_id)? else {continue;};
                let c = self.agent_consent(&agent)?;
                if c.version!=1 || c.paths.len()>32 || !c.read || !c.content.user || (transport != "internal" && !scope.agents.iter().any(|a| a == &agent)) { continue; }
            }
            let explicit = !star && scope.sources.contains(&s.source_id);
            if !(star || explicit) {
                continue;
            }
            // `agent_visible=false`: hidden from external clients unless granted by source id; the App itself sees it.
            if s.policy.agent_visible || explicit || transport == "internal" {
                allowed.push(s.source_id);
            }
        }
        Ok(Some((ClientContext { id: id.into(), scope, allowed_sources: allowed }, revoked.is_some())))
    }

    pub(crate) fn scope_lower_bound(&self, scope: &ClientScope) -> Option<i64> {
        let mut lower = scope.time.from;
        if let Some(days) = scope.time.max_days_back {
            let floor = self.clock.now_ms() - days * 86_400_000;
            lower = Some(lower.unwrap_or(floor).max(floor));
        }
        lower
    }

    pub(crate) fn scope_filter(&self, ctx: &ClientContext, e: &str) -> EventFilter {
        let mut f = EventFilter::default();
        if ctx.allowed_sources.is_empty() {
            f.add("0", vec![]);
        } else {
            f.add(format!("{e}.source_id IN ({})", placeholders(ctx.allowed_sources.len())), ctx.allowed_sources.iter().map(text).collect());
        }
        // A reclassified legacy mirror keeps the intersection with its previous source grant.
        if ctx.allowed_sources.is_empty(){f.add("json_extract(e.payload_json,'$.native_legacy_source_id') IS NULL",vec![]);}else{f.add(format!("(json_extract({e}.payload_json,'$.native_legacy_source_id') IS NULL OR json_extract({e}.payload_json,'$.native_legacy_source_id') IN ({}))",placeholders(ctx.allowed_sources.len())),ctx.allowed_sources.iter().map(text).collect());}
        if ctx.allowed_sources.is_empty(){f.add("NOT EXISTS (SELECT 1 FROM json_each(e.payload_json,'$.native_legacy_sources'))",vec![]);}else{f.add(format!("NOT EXISTS (SELECT 1 FROM json_each({e}.payload_json,'$.native_legacy_sources') legacy WHERE legacy.value NOT IN ({}))",placeholders(ctx.allowed_sources.len())),ctx.allowed_sources.iter().map(text).collect());}
        // Filters also cover data retained after narrowing consent; the index cannot bypass them.
        f.add(format!("NOT EXISTS (SELECT 1 FROM sources a WHERE a.source_id={e}.source_id AND a.kind='agent.transcript' AND (EXISTS (SELECT 1 FROM json_each(a.policy_json,'$.exclude.sessions') x WHERE x.value=json_extract({e}.payload_json,'$.session_ref') OR 'agent:'||json_extract(a.policy_json,'$.agent')||':'||x.value=json_extract({e}.payload_json,'$.session_ref')) OR EXISTS (SELECT 1 FROM json_each(a.policy_json,'$.exclude.projects') x WHERE json_extract({e}.payload_json,'$.project')=x.value OR substr(json_extract({e}.payload_json,'$.project'),1,length(rtrim(x.value,'/'))+1)=rtrim(x.value,'/')||'/')))"),vec![]);
        // Persisted transcripts remain limited to the currently selected file roots.
        for agent in crate::agents::AGENTS {
            let Some(sid)=self.agent_source_id(agent).ok().flatten() else {continue;};
            if !ctx.allowed_sources.contains(&sid) {continue;}
            let Ok(consent)=self.agent_consent(agent) else {continue;};
            let roots=consent.roots();
            let allowed_path=roots.iter().map(|_|"(json_extract(m.value,'$.source_file')=? OR substr(json_extract(m.value,'$.source_file'),1,length(?)+1)=?||'/')").collect::<Vec<_>>().join(" OR ");
            let predicate=if allowed_path.is_empty(){"0".into()}else{allowed_path};
            let mut params=vec![text(&sid)];
            for root in &roots {let path=crate::store::standardize(root).to_string_lossy().trim_end_matches('/').to_owned();params.extend([text(&path),text(&path),text(&path)]);}
            f.add(format!("({e}.source_id<>? OR EXISTS (SELECT 1 FROM events prompt WHERE prompt.event_id=CASE WHEN {e}.kind='ai.work' THEN json_extract({e}.payload_json,'$.prompt_event_id') ELSE {e}.event_id END AND json_extract(prompt.payload_json,'$.builder')='agent-records/2' AND json_array_length(prompt.payload_json,'$.messages')>0 AND NOT EXISTS (SELECT 1 FROM json_each(prompt.payload_json,'$.messages') m WHERE json_extract(m.value,'$.source_file') IS NULL OR NOT ({predicate}))))"),params);
        }
        // Linked notes and external confirmations are live intersections, never manual-source grants.
        let allowed = if ctx.allowed_sources.is_empty() { "NULL".into() } else { placeholders(ctx.allowed_sources.len()) };
        let target = format!("d.source_id IN ({allowed}) AND d.sensitivity<>'protected' AND EXISTS (SELECT 1 FROM sources ds WHERE ds.source_id=d.source_id AND ds.status<>'revoked' AND (ds.kind='agent.transcript' OR json_extract(ds.policy_json,'$.store_text')=1))");
        let mut dependency_params: Vec<P> = ctx.allowed_sources.iter().map(text).collect();
        let mut target=target;
        if !ctx.scope.kinds.iter().any(|k|k=="*") { target+=&format!(" AND d.kind IN ({})",if ctx.scope.kinds.is_empty(){"NULL".into()}else{placeholders(ctx.scope.kinds.len())}); dependency_params.extend(ctx.scope.kinds.iter().map(text)); }
        if !ctx.scope.apps.allow.iter().any(|k|k=="*") { target+=&format!(" AND d.app_bundle_id IN ({})",if ctx.scope.apps.allow.is_empty(){"NULL".into()}else{placeholders(ctx.scope.apps.allow.len())}); dependency_params.extend(ctx.scope.apps.allow.iter().map(text)); }
        if !ctx.scope.apps.deny.is_empty() {target+=&format!(" AND (d.app_bundle_id IS NULL OR d.app_bundle_id NOT IN ({}))",placeholders(ctx.scope.apps.deny.len())); dependency_params.extend(ctx.scope.apps.deny.iter().map(text));}
        if let Some(a)=self.scope_lower_bound(&ctx.scope){target+=" AND COALESCE(d.start_ms,d.ingested_ms)>=?";dependency_params.push(int(a));}
        if let Some(b)=ctx.scope.time.to {target+=" AND COALESCE(d.start_ms,d.ingested_ms)<?";dependency_params.push(int(b));}
        target+=" AND NOT EXISTS (SELECT 1 FROM sources ds,json_each(ds.policy_json,'$.exclude.sessions') x WHERE ds.source_id=d.source_id AND (x.value=json_extract(d.payload_json,'$.session_ref') OR 'agent:'||json_extract(ds.policy_json,'$.agent')||':'||x.value=json_extract(d.payload_json,'$.session_ref'))) AND NOT EXISTS (SELECT 1 FROM sources ds,json_each(ds.policy_json,'$.exclude.projects') x WHERE ds.source_id=d.source_id AND (json_extract(d.payload_json,'$.project')=x.value OR substr(json_extract(d.payload_json,'$.project'),1,length(rtrim(x.value,'/'))+1)=rtrim(x.value,'/')||'/'))";
        target+=" AND NOT EXISTS (SELECT 1 FROM sources ds,json_each(ds.policy_json,'$.exclude.apps') x WHERE ds.source_id=d.source_id AND x.value=d.app_bundle_id)";
        target+=&format!(" AND (COALESCE(json_extract(d.payload_json,'$.record_evidence.confirmation.basis'),'')<>'source_send' OR EXISTS (SELECT 1 FROM events dp JOIN sources ds ON dp.source_id=ds.source_id,json_each(dp.payload_json,'$.messages') m,json_each({e}.payload_json,'$.source_links.refs') explicit WHERE dp.event_id=json_extract(explicit.value,'$.event_id') AND json_extract(ds.policy_json,'$.activity')=1 AND json_extract(m.value,'$.ref')=json_extract(d.payload_json,'$.record_evidence.confirmation.message_id') AND json_extract(m.value,'$.source_hash')=json_extract(d.payload_json,'$.record_evidence.confirmation.source_hash')))" );
        target+=" AND NOT EXISTS (SELECT 1 FROM sources ds,json_each(ds.policy_json,'$.exclude.threads') x WHERE ds.source_id=d.source_id AND x.value=d.thread_id) AND NOT EXISTS (SELECT 1 FROM sources ds,json_each(ds.policy_json,'$.exclude.patterns') x WHERE ds.source_id=d.source_id)";
        target+=" AND NOT EXISTS (SELECT 1 FROM sources ds WHERE ds.source_id=d.source_id AND ds.kind='agent.transcript' AND NOT EXISTS (SELECT 1 FROM events pr WHERE pr.event_id=CASE WHEN d.kind='ai.work' THEN json_extract(d.payload_json,'$.prompt_event_id') ELSE d.event_id END AND json_extract(pr.payload_json,'$.builder')='agent-records/2' AND json_array_length(pr.payload_json,'$.messages')>0 AND NOT EXISTS (SELECT 1 FROM json_each(pr.payload_json,'$.messages') m WHERE json_extract(m.value,'$.source_file') IS NULL OR NOT EXISTS (SELECT 1 FROM json_each(ds.policy_json,'$.paths') root WHERE json_extract(m.value,'$.source_file')=root.value OR substr(json_extract(m.value,'$.source_file'),1,length(rtrim(root.value,'/'))+1)=rtrim(root.value,'/')||'/'))))";
        // Dependencies are flattened by the resolver. Refuse forged partial closures.
        target+=&format!(" AND (json_extract(d.payload_json,'$.source_links') IS NULL OR (json_extract(d.payload_json,'$.source_links.format')='candid-source-links/1' AND json_array_length(d.payload_json,'$.source_links.refs') BETWEEN 1 AND 32 AND NOT EXISTS (SELECT 1 FROM json_each(d.payload_json,'$.source_links.refs') nested WHERE NOT EXISTS (SELECT 1 FROM json_each({e}.payload_json,'$.source_links.refs') allrefs WHERE json_extract(allrefs.value,'$.event_id')=json_extract(nested.value,'$.event_id') AND json_extract(allrefs.value,'$.source_id')=json_extract(nested.value,'$.source_id')))))");
        f.add(format!("(json_extract({e}.payload_json,'$.source_links') IS NULL OR (json_extract({e}.payload_json,'$.source_links.format')='candid-source-links/1' AND json_array_length({e}.payload_json,'$.source_links.refs') BETWEEN 1 AND 32 AND NOT EXISTS (SELECT 1 FROM json_each({e}.payload_json,'$.source_links.refs') link WHERE NOT EXISTS (SELECT 1 FROM events d WHERE d.event_id=json_extract(link.value,'$.event_id') AND d.source_id=json_extract(link.value,'$.source_id') AND {target}))))"),dependency_params.clone());
        // Old display-only references have no authorization identity and stay unavailable.
        f.add(format!("NOT ({e}.kind='note' AND json_extract({e}.payload_json,'$.reference') IS NOT NULL AND json_extract({e}.payload_json,'$.source_links') IS NULL)"),vec![]);
        f.add(format!("(COALESCE(json_extract({e}.payload_json,'$.record_evidence.confirmation.basis'),'')<>'source_send' OR EXISTS (SELECT 1 FROM events d JOIN sources ds ON ds.source_id=d.source_id,json_each(d.payload_json,'$.messages') m WHERE json_extract(m.value,'$.ref')=json_extract({e}.payload_json,'$.record_evidence.confirmation.message_id') AND json_extract(m.value,'$.source_hash')=json_extract({e}.payload_json,'$.record_evidence.confirmation.source_hash') AND ds.kind='agent.transcript' AND json_extract(ds.policy_json,'$.activity')=1 AND {target}))"),dependency_params);
        let scope = &ctx.scope;
        if !scope.kinds.iter().any(|k| k == "*") {
            if scope.kinds.is_empty() {
                f.add("0", vec![]);
            } else {
                f.add(format!("{e}.kind IN ({})", placeholders(scope.kinds.len())), scope.kinds.iter().map(text).collect());
            }
        }
        if !scope.apps.allow.iter().any(|a| a == "*") {
            if scope.apps.allow.is_empty() {
                f.add("0", vec![]);
            } else {
                f.add(format!("{e}.app_bundle_id IN ({})", placeholders(scope.apps.allow.len())), scope.apps.allow.iter().map(text).collect());
            }
        }
        if !scope.apps.deny.is_empty() {
            f.add(
                format!("({e}.app_bundle_id IS NULL OR {e}.app_bundle_id NOT IN ({}))", placeholders(scope.apps.deny.len())),
                scope.apps.deny.iter().map(text).collect(),
            );
        }
        let key = format!("COALESCE({e}.start_ms, {e}.ingested_ms)");
        if let Some(lower) = self.scope_lower_bound(scope) {
            f.add(format!("{key} >= ?"), vec![int(lower)]);
        }
        if let Some(upper) = scope.time.to {
            f.add(format!("{key} < ?"), vec![int(upper)]);
        }
        f
    }

    /// Scope plus the caller's own filters, all applied in SQL before ranking or snippet extraction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn event_filter(
        &self,
        ctx: &ClientContext,
        apps: Option<&Vec<String>>,
        sources: Option<&Vec<String>>,
        kinds: Option<&Vec<String>>,
        thread_id: Option<&str>,
        from: Option<i64>,
        to: Option<i64>,
        snap: Option<i64>,
    ) -> EventFilter {
        let e = "e";
        let mut f = self.scope_filter(ctx, e);
        // The file instruction is canonical for a source-linked screen confirmation.
        // Direct evidence and revision reads retain the screen proof through scope_filter.
        f.add(format!("COALESCE(json_extract({e}.payload_json,'$.record_evidence.confirmation.basis'),'')<>'source_send'"),vec![]);

        f.add("COALESCE(json_extract(e.payload_json,'$.native_hidden'),0)=0", vec![]);
        if let Some(apps) = apps.filter(|a| !a.is_empty()) {
            f.add(format!("{e}.app_bundle_id IN ({})", placeholders(apps.len())), apps.iter().map(text).collect());
        }
        if let Some(sources) = sources.filter(|s| !s.is_empty()) {
            let ph = placeholders(sources.len());
            let mut p: Vec<P> = sources.iter().map(text).collect();
            p.extend(sources.iter().map(text));
            f.add(format!("{e}.source_id IN (SELECT source_id FROM sources WHERE source_id IN ({ph}) OR kind IN ({ph}))"), p);
        }
        if let Some(kinds) = kinds.filter(|k| !k.is_empty()) {
            let projection = kinds.iter().any(|k|k=="ai.prompt");
            let extra=if projection {format!(" OR ({e}.kind='screen.text' AND json_extract({e}.payload_json,'$.record_evidence.confirmation.basis')='outgoing_message')")}else{String::new()};
            f.add(format!("({e}.kind IN ({}){extra})", placeholders(kinds.len())), kinds.iter().map(text).collect());
        }
        if let Some(t) = thread_id {
            f.add(format!("{e}.thread_id = ?"), vec![text(t)]);
        }
        let key = format!("COALESCE({e}.start_ms, {e}.ingested_ms)");
        if let Some(from) = from {
            f.add(format!("{key} >= ?"), vec![int(from)]);
        }
        if let Some(to) = to {
            f.add(format!("{key} < ?"), vec![int(to)]);
        }
        if let Some(snap) = snap {
            f.add(format!("{e}.ingest_seq <= ?"), vec![int(snap)]);
        }
        f
    }

    // MARK: Audit wrapper

    pub(crate) fn audited<Req: Serialize, Res: Serialize>(
        &self,
        tool: &str,
        client_id: &str,
        request: &Req,
        count: impl Fn(&Res) -> Option<i64>,
        body: impl FnOnce(&ClientContext) -> QResult<Res>,
    ) -> Result<Res> {
        let started = Instant::now();
        let Some((ctx, revoked)) = self.load_client(client_id)? else {
            return Err(TimelineError::UnknownClient(client_id.into()));
        };
        let log = |count: Option<i64>, bytes: Option<i64>, denied: Option<&str>| -> Result<()> {
            let mut params = serde_json::to_value(request).unwrap_or(Value::Object(Default::default()));
            if self.meta_value("access_log_params")?.as_deref() == Some("redacted_q") {
                if let Some(o) = params.as_object_mut() {
                    if o.contains_key("q") {
                        o.insert("q".into(), Value::String("<redacted>".into()));
                    }
                }
            }
            let now = self.clock.now_ms();
            self.db.execute(
                "INSERT INTO access_log(log_id,at_ms,client_id,tool,params_json,result_count,result_bytes,duration_ms,denied) VALUES(?,?,?,?,?,?,?,?,?)",
                &[
                    text(uuid_v7(Some(now))),
                    int(now),
                    text(client_id),
                    text(tool),
                    text(json::canonical(&params)),
                    opt_int(count),
                    opt_int(bytes),
                    int(started.elapsed().as_millis() as i64),
                    opt_text(denied),
                ],
            )
        };
        if revoked {
            log(None, None, Some("client_revoked"))?;
            return Err(TimelineError::Denied("client_revoked".into()));
        }
        match body(&ctx) {
            Ok(result) => {
                let bytes = encoded_len(&result) as i64;
                log(count(&result), Some(bytes), None)?;
                Ok(result)
            }
            Err(QErr::ScopeMiss(what)) => {
                log(None, None, Some("out_of_scope"))?;
                Err(TimelineError::NotFound(what))
            }
            Err(QErr::Err(e)) => {
                log(None, None, Some(&e.code()))?;
                Err(e)
            }
        }
    }

    // MARK: Time

    pub(crate) fn window(&self, start: Option<&str>, end: Option<&str>, cursor: Option<&PageCursor>, default_today: bool) -> Result<(Option<i64>, Option<i64>)> {
        if let Some(c) = cursor {
            return Ok((c.ws, c.we));
        }
        let mut s = start.map(|v| self.clock.parse(v, false)).transpose()?;
        let e = end.map(|v| self.clock.parse(v, true)).transpose()?;
        if s.is_none() && e.is_none() && default_today {
            s = Some(self.clock.parse("today", false)?);
        }
        if let (Some(s), Some(e)) = (s, e) {
            if e < s {
                return Err(TimelineError::InvalidInput("end 早于 start".into()));
            }
        }
        Ok((s, e))
    }

    pub(crate) fn iso(&self, ms: Option<i64>) -> Option<String> {
        ms.map(|m| self.clock.iso(m))
    }

    pub(crate) fn time_window(&self, s: Option<i64>, e: Option<i64>) -> TimeWindow {
        TimeWindow { start: self.iso(s), end: self.iso(e) }
    }

    // MARK: Summaries

    pub(crate) fn summary(&self, r: &rusqlite::Row, ctx: &ClientContext) -> EventSummary {
        let mut kind = r.text(1);
        let source_kind = r.text(9);
        let payload = json::parse(&r.text(13));
        if kind=="screen.text" && payload.as_ref().is_some_and(|p|p["record_evidence"]["confirmation"].is_object()) {kind="ai.prompt".into();}
        let (bundle, name) = (r.opt_text(5), r.opt_text(6));
        EventSummary {
            event_id: r.text(0),
            start: self.iso(r.opt_int(2)),
            end: self.iso(r.opt_int(3)),
            time_basis: r.text(4),
            app: if bundle.is_some() || name.is_some() { Some(AppInfo { bundle_id: bundle, name }) } else { None },
            title: self.visible_annotation(&r.text(0),ctx).ok().flatten().and_then(|v|v["title"].as_str().map(String::from)).or_else(||r.opt_text(7)),
            source_id: r.text(8),
            source_label: source_label(&kind, &source_kind, payload.as_ref()),
            layer: r.text(12),
            thread_id: r.opt_text(10),
            sensitivity: r.text(11),
            kind,
        }
    }

    pub(crate) fn summaries(&self, ids: &[String], ctx: &ClientContext) -> Result<HashMap<String, EventSummary>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut f = self.scope_filter(ctx, "e");
        f.add(format!("e.event_id IN ({})", placeholders(ids.len())), ids.iter().map(text).collect());
        let rows = self.db.query(&format!("SELECT {SUMMARY_COLUMNS} {SUMMARY_FROM} WHERE {}", f.sql()), &f.params, |r| self.summary(r,ctx))?;
        Ok(rows.into_iter().map(|s| (s.event_id.clone(), s)).collect())
    }

    pub(crate) fn rank_weights(&self) -> (f64, f64, f64, f64) {
        let v = self.meta_value("rank_weights").ok().flatten().and_then(|t| json::parse(&t));
        let num = |k: &str, d: f64| v.as_ref().and_then(|v| v.get(k)).and_then(Value::as_f64).unwrap_or(d);
        (num("visible", 1.2), num("near_focus", 1.2), num("is_new", 1.3), num("recency_half_life_days", 14.0).max(0.1))
    }

    // MARK: Text helpers

    pub(crate) fn paragraphs(&self, event_ids: &[String], ctx: &ClientContext, fields: Option<&Vec<String>>) -> Result<Vec<TextRow>> {
        if !ctx.scope.text || event_ids.is_empty() {
            return Ok(vec![]);
        }
        let mut sql = format!(
            "SELECT t.event_id,t.field,t.ord,c.text,t.visible,t.focus_distance,t.is_new,t.char_start FROM event_text t JOIN text_chunks c ON c.chunk_id=t.chunk_id JOIN events e ON e.event_id=t.event_id JOIN sources s ON s.source_id=e.source_id
            WHERE (s.kind='agent.transcript' OR json_extract(s.policy_json,'$.store_text')=1) AND NOT EXISTS (SELECT 1 FROM sources old WHERE (old.source_id=json_extract(e.payload_json,'$.native_legacy_source_id') OR old.source_id IN (SELECT value FROM json_each(e.payload_json,'$.native_legacy_sources'))) AND json_extract(old.policy_json,'$.store_text')<>1) AND t.event_id IN ({})",
            placeholders(event_ids.len())
        );
        let mut params: Vec<P> = event_ids.iter().map(text).collect();
        if let Some(fields) = fields.filter(|f| !f.is_empty()) {
            sql += &format!(" AND t.field IN ({})", placeholders(fields.len()));
            params.extend(fields.iter().map(text));
        }
        sql += " ORDER BY t.is_new DESC, COALESCE(t.visible,0) DESC, COALESCE(t.focus_distance,99), t.event_id, t.field, t.ord";
        self.db.query(&sql, &params, |r| TextRow {
            event_id: r.text(0),
            field: r.text(1),
            ord: r.int(2),
            text: r.text(3),
            visible: r.opt_int(4).map(|v| v == 1),
            focus_distance: r.opt_int(5),
            is_new: r.int(6) == 1,
            char_start: r.opt_int(7),
        })
    }

    pub(crate) fn paragraph(&self, t: &TextRow, max_chars: usize) -> Paragraph {
        let (text_, cut) = truncate(&t.text, max_chars);
        Paragraph {
            event_id: t.event_id.clone(),
            field: t.field.clone(),
            chunk_ord: t.ord,
            text: text_,
            truncated: cut,
            attention: Attention { visible: t.visible, focus_distance: t.focus_distance, is_new: t.is_new },
        }
    }

    // MARK: Coverage

    pub(crate) fn coverage(&self, ctx: &ClientContext, from: Option<i64>, to: Option<i64>) -> Result<Coverage> {
        let now = self.clock.now_ms();
        let all: Vec<_> = self.sources()?.into_iter().filter(|s| ctx.allowed_sources.contains(&s.source_id)).collect();
        let mut f = self.event_filter(ctx, None, None, None, None, None, None, None);
        let upper = to.unwrap_or(now);
        if let Some(from) = from {
            f.add("COALESCE(e.end_ms, e.start_ms, e.ingested_ms) >= ?", vec![int(from)]);
        }
        f.add("COALESCE(e.start_ms, e.ingested_ms) < ?", vec![int(upper)]);
        let has_events = self.db.scalar(&format!("SELECT EXISTS(SELECT 1 FROM events e WHERE {})", f.sql()), &f.params)? > 0;
        let mut g = f.clone();
        g.add("e.kind='coverage.gap'", vec![]);
        let reason_text = |r: &str| -> &'static str {
            match r {
                "paused" => "已暂停记录",
                "permission_denied" => "缺少权限，未记录",
                "asleep" => "设备休眠，未记录",
                "crashed" => "采集中断，未记录",
                "source_offline" => "来源离线，未记录",
                _ => "未记录",
            }
        };
        let gaps = self.db.query(
            &format!("SELECT e.start_ms,e.end_ms,e.payload_json FROM events e WHERE {} ORDER BY e.start_ms DESC LIMIT 20", g.sql()),
            &g.params,
            |r| {
                let reason = json::parse(&r.text(2)).and_then(|v| json::str_at(&v, "reason_code").map(String::from)).unwrap_or_else(|| "unknown".into());
                let (s, e) = (r.opt_int(0), r.opt_int(1));
                let span = match s {
                    Some(s) => self.clock.hm(s) + &e.map(|e| format!("–{}", self.clock.hm(e))).unwrap_or_else(|| "起".into()),
                    None => "时间未知".into(),
                };
                CoverageGap { start: self.iso(s), end: self.iso(e), note: format!("{span} {}", reason_text(&reason)), reason_code: reason }
            },
        )?;
        let mut notes: Vec<String> = gaps.iter().map(|g| g.note.clone()).collect();
        for s in &all {
            if s.status == "paused" {
                notes.push(format!("{} 已暂停", s.display_name));
            }
            if s.status == "error" {
                notes.push(format!("{} 同步出错{}", s.display_name, s.last_error.as_ref().map(|e| format!("（{e}）")).unwrap_or_default()));
            }
            if s.status == "active" {
                if let Some(last) = s.last_sync_ms {
                    if now - last > 24 * 3_600_000 {
                        let days = (now - last) / 86_400_000;
                        let span = if days > 0 { format!("{days} 天") } else { format!("{} 小时", (now - last) / 3_600_000) };
                        notes.push(format!("{} {span}未同步", s.display_name));
                    }
                }
            }
        }
        let recording = all.iter().filter(|s| CAPTURE_KINDS.contains(&s.kind.as_str()) && s.status == "active").count() as i64;
        if !has_events {
            notes.push(if recording == 0 {
                "该时段没有事件，且没有处于启用状态的记录来源，无法判断当时是否在记录".into()
            } else {
                "该时段没有事件（记录来源处于启用状态）".into()
            });
        }
        Ok(Coverage { has_events, recording_sources: recording, gaps, notes })
    }

    pub(crate) fn media_refs(&self, event_ids: &[String]) -> Result<Vec<MediaRef>> {
        if event_ids.is_empty() {
            return Ok(vec![]);
        }
        self.db.query(
            &format!(
                "SELECT eb.event_id,b.sha256,eb.role,b.mime_type,b.byte_count FROM event_blobs eb JOIN blobs b ON b.sha256=eb.sha256 WHERE eb.event_id IN ({}) ORDER BY eb.event_id",
                placeholders(event_ids.len())
            ),
            &event_ids.iter().map(text).collect::<Vec<_>>(),
            |r| MediaRef { event_id: r.text(0), sha256: r.text(1), role: r.text(2), mime_type: r.text(3), byte_count: r.int(4) },
        )
    }
}

/// Largest prefix of `items` whose encoded response still fits `max_bytes`.
pub(crate) fn fit<I: Clone, R: Serialize>(items: &[I], max_bytes: i64, build: impl Fn(&[I]) -> R) -> Result<(Vec<I>, bool)> {
    let size = |n: usize| encoded_len(&build(&items[..n])) as i64;
    if size(items.len()) <= max_bytes {
        return Ok((items.to_vec(), false));
    }
    if size(0) > max_bytes {
        return Err(TimelineError::Denied("max_result_bytes_too_small".into()));
    }
    let (mut lo, mut hi) = (0usize, items.len()); // invariant: size(lo) fits, size(hi) does not
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if size(mid) <= max_bytes {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok((items[..lo].to_vec(), true))
}
