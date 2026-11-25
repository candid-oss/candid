//! Read one consistent SQLite snapshot without opening TimelineStore or running migrations.
use crate::*;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub struct Snapshot {
    data: Dataset,
    app_keys: Vec<String>,
    sample_total: Vec<i64>,
    event_times: Vec<i64>,
}
impl Snapshot {
    pub fn read(path: &Path, start_ms: i64, end_ms: i64) -> Result<Self> {
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        // v2 adds AI records, v4 mail/browser source kinds, v5 the derived layer (digests, entities), v9–v10 slimmer
        // stored inputs and selectors, v11 the AI-readable layer and v12 its items and sources.
        // The desktop tables and columns read below are unchanged by v12 and by v13 (the jobs energy ledger).
        if !(1..=13).contains(&version) {
            return Err("离线适配器支持时间线库 v1–v13；未知版本须先更新字段映射，不执行迁移".into());
        }
        let mut data = Dataset {
            format: "candid-measurement/1".into(),
            synthetic: false,
            ..Default::default()
        };
        let mut app_keys = Vec::new();
        let mut sample_total = Vec::new();
        let mut event_times = Vec::new();
        let mut stmt=db.prepare("SELECT e.event_id,e.start_ms,COALESCE(e.app_bundle_id,''),e.kind FROM events e JOIN sources s USING(source_id) WHERE s.kind='mac.desktop' AND e.start_ms>=?1 AND e.start_ms<?2 ORDER BY e.start_ms,e.event_id")?;
        let rows = stmt.query_map([start_ms, end_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut first = None;
        let mut last = None;
        let mut shots = Vec::new();
        for row in rows {
            let (id, t, app, kind) = row?;
            event_times.push(t);
            first.get_or_insert(t);
            last = Some(t);
            *data.counts.entry("desktop_events".into()).or_default() += 1;
            // Names and titles never become output keys.
            if kind == "screen.text" {
                sample_total.push(t);
                let mut text=db.prepare("SELECT c.text FROM event_text et JOIN text_chunks c USING(chunk_id) WHERE et.event_id=?1 AND et.field='body' ORDER BY et.ord")?;
                let paragraphs: Vec<String> = text
                    .query_map([&id], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?;
                app_keys.push(app);
                data.text_records.push(TextRecord {
                    at_ms: t,
                    visit_id: None,
                    text: paragraphs.join("\n"),
                });
            }
            if kind == "screen.shot" {
                let mut stmt=db.prepare("SELECT b.sha256,b.byte_count FROM event_blobs eb JOIN blobs b USING(sha256) WHERE eb.event_id=?1 AND eb.role='original'")?;
                let blob: Vec<(String, i64)> = stmt
                    .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<_, _>>()?;
                if blob.len() != 1 || blob[0].1 < 0 {
                    return Err("截图缺少唯一原件字节元数据，停止统计".into());
                }
                shots.push(Shot {
                    at_ms: t,
                    blob_id: blob[0].0.clone(),
                    bytes: blob[0].1 as u64,
                });
            }
        }
        data.counts
            .insert("text_samples".into(), sample_total.len() as u64);
        data.counts.insert("screenshots".into(), shots.len() as u64);
        data.counts.insert(
            "body_unicode_chars".into(),
            data.text_records
                .iter()
                .map(|r| r.text.chars().count() as u64)
                .sum(),
        );
        data.shots = Some(shots);
        if let (Some(a), Some(b)) = (first, last) {
            data.observed.push(Span {
                start_ms: a,
                end_ms: b.checked_add(1).ok_or("时间超限")?,
            });
        } else {
            return Err("所选范围没有桌面事件".into());
        }
        db.execute_batch("ROLLBACK;")?;
        Ok(Self {
            data,
            app_keys,
            sample_total,
            event_times,
        })
    }
    pub fn measure(&self, start: NaiveDate, end: NaiveDate, tz: Tz) -> Result<Report> {
        let mut report = measure(&self.data, start, end, tz)?;
        for day in &mut report.days {
            let (a, b) = (day.start_ms, day.end_ms);
            let times: Vec<_> = self
                .event_times
                .iter()
                .copied()
                .filter(|t| inside(*t, a, b))
                .collect();
            if times.is_empty() {
                day.observed_ms = 0;
                for metric in day.metrics.values_mut() {
                    *metric = Metric::missing(
                        &metric.method,
                        "所选日没有桌面事件",
                        "选择有数据的日期或补采观测",
                    );
                }
                continue;
            }
            day.observed_ms = times.last().unwrap() - times[0] + 1;
            let mut seen: HashMap<&str, HashSet<&str>> = HashMap::new();
            let mut total = 0;
            let mut repeated = 0;
            for (r, app) in self
                .data
                .text_records
                .iter()
                .zip(&self.app_keys)
                .filter(|(r, _)| inside(r.at_ms, a, b))
            {
                for line in lines(&r.text) {
                    total += line.chars().count();
                    if !seen.entry(app).or_default().insert(line) {
                        repeated += line.chars().count();
                    }
                }
            }
            day.metrics.insert("legacy_app_reference_redundancy".into(),Metric::ratio(repeated as f64,total as f64,"比例","仅历史参考：当日同 App 内重复非空 body 行字符/全部非空 body 行字符；首尾空白不计，Unicode 字符，跨访问也去重；不是按访问冗余度，不能与目标对比；空 App 合为未知组"));
            let samples = self
                .sample_total
                .iter()
                .filter(|t| inside(**t, a, b))
                .count();
            day.metrics.insert("legacy_sample_nonempty".into(),Metric::ratio(self.data.text_records.iter().filter(|r|inside(r.at_ms,a,b) && !r.text.trim().is_empty()).count() as f64,samples as f64,"比例","仅采样参考：有非空 body 的 screen.text 条数/全部 screen.text 条数；不含 OCR，不是有效使用时长覆盖率"));
        }
        Ok(report)
    }
}
