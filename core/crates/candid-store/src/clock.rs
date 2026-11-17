//! Time parsing and formatting in the user's time zone (timeline-v1 §5.1).

use crate::{Result, TimelineError};
use chrono::{DateTime, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike, Utc};
use std::sync::Arc;

pub(crate) fn system_now_ms() -> i64 {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs_f64() * 1000.0).round() as i64
}

/// The time zone used to read and print times.
#[derive(Clone, Debug, PartialEq)]
pub enum Zone {
    /// The system zone (`TZ` or `/etc/localtime`).
    Local,
    Named(chrono_tz::Tz),
}

impl Zone {
    pub fn named(name: &str) -> Option<Zone> {
        name.parse::<chrono_tz::Tz>().ok().map(Zone::Named)
    }

    /// Offset from UTC in seconds at an instant.
    pub fn offset_seconds(&self, ms: i64) -> i32 {
        let utc = utc(ms);
        match self {
            Zone::Local => chrono::Local.offset_from_utc_datetime(&utc.naive_utc()).fix().local_minus_utc(),
            Zone::Named(tz) => tz.offset_from_utc_datetime(&utc.naive_utc()).fix().local_minus_utc(),
        }
    }

    pub fn local(&self, ms: i64) -> NaiveDateTime {
        utc(ms).naive_utc() + Duration::seconds(self.offset_seconds(ms) as i64)
    }

    /// The first instant at or after a local wall time (a wall time skipped by a DST change moves forward).
    pub fn instant(&self, local: NaiveDateTime) -> i64 {
        let mut probe = local;
        for _ in 0..(4 * 60) {
            let r = match self {
                Zone::Local => map_result(chrono::Local.from_local_datetime(&probe)),
                Zone::Named(tz) => map_result(tz.from_local_datetime(&probe)),
            };
            if let Some(ms) = r {
                return ms;
            }
            probe += Duration::minutes(1);
        }
        local.and_utc().timestamp_millis()
    }

    pub fn day_start(&self, date: NaiveDate) -> i64 {
        self.instant(date.and_time(NaiveTime::MIN))
    }
}

fn map_result<Tz: TimeZone>(r: LocalResult<DateTime<Tz>>) -> Option<i64> {
    match r {
        LocalResult::Single(d) => Some(d.timestamp_millis()),
        LocalResult::Ambiguous(a, b) => Some(a.timestamp_millis().min(b.timestamp_millis())),
        LocalResult::None => None,
    }
}

fn utc(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

/// Current time and time zone. Tests pass a fixed instant.
#[derive(Clone)]
pub struct Clock {
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub zone: Zone,
}

impl Default for Clock {
    fn default() -> Self {
        Clock { now: Arc::new(system_now_ms), zone: Zone::Local }
    }
}

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Clock({:?})", self.zone)
    }
}

impl Clock {
    pub fn fixed(now_ms: i64, zone: Zone) -> Clock {
        Clock { now: Arc::new(move || now_ms), zone }
    }

    pub fn with(now: impl Fn() -> i64 + Send + Sync + 'static, zone: Zone) -> Clock {
        Clock { now: Arc::new(now), zone }
    }

    pub fn now_ms(&self) -> i64 {
        (self.now)()
    }

    fn start_of_day(&self, ms: i64) -> i64 {
        self.zone.day_start(self.zone.local(ms).date())
    }

    fn end_of_day(&self, ms: i64) -> i64 {
        let date = self.zone.local(ms).date();
        self.zone.day_start(date.succ_opt().unwrap_or(date))
    }

