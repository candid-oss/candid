use super::core::*;
use super::types::*;
use crate::db::{int, text, RowExt, P};
use crate::ids::{sha256_hex, uuid_v7};
use crate::json;
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use base64::Engine;
use std::collections::{BTreeMap, BTreeSet, HashMap};

impl TimelineStore {
    // MARK: status

    pub fn status(&self, client_id: &str) -> Result<StatusResponse> {
        self.audited("status", client_id, &StatusRequest {}, |r: &StatusResponse| Some(r.sources.len() as i64), |ctx| {
            let now = self.clock.now_ms();
            let db_bytes: i64 = ["timeline.sqlite", "timeline.sqlite-wal"]
                .iter()
                .map(|f| std::fs::metadata(self.root.join(f)).map(|m| m.len() as i64).unwrap_or(0))
                .sum();
            let all: Vec<_> = self.sources()?.into_iter().filter(|s| ctx.allowed_sources.contains(&s.source_id)).collect();
            let window_minutes = 10;
            let mut last_capture: Option<i64> = None;
            let mut recording = false;
            let statuses = all
                .iter()
                .map(|s| {
                    if CAPTURE_KINDS.contains(&s.kind.as_str()) && s.status == "active" {
                        if let Some(last) = self.last_ingest_ms(&s.source_id).ok().flatten() {
                            last_capture = Some(last_capture.unwrap_or(0).max(last));
                            if now - last <= window_minutes * 60_000 {
                                recording = true;
                            }
                        }
                    }
                    StatusSource {
                        source_id: s.source_id.clone(),
                        kind: s.kind.clone(),
                        display_name: s.display_name.clone(),
                        status: s.status.clone(),
                        last_sync: self.iso(s.last_sync_ms),
                        lag_seconds: s.last_sync_ms.map(|l| ((now - l) / 1000).max(0)),
                        last_ingest: self.iso(self.last_ingest_ms(&s.source_id).ok().flatten()),
                        last_error: if s.status == "error" { s.last_error.clone() } else { None },
                    }
                })
                .collect();
            let (available, detail) = (self.agent_probe)();
            Ok(StatusResponse {
                store: StatusStore {
                    store_id: self.identity.store_id.clone(),
                    schema_version: self.identity.schema_version,
                    db_bytes,
                    blob_bytes: self.db.scalar("SELECT COALESCE(SUM(byte_count),0) FROM blobs", &[])?,
                    events: self.db.scalar("SELECT COUNT(*) FROM events", &[])?,
                    ingest_seq: self.current_seq()?,
                },
                sources: statuses,
                capture: StatusCapture { recording, last_event: self.iso(last_capture), window_minutes },
                agent: StatusAgent { available, detail },
            })
        })
    }

    // MARK: overview

