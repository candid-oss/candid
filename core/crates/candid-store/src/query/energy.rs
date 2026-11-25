//! The energy ledger (power v1 §3.6): what background work cost, by day and job kind, and what is waiting.
//! Counts and seconds only, never content. The settings line in the app, `core.log`'s hourly line and the
//! `energy_summary` tool all read this one query.
use crate::{
    db::{int, RowExt},
    Result, TimelineStore,
};
use serde_json::{json, Value};

impl TimelineStore {
    /// `start`/`end` as in the other tools (default: today). Days are the user's local days.
    pub fn energy_summary(&self, args: &Value, client_id: &str) -> Result<Value> {
        self.audited("energy_summary", client_id, args, |r: &Value| r["days"].as_array().map(|d| d.len() as i64), |_| {
            Ok(self.energy_summary_inner(args["start"].as_str(), args["end"].as_str())?)
        })
    }

    pub(crate) fn energy_summary_inner(&self, start: Option<&str>, end: Option<&str>) -> Result<Value> {
        let (ws, we) = self.window(start, end, None, true)?;
        let ws = ws.unwrap_or(0);
        let we = we.unwrap_or(i64::MAX);
        let offset = self.clock.zone.offset_seconds(self.clock.now_ms());
        let rows = self.db.query(
            &format!(
                "SELECT strftime('%Y-%m-%d', updated_ms/1000 + {offset}, 'unixepoch'), kind, COUNT(*), COALESCE(SUM(cpu_ms),0), COALESCE(SUM(wall_ms),0),
                   COALESCE(SUM(CASE WHEN power_state='ac' THEN 0 ELSE cpu_ms END),0), SUM(manual)
                 FROM jobs INDEXED BY jobs_updated WHERE updated_ms>=? AND updated_ms<? AND wall_ms IS NOT NULL GROUP BY 1,2 ORDER BY 1,2"
            ),
            &[int(ws), int(we)],
            |r| (r.text(0), r.text(1), r.int(2), r.int(3), r.int(4), r.int(5), r.int(6)),
        )?;
        let seconds = |ms: i64| (ms as f64 / 100.0).round() / 10.0;
        let mut days: Vec<Value> = vec![];
        for (day, kind, runs, cpu, wall, battery, manual) in rows {
            if days.last().is_none_or(|d| d["date"] != day) {
                days.push(json!({"date": day, "runs": 0, "cpu_seconds": 0.0, "wall_seconds": 0.0, "battery_cpu_seconds": 0.0, "kinds": []}));
            }
            let d = days.last_mut().unwrap();
            d["runs"] = json!(d["runs"].as_i64().unwrap_or(0) + runs);
            for (key, ms) in [("cpu_seconds", cpu), ("wall_seconds", wall), ("battery_cpu_seconds", battery)] {
                d[key] = json!(seconds((d[key].as_f64().unwrap_or(0.0) * 1000.0).round() as i64 + ms));
            }
            d["kinds"].as_array_mut().unwrap().push(json!({
                "kind": kind, "runs": runs, "manual_runs": manual,
                "cpu_seconds": seconds(cpu), "wall_seconds": seconds(wall), "battery_cpu_seconds": seconds(battery),
            }));
        }
        // Waiting is a state, not a day's history: everything queued now, by reason and kind.
        let waiting = self.db.query(
            "SELECT COALESCE(error_code,'queued'), kind, COUNT(*) FROM jobs INDEXED BY jobs_pending WHERE state='queued' AND state IN ('queued','running') GROUP BY 1,2 ORDER BY 1,2",
            &[],
            |r| json!({"reason": r.text(0), "kind": r.text(1), "count": r.int(2)}),
        )?;
        let total: i64 = waiting.iter().filter(|w| w["reason"] != "queued").map(|w| w["count"].as_i64().unwrap_or(0)).sum();
        Ok(json!({
            "start_ms": ws, "end_ms": if we == i64::MAX { Value::Null } else { json!(we) },
            "days": days,
            "waiting": {"total": total, "items": waiting},
        }))
    }

    /// One count-only line for `core.log`: the last hour's background runs and CPU, and what waits.
    pub fn energy_log_line(&self, from_ms: i64, to_ms: i64) -> Result<String> {
        let (runs, cpu, battery) = self.db.first(
            "SELECT COUNT(*), COALESCE(SUM(cpu_ms),0), COALESCE(SUM(CASE WHEN power_state='ac' THEN 0 ELSE cpu_ms END),0) FROM jobs INDEXED BY jobs_updated WHERE updated_ms>=? AND updated_ms<? AND wall_ms IS NOT NULL",
            &[int(from_ms), int(to_ms)],
            |r| (r.int(0), r.int(1), r.int(2)),
        )?.unwrap_or((0, 0, 0));
        let waiting = self.db.scalar("SELECT COUNT(*) FROM jobs INDEXED BY jobs_pending WHERE state='queued' AND state IN ('queued','running') AND error_code GLOB 'waiting_*'", &[])?;
        Ok(format!("能耗：近一小时后台作业 {runs} 个，CPU {:.1}s（电池上 {:.1}s），等待 {waiting} 项", cpu as f64 / 1000.0, battery as f64 / 1000.0))
    }

    /// Adds one run's cost to its job: thread CPU of the worker plus the voice service's own `cpu_seconds`, wall
    /// time, and the power state it ran on.
    pub fn job_account(&self, job_id: &str, cpu_ms: i64, wall_ms: i64, power_state: &str) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET cpu_ms=COALESCE(cpu_ms,0)+?, wall_ms=COALESCE(wall_ms,0)+?, power_state=? WHERE job_id=?",
            &[int(cpu_ms.max(0)), int(wall_ms.max(0)), crate::db::text(power_state), crate::db::text(job_id)],
        )
    }

    /// The app's own background work (`app.` kinds) is recorded as finished jobs so the ledger covers it too.
    /// One row per kind and subject; repeated runs add up.
    pub fn app_work_account(&self, kind: &str, subject: &str, cpu_ms: i64, wall_ms: i64) -> Result<()> {
        if !kind.starts_with("app.") || crate::job_priority::class_of(kind).is_none() || subject.is_empty() {
            return Err(crate::TimelineError::InvalidInput("未登记的 App 后台作业".into()));
        }
        let power = self.power_state_now()?;
        let id = self.enqueue_job(kind, subject, "app/1", None, None)?;
        let now = self.clock.now_ms();
        self.db.execute("UPDATE jobs SET state='done',attempts=attempts+1,updated_ms=? WHERE job_id=?", &[int(now), crate::db::text(&id)])?;
        self.job_account(&id, cpu_ms, wall_ms, power)
    }
}
