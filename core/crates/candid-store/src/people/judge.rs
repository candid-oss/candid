//! Which alias pairs are worth asking a model about, and what to do with the answer (docs/plans/data-layer-v1.md
//! §1.3–1.4, call #7).
//!
//! 1. **Candidates** (rules only): the same name in different apps, one name containing the other, a mail display
//!    name equal to a chat name, a recording speaker name equal to an alias. Pairs vetoed by rule, already corrected
//!    by you, already in one person, or already answered (see [`TimelineStore::people_pair_key`]) are left out.
//! 2. **Job** `people.judge` (P3, feature `people-judge`, `summary` role): at most [`BATCH`] pairs per call; the prompt
//!    is `core/prompts/people-judge.md` (`<root>/prompts/people-judge.md` overrides it).
//! 3. **Answer**: `same | different | unsure` plus one sentence per pair. `same` becomes an automatic merge only with
//!    a rule-level corroboration ([`TimelineStore::people_corroboration`]); otherwise it is shown as a candidate.
//!    Results are cached as [`PeopleVerdict`]s, so recomputing people never calls a model.
//!
//! The cache key of a pair is a hash of both people's alias sets, the prompt (version and text) and the model: a new
//! alias on either side, an edited prompt or a different model asks again; `unsure` is not asked again for 30 days.

use super::*;
use crate::{sha256_hex, TimelineError};

pub(crate) const FEATURE: &str = "people-judge";
pub(crate) const JOB_KIND: &str = "people.judge";
const PROMPT: &str = include_str!("../../../../prompts/people-judge.md");
const PROMPT_VERSION: &str = "people-judge/1";
/// Pairs per model call.
pub const BATCH: usize = 10;
/// Batches queued per scan; the next scan follows when a verdict lands.
const MAX_BATCHES: usize = 5;
const MAX_TRIES: i64 = 3;
const UNSURE_MS: i64 = 30 * 86_400_000;
const SCAN: &str = "people.judge.scan/1";
const MESSAGES_PER_SIDE: i64 = 4;
const MESSAGE_CHARS: usize = 80;

fn bad(s: &str) -> TimelineError {
    TimelineError::InvalidInput(s.into())
}

/// Verdict, cache key and time of an answer already given about two people.
type Answers = HashMap<(String, String), (String, String, i64)>;

/// A pair of alias keys the rules find worth asking about.
#[derive(Clone, Debug, PartialEq)]
pub struct PeopleCandidate {
    pub a: String,
    pub b: String,
    /// `same_name`, `contains`, `mail_name` or `speaker_name`.
    pub reason: &'static str,
    /// Cache key of the question (alias sets of both people, prompt, model).
    pub key: String,
}

/// What the rules know about one alias key, from all messages.
struct AliasInfo {
    kind: &'static str,
    names: BTreeSet<String>,
    count: i64,
    groups: HashSet<String>,
    mail_names: HashSet<String>,
}

fn clip(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n { flat } else { flat.chars().take(n).collect::<String>() + "…" }
}

fn has_cjk(s: &str) -> bool {
    !s.is_ascii()
}

/// A name long enough to compare: two characters of Chinese, four letters otherwise.
fn comparable(n: &str) -> bool {
    let len = n.chars().count();
    !generic_speaker(n) && if has_cjk(n) { len >= 2 } else { len >= 4 }
}

/// `short` sits at the start or end of `long`, or is one of its words.
fn contained(short: &str, long: &str) -> bool {
    long.len() > short.len() && (long.starts_with(short) || long.ends_with(short) || long.split(|c: char| !c.is_alphanumeric()).any(|w| w == short))
}

