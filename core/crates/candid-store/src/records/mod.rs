//! Deterministic L0 -> L1 reconstruction. No SQL mutation; revisions retain their evidence.
use crate::{sha256_hex, IngestEvent, Result, TimelineError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: &str) -> TimelineError {
    TimelineError::InvalidInput(message.into())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectIdentity {
    pub key: String,
    pub basis: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    pub text: String,
    pub occurrence: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confirmation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    pub observation: String,
    pub message_id: String,
    pub text: String,
    /// Only a source send event or a source-positioned outgoing message qualifies.
    pub basis: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub format: String,
    pub object: ObjectIdentity,
    pub field: Option<String>,
    pub input_value: Option<String>,
    pub input_status: String,
    pub active: bool,
    pub reading: Vec<Reading>,
    pub confirmation: Option<Confirmation>,
}
impl Evidence {
    pub fn validate(&self) -> Result<()> {
        if !["candid-record-evidence/1", "candid-record-evidence/2"].contains(&self.format.as_str())
            || self.object.key.is_empty()
            || !["path", "url", "sidebar", "title", "window"].contains(&self.object.basis.as_str())
            || ![
                "captured",
                "disabled",
                "unavailable",
                "protected",
                "composing",
            ]
            .contains(&self.input_status.as_str())
        {
            return Err(invalid("记录证据版本、对象或读取状态无效"));
        }
        if self.input_value.is_some()
            && (self.input_status != "captured"
                || self.field.as_ref().is_none_or(|f| f.is_empty())
                || !self.active)
        {
            return Err(invalid("无权限、无字段或非活动证据不能带输入值"));
        }
        if let Some(c) = &self.confirmation {
            if self.format=="candid-record-evidence/2" && c.basis=="source_send" && c.source_hash.as_ref().is_none_or(|h|h.len()!=64 || !h.bytes().all(|c|c.is_ascii_hexdigit())) {return Err(invalid("来源修订无效"));}
            if c.source_hash.is_some() && self.format != "candid-record-evidence/2" {return Err(invalid("来源修订需要证据版本2"));}

            if c.observation.is_empty()
                || c.message_id.is_empty()
                || c.text.is_empty()
                || !["source_send", "outgoing_message"].contains(&c.basis.as_str())
                || !self.active
            {
                return Err(invalid("发送确认缺少来源消息身份"));
            }
        }
        if !self.active && (!self.reading.is_empty() || self.confirmation.is_some()) {
            return Err(invalid("非活动证据不能带正文"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub source: String,
    pub key: String,
    pub at_ms: i64,
    pub sequence: i64,
    pub app: String,
    pub evidence: Evidence,
}
/// Existing event payload is an explicit, versioned compatibility boundary.
pub fn validate_payload(event: &IngestEvent) -> Result<()> {
    if let Some(links)=event.payload.get("source_links") {
        let refs=links["refs"].as_array().ok_or_else(||invalid("来源引用列表无效"))?;
        if event.kind!="note" || links["format"]!="candid-source-links/1" || refs.is_empty() || refs.len()>32 || refs.iter().any(|r|r["event_id"].as_str().is_none_or(str::is_empty)||r["source_id"].as_str().is_none_or(str::is_empty)) {return Err(invalid("来源引用版本或目标无效"));}
    }

    if let Some(raw) = event.payload.get("record_evidence") {
        let e: Evidence =
            serde_json::from_value(raw.clone()).map_err(|_| invalid("记录证据格式无效"))?;
        e.validate()?;
        if !["screen.text", "coverage.gap"].contains(&event.kind.as_str())
            || event.sensitivity == "protected"
        {
            return Err(invalid("记录证据只能附于获准文字观测"));
        }
    }
    Ok(())
}
pub fn from_event(source: &str, event: &IngestEvent) -> Result<Option<Observation>> {
    validate_payload(event)?;
    if (event.kind != "screen.text" && event.payload.get("record_evidence").is_none())
        || event.sensitivity == "protected"
    {
        return Ok(None);
    }
    let evidence = if let Some(raw) = event.payload.get("record_evidence") {
        serde_json::from_value(raw.clone()).map_err(|_| invalid("记录证据格式无效"))?
    } else {
        // Historical text is reading only: Return and focused labels never establish an input or send.
        Evidence {
            format: "candid-record-evidence/1".into(),
            object: ObjectIdentity {
                key: event
                    .url
                    .clone()
                    .or(event.title.clone())
                    .unwrap_or_else(|| event.source_key.clone()),
                basis: if event.url.is_some() {
                    "url"
                } else if event.title.is_some() {
                    "title"
                } else {
                    "window"
                }
                .into(),
            },
            field: None,
            input_value: None,
            input_status: "unavailable".into(),
            active: true,
            reading: event
                .text
                .iter()
                .filter(|t| t.field == "body")
                .flat_map(|t| t.content.lines())
                .filter(|t| !t.trim().is_empty())
                .map(|t| Reading {
                    text: t.into(),
                    occurrence: None,
                })
                .collect(),
            confirmation: None,
        }
    };
    Ok(Some(Observation {
        source: source.into(),
        key: event.source_key.clone(),
        at_ms: event.start_ms.ok_or_else(|| invalid("重建需要观测时间"))?,
        sequence: 0,
        app: event.app_bundle_id.clone().unwrap_or_default(),
        evidence,
    }))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub format: String,
    pub revisit_ms: i64,
    pub input_idle_ms: i64,
}
impl Default for Rules {
    fn default() -> Self {
        Self {
            format: "candid-record-rules/1".into(),
            revisit_ms: 600_000,
            input_idle_ms: 120_000,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visit {
    pub id: String,
    pub source: String,
    pub app: String,
    pub object: ObjectIdentity,
    pub active: Vec<[i64; 2]>,
    pub observations: Vec<String>,
}
impl Visit {
    pub fn dwell_ms(&self) -> i64 {
        self.active.iter().map(|s| s[1] - s[0]).sum()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub id: String,
    pub visit: String,
    pub field: String,
    pub observations: Vec<String>,
    pub final_text: String,
    pub sent: Option<Confirmation>,
    pub longest_before_deletion: Option<String>,
    pub ended_by: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRecord {
    pub slot: usize,
    pub visit: String,
    pub text: String,
    pub observations: Vec<String>,
    pub occurrence: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub format: String,
    pub rules: Rules,
    pub evidence: Vec<Observation>,
    pub visits: Vec<Visit>,
    pub inputs: Vec<Input>,
    pub reading: Vec<ReadRecord>,
    pub gaps: Vec<String>,
}
fn identity(o: &Observation) -> String {
    format!("{}:{}", o.source.len(), o.source) + &o.key
}
fn trigrams(s: &str) -> BTreeSet<String> {
    let c: Vec<_> = s.chars().collect();
    c.windows(3).map(|x| x.iter().collect()).collect()
}
fn similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b) = (trigrams(a), trigrams(b));
    !a.is_empty() && a.intersection(&b).count() as f64 / a.union(&b).count() as f64 >= 0.8
}
pub fn rebuild(observations: &[Observation], rules: Rules) -> Result<Revision> {
    if rules.format != "candid-record-rules/1"
        || rules.revisit_ms <= 0
        || rules.input_idle_ms <= 0
    {
        return Err(invalid("构造规则版本或间隔无效"));
    }
    let mut unique = BTreeMap::new();
    for o in observations {
        o.evidence.validate()?;
        if o.key.is_empty() || o.source.is_empty() {
            return Err(invalid("观测缺少来源身份"));
        }
        if let Some(old) = unique.insert(identity(o), o.clone()) {
            if old != *o {
                return Err(invalid("同一观测身份内容冲突"));
            }
        }
    }
    let mut evidence: Vec<_> = unique.into_values().collect();
    evidence.sort_by_key(|o| (o.at_ms, o.sequence, o.source.clone(), o.key.clone()));
    let mut r = Revision {
        format: "candid-record-revision/1".into(),
        rules,
        evidence: evidence.clone(),
        visits: vec![],
        inputs: vec![],
        reading: vec![],
        gaps: vec![],
    };
    let mut current: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    let mut last_left: BTreeMap<usize, i64> = BTreeMap::new();
    let mut pending: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut observation_input = BTreeMap::new();
    let mut longest: BTreeMap<usize, String> = BTreeMap::new();
    let mut last_input_change: BTreeMap<usize, i64> = BTreeMap::new();
    let mut messages = BTreeSet::new();
    for o in &evidence {
        let e = &o.evidence;
        let previous = current.remove(&o.source);
        let same = previous.filter(|(i, _)| {
            e.active && r.visits[*i].app == o.app && r.visits[*i].object == e.object
        });
        if let Some((i, start)) = previous {
            if o.at_ms > start {
                r.visits[i].active.push([start, o.at_ms]);
            }
            if same.is_none() {
                last_left.insert(i, o.at_ms);
                pending.retain(|(v, _), input| {
                    if *v == i {
                        r.inputs[*input].ended_by = "leave".into();
                        false
                    } else {
                        true
                    }
                });
            }
        }
        if !e.active {
            r.gaps.push(identity(o));
            continue;
        }
        let visit = same
            .map(|p| p.0)
            .or_else(|| {
                r.visits
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(i, v)| {
                        v.source == o.source
                            && v.app == o.app
                            && v.object == e.object
                            && last_left
                                .get(i)
                                .is_some_and(|t| o.at_ms - t <= r.rules.revisit_ms)
                    })
                    .map(|(i, _)| i)
            })
            .unwrap_or_else(|| {
                let i = r.visits.len();
                r.visits.push(Visit {
                    id: identity(o),
                    source: o.source.clone(),
                    app: o.app.clone(),
                    object: e.object.clone(),
                    active: vec![],
                    observations: vec![],
                });
                i
            });
        current.insert(o.source.clone(), (visit, o.at_ms));
        r.visits[visit].observations.push(identity(o));
        if e.input_value.as_deref() == Some("") && e.reading.is_empty() {
            r.gaps.push(identity(o));
        }
        if e.input_status != "captured" {
            r.gaps.push(identity(o));
            pending.retain(|(v, _), input| {
                if *v == visit {
                    r.inputs[*input].ended_by = e.input_status.clone();
                    false
                } else {
                    true
                }
            });
        }
        if let (Some(field), Some(value)) = (&e.field, &e.input_value) {
            let key = (visit, field.clone());
            if let Some(i) = pending.get(&key).copied() {
                if !value.is_empty()
                    && *value != r.inputs[i].final_text
                    && last_input_change
                        .get(&i)
                        .is_some_and(|at| o.at_ms - at > r.rules.input_idle_ms)
                {
                    r.inputs[i].ended_by = "idle".into();
                    pending.remove(&key);
                }
            }
            pending.retain(|(v, f), input| {
                if *v == visit && f != field {
                    r.inputs[*input].ended_by = "field_change".into();
                    false
                } else {
                    true
                }
            });
            if value.is_empty() {
                if let Some(i) = pending.remove(&key) {
                    r.inputs[i].observations.push(identity(o));
                    observation_input.insert((o.source.clone(), o.key.clone()), i);
                    r.inputs[i].ended_by = "clear_candidate".into();
                }
            } else {
                let i = *pending.entry(key).or_insert_with(|| {
                    let i = r.inputs.len();
                    r.inputs.push(Input {
                        id: identity(o),
                        visit: r.visits[visit].id.clone(),
                        field: field.clone(),
                        observations: vec![],
                        final_text: String::new(),
                        sent: None,
                        longest_before_deletion: None,
                        ended_by: "snapshot".into(),
                    });
                    i
                });
                let input = &mut r.inputs[i];
                if value.chars().count() * 2 < input.final_text.chars().count()
                    && input
                        .longest_before_deletion
                        .as_ref()
                        .is_none_or(|v| v.chars().count() < input.final_text.chars().count())
                {
                    input.longest_before_deletion = Some(
                        longest
                            .get(&i)
                            .cloned()
                            .unwrap_or_else(|| input.final_text.clone()),
                    );
                }
                if longest
                    .get(&i)
                    .is_none_or(|v| v.chars().count() < value.chars().count())
                {
                    longest.insert(i, value.clone());
                }
                if input.final_text != *value {
                    last_input_change.insert(i, o.at_ms);
                }
                input.final_text = value.clone();
                input.observations.push(identity(o));
                observation_input.insert((o.source.clone(), o.key.clone()), i);
            }
        }
        if let Some(c) = &e.confirmation {
            let key = (o.source.clone(), c.message_id.clone());
            if let Some(i) = observation_input
                .get(&(o.source.clone(), c.observation.clone()))
                .copied()
            {
                if r.inputs[i].visit == r.visits[visit].id
                    && r.inputs[i].final_text == c.text
                    && messages.insert(key)
                {
                    let i = if r.inputs[i].sent.is_some() {
                        let mut input = r.inputs[i].clone();
                        input.id = identity(o);
                        input.observations = vec![
                            identity(o),
                            format!("{}:{}{}", o.source.len(), o.source, c.observation),
                        ];
                        input.longest_before_deletion = None;
                        r.inputs.push(input);
                        r.inputs.len() - 1
                    } else {
                        i
                    };
                    r.inputs[i].sent = Some(c.clone());
                    r.inputs[i].ended_by = "send".into();
                    pending.retain(|_, v| *v != i);
                    r.inputs[i].observations.push(identity(o));
                } else if r.inputs[i].sent.as_ref() != Some(c) {
                    r.gaps.push(identity(o));
                }
            } else {
                r.gaps.push(identity(o));
            }
        }
        for (slot, p) in e.reading.iter().enumerate() {
            if p.text.trim().is_empty() {
                continue;
            }
            let existing = r.reading.iter_mut().find(|x| {
                x.visit == r.visits[visit].id
                    && match (&p.occurrence, &x.occurrence) {
                        (Some(a), Some(b)) => a == b,
                        (None, None) => x.slot == slot && similar(&x.text, &p.text),
                        _ => false,
                    }
            });
            if let Some(x) = existing {
                x.observations.push(identity(o));
                if p.text.chars().count() > x.text.chars().count() {
                    x.text = p.text.clone();
                }
            } else {
                r.reading.push(ReadRecord {
                    slot,
                    visit: r.visits[visit].id.clone(),
                    text: p.text.clone(),
                    observations: vec![identity(o)],
                    occurrence: p.occurrence.clone(),
                });
            }
        }
    }
    for i in &mut r.inputs {
        if i.longest_before_deletion.as_ref() == Some(&i.final_text) {
            i.longest_before_deletion = None;
        }
    }
    Ok(r)
}
impl Revision {
    pub fn verify(&self) -> Result<()> {
        if self.format != "candid-record-revision/1"
            || rebuild(&self.evidence, self.rules.clone())? != *self
        {
            return Err(invalid("修订核对失败"));
        }
        Ok(())
    }
    pub fn id(&self) -> Result<String> {
        self.verify()?;
        Ok(sha256_hex(serde_json::to_vec(self)?))
    }
}
/// The coordinator persists the selected revision pointer atomically with its audit entry.
#[derive(Default)]
pub struct Revisions {
    pub revisions: BTreeMap<String, Revision>,
    pub selected: Option<String>,
}
impl Revisions {
    pub fn stage(&mut self, revision: Revision) -> Result<String> {
        let id = revision.id()?;
        self.revisions.insert(id.clone(), revision);
        Ok(id)
    }
    pub fn select(&mut self, id: &str) -> Result<()> {
        self.revisions
            .get(id)
            .ok_or_else(|| invalid("修订不存在"))?
            .verify()?;
        self.selected = Some(id.into());
        Ok(())
    }
}

mod persisted;

mod annotations;
