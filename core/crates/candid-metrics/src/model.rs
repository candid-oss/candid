use crate::{duration, inside, union, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub start_ms: i64,
    pub end_ms: i64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Visit {
    pub id: String,
    pub object_id: Option<String>,
    pub active: Vec<Span>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextRecord {
    pub at_ms: i64,
    pub visit_id: Option<String>,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputSession {
    pub id: String,
    pub at_ms: i64,
    pub sent: bool,
    pub expected_text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputRecord {
    pub at_ms: i64,
    pub session_id: String,
    pub visit_id: String,
    pub text: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Shot {
    pub at_ms: i64,
    pub bytes: u64,
    pub blob_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub span: Span,
    pub cpu_ms: u64,
    pub wakeups: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Searchable {
    pub occurred_ms: i64,
    pub searchable_ms: Option<i64>,
    pub poll_interval_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextItem {
    pub kind: String,
    pub span: Span,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}
/// None means unknown, Some([]) means observed zero. IDs are scoped to this input.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub format: String,
    pub synthetic: bool,
    pub observed: Vec<Span>,
    #[serde(default)]
    pub context_items: Vec<ContextItem>,
    pub visits: Option<Vec<Visit>>,
    pub effective: Option<Vec<Span>>,
    pub covered: Option<Vec<Span>>,
    pub inputs: Option<Vec<InputSession>>,
    pub input_records: Vec<InputRecord>,
    pub text_records: Vec<TextRecord>,
    pub shots: Option<Vec<Shot>>,
    pub resources: Option<Vec<Resource>>,
    pub searchable: Option<Vec<Searchable>>,
    /// Numeric-only counts; never copy source metadata or content here.
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    pub value: Option<f64>,
    pub numerator: Option<f64>,
    pub denominator: Option<f64>,
    pub unit: String,
    pub method: String,
    pub unavailable: Option<String>,
    pub collect_next: Option<String>,
}
impl Metric {
    pub(crate) fn ratio(n: f64, d: f64, unit: &str, method: &str) -> Self {
        let n = if n == 0. { 0. } else { n };
        Self {
            value: (d > 0.).then_some(n / d),
            numerator: Some(n),
            denominator: Some(d),
            unit: unit.into(),
            method: method.into(),
            unavailable: (d == 0.).then(|| "分母为零".into()),
            collect_next: None,
        }
    }
    pub fn missing(method: &str, why: &str, next: &str) -> Self {
        Self {
            value: None,
            numerator: None,
            denominator: None,
            unit: String::new(),
            method: method.into(),
            unavailable: Some(why.into()),
            collect_next: Some(next.into()),
        }
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct DayReport {
    pub date: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub observed_ms: i64,
    pub metrics: BTreeMap<String, Metric>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub format: String,
    pub synthetic: bool,
    pub timezone: String,
    pub input_observed: Vec<Span>,
    pub counts_scope: String,
    pub range_basis: String,
    pub requested_start: String,
    pub requested_end_exclusive: String,
    pub counts: BTreeMap<String, u64>,
    pub days: Vec<DayReport>,
}

impl Dataset {
    pub fn validate(&self) -> Result<()> {
        if self.counts.keys().any(|k| {
            ![
                "desktop_events",
                "text_samples",
                "screenshots",
                "body_unicode_chars",
            ]
            .contains(&k.as_str())
        }) {
            return Err("统计键不在允许清单内".into());
        }
        if self.format != "candid-measurement/1" {
            return Err("不支持的测量格式".into());
        }
        let valid = |s: &Span| {
            s.start_ms < s.end_ms
                && s.start_ms >= -62_135_596_800_000
                && s.end_ms <= 253_402_300_800_000
        };
        if self.observed.is_empty() || !self.observed.iter().all(valid) {
            return Err("测量范围为空或无效".into());
        }
        for spans in [&self.effective, &self.covered].into_iter().flatten() {
            if !spans.iter().all(valid) {
                return Err("无效时段".into());
            }
        }
        for spans in [&self.effective, &self.covered].into_iter().flatten() {
            if spans.iter().any(|s| {
                duration(&union(&self.observed, s.start_ms, s.end_ms)) != s.end_ms - s.start_ms
            }) {
                return Err("时段超出观测范围".into());
            }
        }
        if self.covered.is_some() && self.effective.is_none() {
            return Err("文字覆盖需要有效使用时段".into());
        }
        let mut ids = HashSet::new();
        if let Some(visits) = &self.visits {
            for v in visits {
                if v.id.is_empty()
                    || !ids.insert(v.id.as_str())
                    || v.active.is_empty()
                    || !v.active.iter().all(valid)
                {
                    return Err("访问身份或活动时段无效".into());
                }
                if v.active.iter().any(|s| {
                    duration(&union(&self.observed, s.start_ms, s.end_ms)) != s.end_ms - s.start_ms
                }) {
                    return Err("访问超出观测范围".into());
                }
            }
        }
        let mut sessions = HashSet::new();
        if let Some(inputs) = &self.inputs {
            for i in inputs {
                if i.id.is_empty() || !sessions.insert(i.id.as_str()) {
                    return Err("输入身份重复或为空".into());
                }
            }
        }
        for r in &self.input_records {
            if self.inputs.is_none()
                || !sessions.contains(r.session_id.as_str())
                || !ids.contains(r.visit_id.as_str())
            {
                return Err("输入记录缺少输入或访问标注".into());
            }
        }
        let mut blob_sizes = HashMap::new();
        for s in self.shots.iter().flatten() {
            if s.blob_id.is_empty()
                || blob_sizes
                    .insert(&s.blob_id, s.bytes)
                    .is_some_and(|n| n != s.bytes)
            {
                return Err("原件身份为空或字节元数据不一致".into());
            }
        }
        for r in &self.text_records {
            if let Some(id) = &r.visit_id {
                if !ids.contains(id.as_str()) {
                    return Err("文字引用未知访问".into());
                }
            }
        }
        let has = |t| {
            self.observed
                .iter()
                .any(|s| inside(t, s.start_ms, s.end_ms))
        };
        if self.text_records.iter().any(|r| !has(r.at_ms))
            || self.input_records.iter().any(|r| !has(r.at_ms))
            || self.inputs.iter().flatten().any(|r| !has(r.at_ms))
            || self.shots.iter().flatten().any(|r| !has(r.at_ms))
            || self.searchable.iter().flatten().any(|r| {
                !has(r.occurred_ms)
                    || r.searchable_ms
                        .is_some_and(|t| t < r.occurred_ms || t > 253_402_300_800_000)
                    || r.poll_interval_ms == 0
            })
        {
            return Err("记录时间无效或超出观测范围".into());
        }
        let mut context_ids = HashMap::new();
        for item in &self.context_items {
            if let Some(id) = &item.id {
                if id.is_empty() || context_ids.insert(id, item).is_some() {
                    return Err("情境条目身份无效".into());
                }
            }
        }
        for item in &self.context_items {
            if ![
                "usage_segment",
                "audio_recording",
                "audio_segment",
                "speech_segment",
                "calendar_item",
                "mail_item",
                "gap.locked",
                "gap.paused",
                "gap.idle",
                "gap.permission",
                "gap.no_text",
            ]
            .contains(&item.kind.as_str())
                || !valid(&item.span)
                || duration(&union(&self.observed, item.span.start_ms, item.span.end_ms))
                    != item.span.end_ms - item.span.start_ms
            {
                return Err("情境条目无效".into());
            }
            if ["audio_segment", "speech_segment"].contains(&item.kind.as_str()) {
                let parent = item
                    .parent_id
                    .as_ref()
                    .and_then(|id| context_ids.get(id))
                    .ok_or("录音分段缺少父会话")?;
                if parent.kind != "audio_recording"
                    || item.span.start_ms < parent.span.start_ms
                    || item.span.end_ms > parent.span.end_ms
                {
                    return Err("录音分段超出会话".into());
                }
            }
            if item.kind == "speech_segment"
                && (item.text.as_ref().is_none_or(|s| s.trim().is_empty())
                    || item.speaker.as_ref().is_none_or(|s| s.trim().is_empty()))
            {
                return Err("合成转写缺少文字或说话人".into());
            }
        }
        if let Some(rs) = &self.resources {
            if rs.iter().any(|r| {
                !valid(&r.span)
                    || duration(&union(&self.observed, r.span.start_ms, r.span.end_ms))
                        != r.span.end_ms - r.span.start_ms
            }) {
                return Err("资源采样范围无效".into());
            }
            let spans: Vec<_> = rs.iter().map(|r| r.span.clone()).collect();
            if duration(&union(&spans, i64::MIN, i64::MAX)) != duration(&spans) {
                return Err("资源采样重叠".into());
            }
        }
        Ok(())
    }
}
