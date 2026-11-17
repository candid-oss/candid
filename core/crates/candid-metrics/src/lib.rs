//! Offline, numeric-only measurement. Evaluation inputs are separate from the store schema.
mod interval;
mod model;
pub mod timeline;
use chrono::{Days, NaiveDate, TimeZone};
use chrono_tz::Tz;
pub(crate) use interval::{duration, inside, intersection, lines, union};
pub use model::*;
use std::collections::{BTreeMap, HashMap, HashSet};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const REDUNDANCY: &str = "同一访问内按时间扫描非空行，首尾空白不计；相同行后续出现的 Unicode 字符数/全部非空行字符数；含阅读与输入；保留跨日已见集合；不作模糊去重";

pub fn measure(data: &Dataset, start: NaiveDate, end: NaiveDate, tz: Tz) -> Result<Report> {
    data.validate()?;
    if end <= start || (end - start).num_days() > 3660 {
        return Err("日期范围必须为 1–3660 天，结束日不含在内".into());
    }
    let mut days = Vec::new();
    let mut date = start;
    while date < end {
        let next = date.checked_add_days(Days::new(1)).ok_or("日期超限")?;
        let midnight = |d: NaiveDate| {
            tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .map(|d| d.timestamp_millis())
                .ok_or("本地午夜不唯一或不存在，请选择其他日期边界")
        };
        let (a, b) = (midnight(date)?, midnight(next)?);
        let mut m = BTreeMap::new();
        let mut seen: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut total = 0;
        let mut repeated = 0;
        let mut missing_visit = false;
        let mut texts: Vec<_> = data
            .text_records
            .iter()
            .map(|r| (r.at_ms, r.visit_id.as_deref(), r.text.as_str()))
            .chain(
                data.input_records
                    .iter()
                    .map(|r| (r.at_ms, Some(r.visit_id.as_str()), r.text.as_str())),
            )
            .filter(|r| r.0 < b)
            .collect();
        texts.sort_by_key(|r| r.0);
        for (t, visit, text) in texts {
            for line in lines(text) {
                let new = visit.map(|id| seen.entry(id).or_default().insert(line));
                if inside(t, a, b) {
                    total += line.chars().count();
                    if new == Some(false) {
                        repeated += line.chars().count();
                    }
                    if visit.is_none() {
                        missing_visit = true;
                    }
                }
            }
        }
        m.insert(
            "redundancy".into(),
            if data.visits.is_some() && !missing_visit {
                Metric::ratio(repeated as f64, total as f64, "比例", REDUNDANCY)
            } else {
                Metric::missing(
                    REDUNDANCY,
                    "缺少可靠的访问归属",
                    "采集对象身份与访问边界；由 R2 构造并标注访问，不能用同 App 代替",
                )
            },
        );
        let visits: Vec<_> = data
            .visits
            .iter()
            .flatten()
            .filter(|v| duration(&union(&v.active, a, b)) > 0)
            .collect();
        m.insert("object_identification".into(), if data.visits.is_some() { Metric::ratio(visits.iter().filter(|v| v.object_id.as_ref().is_some_and(|s| !s.trim().is_empty())).count() as f64,visits.len() as f64,"比例","与当日相交的访问中有稳定网址/路径/对话身份的访问数/访问数；标题不算；重访沿用对象身份") } else { Metric::missing("有稳定对象身份的访问/访问", "旧库没有访问单位与稳定对象身份", "受控采集网址、路径、对话身份并校验访问边界") });
        m.insert("visit_fragmentation".into(), if data.visits.is_some() && !missing_visit { Metric::ratio(data.text_records.iter().filter(|r| inside(r.at_ms,a,b)).count() as f64, visits.len() as f64,"阅读记录/访问日","当日阅读文字记录数/当日相交访问数；图片与输入各为独立单位，不计作阅读碎片；跨日访问各日计一次") } else { Metric::missing("阅读记录/访问", "没有可靠访问边界", "R2 构造访问后重测，保留无文字访问作为分母") });
        let inputs: Vec<_> = data
            .inputs
            .iter()
            .flatten()
            .filter(|s| inside(s.at_ms, a, b))
            .collect();
        let sent: Vec<_> = inputs.iter().filter(|s| s.sent).collect();
        let complete = sent
            .iter()
            .filter(|s| {
                let records: Vec<_> = data
                    .input_records
                    .iter()
                    .filter(|r| r.session_id == s.id)
                    .collect();
                records.len() == 1 && !records[0].truncated && records[0].text == s.expected_text
            })
            .count();
        let fragments = inputs
            .iter()
            .map(|s| {
                data.input_records
                    .iter()
                    .filter(|r| r.session_id == s.id)
                    .count()
            })
            .sum::<usize>();
        m.insert("input_completeness".into(), if data.inputs.is_some() { Metric::ratio(complete as f64,sent.len() as f64,"比例","发出的完整内容精确保存为一条且未截断的输入数/已标注发出次数；零记录、截断、多条拼合均不算完整；按输入发生日归属") } else { Metric::missing("完整单条输入/发出次数", "没有输入框值与可靠发出信号", "受控输入及发出真值对照：保留完整输入框值、发出信号与最终记录") });
        m.insert("input_fragmentation".into(), if data.inputs.is_some() { Metric::ratio(fragments as f64,inputs.len() as f64,"记录/输入","每个标注输入（含草稿）的输出记录数之和/标注输入数；丢失输入保留在分母；跨日片段跟随输入发生日") } else { Metric::missing("记录/输入", "没有可靠输入会话标注", "以输入框值流与受控标注建立输入会话") });
        m.insert("text_coverage".into(), if let (Some(e),Some(c)) = (&data.effective,&data.covered) { let eff = union(e,a,b); Metric::ratio(intersection(&eff,&union(c,a,b)) as f64,duration(&eff) as f64,"比例","有效使用时段与可用 AX/OCR 文字时段交集的并集毫秒/有效使用并集毫秒；排除锁屏、暂停、离开；权限缺失和无文字有效时段保留在分母；不向下一采样外推") } else { Metric::missing("可用文字覆盖毫秒/有效使用毫秒", "旧库没有连续有效使用与文字有效区间信号", "受控运行记录锁屏/空闲/暂停及 AX/OCR 可用区间；不要把采样命中率当时长覆盖率") });
        m.insert("screenshot_bytes".into(), if let Some(shots) = &data.shots { Metric::ratio(shots.iter().filter(|s| inside(s.at_ms,a,b)).map(|s| s.bytes as f64).sum(),1.,"字节/所选日","当日截图事件所关联 original 字节数之和；同图多次引用重复计数；部分日不外推 24 小时；另报当日不同原件字节数；不据此推定画面变化覆盖") } else { Metric::missing("截图字节/日","缺少截图字节元数据","记录每张截图原件字节数") });
        if let Some(shots) = &data.shots {
            let unique: HashMap<_, _> = shots
                .iter()
                .filter(|s| inside(s.at_ms, a, b))
                .map(|s| (&s.blob_id, s.bytes))
                .collect();
            m.insert("screenshot_unique_bytes".into(),Metric::ratio(unique.values().map(|n| *n as f64).sum(),1.,"字节/所选日","当日引用的不同 original 原件字节；跨日共享原件各日计一次，不代表实际新增磁盘分配"));
        }
        let resources: Vec<_> = data
            .resources
            .iter()
            .flatten()
            .filter(|r| r.span.start_ms >= a && r.span.end_ms <= b)
            .collect();
        let crossing = data.resources.iter().flatten().any(|r| {
            r.span.start_ms < b
                && r.span.end_ms > a
                && !(r.span.start_ms >= a && r.span.end_ms <= b)
        });
        let wall = resources
            .iter()
            .map(|r| r.span.end_ms - r.span.start_ms)
            .sum::<i64>();
        for (key, num, scale, unit, method) in [
            (
                "capture_cpu",
                resources.iter().map(|r| r.cpu_ms as f64).sum::<f64>(),
                100.,
                "单核百分比",
                "被测采集进程组 CPU 毫秒/采样墙钟毫秒 ×100；报告采样时长，不外推未采样时段",
            ),
            (
                "capture_wakeups",
                resources.iter().map(|r| r.wakeups as f64).sum::<f64>(),
                1000.,
                "次/秒",
                "被测采集进程组唤醒次数/采样墙钟毫秒 ×1000；报告采样时长",
            ),
        ] {
            let mut metric = if data.resources.is_some() && !crossing {
                Metric::ratio(num, wall as f64, unit, method)
            } else {
                Metric::missing(
                    method,
                    if crossing {
                        "资源计数跨日，不能按时长假定均摊"
                    } else {
                        "没有同期 CPU/唤醒计数"
                    },
                    "按本地午夜切分采集进程组累计 CPU 与唤醒计数；另测暂停对照，固定工作负载",
                )
            };
            metric.value = metric.value.map(|v| v * scale);
            m.insert(key.into(), metric);
        }
        let mut latencies: Vec<_> = data
            .searchable
            .iter()
            .flatten()
            .filter(|r| inside(r.occurred_ms, a, b))
            .filter_map(|r| r.searchable_ms.map(|t| t - r.occurred_ms))
            .collect();
        latencies.sort();
        if let Some(probes) = &data.searchable {
            let probes: Vec<_> = probes
                .iter()
                .filter(|r| inside(r.occurred_ms, a, b))
                .collect();
            m.insert(
                "search_probe_unresolved".into(),
                Metric::ratio(
                    probes.iter().filter(|r| r.searchable_ms.is_none()).count() as f64,
                    probes.len() as f64,
                    "比例",
                    "截至评估输入截止时尚未命中的探针数/全部探针数；均值和 p95 必须结合本项阅读",
                ),
            );
            m.insert(
                "search_probe_poll_interval_max".into(),
                if let Some(max_interval) = probes.iter().map(|r| r.poll_interval_ms).max() {
                    Metric::ratio(
                        max_interval as f64,
                        1.,
                        "毫秒",
                        "探针的最大轮询间隔，表示轮询造成的时间分辨率",
                    )
                } else {
                    Metric::missing("探针的最大轮询间隔", "没有探针样本", "运行受控唯一查询探针")
                },
            );
        }
        m.insert("searchable_latency_mean".into(),if data.searchable.is_some() { Metric::ratio(latencies.iter().map(|n|*n as f64).sum(),latencies.len() as f64,"毫秒","首次探针搜索命中时间减发生时间；均值；仅包含已观测命中，探针轮询间隔是测量误差上界") } else { Metric::missing("首次搜索命中时间−发生时间","旧库只有入库时间，没有首次可搜索的探针信号", "以合成唯一查询探针记录发生、提交和首次搜索命中；连同未命中与超时次数报告") });
        m.insert(
            "searchable_latency_p95".into(),
            if data.searchable.is_some() && !latencies.is_empty() {
                Metric::ratio(
                    latencies[(latencies.len() * 95).div_ceil(100) - 1] as f64,
                    1.,
                    "毫秒",
                    "已命中探针延迟的 nearest-rank p95；分母 1 为分位数展示，样本量见均值分母",
                )
            } else {
                Metric::missing(
                    "已命中探针 nearest-rank p95",
                    "没有搜索命中样本",
                    "运行受控唯一查询探针",
                )
            },
        );
        let observed_ms = duration(&union(&data.observed, a, b));
        if observed_ms == 0 {
            for metric in m.values_mut() {
                *metric = Metric::missing(
                    &metric.method,
                    "所选日没有观测数据",
                    "选择有数据的日期或补采观测",
                );
            }
        }
        days.push(DayReport {
            date: date.to_string(),
            start_ms: a,
            end_ms: b,
            observed_ms,
            metrics: m,
        });
        date = next;
    }
    let mut counts = data.counts.clone();
    for item in &data.context_items {
        *counts.entry(item.kind.clone()).or_default() += 1;
    }
    if let Some(v) = &data.visits {
        counts.insert("visits".into(), v.len() as u64);
    }
    if let Some(v) = &data.inputs {
        counts.insert("input_sessions".into(), v.len() as u64);
        counts.insert(
            "sent_inputs".into(),
            v.iter().filter(|i| i.sent).count() as u64,
        );
    }
    if let Some(v) = &data.shots {
        counts.insert("screenshots".into(), v.len() as u64);
    }
    counts.insert("reading_records".into(), data.text_records.len() as u64);
    if data.inputs.is_some() {
        counts.insert("input_records".into(), data.input_records.len() as u64);
    }
    Ok(Report {
        format: "candid-daily-metrics/1".into(),
        synthetic: data.synthetic,
        timezone: tz.to_string(),
        input_observed: data.observed.clone(),
        counts_scope: "输入数据整体；每日指标按请求日期裁切".into(),
        range_basis:
            "输入声明的观测范围；时间线适配器只取所选事件首末边界，表示数据跨度，不表示持续采集"
                .into(),
        requested_start: start.to_string(),
        requested_end_exclusive: end.to_string(),
        counts,
        days,
    })
}
