use super::core::*;
use super::types::*;
use crate::chunk::TextChunker;
use crate::db::{int, placeholders, text, RowExt, P};
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use std::collections::HashMap;

const ALL_FIELDS: &[&str] = &["prompt", "body", "selection", "title", "transcript", "ocr", "vlm", "message", "subject", "note", "quoted"];
const CANDIDATE_POOL: i64 = 1000;
const SCAN_LIMIT: i64 = 5000;

#[derive(Clone)]
struct Candidate {
    event_id: String,
    field: Option<String>,
    ord: Option<i64>,
    text: Option<String>,
    visible: Option<bool>,
    focus_distance: Option<i64>,
    is_new: bool,
    char_start: Option<i64>,
    relevance: f64,
}

impl Candidate {
    fn meta(event_id: String, relevance: f64, is_new: bool) -> Candidate {
        Candidate { event_id, field: None, ord: None, text: None, visible: None, focus_distance: None, is_new, char_start: None, relevance }
    }
}

/// Scalar offsets of every case-insensitive (ASCII) occurrence of any term.
fn occurrences(terms: &[String], text_: &str) -> Vec<(usize, usize)> {
    let hay: Vec<char> = ascii_lower(text_).chars().collect();
    let mut out = Vec::new();
    for term in terms {
        let needle: Vec<char> = ascii_lower(term).chars().collect();
        if needle.is_empty() || hay.len() < needle.len() {
            continue;
        }
        let mut i = 0;
        while i <= hay.len() - needle.len() {
            if hay[i..i + needle.len()] == needle[..] {
                out.push((i, i + needle.len()));
                i += needle.len();
            } else {
                i += 1;
            }
        }
    }
    out.sort_by_key(|o| o.0);
    out
}

fn make_snippet(text_: &str, terms: &[String], max_chars: usize) -> (Snippet, Option<usize>) {
    let scalars: Vec<char> = text_.chars().collect();
    let matches = occurrences(terms, text_);
    let first = matches.first().map(|m| m.0);
    if scalars.len() <= max_chars {
        let highlights = matches.iter().map(|m| vec![m.0 as i64, m.1 as i64]).collect();
        return (Snippet { text: text_.to_string(), highlights, truncated: false }, first);
    }
    let mut start = first.unwrap_or(0).saturating_sub(max_chars / 4);
    if start + max_chars > scalars.len() {
        start = scalars.len() - max_chars;
    }
    let end = start + max_chars;
    let prefix = usize::from(start > 0);
    let mut view = String::new();
    if prefix == 1 {
        view.push('…');
    }
    view.extend(&scalars[start..end]);
    if end < scalars.len() {
        view.push('…');
    }
    let highlights = matches
        .iter()
        .filter(|m| m.0 >= start && m.1 <= end)
        .map(|m| vec![(m.0 - start + prefix) as i64, (m.1 - start + prefix) as i64])
        .collect();
    (Snippet { text: view, highlights, truncated: true }, first)
}

fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn text_candidate(r: &rusqlite::Row, relevance: f64) -> Candidate {
    Candidate {
        event_id: r.text(0),
        field: Some(r.text(1)),
        ord: Some(r.int(2)),
        text: Some(r.text(7)),
        visible: r.opt_int(3).map(|v| v == 1),
        focus_distance: r.opt_int(4),
        is_new: r.int(5) == 1,
        char_start: r.opt_int(6),
        relevance,
    }
}