/// Latin words of a name with bracketed notes and mail tails dropped, in sorted order: "赵康然 Zhao Kangran - a@b.c",
/// "Kangran Zhao (250010172)" and "kangran zhao" all give `kangran zhao`. At least two words, or nothing.
fn word_set(name: &str) -> Option<String> {
    let mut plain = String::new();
    let mut depth = 0;
    for c in name.split(" - ").next().unwrap_or("").chars() {
        match c {
            '(' | '（' | '[' => depth += 1,
            ')' | '）' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let mut words: Vec<String> = plain.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| w.len() >= 2).map(str::to_lowercase).collect();
    words.sort();
    (words.len() >= 2).then(|| words.join(" "))
}

impl TimelineStore {
    pub(crate) fn judge_prompt(&self) -> String {
        std::fs::read_to_string(self.root.join("prompts").join("people-judge.md")).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| PROMPT.to_string())
    }

    /// The model that would judge, as `provider/model` (or null without one); part of the cache key.
    pub(crate) fn judge_model(&self) -> Result<Value> {
        let c = self.model_config()?;
        Ok(["summary", "title"].iter().find_map(|role| c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == *role && a["model"].is_object()).map(|a| a["model"].clone())).unwrap_or(Value::Null))
    }

    fn judge_alias_infos(&self) -> Result<BTreeMap<String, AliasInfo>> {
        let raws = self.people_raws(0)?;
        let groups = self.people_groups(&raws, true)?;
        let mut out: BTreeMap<String, AliasInfo> = BTreeMap::new();
        for r in &raws {
            let Some((key, kind, alias, display)) = classify(r, &groups) else { continue };
            if kind == "self" {
                continue;
            }
            let info = out.entry(key).or_insert_with(|| AliasInfo { kind, names: BTreeSet::new(), count: 0, groups: HashSet::new(), mail_names: HashSet::new() });
            info.count += 1;
            match kind {
                "handle" => {
                    info.names.insert(alias.split_once('/').map(|x| x.1).unwrap_or(&alias).to_string());
                }
                "email" => {
                    if let Some(d) = display.as_deref().map(normalize).filter(|d| !d.is_empty()) {
                        info.names.insert(d.clone());
                        info.mail_names.insert(d);
                    }
                }
                _ => {
                    info.names.insert(alias);
                }
            }
            if kind == "handle" && groups.contains(&r.conv) {
                info.groups.insert(r.conv.clone());
            }
        }
        Ok(out)
    }

    /// `key → person id` as last computed (rejected people left out).
    fn people_registry(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .db
            .query("SELECT key,value FROM store_meta WHERE key GLOB 'people.node/1:*' AND value<>'-'", &[], |r| (r.text(0), r.text(1)))?
            .into_iter()
            .map(|(k, v)| (k[NODE.len()..].to_string(), v))
            .collect())
    }

    /// Cache key of the question about two people: both alias sets, the prompt and the model.
    fn people_pair_key(&self, members: &HashMap<String, Vec<String>>, ea: &str, eb: &str) -> Result<String> {
        let mut sides = [members.get(ea).cloned().unwrap_or_default(), members.get(eb).cloned().unwrap_or_default()];
        sides.iter_mut().for_each(|s| s.sort());
        sides.sort();
        Ok(sha256_hex(&serde_json::to_vec(&json!([sides, PROMPT_VERSION, sha256_hex(self.judge_prompt().as_bytes()), self.judge_model()?]))?))
    }

    fn registry_members(registry: &HashMap<String, String>) -> HashMap<String, Vec<String>> {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for (k, id) in registry {
            m.entry(id.clone()).or_default().push(k.clone());
        }
        m
    }

    /// Answers already given, per pair of people: verdict, cache key, time.
    fn people_answers(&self, registry: &HashMap<String, String>) -> Result<Answers> {
        let mut answered = HashMap::new();
        for (v, at) in self.people_verdicts()? {
            if let (Some(x), Some(y)) = (registry.get(&v.a), registry.get(&v.b)) {
                let k = if x <= y { (x.clone(), y.clone()) } else { (y.clone(), x.clone()) };
                answered.insert(k, (v.verdict.clone(), v.cache_key.clone().unwrap_or_default(), at));
            }
        }
        Ok(answered)
    }

    /// Whether the question with cache key `key` still needs asking about these two people.
    fn needs_asking(answered: &Answers, ea: &str, eb: &str, key: &str, now: i64) -> bool {
        let k = if ea <= eb { (ea.to_string(), eb.to_string()) } else { (eb.to_string(), ea.to_string()) };
        match answered.get(&k) {
            Some((verdict, cached, at)) => !(cached == key || (verdict == "unsure" && now - at < UNSURE_MS)),
            None => true,
        }
    }

    /// Alias pairs worth asking about, most active first. Pure rules, no model.
    pub fn people_candidates(&self) -> Result<Vec<PeopleCandidate>> {
        let infos = self.judge_alias_infos()?;
        let registry = self.people_registry()?;
        let members = Self::registry_members(&registry);
        let corrections = self.people_corrections()?;
        let now = self.clock.now_ms();

        // name → keys that go by it
        let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (k, i) in &infos {
            if !registry.contains_key(k) {
                continue;
            }
            for n in i.names.iter().filter(|n| comparable(n)) {
                by_name.entry(n.as_str()).or_default().push(k.as_str());
            }
        }
        // The same words in another order or with notes around them: matched exactly, never by containment.
        let sets: Vec<(String, &str)> = infos.iter().filter(|(k, _)| registry.contains_key(*k)).flat_map(|(k, i)| i.names.iter().filter_map(|n| word_set(n)).map(move |w| (format!("\u{1}{w}"), k.as_str()))).collect();
        for (w, k) in &sets {
            let keys = by_name.entry(w.as_str()).or_default();
            if !keys.contains(k) {
                keys.push(k);
            }
        }
        let mut raw: Vec<(String, String, &'static str)> = vec![];
        let reason = |a: &str, b: &str, exact: bool| -> &'static str {
            let (ka, kb) = (infos[a].kind, infos[b].kind);
            if ka == "name" || kb == "name" {
                "speaker_name"
            } else if ka == "email" || kb == "email" {
                "mail_name"
            } else if exact {
                "same_name"
            } else {
                "contains"
            }
        };
        for keys in by_name.values() {
            for (i, a) in keys.iter().enumerate() {
                for b in &keys[i + 1..] {
                    if a != b {
                        raw.push((a.to_string(), b.to_string(), reason(a, b, true)));
                    }
                }
            }
        }
        let names: Vec<&str> = by_name.keys().copied().filter(|n| !n.starts_with('\u{1}')).collect();
        for short in &names {
            for long in names.iter().filter(|l| contained(short, l)) {
                for a in &by_name[short] {
                    for b in &by_name[long] {
                        if a != b {
                            raw.push((a.to_string(), b.to_string(), reason(a, b, false)));
                        }
                    }
                }
            }
        }

        let answered = self.people_answers(&registry)?;
        let split_between = |ea: &str, eb: &str| -> bool {
            let (ma, mb) = (members.get(ea), members.get(eb));
            corrections.splits.iter().any(|(x, y)| {
                let side = |k: &str, m: Option<&Vec<String>>| m.is_some_and(|m| m.iter().any(|z| z == k));
                (side(x, ma) && side(y, mb)) || (side(y, ma) && side(x, mb))
            })
        };

        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut out: Vec<(i64, PeopleCandidate)> = vec![];
        for (a, b, why) in raw {
            let (Some(ea), Some(eb)) = (registry.get(&a), registry.get(&b)) else { continue };
            if ea == eb || ea == SELF_ENTITY || eb == SELF_ENTITY {
                continue;
            }
            let pair_of_people = if ea <= eb { (ea.clone(), eb.clone()) } else { (eb.clone(), ea.clone()) };
            if seen.contains(&pair_of_people) {
                continue;
            }
            let (ia, ib) = (&infos[&a], &infos[&b]);
            if ia.kind == "handle" && ib.kind == "handle" && ia.groups.intersection(&ib.groups).next().is_some() {
                continue;
            }
            if ia.kind == "email" && ib.kind == "email" && !ia.mail_names.is_empty() && !ib.mail_names.is_empty() && ia.mail_names.is_disjoint(&ib.mail_names) {
                continue;
            }
            if split_between(ea, eb) {
                continue;
            }
            let key = self.people_pair_key(&members, ea, eb)?;
            if !Self::needs_asking(&answered, ea, eb, &key, now) {
                continue;
            }
            if self.automation_get(&format!("people.judge.tries/1:{key}"))?.as_i64().unwrap_or(0) >= MAX_TRIES {
                continue;
            }
            seen.insert(pair_of_people);
            let (first, second) = if a <= b { (a, b) } else { (b, a) };
            out.push((ia.count + ib.count, PeopleCandidate { a: first, b: second, reason: why, key }));
        }
        out.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.a.cmp(&y.1.a)).then(x.1.b.cmp(&y.1.b)));
        Ok(out.into_iter().map(|x| x.1).collect())
    }

    /// Queues batches of candidates for the `people.judge` job when the feature is on and something changed.
    /// Called from the background tick; cheap when nothing changed.
    pub fn people_judge_scan(&self) -> Result<Value> {
        if self.feature_settings()?["items"][FEATURE]["enabled"] != true {
            return Ok(json!({"changed": false, "disabled": true}));
        }
        let state = json!([self.meta_value(STATE)?, self.meta_value(LLM_REV)?, sha256_hex(self.judge_prompt().as_bytes()), self.judge_model()?]);
        if self.automation_get(SCAN)? == state {
            return Ok(json!({"changed": false}));
        }
        let queued_now = self.db.scalar("SELECT COUNT(*) FROM jobs WHERE kind=? AND state IN ('queued','running')", &[text(JOB_KIND)])? as usize;
        let room = MAX_BATCHES.saturating_sub(queued_now);
        let candidates = self.people_candidates()?;
        let version = sha256_hex(&serde_json::to_vec(&json!([PROMPT_VERSION, sha256_hex(self.judge_prompt().as_bytes()), self.model_config()?, self.feature_settings()?["items"][FEATURE]]))?);
        let mut queued = 0;
        for chunk in candidates.chunks(BATCH).take(room) {
            let id = sha256_hex(chunk.iter().map(|c| c.key.as_str()).collect::<Vec<_>>().join("|").as_bytes())[..24].to_string();
            let pairs: Vec<Value> = chunk.iter().map(|c| json!({"a": c.a, "b": c.b, "reason": c.reason, "key": c.key})).collect();
            self.automation_put(&format!("people.judge.input/1:{id}"), &json!({"pairs": pairs}))?;
            for c in chunk {
                let k = format!("people.judge.tries/1:{}", c.key);
                let n = self.automation_get(&k)?.as_i64().unwrap_or(0);
                self.automation_put(&k, &json!(n + 1))?;
            }
            self.job_command(&json!({"action":"enqueue","kind":JOB_KIND,"feature":FEATURE,"input_ref":id,"config_version":version}))?;
            queued += 1;
        }
        // Remember the state only when everything found has been queued; otherwise the next tick continues.
        if candidates.len() <= room * BATCH {
            self.automation_put(SCAN, &state)?;
        }
        Ok(json!({"changed": queued > 0, "queued": queued, "candidates": candidates.len()}))
    }

    /// One person's block of the model input.
    fn judge_side(&self, entity: &str, members: &HashMap<String, Vec<String>>) -> Result<String> {
        let name = self.db.first("SELECT name FROM entities WHERE entity_id=?", &[text(entity)], |r| r.text(0))?.unwrap_or_default();
        let mut keys = members.get(entity).cloned().unwrap_or_default();
        keys.sort();
        let shown: Vec<String> = keys
            .iter()
            .take(6)
            .map(|k| match k.strip_prefix("email:") {
                Some(a) if !a.contains('@') => "邮箱（地址已隐去）".to_string(),
                _ => k.clone(),
            })
            .collect();
        let mut out = format!("名字：{name}\n  别名：{}\n", shown.join("、"));
        let places = self.db.query(
            "SELECT c.app,c.title,c.kind,(SELECT COUNT(DISTINCT o.sender) FROM messages o WHERE o.conversation_id=c.conversation_id AND o.actor<>'self')>=2
             FROM messages m JOIN conversations c ON c.conversation_id=m.conversation_id WHERE m.sender_entity_id=? GROUP BY c.conversation_id ORDER BY MAX(m.sent_ms) DESC LIMIT 4",
            &[text(entity)],
            |r| (r.opt_text(0), r.opt_text(1), r.text(2), r.int(3) == 1),
        )?;
        if !places.is_empty() {
            let parts: Vec<String> = places
                .iter()
                .map(|(app, title, kind, group)| {
                    let what = match (kind.as_str(), group) {
                        ("mail", _) => "邮件",
                        ("meeting", _) => "录音",
                        (_, true) => "群",
                        _ => "私聊",
                    };
                    let title = title.as_deref().map(|t| clip(&chat_title(t), 30)).unwrap_or_default();
                    format!("{} {what}「{title}」", app.as_deref().unwrap_or(""))
                })
                .collect();
            out += &format!("  出现在：{}\n", parts.join("；"));
        }
        let msgs = self.db.query(
            "SELECT m.text FROM messages m WHERE m.sender_entity_id=? AND m.actor<>'self'
               AND EXISTS (SELECT 1 FROM message_sources s WHERE s.message_id=m.message_id)
               AND NOT EXISTS (SELECT 1 FROM message_sources s JOIN events e ON e.event_id=s.event_id WHERE s.message_id=m.message_id AND e.sensitivity='protected')
             ORDER BY m.sent_ms DESC,m.seq DESC LIMIT ?",
            &[text(entity), int(MESSAGES_PER_SIDE)],
            |r| r.text(0),
        )?;
        if !msgs.is_empty() {
            out += "  最近说过：\n";
            for t in msgs {
                out += &format!("   - {}\n", clip(&t, MESSAGE_CHARS));
            }
        }
        Ok(out)
    }

    /// What the model reads for these pairs; pairs that stopped being worth asking are dropped. Returns the model
    /// input and the pairs kept (with their cache keys), or `None` when nothing is left.
    pub(crate) fn judge_input(&self, pairs: &[Value]) -> Result<Option<(String, Vec<Value>)>> {
        let registry = self.people_registry()?;
        let members = Self::registry_members(&registry);
        let corrections = self.people_corrections()?;
        let answered = self.people_answers(&registry)?;
        let now = self.clock.now_ms();
        let mut content = String::new();
        let mut kept = vec![];
        for p in pairs {
            let (a, b) = (p["a"].as_str().unwrap_or(""), p["b"].as_str().unwrap_or(""));
            let (Some(ea), Some(eb)) = (registry.get(a), registry.get(b)) else { continue };
            if ea == eb || corrections.splits.contains(&pair(a, b)) {
                continue;
            }
            // The question may have changed since it was queued (a new model, say): ask with today's key.
            let key = self.people_pair_key(&members, ea, eb)?;
            if !Self::needs_asking(&answered, ea, eb, &key, now) {
                continue;
            }
            let n = kept.len() + 1;
            let why = match p["reason"].as_str().unwrap_or("") {
                "same_name" => "不同应用里名字相同",
                "contains" => "一方的名字包含另一方",
                "mail_name" => "邮件显示名与聊天名相同或相近",
                _ => "录音说话人名与某个别名相同",
            };
            content += &format!("## 第 {n} 对（规则线索：{why}）\nA {}\nB {}\n", self.judge_side(ea, &members)?, self.judge_side(eb, &members)?);
            kept.push(json!({"id": n, "a": a, "b": b, "key": key}));
        }
        Ok((!kept.is_empty()).then_some((content, kept)))
    }

    pub(crate) fn people_judge_prepare(&self, job: &Value) -> Result<Value> {
        let id = job["input_ref"].as_str().ok_or_else(|| bad("缺少批次"))?;
        let input = self.automation_get(&format!("people.judge.input/1:{id}"))?;
        let pairs = input["pairs"].as_array().cloned().unwrap_or_default();
        let c = self.model_config()?;
        let a = ["summary", "title"]
            .iter()
            .find_map(|role| c["roles"].as_array().into_iter().flatten().find(|a| a["role"] == *role && a["model"].is_object()))
            .ok_or_else(|| bad("waiting_model"))?;
        let provider = c["providers"].as_array().into_iter().flatten().find(|p| p["provider_id"] == a["model"]["provider_id"] && p["enabled"] == true).ok_or_else(|| bad("waiting_model"))?;
        self.feature_check(FEATURE, &json!({}), Some(provider))?;
        let (content, kept) = self.judge_input(&pairs)?.ok_or_else(|| bad("nothing_to_judge"))?;
        let prompt = self.judge_prompt();
        let schema = json!({"type":"object","properties":{"results":{"type":"array","items":{"type":"object","properties":{
            "id":{"type":"integer"},"verdict":{"type":"string","enum":["same","different","unsure"]},"reason":{"type":"string"}},
            "required":["id","verdict","reason"],"additionalProperties":false}}},"required":["results"],"additionalProperties":false});
        self.workflow_budget(&c, a, prompt.len() + content.len())?;
        Ok(json!({
            "job": job, "speech": false, "step": FEATURE, "provider": provider, "assignment": a,
            "prompt": prompt, "content": content, "json_mode": true, "response_schema": schema, "max_tokens": 2048,
            "json_repair": true, "pairs": kept,
            "messages": [{"role":"system","content":prompt},{"role":"user","content":content}],
            "config_revision": sha256_hex(&serde_json::to_vec(&c)?), "feature_revision": self.feature_settings()?["items"][FEATURE],
        }))
    }

    pub(crate) fn people_judge_complete(&self, r: &Value, output: &Value) -> Result<Value> {
        if sha256_hex(&serde_json::to_vec(&self.model_config()?)?) != r["config_revision"] || self.feature_settings()?["items"][FEATURE] != r["feature_revision"] {
            return Err(bad("source_or_configuration_changed"));
        }
        let job = self.job_command(&json!({"action":"get","id":r["job"]["id"]}))?;
        if job["state"] != "running" || job["attempt"] != r["job"]["attempt"] || job["owner"] != r["job"]["owner"] || job["lease_until_ms"].as_i64().unwrap_or(0) <= self.clock.now_ms() {
            return Err(TimelineError::Denied("stale_job_lease".into()));
        }
        let raw = output.as_str().ok_or_else(|| bad("invalid_output"))?;
        let model = r["assignment"]["model"]["model_id"].as_str().unwrap_or("");
        let pairs = r["pairs"].as_array().cloned().unwrap_or_default();
        // Each verdict is its own small transaction; a reply that fails halfway is asked again and overwrites them.
        let written = self.people_judge_apply(&pairs, raw, model)?;
        self.db.execute("UPDATE jobs SET state='done',lease_until_ms=NULL,output_ref=? WHERE job_id=?", &[text(format!("people.judge:{}", pairs.len())), text(r["job"]["id"].as_str().unwrap_or(""))])?;
        Ok(written)
    }

    /// Parses a model reply for `pairs` (`id`, `a`, `b`, `key`) and stores the verdicts; a pair the reply leaves out
    /// is recorded as `unsure`. Also used by tests and offline replays.
    pub fn people_judge_apply(&self, pairs: &[Value], raw: &str, model: &str) -> Result<Value> {
        let parsed = crate::screen_excerpt::parse_json_output(raw).ok_or_else(|| bad("invalid_output"))?;
        let results = parsed["results"].as_array().ok_or_else(|| bad("invalid_output"))?;
        let registry = self.people_registry()?;
        let (mut same, mut merged, mut different, mut unsure) = (0, 0, 0, 0);
        for p in pairs {
            let (a, b) = (p["a"].as_str().unwrap_or(""), p["b"].as_str().unwrap_or(""));
            let res = results.iter().find(|x| x["id"] == p["id"]);
            let verdict = res.and_then(|x| x["verdict"].as_str()).filter(|v| ["same", "different", "unsure"].contains(v)).unwrap_or("unsure");
            let mut reason = res.and_then(|x| x["reason"].as_str()).map(|s| clip(s, 120)).unwrap_or_default();
            let mut corroborated = false;
            if verdict == "same" {
                same += 1;
                if let Some(why) = self.people_corroboration(a, b)? {
                    corroborated = true;
                    merged += 1;
                    reason = format!("{reason}（佐证：{why}）");
                }
            }
            match verdict {
                "different" => different += 1,
                "unsure" => unsure += 1,
                _ => {}
            }
            // An older answer about the same two people (under other keys) must not outlive the new one.
            if let (Some(ea), Some(eb)) = (registry.get(a), registry.get(b)) {
                for (old, _) in self.people_verdicts()? {
                    if (old.a != pair(a, b).0 || old.b != pair(a, b).1)
                        && matches!((registry.get(&old.a), registry.get(&old.b)), (Some(x), Some(y)) if (x == ea && y == eb) || (x == eb && y == ea))
                    {
                        let (x, y) = pair(&old.a, &old.b);
                        self.db.execute("DELETE FROM store_meta WHERE key=?", &[text(format!("{LLM}{x}{SEP}{y}"))])?;
                    }
                }
            }
            self.people_set_verdict(&PeopleVerdict {
                a: a.into(),
                b: b.into(),
                verdict: verdict.into(),
                corroborated,
                reason,
                model_ref: Some(model.into()),
                prompt_version: Some(PROMPT_VERSION.into()),
                cache_key: p["key"].as_str().map(String::from),
            })?;
        }
        Ok(json!({"pairs": pairs.len(), "same": same, "auto_merge": merged, "different": different, "unsure": unsure}))
    }

    /// What the model reads for these pairs, for offline replays on a library copy.
    #[doc(hidden)]
    pub fn judge_input_for_replay(&self, pairs: &[Value]) -> Result<Option<(String, Vec<Value>)>> {
        self.judge_input(pairs)
    }

    #[doc(hidden)]
    pub fn judge_prompt_for_replay(&self) -> String {
        self.judge_prompt()
    }

    /// A rule-level reason to believe two aliases are one person, if there is one:
    /// `shared_thread` (both aliases speak in one mail thread, or in one private chat — which has a single other
    /// person) or `email_pinyin` (an email address prefix matches the pinyin / spelling of a name on the other side).
    /// "Two sources mention the same thing in the same period" is not implemented yet.
    pub fn people_corroboration(&self, a: &str, b: &str) -> Result<Option<String>> {
        let registry = self.people_registry()?;
        let (Some(ea), Some(eb)) = (registry.get(a), registry.get(b)) else { return Ok(None) };
        if ea == eb {
            return Ok(None);
        }
        let shared = self.db.scalar(
            "SELECT COUNT(*) FROM (SELECT m.conversation_id FROM messages m JOIN conversations c ON c.conversation_id=m.conversation_id
               WHERE m.sender_entity_id=? AND c.kind IN ('mail','chat')
                 AND m.conversation_id IN (SELECT conversation_id FROM messages WHERE sender_entity_id=?)
                 AND (c.kind='mail' OR (SELECT COUNT(DISTINCT o.sender_entity_id) FROM messages o WHERE o.conversation_id=c.conversation_id AND o.actor<>'self')<=2)
               GROUP BY m.conversation_id)",
            &[text(ea), text(eb)],
        )?;
        if shared > 0 {
            return Ok(Some("同一邮件线程或私聊里都出现过".into()));
        }
        let side = |id: &str| -> Result<(Vec<String>, Vec<String>)> {
            let rows = self.db.query("SELECT alias_kind,alias FROM entity_aliases WHERE entity_id=?", &[text(id)], |r| (r.text(0), r.text(1)))?;
            let name = self.db.first("SELECT name FROM entities WHERE entity_id=?", &[text(id)], |r| r.text(0))?.unwrap_or_default();
            let mut names = vec![normalize(&name)];
            let mut emails = vec![];
            for (kind, alias) in rows {
                match kind.as_str() {
                    "email" if alias.contains('@') => emails.push(alias),
                    "handle" => names.push(alias.split_once('/').map(|x| x.1.to_string()).unwrap_or(alias)),
                    "name" => names.push(alias),
                    _ => {}
                }
            }
            Ok((names, emails))
        };
        let (sa, sb) = (side(ea)?, side(eb)?);
        for ((_, emails), (names, _)) in [(&sa, &sb), (&sb, &sa)] {
            for addr in emails {
                let prefix = pinyin::email_prefix(addr);
                if prefix.len() < 4 {
                    continue;
                }
                for n in names {
                    if pinyin::email_forms(n).iter().chain(pinyin::latin_forms(n).iter()).any(|f| *f == prefix) {
                        return Ok(Some(format!("邮箱前缀 {prefix} 与名字「{n}」吻合")));
                    }
                }
            }
        }
        Ok(None)
    }

    fn judge_label(&self, key: &str, registry: &HashMap<String, String>) -> Result<Value> {
        let entity = registry.get(key).cloned().unwrap_or_default();
        let name = self.db.first("SELECT name FROM entities WHERE entity_id=?", &[text(&entity)], |r| r.text(0))?.unwrap_or_default();
        let aliases: Vec<String> = registry.iter().filter(|(_, v)| **v == entity).map(|(k, _)| k.clone()).collect();
        let mut aliases = aliases;
        aliases.sort();
        Ok(json!({"key": key, "person_id": entity, "name": name, "aliases": aliases}))
    }

    /// What needs you, and what was merged for you: pairs the model called the same person without a supporting
    /// rule (to merge or reject), and automatic merges (to undo). Both come from cached verdicts only.
    pub fn people_candidate_list(&self) -> Result<Value> {
        let _ = self.people_sync();
        let registry = self.people_registry()?;
        let members = Self::registry_members(&registry);
        let corrections = self.people_corrections()?;
        let split = |ea: &str, eb: &str| {
            let (ma, mb) = (members.get(ea), members.get(eb));
            corrections.splits.iter().any(|(x, y)| {
                let side = |k: &str, m: Option<&Vec<String>>| m.is_some_and(|m| m.iter().any(|z| z == k));
                (side(x, ma) && side(y, mb)) || (side(y, ma) && side(x, mb))
            })
        };
        let (mut candidates, mut auto) = (vec![], vec![]);
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (v, at) in self.people_verdicts()? {
            if v.verdict != "same" {
                continue;
            }
            let (Some(ea), Some(eb)) = (registry.get(&v.a), registry.get(&v.b)) else { continue };
            if split(ea, eb) || corrections.merges.contains(&pair(&v.a, &v.b)) {
                continue;
            }
            let mut item = json!({"a": v.a, "b": v.b, "reason": v.reason, "model": v.model_ref, "at_ms": at,
                "a_person": self.judge_label(&v.a, &registry)?, "b_person": self.judge_label(&v.b, &registry)?});
            if ea == eb {
                if v.corroborated {
                    item["person_id"] = json!(ea);
                    auto.push(item);
                }
            } else if !v.corroborated && seen.insert(if ea <= eb { (ea.clone(), eb.clone()) } else { (eb.clone(), ea.clone()) }) {
                candidates.push(item);
            }
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c["at_ms"].as_i64().unwrap_or(0)));
        auto.sort_by_key(|c| std::cmp::Reverse(c["at_ms"].as_i64().unwrap_or(0)));
        Ok(json!({"candidates": candidates, "auto_merged": auto}))
    }

    /// Automatic merges inside one person, for the person card.
    pub(crate) fn people_auto_merges(&self, person: &str) -> Result<Vec<Value>> {
        let all = self.people_candidate_list()?;
        Ok(all["auto_merged"].as_array().into_iter().flatten().filter(|m| m["person_id"] == person).cloned().collect())
    }
}