    /// Parses RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`now` and relative `-2h`/`+30m`/`-3d`.
    /// For an exclusive range end, a date-only value or `today`/`yesterday` means the end of that day.
    pub fn parse(&self, text: &str, as_end: bool) -> Result<i64> {
        let s = text.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\u{a0}' || c == '\u{3000}');
        let now = self.now_ms();
        match s.to_lowercase().as_str() {
            "now" => return Ok(now),
            "today" => return Ok(if as_end { self.end_of_day(now) } else { self.start_of_day(now) }),
            "yesterday" => {
                let today = self.zone.local(now).date();
                let y = today.pred_opt().unwrap_or(today);
                let start = self.zone.day_start(y);
                return Ok(if as_end { self.zone.day_start(today) } else { start });
            }
            _ => {}
        }
        let chars: Vec<char> = s.chars().collect();
        if chars.len() >= 3 && (chars[0] == '-' || chars[0] == '+') && "smhdw".contains(chars[chars.len() - 1]) {
            let middle: String = chars[1..chars.len() - 1].iter().collect();
            if let Ok(amount) = middle.parse::<f64>() {
                if amount.is_finite() {
                    let unit = match chars[chars.len() - 1] {
                        's' => 1.0,
                        'm' => 60.0,
                        'h' => 3600.0,
                        'd' => 86400.0,
                        _ => 604800.0,
                    };
                    let delta = amount * unit * if chars[0] == '-' { -1.0 } else { 1.0 };
                    return Ok(((now as f64 / 1000.0 + delta) * 1000.0).round() as i64);
                }
            }
        }
        if let Some(ms) = parse_internet_date_time(s) {
            return Ok(ms);
        }
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 3 && parts[0].len() == 4 && parts[1].len() == 2 && parts[2].len() == 2 {
            if let (Ok(y), Ok(m), Ok(d)) = (parts[0].parse::<i32>(), parts[1].parse::<u32>(), parts[2].parse::<u32>()) {
                if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
                    let start = self.zone.day_start(date);
                    return Ok(if as_end { self.zone.day_start(date.succ_opt().unwrap_or(date)) } else { start });
                }
            }
        }
        Err(TimelineError::InvalidInput(format!("无法解析时间 {text}")))
    }

    /// `2027-01-15T00:00:00.000+08:00` in this clock's zone (`Z` when the offset is zero).
    pub fn iso(&self, ms: i64) -> String {
        let offset = self.zone.offset_seconds(ms);
        let local = utc(ms).naive_utc() + Duration::seconds(offset as i64);
        let millis = ms.rem_euclid(1000);
        let base = format!("{}.{:03}", local.format("%Y-%m-%dT%H:%M:%S"), millis);
        if offset == 0 {
            return base + "Z";
        }
        let sign = if offset < 0 { '-' } else { '+' };
        let a = offset.abs();
        format!("{base}{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
    }

    /// `HH:mm` in this clock's zone.
    pub fn hm(&self, ms: i64) -> String {
        self.zone.local(ms).format("%H:%M").to_string()
    }

    pub fn hour_of(&self, ms: i64) -> u32 {
        self.zone.local(ms).hour()
    }

    /// Start of the next local hour after `ms`.
    pub fn next_hour(&self, ms: i64) -> i64 {
        let local = self.zone.local(ms);
        let truncated = local.date().and_hms_opt(local.hour(), 0, 0).unwrap_or(local);
        let next = truncated + Duration::hours(1);
        let candidate = self.zone.instant(next);
        if candidate > ms {
            candidate
        } else {
            ms + 3_600_000 - (ms.rem_euclid(3_600_000))
        }
    }
}

/// RFC 3339 date-time (`T` separator), with or without fractional seconds.
pub(crate) fn parse_internet_date_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[10] != b'T' {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp_millis());
    }
    DateTime::<FixedOffset>::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z").ok().map(|d| d.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY_START: i64 = 1_799_942_400_000;

    fn clock() -> Clock {
        Clock::fixed(1_800_000_000_000, Zone::named("Asia/Shanghai").unwrap())
    }

    #[test]
    fn relative_and_absolute_time_parsing() {
        let c = clock();
        assert_eq!(c.parse("now", false).unwrap(), 1_800_000_000_000);
        assert_eq!(c.parse("-2h", false).unwrap(), 1_800_000_000_000 - 7_200_000);
        assert_eq!(c.parse("+30m", false).unwrap(), 1_800_000_000_000 + 1_800_000);
        assert_eq!(c.parse("today", false).unwrap(), TODAY_START);
        assert_eq!(c.parse("yesterday", false).unwrap(), TODAY_START - 86_400_000);
        assert_eq!(c.parse("today", true).unwrap(), TODAY_START + 86_400_000);
        assert_eq!(c.parse("2027-01-15", false).unwrap(), TODAY_START);
        assert_eq!(c.parse("2027-01-15", true).unwrap(), TODAY_START + 86_400_000);
        assert_eq!(c.parse("2027-01-15T00:00:00+08:00", false).unwrap(), TODAY_START);
        assert_eq!(c.parse("2027-01-14T16:00:00.500Z", false).unwrap(), TODAY_START + 500);
        assert_eq!(c.iso(TODAY_START), "2027-01-15T00:00:00.000+08:00");
        for bad in ["", "tomorrow-ish", "2027-13-40", "-h", "yesterday!"] {
            assert!(c.parse(bad, false).is_err(), "{bad}");
        }
    }
}