impl TimelineStore {
    pub fn search(&self, req: &SearchRequest, client_id: &str) -> Result<SearchResponse> {
        self.audited("search", client_id, req, |r: &SearchResponse| Some(r.hits.len() as i64), |ctx| {
            let limit = clamp(req.limit.unwrap_or(10), 1, 50);
            let max_chars = clamp(req.max_chars.unwrap_or(600), 40, 4000) as usize;
            let attention = req.attention.clone().unwrap_or_else(|| "focus".into());
            if attention != "focus" && attention != "all" {
                return Err(TimelineError::InvalidInput("attention 必须是 focus 或 all".into()).into());
            }
            if let Some(fields) = &req.fields {
                for f in fields {
                    if !ALL_FIELDS.contains(&f.as_str()) {
                        return Err(TimelineError::InvalidInput(format!("fields: {f}")).into());
                    }
                }
            }
            let mut base = req.clone();
            base.cursor = None;
            base.limit = None;
            let fp = request_fingerprint(&base);
            let cursor = req.cursor.as_deref().filter(|s| !s.trim().is_empty()).map(|c| decode_cursor(c, "search", &fp)).transpose()?;
            let snap = match &cursor {
                Some(c) => c.snap,
                None => self.current_seq()?,
            };
            let (ws, we) = self.window(req.start.as_deref(), req.end.as_deref(), cursor.as_ref(), false)?;
            let offset = cursor.as_ref().and_then(|c| c.off).unwrap_or(0).max(0) as usize;

            let q = TextChunker::normalise(req.q.as_deref().unwrap_or("")).replace('\n', " ");
            let mut terms: Vec<String> = Vec::new();
            for t in q.split(' ').filter(|t| !t.is_empty()) {
                if !terms.iter().any(|x| x == t) {
                    terms.push(t.to_string());
                }
            }
            if terms.len() > 8 || terms.iter().any(|t| t.chars().count() > 128) {
                return Err(TimelineError::InvalidInput("查询词过多或过长".into()).into());
            }
            let long_terms: Vec<String> = terms.iter().filter(|t| t.chars().count() >= 3).cloned().collect();
            let short_terms: Vec<String> = terms.iter().filter(|t| t.chars().count() < 3).cloned().collect();

            let mut filter = self.event_filter(ctx, req.apps.as_ref(), req.sources.as_ref(), req.kinds.as_ref(), req.thread_id.as_deref(), ws, we, Some(snap));
            filter.add("NOT EXISTS (SELECT 1 FROM sources a WHERE a.source_id=e.source_id AND a.kind='agent.transcript' AND COALESCE(json_extract(a.policy_json,'$.search'),0)<>1)", vec![]);
            let mut text_filter = filter.clone();
            text_filter.add("EXISTS (SELECT 1 FROM sources s WHERE s.source_id=e.source_id AND (s.kind='agent.transcript' OR json_extract(s.policy_json,'$.store_text')=1)) AND NOT EXISTS (SELECT 1 FROM sources old WHERE (old.source_id=json_extract(e.payload_json,'$.native_legacy_source_id') OR old.source_id IN (SELECT value FROM json_each(e.payload_json,'$.native_legacy_sources'))) AND json_extract(old.policy_json,'$.store_text')<>1)",vec![]);
            let fields = req.fields.as_ref().filter(|f| !f.is_empty());
            if let Some(fields) = fields {
                text_filter.add(format!("et.field IN ({})", placeholders(fields.len())), fields.iter().map(text).collect());
            }
            if attention == "focus" {
                text_filter.add("et.is_new=1 AND COALESCE(et.visible,1)=1 AND COALESCE(et.focus_distance,0)<=3", vec![]);
            }

            let mut candidates: Vec<Candidate> = Vec::new();
            let mut scan_limited = false;
            let (w_visible, w_near, w_new, half_life) = self.rank_weights();

            if terms.is_empty() {
                // filter-only listing: newest events, with their most attentive paragraph as the snippet
                let ids = self.db.query(
                    &format!("SELECT e.event_id FROM events e WHERE {} ORDER BY {SORT_KEY} DESC, e.event_id DESC LIMIT ?", filter.sql()),
                    &filter.with(vec![int(CANDIDATE_POOL)]),
                    |r| r.text(0),
                )?;
                let rows = self.paragraphs(&ids, ctx, fields)?;
                let mut first_by_event: HashMap<String, TextRow> = HashMap::new();
                for r in rows {
                    let attentive = attention == "all" || (r.is_new && r.visible != Some(false) && r.focus_distance.unwrap_or(0) <= 3);
                    if attentive && !first_by_event.contains_key(&r.event_id) {
                        first_by_event.insert(r.event_id.clone(), r);
                    }
                }
                for id in ids {
                    match first_by_event.get(&id) {
                        Some(t) => candidates.push(Candidate {
                            event_id: id.clone(),
                            field: Some(t.field.clone()),
                            ord: Some(t.ord),
                            text: Some(t.text.clone()),
                            visible: t.visible,
                            focus_distance: t.focus_distance,
                            is_new: t.is_new,
                            char_start: t.char_start,
                            relevance: 1.0,
                        }),
                        None => candidates.push(Candidate::meta(id, 0.5, false)),
                    }
                }
            } else {
                let mut short_conds = String::new();
                let mut short_params: Vec<P> = Vec::new();
                for t in &short_terms {
                    short_conds += " AND instr(lower(c.text), ?) > 0";
                    short_params.push(text(ascii_lower(t)));
                }
                if ctx.scope.text {
                    if !long_terms.is_empty() {
                        let m = long_terms.iter().map(|t| fts_phrase(t)).collect::<Vec<_>>().join(" AND ");
                        let mut params = vec![text(m)];
                        params.extend(text_filter.params.iter().cloned());
                        params.extend(short_params.iter().cloned());
                        params.push(int(CANDIDATE_POOL));
                        let rows = self.db.query(
                            &format!(
                                "SELECT et.event_id,et.field,et.ord,et.visible,et.focus_distance,et.is_new,et.char_start,c.text,bm25(text_fts)
                            FROM text_fts JOIN text_chunks c ON c.chunk_id=text_fts.rowid JOIN event_text et ON et.chunk_id=c.chunk_id JOIN events e ON e.event_id=et.event_id
                            WHERE text_fts MATCH ? AND {}{short_conds} ORDER BY bm25(text_fts) LIMIT ?",
                                text_filter.sql()
                            ),
                            &params,
                            |r| text_candidate(r, 1.0 + (-r.double(8)).min(5.0)),
                        )?;
                        candidates.extend(rows);
                    } else {
                        // Two-character (e.g. Chinese) queries: structured filter first, then a bounded literal scan.
                        let inner = format!(
                            "SELECT et.event_id AS event_id,et.field AS field,et.ord AS ord,et.visible AS visible,et.focus_distance AS focus_distance,et.is_new AS is_new,
                                   et.char_start AS char_start,c.text AS text FROM event_text et JOIN events e ON e.event_id=et.event_id JOIN text_chunks c ON c.chunk_id=et.chunk_id
                            WHERE {} ORDER BY {SORT_KEY} DESC, et.rowid DESC LIMIT ?",
                            text_filter.sql()
                        );
                        let conds = short_terms.iter().map(|_| "instr(lower(text), ?) > 0").collect::<Vec<_>>().join(" AND ");
                        let mut params = text_filter.with(vec![int(SCAN_LIMIT)]);
                        params.extend(short_terms.iter().map(|t| text(ascii_lower(t))));
                        params.push(int(CANDIDATE_POOL));
                        let rows = self.db.query(
                            &format!("SELECT event_id,field,ord,visible,focus_distance,is_new,char_start,text,0 FROM ({inner}) AS scanned WHERE {conds} LIMIT ?"),
                            &params,
                            |r| text_candidate(r, 1.0),
                        )?;
                        candidates.extend(rows);
                        let scanned = self.db.scalar(
                            &format!(
                                "SELECT COUNT(*) FROM (SELECT 1 FROM event_text et JOIN events e ON e.event_id=et.event_id WHERE {} LIMIT ?)",
                                text_filter.sql()
                            ),
                            &text_filter.with(vec![int(SCAN_LIMIT + 1)]),
                        )?;
                        scan_limited = scanned > SCAN_LIMIT;
                    }
                }
                // titles, app names and URLs match regardless of paragraph attention (and when text is not permitted)
                let meta_expr = "lower(COALESCE(title,'') || ' ' || COALESCE(app_name,'') || ' ' || COALESCE(url,''))";
                let short_meta: Vec<String> = short_terms.iter().map(|_| format!("instr({meta_expr}, ?) > 0")).collect();
                if !long_terms.is_empty() {
                    let m = long_terms.iter().map(|t| fts_phrase(t)).collect::<Vec<_>>().join(" AND ");
                    let short_sql = if short_meta.is_empty() {
                        String::new()
                    } else {
                        " AND ".to_string()
                            + &short_meta
                                .join(" AND ")
                                .replace("title", "e.title")
                                .replace("app_name", "e.app_name")
                                .replace("url", "e.url")
                    };
                    let mut params = vec![text(m)];
                    params.extend(filter.params.iter().cloned());
                    params.extend(short_terms.iter().map(|t| text(ascii_lower(t))));
                    params.push(int(CANDIDATE_POOL));
                    let ids = self.db.query(
                        &format!(
                            "SELECT e.event_id, bm25(meta_fts) FROM meta_fts JOIN events e ON e.rowid=meta_fts.rowid
                        WHERE meta_fts MATCH ? AND {}{short_sql} ORDER BY bm25(meta_fts) LIMIT ?",
                            filter.sql()
                        ),
                        &params,
                        |r| (r.text(0), 1.5 + (-r.double(1)).min(5.0)),
                    )?;
                    for (id, rel) in ids {
                        candidates.push(Candidate::meta(id, rel, true));
                    }
                } else {
                    let mut params = filter.with(vec![int(SCAN_LIMIT)]);
                    params.extend(short_terms.iter().map(|t| text(ascii_lower(t))));
                    params.push(int(CANDIDATE_POOL));
                    let ids = self.db.query(
                        &format!(
                            "SELECT event_id FROM (SELECT e.event_id AS event_id, e.title AS title, e.app_name AS app_name, e.url AS url FROM events e WHERE {}
                                              ORDER BY {SORT_KEY} DESC LIMIT ?) WHERE {} LIMIT ?",
                            filter.sql(),
                            short_meta.join(" AND ")
                        ),
                        &params,
                        |r| r.text(0),
                    )?;
                    for id in ids {
                        candidates.push(Candidate::meta(id, 1.5, true));
                    }
                }
            }

            // Manual labels are a live projection, never copied into the raw-text index.
            if ctx.scope.text && ctx.scope.derived && !terms.is_empty() {
                let ids=self.db.query(&format!("SELECT DISTINCT e.event_id FROM events e JOIN corrections c ON c.target_id=e.event_id WHERE c.target_type='event_label' AND json_extract(c.value_json,'$.format')='candid-annotation/1' AND ({})",filter.sql()),&filter.params,|r|r.text(0))?;
                for id in ids {
                    if let Some(v)=self.visible_annotation(&id,ctx)? {
                        let label=format!("{} {}",v["title"].as_str().unwrap_or(""),v["notes"].as_str().unwrap_or(""));
                        if terms.iter().all(|t|ascii_lower(&label).contains(&ascii_lower(t))) {
                            let mut c=Candidate::meta(id,2.0,true); c.text=Some(label); c.field=None; candidates.push(c);
                        }
                    }
                }
            }

            // rank: best candidate per event (relevance × attention × recency), deterministic tie-break
            let mut unique: Vec<String> = candidates.iter().map(|c| c.event_id.clone()).collect();
            unique.sort();
            unique.dedup();
            let times = self.event_times(&unique)?;
            let now = self.clock.now_ms();
            let mut best: HashMap<String, (Candidate, f64)> = HashMap::new();
            let mut best_text: HashMap<String, (Candidate, f64)> = HashMap::new();
            for c in candidates {
                let t = times.get(&c.event_id).copied().unwrap_or(now);
                let age_days = ((now - t) as f64 / 86_400_000.0).max(0.0);
                let recency = 0.5 + 0.5 * 0.5f64.powf(age_days / half_life);
                let mut att = 1.0;
                if c.visible == Some(true) {
                    att *= w_visible;
                }
                if matches!(c.focus_distance, Some(d) if d <= 3) {
                    att *= w_near;
                }
                if c.is_new {
                    att *= w_new;
                }
                let score = c.relevance * att * recency;
                if best.get(&c.event_id).map(|b| score > b.1).unwrap_or(true) {
                    best.insert(c.event_id.clone(), (c.clone(), score));
                }
                if c.text.is_some() && best_text.get(&c.event_id).map(|b| score > b.1).unwrap_or(true) {
                    best_text.insert(c.event_id.clone(), (c, score));
                }
            }
            // an event matched by both its title and a paragraph shows the paragraph (more informative) with the higher score
            for (id, entry) in best.iter_mut() {
                if entry.0.text.is_none() {
                    if let Some(t) = best_text.get(id) {
                        entry.0 = t.0.clone();
                    }
                }
            }
            let mut ranked: Vec<(Candidate, f64)> = best.into_values().collect();
            ranked.sort_by(|a, b| {
                if a.1 != b.1 {
                    return b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal);
                }
                let ta = times.get(&a.0.event_id).copied().unwrap_or(0);
                let tb = times.get(&b.0.event_id).copied().unwrap_or(0);
                tb.cmp(&ta).then_with(|| b.0.event_id.cmp(&a.0.event_id))
            });
            let page: Vec<(Candidate, f64)> = ranked.iter().skip(offset).take(limit as usize).cloned().collect();
            let ids: Vec<String> = page.iter().map(|p| p.0.event_id.clone()).collect();
            let summary_by_id = self.summaries(&ids, ctx)?;
            let mut hits = Vec::new();
            for (c, score) in &page {
                let Some(s) = summary_by_id.get(&c.event_id) else { continue };
                let (mut snippet, mut att) = (None, None);
                let ref_;
                if let (Some(t), true) = (&c.text, ctx.scope.text) {
                    let (snip, first) = make_snippet(t, &terms, max_chars);
                    snippet = Some(snip);
                    att = Some(Attention { visible: c.visible, focus_distance: c.focus_distance, is_new: c.is_new });
                    let begin = c.char_start.unwrap_or(0) + first.unwrap_or(0) as i64;
                    let term_len = terms.first().map(|t| t.chars().count() as i64).unwrap_or(0);
                    ref_ = Some(EventRef {
                        event_id: c.event_id.clone(),
                        field: c.field.clone(),
                        chunk_ord: c.ord,
                        char_range: c.char_start.map(|_| vec![begin, begin + term_len]),
                    });
                } else if let (Some(title), false) = (&s.title, terms.is_empty()) {
                    snippet = Some(make_snippet(title, &terms, max_chars).0);
                    ref_ = Some(EventRef { event_id: c.event_id.clone(), field: Some("title".into()), chunk_ord: None, char_range: None });
                } else {
                    ref_ = Some(EventRef { event_id: c.event_id.clone(), field: None, chunk_ord: None, char_range: None });
                }
                hits.push(SearchHit {
                    event_id: c.event_id.clone(),
                    kind: s.kind.clone(),
                    start: s.start.clone(),
                    app: s.app.clone(),
                    title: s.title.clone(),
                    source_label: s.source_label.clone(),
                    layer: s.layer.clone(),
                    snippet,
                    attention: att,
                    score: (score * 1000.0).round() / 1000.0,
                    r#ref: ref_,
                });
            }
            let cov = self.coverage(ctx, ws, we)?;
            let new_records = self.current_seq()? > snap;
            let window = self.time_window(ws, we);
            let total = ranked.len();
            let next_cursor = |consumed: usize| -> Option<String> {
                let used = offset + consumed;
                (used < total).then(|| {
                    let mut c = PageCursor::new("search", snap, &fp);
                    c.off = Some(used as i64);
                    c.ws = ws;
                    c.we = we;
                    encode_cursor(&c)
                })
            };
            let build = |kept: &[SearchHit], trimmed: bool, next: Option<String>| SearchResponse {
                window: window.clone(),
                hits: kept.to_vec(),
                next_cursor: next,
                has_new_records: new_records,
                truncated_by_size: trimmed,
                scan_limited,
                attention: attention.clone(),
                coverage: cov.clone(),
            };
            let (kept, trimmed) = fit(&hits, ctx.scope.max_result_bytes, |k| build(k, false, next_cursor(k.len())))?;
            let consumed = if kept.len() == hits.len() { page.len() } else { kept.len() };
            Ok(build(&kept, trimmed, next_cursor(consumed)))
        })
    }

    fn event_times(&self, ids: &[String]) -> Result<HashMap<String, i64>> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(400) {
            let rows = self.db.query(
                &format!("SELECT event_id, COALESCE(start_ms, ingested_ms) FROM events WHERE event_id IN ({})", placeholders(chunk.len())),
                &chunk.iter().map(text).collect::<Vec<_>>(),
                |r| (r.text(0), r.int(1)),
            )?;
            out.extend(rows);
        }
        Ok(out)
    }
}