    pub fn overview(&self, req: &OverviewRequest, client_id: &str) -> Result<OverviewResponse> {
        self.audited("overview", client_id, req, |r: &OverviewResponse| Some(r.groups.len() as i64), |ctx| {
            let group_by = req.group_by.clone().unwrap_or_else(|| "app".into());
            if !["app", "hour", "source", "episode"].contains(&group_by.as_str()) {
                return Err(TimelineError::InvalidInput(format!("group_by: {group_by}")).into());
            }
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), None, true)?;
            let we_ms = we.unwrap_or_else(|| self.clock.now_ms());
            let we = Some(we_ms);
            let f = self.event_filter(ctx, None, None, None, None, ws, we, None);
            let total = self.db.scalar(&format!("SELECT COUNT(*) FROM events e WHERE {}", f.sql()), &f.params)?;
            // Focus time per app. `app.focus` rows only mark switches, so a span without an explicit end runs until the
            // last observation the same source recorded before the next switch, plus a one-minute tail.
            let focus_spans = || -> Result<Vec<(String, String, i64, i64)>> {
                let mut g = f.clone();
                g.add("e.kind='app.focus' AND e.start_ms IS NOT NULL", vec![]);
                let rows = self.db.query(
                    &format!(
                        "SELECT e.source_id,COALESCE(e.app_bundle_id,'?'),COALESCE(e.app_name,e.app_bundle_id,'?'),e.start_ms,e.end_ms FROM events e WHERE {} ORDER BY e.source_id,e.start_ms",
                        g.sql()
                    ),
                    &g.params,
                    |r| (r.text(0), r.text(1), r.text(2), r.int(3), r.opt_int(4)),
                )?;
                let mut spans = Vec::new();
                for (i, r) in rows.iter().enumerate() {
                    let next = rows.get(i + 1).filter(|n| n.0 == r.0).map(|n| n.3);
                    let end = match r.4 {
                        Some(explicit) => explicit,
                        None => {
                            let last = self.db.scalar(
                                "SELECT COALESCE(MAX(start_ms),0) FROM events WHERE source_id=? AND kind IN ('app.focus','window.focus','screen.text','screen.shot') AND start_ms>=? AND start_ms<?",
                                &[text(&r.0), int(r.3), int(next.unwrap_or(i64::MAX))],
                            )?;
                            let mut end = last.max(r.3) + 60_000;
                            if let Some(n) = next {
                                end = end.min(n);
                            }
                            end
                        }
                    };
                    spans.push((r.1.clone(), r.2.clone(), r.3.max(ws.unwrap_or(r.3)), end.min(we_ms)));
                }
                Ok(spans.into_iter().filter(|s| s.3 > s.2).collect())
            };
            let mut groups: Vec<OverviewGroup> = Vec::new();
            match group_by.as_str() {
                "app" => {
                    let mut seconds: HashMap<String, (String, f64)> = HashMap::new();
                    let mut spans = focus_spans()?;
                    for s in &spans {
                        seconds.entry(s.0.clone()).or_insert((s.1.clone(), 0.0)).1 += (s.3 - s.2) as f64 / 1000.0;
                    }
                    // Visits: each time an app comes to the front after another app (or after five minutes away).
                    spans.sort_by_key(|s| (s.2, s.3));
                    let mut switches: HashMap<String, i64> = HashMap::new();
                    let mut last: Option<(String, i64)> = None;
                    for s in &spans {
                        let same = last.as_ref().is_some_and(|(app, end)| *app == s.0 && s.2 - *end <= 300_000);
                        if !same {
                            *switches.entry(s.0.clone()).or_insert(0) += 1;
                        }
                        let end = if same { last.as_ref().map_or(s.3, |(_, e)| (*e).max(s.3)) } else { s.3 };
                        last = Some((s.0.clone(), end));
                    }
                    let mut counts: HashMap<String, i64> = HashMap::new();
                    for (k, n) in self.db.query(
                        &format!("SELECT COALESCE(e.app_bundle_id,'(none)'),COUNT(*) FROM events e WHERE {} GROUP BY 1", f.sql()),
                        &f.params,
                        |r| (r.text(0), r.int(1)),
                    )? {
                        counts.insert(k, n);
                    }
                    let keys: BTreeSet<String> = seconds.keys().chain(counts.keys()).cloned().collect();
                    groups = keys
                        .into_iter()
                        .map(|k| OverviewGroup {
                            label: seconds.get(&k).map(|s| s.0.clone()).unwrap_or_else(|| k.clone()),
                            seconds: seconds.get(&k).map(|s| s.1),
                            events: counts.get(&k).copied().unwrap_or(0),
                            switches: switches.get(&k).copied(),
                            key: k,
                        })
                        .collect();
                    groups.sort_by(|a, b| {
                        b.seconds
                            .unwrap_or(0.0)
                            .partial_cmp(&a.seconds.unwrap_or(0.0))
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(b.events.cmp(&a.events))
                            .then(a.key.cmp(&b.key))
                    });
                }
                "hour" => {
                    let mut per_hour: BTreeMap<String, (f64, i64)> = BTreeMap::new();
                    let label = |ms: i64| format!("{:02}:00", self.clock.hour_of(ms));
                    for s in focus_spans()? {
                        let mut cursor = s.2;
                        while cursor < s.3 {
                            let stop = s.3.min(self.clock.next_hour(cursor));
                            per_hour.entry(label(cursor)).or_insert((0.0, 0)).0 += (stop - cursor) as f64 / 1000.0;
                            cursor = stop;
                        }
                    }
                    for ms in self.db.query(&format!("SELECT COALESCE(e.start_ms,e.ingested_ms) FROM events e WHERE {}", f.sql()), &f.params, |r| r.int(0))? {
                        per_hour.entry(label(ms)).or_insert((0.0, 0)).1 += 1;
                    }
                    groups = per_hour
                        .into_iter()
                        .map(|(k, v)| OverviewGroup { label: k.clone(), key: k, seconds: Some(v.0), events: v.1, switches: None })
                        .collect();
                }
                "source" => {
                    groups = self.db.query(
                        &format!(
                            "SELECT s.source_id,s.display_name,COUNT(*) FROM events e JOIN sources s ON s.source_id=e.source_id WHERE {} GROUP BY s.source_id ORDER BY COUNT(*) DESC",
                            f.sql()
                        ),
                        &f.params,
                        |r| OverviewGroup { key: r.text(0), label: r.text(1), seconds: None, events: r.int(2), switches: None },
                    )?;
                }
                _ => {
                    if ctx.scope.derived {
                        groups = self
                            .episode_rows(ctx, ws, we, None, 50, None, None, None)?
                            .into_iter()
                            .map(|e| OverviewGroup {
                                label: e.title.clone().unwrap_or_else(|| e.episode_id.clone()),
                                key: e.episode_id,
                                seconds: None,
                                events: e.evidence_count,
                                switches: None,
                            })
                            .collect();
                    }
                }
            }
            let episodes: Vec<OverviewItem> = if ctx.scope.derived {
                self.episode_rows(ctx, ws, we, None, 20, None, None, None)?
                    .into_iter()
                    .map(|e| OverviewItem { id: e.episode_id, title: e.title, start: Some(e.start), end: Some(e.end), detail: Some(e.kind) })
                    .collect()
            } else {
                vec![]
            };
            let items = |kind: &str| -> Result<Vec<OverviewItem>> {
                let mut g = f.clone();
                g.add("e.kind=?", vec![text(kind)]);
                self.db.query(
                    &format!("SELECT e.event_id,e.title,e.start_ms,e.end_ms,e.payload_json FROM events e WHERE {} ORDER BY {SORT_KEY} LIMIT ?", g.sql()),
                    &g.with(vec![int(20)]),
                    |r| {
                        let payload = json::parse(&r.text(4));
                        let detail = payload
                            .as_ref()
                            .and_then(|p| json::str_at(p, "reason_code").or_else(|| json::str_at(p, "calendar_name")))
                            .map(String::from);
                        OverviewItem { id: r.text(0), title: r.opt_text(1), start: self.iso(r.opt_int(2)), end: self.iso(r.opt_int(3)), detail }
                    },
                )
            };
            let calendar = items("calendar.event")?;
            let recordings = items("audio.recording")?;
            let gaps = items("coverage.gap")?;
            let cov = self.coverage(ctx, ws, we)?;
            let window = self.time_window(ws, we);
            let build = |g: &[OverviewGroup], trimmed: bool| OverviewResponse {
                window: window.clone(),
                group_by: group_by.clone(),
                total_events: total,
                groups: g.to_vec(),
                episodes: episodes.clone(),
                calendar: calendar.clone(),
                recordings: recordings.clone(),
                gaps: gaps.clone(),
                coverage: cov.clone(),
                truncated_by_size: trimmed,
            };
            let (kept, trimmed) = fit(&groups, ctx.scope.max_result_bytes, |g| build(g, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    // MARK: episodes (shared)

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn episode_rows(
        &self,
        ctx: &ClientContext,
        from: Option<i64>,
        to: Option<i64>,
        statuses: Option<&Vec<String>>,
        limit: i64,
        after: Option<(i64, String)>,
        episode_id: Option<&str>,
        rev: Option<i64>,
    ) -> Result<Vec<EpisodeSummary>> {
        let latest = "ep.rev = (SELECT MAX(rev) FROM episodes x WHERE x.episode_id=ep.episode_id)";
        let mut clauses: Vec<String> = vec![latest.into()];
        let mut params: Vec<P> = vec![];
        if let Some(id) = episode_id {
            clauses = vec!["ep.episode_id = ?".into()];
            params = vec![text(id)];
            match rev {
                Some(r) => {
                    clauses.push("ep.rev = ?".into());
                    params.push(int(r));
                }
                None => clauses.push(latest.into()),
            }
        } else {
            let default: Vec<String> = ["draft", "confirmed", "edited"].iter().map(|s| s.to_string()).collect();
            let st = statuses.filter(|s| !s.is_empty()).unwrap_or(&default);
            clauses.push(format!("ep.status IN ({})", crate::db::placeholders(st.len())));
            params.extend(st.iter().map(text));
        }
        if let Some(from) = from {
            clauses.push("ep.end_ms >= ?".into());
            params.push(int(from));
        }
        if let Some(to) = to {
            clauses.push("ep.start_ms < ?".into());
            params.push(int(to));
        }
        if let Some(lower) = self.scope_lower_bound(&ctx.scope) {
            clauses.push("ep.start_ms >= ?".into());
            params.push(int(lower));
        }
        if let Some(upper) = ctx.scope.time.to {
            clauses.push("ep.start_ms < ?".into());
            params.push(int(upper));
        }
        if let Some((t, id)) = after {
            clauses.push("(ep.start_ms > ? OR (ep.start_ms = ? AND ep.episode_id > ?))".into());
            params.extend([int(t), int(t), text(id)]);
        }
        // An AI-derived summary inherits every AI input's current permission, including retained-but-revoked data.
        let agent_scope=self.scope_filter(ctx,"e");
        clauses.push(format!("NOT EXISTS (SELECT 1 FROM episode_evidence ae JOIN events e ON e.event_id=ae.event_id WHERE ae.episode_id=ep.episode_id AND ae.rev=ep.rev AND e.kind IN ('ai.prompt','ai.work') AND NOT ({}))",agent_scope.sql()));
        params.extend(agent_scope.params);
        if !ctx.scope.text {clauses.push("NOT EXISTS (SELECT 1 FROM episode_evidence ae JOIN events e ON e.event_id=ae.event_id WHERE ae.episode_id=ep.episode_id AND ae.rev=ep.rev AND e.kind IN ('ai.prompt','ai.work'))".into());}
        params.push(int(limit));
        let rows = self.db.query(
            &format!(
                "SELECT ep.episode_id,ep.rev,ep.title,ep.summary,ep.start_ms,ep.end_ms,ep.kind,ep.status,ep.producer,ep.model_ref FROM episodes ep
            WHERE {} ORDER BY ep.start_ms, ep.episode_id LIMIT ?",
                clauses.join(" AND ")
            ),
            &params,
            |r| (r.text(0), r.int(1), r.opt_text(2), r.opt_text(3), r.int(4), r.int(5), r.text(6), r.text(7), r.text(8), r.opt_text(9)),
        )?;
        let scope = self.scope_filter(ctx, "e");
        rows.into_iter()
            .map(|r| {
                let mut p = vec![text(&r.0), int(r.1)];
                p.extend(scope.params.iter().cloned());
                let count = self.db.scalar(
                    &format!("SELECT COUNT(*) FROM episode_evidence ee JOIN events e ON e.event_id=ee.event_id WHERE ee.episode_id=? AND ee.rev=? AND {}", scope.sql()),
                    &p,
                )?;
                Ok(EpisodeSummary {
                    episode_id: r.0,
                    rev: r.1,
                    title: r.2,
                    summary: r.3,
                    start: self.clock.iso(r.4),
                    end: self.clock.iso(r.5),
                    kind: r.6,
                    status: r.7,
                    producer: r.8,
                    model_ref: r.9,
                    evidence_count: count,
                })
            })
            .collect()
    }

    // MARK: timeline

    pub fn timeline(&self, req: &TimelineRequest, client_id: &str) -> Result<TimelineResponse> {
        self.audited("timeline", client_id, req, |r: &TimelineResponse| Some(r.entries.len() as i64), |ctx| {
            let level = req.level.clone().unwrap_or_else(|| "event".into());
            if level != "event" && level != "episode" {
                return Err(TimelineError::InvalidInput(format!("level: {level}")).into());
            }
            let limit = clamp(req.limit.unwrap_or(10), 1, 50);
            let mut base = req.clone();
            base.cursor = None;
            base.limit = None;
            let fp = request_fingerprint(&base);
            let cursor = req.cursor.as_deref().filter(|s| !s.trim().is_empty()).map(|c| decode_cursor(c, "timeline", &fp)).transpose()?;
            let snap = match &cursor {
                Some(c) => c.snap,
                None => self.current_seq()?,
            };
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), cursor.as_ref(), true)?;
            let cov = self.coverage(ctx, ws, we)?;
            let new_records = self.current_seq()? > snap;
            let mut entries: Vec<TimelineEntry> = Vec::new();
            let mut cursors: Vec<PageCursor> = Vec::new(); // cursor that resumes after entry i
            let more;
            let after = cursor.as_ref().and_then(|c| match (c.t, &c.id) {
                (Some(t), Some(id)) => Some((t, id.clone())),
                _ => None,
            });
            if level == "episode" {
                if !ctx.scope.derived {
                    return Err(TimelineError::Denied("derived_not_permitted".into()).into());
                }
                let rows = self.episode_rows(ctx, ws, we, None, limit + 1, after, None, None)?;
                more = rows.len() as i64 > limit;
                for r in rows.into_iter().take(limit as usize) {
                    let mut c = PageCursor::new("timeline", snap, &fp);
                    c.t = Some(self.clock.parse(&r.start, false)?);
                    c.id = Some(r.episode_id.clone());
                    c.ws = ws;
                    c.we = we;
                    cursors.push(c);
                    entries.push(TimelineEntry { r#type: "episode".into(), event: None, episode: Some(r), preview: None });
                }
            } else {
                let mut f = self.event_filter(ctx, req.apps.as_ref(), req.sources.as_ref(), req.kinds.as_ref(), None, ws, we, Some(snap));
                if let Some((t, id)) = &after {
                    f.add(format!("({SORT_KEY} > ? OR ({SORT_KEY} = ? AND e.event_id > ?))"), vec![int(*t), int(*t), text(id)]);
                }
                let rows = self.db.query(
                    &format!("SELECT {SUMMARY_COLUMNS},{SORT_KEY} {SUMMARY_FROM} WHERE {} ORDER BY {SORT_KEY}, e.event_id LIMIT ?", f.sql()),
                    &f.with(vec![int(limit + 1)]),
                    |r| (self.summary(r,ctx), r.int(14)),
                )?;
                more = rows.len() as i64 > limit;
                let page: Vec<_> = rows.into_iter().take(limit as usize).collect();
                let ids: Vec<String> = page.iter().map(|p| p.0.event_id.clone()).collect();
                let mut preview: HashMap<String, String> = HashMap::new();
                for t in self.paragraphs(&ids, ctx, None)? {
                    preview.entry(t.event_id.clone()).or_insert_with(|| truncate(&t.text, 120).0);
                }
                for (s, key) in page {
                    let mut c = PageCursor::new("timeline", snap, &fp);
                    c.t = Some(key);
                    c.id = Some(s.event_id.clone());
                    c.ws = ws;
                    c.we = we;
                    cursors.push(c);
                    let p = preview.get(&s.event_id).cloned();
                    entries.push(TimelineEntry { r#type: "event".into(), event: Some(s), episode: None, preview: p });
                }
            }
            let window = self.time_window(ws, we);
            let total = entries.len();
            let build = |kept: &[TimelineEntry], trimmed: bool| {
                let has_next = trimmed || (more && kept.len() == total);
                TimelineResponse {
                    window: window.clone(),
                    level: level.clone(),
                    entries: kept.to_vec(),
                    next_cursor: if has_next && !kept.is_empty() { Some(encode_cursor(&cursors[kept.len() - 1])) } else { None },
                    has_new_records: new_records,
                    truncated_by_size: trimmed,
                    coverage: cov.clone(),
                }
            };
            let (kept, trimmed) = fit(&entries, ctx.scope.max_result_bytes, |k| build(k, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    // MARK: context

    pub fn context(&self, req: &ContextRequest, client_id: &str) -> Result<ContextResponse> {
        self.audited("context", client_id, req, |r: &ContextResponse| Some(r.events.len() as i64), |ctx| {
            let at = self.clock.parse(&req.at, false)?;
            let before = clamp(req.before.unwrap_or(5), 0, 24 * 60);
            let after = clamp(req.after.unwrap_or(5), 0, 24 * 60);
            let include: BTreeSet<String> = req.include.clone().unwrap_or_else(|| vec!["text".into()]).into_iter().collect();
            let (from, to) = (at - before * 60_000, at + after * 60_000);
            let mut f = self.event_filter(ctx, None, None, None, None, None, None, None);
            f.add(
                "COALESCE(e.start_ms,e.ingested_ms) < ? AND COALESCE(e.end_ms,e.start_ms,e.ingested_ms) >= ?",
                vec![int(to), int(from)],
            );
            let all = self.db.query(
                &format!("SELECT {SUMMARY_COLUMNS} {SUMMARY_FROM} WHERE {} ORDER BY {SORT_KEY}, e.event_id LIMIT 200", f.sql()),
                &f.params,
                |r| self.summary(r,ctx),
            )?;
            let recordings: Vec<_> = all.iter().filter(|s| s.kind == "audio.recording").cloned().collect();
            let calendar: Vec<_> = all.iter().filter(|s| s.kind == "calendar.event").cloned().collect();
            let events: Vec<_> = all.iter().filter(|s| s.kind != "audio.recording" && s.kind != "calendar.event").cloned().collect();
            let ids: Vec<String> = all.iter().map(|s| s.event_id.clone()).collect();
            let paragraphs: Vec<Paragraph> = if include.contains("text") {
                self.paragraphs(&ids, ctx, None)?
                    .iter()
                    .filter(|p| p.is_new && p.visible != Some(false) && p.focus_distance.unwrap_or(0) <= 3)
                    .take(10)
                    .map(|p| self.paragraph(p, 600))
                    .collect()
            } else {
                vec![]
            };
            let refs = if include.contains("media_refs") && ctx.scope.media { self.media_refs(&ids)? } else { vec![] };
            let cov = self.coverage(ctx, Some(from), Some(to))?;
            let build = |kept: &[EventSummary], trimmed: bool| ContextResponse {
                at: self.clock.iso(at),
                window: self.time_window(Some(from), Some(to)),
                events: kept.to_vec(),
                focus_paragraphs: paragraphs.clone(),
                recordings: recordings.clone(),
                calendar: calendar.clone(),
                media_refs: refs.clone(),
                coverage: cov.clone(),
                truncated_by_size: trimmed,
            };
            let (kept, trimmed) = fit(&events, ctx.scope.max_result_bytes, |k| build(k, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    // MARK: get_event

    pub fn get_event(&self, req: &GetEventRequest, client_id: &str) -> Result<GetEventResponse> {
        self.audited("get_event", client_id, req, |_: &GetEventResponse| Some(1), |ctx| {
            let include: BTreeSet<String> = req.include.clone().unwrap_or_else(|| vec!["text".into()]).into_iter().collect();
            let Some(s) = self.summaries(std::slice::from_ref(&req.event_id), ctx)?.remove(&req.event_id) else {
                if self.db.scalar("SELECT COUNT(*) FROM events WHERE event_id=?", &[text(&req.event_id)])? > 0 {
                    return Err(QErr::ScopeMiss(req.event_id.clone()));
                }
                return Err(TimelineError::NotFound(req.event_id.clone()).into());
            };
            let row = self
                .db
                .first(
                    "SELECT payload_json,url,parent_event_id,thread_id,COALESCE(start_ms,ingested_ms),source_id FROM events WHERE event_id=?",
                    &[text(&req.event_id)],
                    |r| (json::parse(&r.text(0)).unwrap_or_else(|| serde_json::json!({})), r.opt_text(1), r.opt_text(2), r.opt_text(3), r.int(4), r.text(5)),
                )?
                .ok_or_else(|| TimelineError::NotFound(req.event_id.clone()))?;
            let mut paras: Vec<Paragraph> = if include.contains("text") {
                self.paragraphs(std::slice::from_ref(&req.event_id), ctx, None)?.iter().map(|p| self.paragraph(p, 20_000)).collect()
            } else {
                vec![]
            };
            let thread = match &row.3 {
                Some(tid) => self.db.first("SELECT thread_id,kind,title,started_ms,ended_ms FROM threads WHERE thread_id=?", &[text(tid)], |r| ThreadInfo {
                    thread_id: r.text(0),
                    kind: r.text(1),
                    title: r.opt_text(2),
                    started: self.iso(r.opt_int(3)),
                    ended: self.iso(r.opt_int(4)),
                })?,
                None => None,
            };
            let mut neighbors = Vec::new();
            if include.contains("neighbors") {
                let mut f = self.event_filter(ctx,None,None,None,None,None,None,None);
                f.add("e.source_id=? AND e.event_id<>?", vec![text(&row.5), text(&req.event_id)]);
                let before = self.db.query(
                    &format!("SELECT {SUMMARY_COLUMNS} {SUMMARY_FROM} WHERE {} AND {SORT_KEY} <= ? ORDER BY {SORT_KEY} DESC LIMIT 3", f.sql()),
                    &f.with(vec![int(row.4)]),
                    |r| self.summary(r,ctx),
                )?;
                let after = self.db.query(
                    &format!("SELECT {SUMMARY_COLUMNS} {SUMMARY_FROM} WHERE {} AND {SORT_KEY} > ? ORDER BY {SORT_KEY} LIMIT 3", f.sql()),
                    &f.with(vec![int(row.4)]),
                    |r| self.summary(r,ctx),
                )?;
                neighbors = before.into_iter().rev().chain(after).collect();
            }
            let refs = if include.contains("media_refs") && ctx.scope.media {
                self.media_refs(std::slice::from_ref(&req.event_id))?
            } else {
                vec![]
            };
            let mut episodes = Vec::new();
            if include.contains("derived") && ctx.scope.derived {
                let ids = self.db.query("SELECT DISTINCT episode_id FROM episode_evidence WHERE event_id=?", &[text(&req.event_id)], |r| r.text(0))?;
                let statuses: Vec<String> = ["draft", "confirmed", "edited", "rejected"].iter().map(|s| s.to_string()).collect();
                for id in ids {
                    episodes.extend(self.episode_rows(ctx, None, None, Some(&statuses), 1, None, Some(&id), None)?);
                }
            }
            // Record evidence contains input and reading text as well as metadata.
            // It must obey the same text scope as the paragraph projection.
            let mut payload = row.0.clone();
            if let Some(annotation)=self.visible_annotation(&req.event_id,ctx)? {payload["manual_metadata"]=annotation;}
            if row.0["record_evidence"]["confirmation"].is_object() && ctx.scope.text && include.contains("text") {
                if let Some(body)=row.0["record_evidence"]["confirmation"]["text"].as_str() {
                    paras=vec![Paragraph{event_id:req.event_id.clone(),field:"prompt".into(),chunk_ord:0,text:body.into(),truncated:false,attention:Attention{visible:None,focus_distance:None,is_new:true}}];
                    payload["origin"]=serde_json::json!("screen");
                }
            }

            if !ctx.scope.text || !include.contains("text")
                || !self.source_row(&row.5)?.is_some_and(|source| source.policy.store_text)
            {
                if let Some(fields) = payload.as_object_mut() {
                    fields.remove("record_evidence");
                }
            }
            let build = |p: &[Paragraph], trimmed: bool| GetEventResponse {
                event: s.clone(),
                payload: payload.clone(),
                url: row.1.clone(),
                parent_event_id: row.2.clone(),
                paragraphs: p.to_vec(),
                thread: thread.clone(),
                neighbors: neighbors.clone(),
                media_refs: refs.clone(),
                episodes: episodes.clone(),
                truncated_by_size: trimmed,
            };
            let (kept, trimmed) = fit(&paras, ctx.scope.max_result_bytes, |p| build(p, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    // MARK: episodes / episode

    pub fn episodes(&self, req: &EpisodesRequest, client_id: &str) -> Result<EpisodesResponse> {
        self.audited("episodes", client_id, req, |r: &EpisodesResponse| Some(r.episodes.len() as i64), |ctx| {
            if !ctx.scope.derived {
                return Err(TimelineError::Denied("derived_not_permitted".into()).into());
            }
            let limit = clamp(req.limit.unwrap_or(10), 1, 50);
            let mut base = req.clone();
            base.cursor = None;
            base.limit = None;
            let fp = request_fingerprint(&base);
            let cursor = req.cursor.as_deref().filter(|s| !s.trim().is_empty()).map(|c| decode_cursor(c, "episodes", &fp)).transpose()?;
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), cursor.as_ref(), false)?;
            for st in req.status.iter().flatten() {
                if !["draft", "confirmed", "edited", "rejected", "superseded"].contains(&st.as_str()) {
                    return Err(TimelineError::InvalidInput(format!("status {st}")).into());
                }
            }
            let after = cursor.as_ref().and_then(|c| match (c.t, &c.id) {
                (Some(t), Some(id)) => Some((t, id.clone())),
                _ => None,
            });
            let rows = self.episode_rows(ctx, ws, we, req.status.as_ref(), limit + 1, after, None, None)?;
            let page: Vec<EpisodeSummary> = rows.iter().take(limit as usize).cloned().collect();
            let cur = |r: &EpisodeSummary| -> Option<String> {
                self.clock.parse(&r.start, false).ok().map(|t| {
                    let mut c = PageCursor::new("episodes", 0, &fp);
                    c.t = Some(t);
                    c.id = Some(r.episode_id.clone());
                    c.ws = ws;
                    c.we = we;
                    encode_cursor(&c)
                })
            };
            let window = self.time_window(ws, we);
            let build = |kept: &[EpisodeSummary], trimmed: bool| {
                let has_next = trimmed || (rows.len() as i64 > limit && kept.len() == page.len());
                EpisodesResponse {
                    window: window.clone(),
                    episodes: kept.to_vec(),
                    next_cursor: if has_next { kept.last().and_then(cur) } else { None },
                    truncated_by_size: trimmed,
                }
            };
            let (kept, trimmed) = fit(&page, ctx.scope.max_result_bytes, |k| build(k, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    pub fn episode(&self, req: &EpisodeRequest, client_id: &str) -> Result<EpisodeResponse> {
        self.audited("episode", client_id, req, |r: &EpisodeResponse| Some(r.evidence.len() as i64), |ctx| {
            if !ctx.scope.derived {
                return Err(TimelineError::Denied("derived_not_permitted".into()).into());
            }
            if self.db.scalar("SELECT COUNT(*) FROM episodes WHERE episode_id=?", &[text(&req.episode_id)])? == 0 {
                return Err(TimelineError::NotFound(req.episode_id.clone()).into());
            }
            let Some(ep) = self.episode_rows(ctx, None, None, None, 1, None, Some(&req.episode_id), req.rev)?.into_iter().next() else {
                return Err(QErr::ScopeMiss(req.episode_id.clone()));
            };
            let mut f = self.scope_filter(ctx, "e");
            f.add("ee.episode_id=? AND ee.rev=?", vec![text(&req.episode_id), int(ep.rev)]);
            let evidence = self.db.query(
                &format!(
                    "SELECT {SUMMARY_COLUMNS},ee.role,ee.selector_json {SUMMARY_FROM} JOIN episode_evidence ee ON ee.event_id=e.event_id WHERE {} ORDER BY {SORT_KEY}, e.event_id",
                    f.sql()
                ),
                &f.params,
                |r| { let event = self.summary(r,ctx); let selector = r.opt_text(15).and_then(|t| json::parse(&t)).map(|v| self.hydrate_screen_selector(&event.event_id, v)); EpisodeEvidence { event, role: r.text(14), selector } },
            )?;
            let total = self.db.scalar("SELECT COUNT(*) FROM episode_evidence WHERE episode_id=? AND rev=?", &[text(&req.episode_id), int(ep.rev)])?;
            let shown = evidence.len() as i64;
            let build = |kept: &[EpisodeEvidence], trimmed: bool| EpisodeResponse {
                episode: ep.clone(),
                evidence: kept.to_vec(),
                hidden_evidence: total - shown,
                truncated_by_size: trimmed,
            };
            let (kept, trimmed) = fit(&evidence, ctx.scope.max_result_bytes, |k| build(k, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    // MARK: thread

    pub fn thread(&self, req: &ThreadRequest, client_id: &str) -> Result<ThreadResponse> {
        self.audited("thread", client_id, req, |r: &ThreadResponse| Some(r.messages.len() as i64), |ctx| {
            let limit = clamp(req.limit.unwrap_or(20), 1, 50);
            let mut base = req.clone();
            base.cursor = None;
            base.limit = None;
            let fp = request_fingerprint(&base);
            let cursor = req.cursor.as_deref().filter(|s| !s.trim().is_empty()).map(|c| decode_cursor(c, "thread", &fp)).transpose()?;
            let snap = match &cursor {
                Some(c) => c.snap,
                None => self.current_seq()?,
            };
            let Some((info, source)) = self.db.first(
                "SELECT thread_id,kind,title,started_ms,ended_ms,source_id FROM threads WHERE thread_id=?",
                &[text(&req.thread_id)],
                |r| {
                    (
                        ThreadInfo { thread_id: r.text(0), kind: r.text(1), title: r.opt_text(2), started: self.iso(r.opt_int(3)), ended: self.iso(r.opt_int(4)) },
                        r.text(5),
                    )
                },
            )?
            else {
                return Err(TimelineError::NotFound(req.thread_id.clone()).into());
            };
            if !ctx.allowed_sources.contains(&source) {
                return Err(QErr::ScopeMiss(req.thread_id.clone()));
            }
            let mut f = self.event_filter(ctx, None, None, None, Some(&req.thread_id), None, None, Some(snap));
            if let (Some(t), Some(id)) = (cursor.as_ref().and_then(|c| c.t), cursor.as_ref().and_then(|c| c.id.clone())) {
                f.add(format!("({SORT_KEY} > ? OR ({SORT_KEY} = ? AND e.event_id > ?))"), vec![int(t), int(t), text(id)]);
            }
            let rows = self.db.query(
                &format!("SELECT {SUMMARY_COLUMNS},{SORT_KEY} {SUMMARY_FROM} WHERE {} ORDER BY {SORT_KEY}, e.event_id LIMIT ?", f.sql()),
                &f.with(vec![int(limit + 1)]),
                |r| (r.text(13), self.summary(r,ctx), r.int(14)),
            )?;
            let page: Vec<_> = rows.iter().take(limit as usize).cloned().collect();
            let ids: Vec<String> = page.iter().map(|p| p.1.event_id.clone()).collect();
            let mut by_event: HashMap<String, Vec<TextRow>> = HashMap::new();
            for t in self.paragraphs(&ids, ctx, None)? {
                by_event.entry(t.event_id.clone()).or_default().push(t);
            }
            let messages: Vec<ThreadMessage> = page
                .iter()
                .map(|(raw, s, _)| {
                    let payload = json::parse(raw);
                    let mut parts: Vec<&TextRow> = by_event.get(&s.event_id).map(|v| v.iter().collect()).unwrap_or_default();
                    parts.sort_by(|a, b| a.field.cmp(&b.field).then(a.ord.cmp(&b.ord)));
                    let joined = parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n\n");
                    let (t, cut) = truncate(&joined, 4000);
                    ThreadMessage {
                        event_id: s.event_id.clone(),
                        kind: s.kind.clone(),
                        start: s.start.clone(),
                        role: payload.as_ref().and_then(|p| json::str_at(p, "role")).map(String::from),
                        speaker: payload.as_ref().and_then(|p| json::str_at(p, "speaker_label")).map(String::from),
                        text: ctx.scope.text.then_some(t),
                        truncated: cut,
                        source_label: s.source_label.clone(),
                    }
                })
                .collect();
            let new_records = self.current_seq()? > snap;
            let cur = |i: usize| {
                let mut c = PageCursor::new("thread", snap, &fp);
                c.t = Some(page[i].2);
                c.id = Some(page[i].1.event_id.clone());
                encode_cursor(&c)
            };
            let build = |kept: &[ThreadMessage], trimmed: bool| {
                let has_next = trimmed || (rows.len() as i64 > limit && kept.len() == page.len());
                ThreadResponse {
                    thread: info.clone(),
                    messages: kept.to_vec(),
                    next_cursor: if has_next && !kept.is_empty() { Some(cur(kept.len() - 1)) } else { None },
                    has_new_records: new_records,
                    truncated_by_size: trimmed,
                }
            };
            let (kept, trimmed) = fit(&messages, ctx.scope.max_result_bytes, |k| build(k, false))?;
            Ok(build(&kept, trimmed))
        })
    }

    // MARK: media

    pub fn media(&self, req: &MediaRequest, client_id: &str) -> Result<MediaResponse> {
        self.audited("media", client_id, req, |_: &MediaResponse| Some(1), |ctx| {
            if !ctx.scope.media {
                return Err(TimelineError::Denied("media_not_permitted".into()).into());
            }
            let variant = req.variant.clone().unwrap_or_else(|| "preview".into());
            if variant != "preview" && variant != "original" {
                return Err(TimelineError::InvalidInput(format!("variant: {variant}")).into());
            }
            if !self.summaries(std::slice::from_ref(&req.event_id), ctx)?.contains_key(&req.event_id) {
                if self.db.scalar("SELECT COUNT(*) FROM events WHERE event_id=?", &[text(&req.event_id)])? > 0 {
                    return Err(QErr::ScopeMiss(req.event_id.clone()));
                }
                return Err(TimelineError::NotFound(req.event_id.clone()).into());
            }
            let blob = |role: &str| {
                self.db.first(
                    "SELECT b.sha256,b.mime_type,b.byte_count,b.rel_path FROM event_blobs eb JOIN blobs b ON b.sha256=eb.sha256 WHERE eb.event_id=? AND eb.role=? LIMIT 1",
                    &[text(&req.event_id), text(role)],
                    |r| (r.text(0), r.text(1), r.int(2), r.text(3)),
                )
            };
            let found = match blob(if variant == "preview" { "preview" } else { "original" })? {
                Some(b) => Some((b, variant.clone())),
                None if variant == "preview" => blob("original")?.map(|b| (b, "original".to_string())),
                None => None,
            };
            let Some((b, served)) = found else {
                return Err(TimelineError::NotFound("该事件没有媒体".into()).into());
            };
            let budget = ctx.scope.max_result_bytes / 2; // base64 inflates by 4/3 and the envelope needs room
            let mut data = None;
            let (mut served_mime, mut served_bytes) = (b.1.clone(), b.2);
            if b.2 <= budget * 3 / 4 {
                let bytes = std::fs::read(self.root.join(&b.3)).map_err(TimelineError::from)?;
                if sha256_hex(&bytes) != b.0 {
                    return Err(TimelineError::BlobMismatch("原件 hash 与记录不一致".into()).into());
                }
                // Compressed JSON is served as the JSON it stands for.
                let (bytes, mime) = crate::json_blobs::logical_bytes(bytes, &b.1).map_err(TimelineError::from)?;
                if bytes.len() as i64 <= budget * 3 / 4 {
                    served_mime = mime;
                    served_bytes = bytes.len() as i64;
                    data = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
                }
            }
            Ok(MediaResponse {
                event_id: req.event_id.clone(),
                sha256: b.0,
                mime_type: served_mime,
                byte_count: served_bytes,
                variant_served: served,
                too_large: data.is_none(),
                base64: data,
                range_applied: false,
            })
        })
    }

    // MARK: usage

    pub fn usage(&self, req: &UsageRequest, client_id: &str) -> Result<UsageResponse> {
        self.audited("usage", client_id, req, |r: &UsageResponse| Some(r.rows.len() as i64), |ctx| {
            let group_by = req.group_by.clone().unwrap_or_else(|| "feature".into());
            if !["feature", "model", "provider", "day"].contains(&group_by.as_str()) {
                return Err(TimelineError::InvalidInput(format!("group_by: {group_by}")).into());
            }
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), None, false)?;
            let offset_seconds = self.clock.zone.offset_seconds(self.clock.now_ms());
            let key = match group_by.as_str() {
                "feature" => "feature".to_string(),
                "model" => "provider_id || '/' || model_id".to_string(),
                "provider" => "provider_id".to_string(),
                _ => format!("strftime('%Y-%m-%d', started_ms/1000 + {offset_seconds}, 'unixepoch')"),
            };
            let mut clauses = vec!["1=1".to_string()];
            let mut params: Vec<P> = vec![];
            if let Some(ws) = ws {
                clauses.push("started_ms >= ?".into());
                params.push(int(ws));
            }
            if let Some(we) = we {
                clauses.push("started_ms < ?".into());
                params.push(int(we));
            }
            let mut rows = self.db.query(
                &format!(
                    "SELECT {key}, COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                       COALESCE(SUM(CASE WHEN cost_source='provider' THEN cost END),0), COALESCE(SUM(CASE WHEN cost_source='estimated' THEN cost END),0),
                       SUM(CASE WHEN cost_source IS NULL OR cost_source='unknown' THEN 1 ELSE 0 END), COUNT(DISTINCT currency), MIN(currency)
                FROM ai_calls WHERE {} GROUP BY 1 ORDER BY 1",
                    clauses.join(" AND ")
                ),
                &params,
                |r| UsageRow {
                    origin: "app".into(),
                    key: r.text(0),
                    calls: r.int(1),
                    input_tokens: r.int(2),
                    output_tokens: r.int(3),
                    cache_read_tokens: r.int(4),
                    cache_write_tokens: r.int(5),
                    cost_actual: r.double(6),
                    cost_estimated: r.double(7),
                    cost_unknown_calls: r.int(8),
                    currency: if r.int(9) > 1 { Some("mixed".into()) } else { r.opt_text(10) },
                },
            )?;
            if req.include_external == Some(true) {
                let kinds = vec!["ai.turn".to_string()];
                let mut f = self.event_filter(ctx, None, None, Some(&kinds), None, ws, we, None);
                f.add("json_extract(e.payload_json,'$.usage') IS NOT NULL", vec![]);
                let ext_key = match group_by.as_str() {
                    "feature" | "provider" => "s.kind".to_string(),
                    "model" => "COALESCE(json_extract(e.payload_json,'$.model'),'unknown')".to_string(),
                    _ => format!("strftime('%Y-%m-%d', COALESCE(e.start_ms,e.ingested_ms)/1000 + {offset_seconds}, 'unixepoch')"),
                };
                let tok = |n: &str| format!("COALESCE(SUM(json_extract(e.payload_json,'$.usage.{n}')),0)");
                rows.extend(self.db.query(
                    &format!(
                        "SELECT {ext_key}, COUNT(*), {}, {}, {}, {}
                    FROM events e JOIN sources s ON s.source_id=e.source_id WHERE {} GROUP BY 1 ORDER BY 1",
                        tok("input"),
                        tok("output"),
                        tok("cache_read"),
                        tok("cache_write"),
                        f.sql()
                    ),
                    &f.params,
                    |r| UsageRow {
                        origin: "external".into(),
                        key: r.text(0),
                        calls: r.int(1),
                        input_tokens: r.int(2),
                        output_tokens: r.int(3),
                        cache_read_tokens: r.int(4),
                        cache_write_tokens: r.int(5),
                        cost_actual: 0.0,
                        cost_estimated: 0.0,
                        cost_unknown_calls: r.int(1),
                        currency: None,
                    },
                )?);
            }
            let mut total = UsageRow {
                origin: "all".into(),
                key: "total".into(),
                calls: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_actual: 0.0,
                cost_estimated: 0.0,
                cost_unknown_calls: 0,
                currency: None,
            };
            for r in &rows {
                total.calls += r.calls;
                total.input_tokens += r.input_tokens;
                total.output_tokens += r.output_tokens;
                total.cache_read_tokens += r.cache_read_tokens;
                total.cache_write_tokens += r.cache_write_tokens;
                total.cost_actual += r.cost_actual;
                total.cost_estimated += r.cost_estimated;
                total.cost_unknown_calls += r.cost_unknown_calls;
                if let Some(c) = &r.currency {
                    total.currency = Some(if total.currency.is_none() || total.currency.as_ref() == Some(c) { c.clone() } else { "mixed".into() });
                }
            }
            Ok(UsageResponse { window: self.time_window(ws, we), group_by, rows, totals: total })
        })
    }

    // MARK: propose

    pub fn propose(&self, req: &ProposeRequest, client_id: &str) -> Result<ProposeResponse> {
        self.audited("propose", client_id, req, |_: &ProposeResponse| Some(1), |ctx| {
            if !ctx.scope.propose {
                return Err(TimelineError::Denied("propose_not_permitted".into()).into());
            }
            if !ctx.scope.propose_kinds.is_empty() && !ctx.scope.propose_kinds.contains(&req.kind) {
                return Err(TimelineError::Denied("propose_kind_not_permitted".into()).into());
            }
            let kind_len = req.kind.chars().count();
            if kind_len == 0 || kind_len > 64 || !req.payload.is_object() {
                return Err(TimelineError::InvalidInput("提案需要 kind 和对象 payload".into()).into());
            }
            let evidence: Vec<String> = req.evidence.clone().unwrap_or_default().into_iter().collect::<BTreeSet<_>>().into_iter().collect();
            if evidence.len() > 100 {
                return Err(TimelineError::InvalidInput("证据过多".into()).into());
            }
            let visible = self.summaries(&evidence, ctx)?;
            if visible.len() != evidence.len() {
                return Err(TimelineError::Denied("evidence_out_of_scope".into()).into());
            }
            let now = self.clock.now_ms();
            let id = uuid_v7(Some(now));
            self.db.execute(
                "INSERT INTO proposals(proposal_id,client_id,kind,payload_json,evidence_json,status,created_ms) VALUES(?,?,?,?,?,'pending',?)",
                &[text(&id), text(&ctx.id), text(&req.kind), text(json::canonical(&req.payload)), text(json::canonical_of(&evidence)), int(now)],
            )?;
            Ok(ProposeResponse { proposal_id: id, status: "pending".into() })
        })
    }
}
