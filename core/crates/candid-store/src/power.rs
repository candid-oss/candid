//! When background work may spend power: the one decision for the whole repository (power v1 §3.1, §3.2, §9.1).
//!
//! The platform layer reports the Mac's power state; the core keeps the latest report and the user's pace
//! (随时 / 省电 / 手动) in `store_meta`. [`TimelineStore::power_allows`] is the only place that answers "may this
//! work spend power now": `feature_check` asks it before every background model call, the pending-job query asks
//! it to skip work that would only be refused, and the app asks it (`POST /local/power {"action":"allows"}`)
//! before its own OCR and vision passes.
//!
//! The control state is separate from the evidence: `system.state` power events in the timeline stay evidence and
//! are never read here.
use crate::{
    db::{int, text, RowExt},
    job_priority::Cost,
    Result, TimelineError, TimelineStore,
};
use serde_json::{json, Value};

pub const STATE_KEY: &str = "power.state/1";
pub const PACE_KEY: &str = "power.pace/1";
/// A report older than this is unknown, and unknown counts as battery.
pub const STALE_MS: i64 = 10 * 60_000;
pub const PACES: [&str; 3] = ["anytime", "saver", "manual"];
pub const DEFAULT_PACE: &str = "saver";
/// Waiting reasons the gate gives; the pending-job query re-checks them whenever the gate changes.
pub const REASONS: [&str; 3] = ["waiting_power", "waiting_thermal", "waiting_manual"];

/// What the gate blocks right now: every automatic job (`all`, manual pace) or only heavy ones (`heavy`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PowerGate {
    pub all: Option<&'static str>,
    pub heavy: Option<&'static str>,
}

impl TimelineStore {
    fn power_meta(&self, key: &str) -> Result<Option<String>> {
        self.db.first("SELECT value FROM store_meta WHERE key=?", &[text(key)], |r| r.text(0))
    }

    /// The user's pace; `saver` until chosen.
    pub fn power_pace(&self) -> Result<String> {
        Ok(self.power_meta(PACE_KEY)?.filter(|p| PACES.contains(&p.as_str())).unwrap_or_else(|| DEFAULT_PACE.into()))
    }

    /// Stores a report from the platform layer (or `candid serve --power`). Fields: `external_power: bool|null`,
    /// `battery_percent: int|null`, `low_power_mode: bool`, `thermal: nominal|fair|serious|critical|null`,
    /// `idle_seconds: int|null`; `declared: true` (command line only) never goes stale. Returns the status and
    /// whether the gate's decision changed (the caller then wakes the background worker).
    pub fn power_report(&self, v: &Value) -> Result<Value> {
        let invalid = |what: &str| TimelineError::InvalidInput(format!("电源状态无效：{what}"));
        let opt_bool = |k: &str| -> Result<Value> {
            match &v[k] { Value::Null => Ok(Value::Null), Value::Bool(b) => Ok(json!(b)), _ => Err(invalid(k)) }
        };
        let opt_int = |k: &str, max: i64| -> Result<Value> {
            match &v[k] {
                Value::Null => Ok(Value::Null),
                n => n.as_i64().filter(|n| (0..=max).contains(n)).map(|n| json!(n)).ok_or_else(|| invalid(k)),
            }
        };
        let thermal = match &v["thermal"] {
            Value::Null => Value::Null,
            Value::String(s) if ["nominal", "fair", "serious", "critical"].contains(&s.as_str()) => json!(s),
            _ => return Err(invalid("thermal")),
        };
        let low = match &v["low_power_mode"] { Value::Null => false, Value::Bool(b) => *b, _ => return Err(invalid("low_power_mode")) };
        let state = json!({
            "external_power": opt_bool("external_power")?,
            "battery_percent": opt_int("battery_percent", 100)?,
            "low_power_mode": low,
            "thermal": thermal,
            "idle_seconds": opt_int("idle_seconds", i64::MAX)?,
            "declared": v["declared"] == true,
            "reported_ms": self.clock.now_ms(),
        });
        let before = self.power_gate()?;
        self.db.execute("INSERT OR REPLACE INTO store_meta VALUES(?,?)", &[text(STATE_KEY), text(state.to_string())])?;
        self.power_changed(before)
    }

    /// At core start: `Some(ac)` stores a declared state (`candid serve --power`); `None` drops an earlier
    /// declaration so the state is unknown until the platform layer reports.
    pub fn power_declare(&self, external_power: Option<bool>) -> Result<()> {
        match external_power {
            Some(ac) => self.power_report(&json!({"external_power": ac, "declared": true})).map(|_| ()),
            None => self.db.execute("DELETE FROM store_meta WHERE key=? AND json_extract(value,'$.declared')=1", &[text(STATE_KEY)]),
        }
    }

    pub fn power_set_pace(&self, pace: &str) -> Result<Value> {
        if !PACES.contains(&pace) {
            return Err(TimelineError::InvalidInput("后台处理只能是 anytime、saver 或 manual".into()));
        }
        let before = self.power_gate()?;
        self.db.execute("INSERT OR REPLACE INTO store_meta VALUES(?,?)", &[text(PACE_KEY), text(pace)])?;
        self.power_changed(before)
    }

    /// A changed decision bumps the workflows revision, so the background worker re-reads the queue at once and
    /// the jobs the gate held start (or stop being picked).
    fn power_changed(&self, before: PowerGate) -> Result<Value> {
        let changed = self.power_gate()? != before;
        if changed {
            self.db.execute("UPDATE background_revisions SET revision=revision+1 WHERE domain='workflows'", &[])?;
        }
        let mut status = self.power_status()?;
        status["changed"] = json!(changed);
        Ok(status)
    }

    /// The stored report plus `pace`, `stale`, `power_state` (`ac`, `battery`, `unknown`), `heavy_allowed` and the
    /// `reason` heavy work waits for (`waiting_power`, `waiting_thermal`, `waiting_manual`, or null).
    pub fn power_status(&self) -> Result<Value> {
        let mut state: Value = self.power_meta(STATE_KEY)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| {
            json!({"external_power":null,"battery_percent":null,"low_power_mode":false,"thermal":null,"idle_seconds":null,"declared":false,"reported_ms":null})
        });
        let stale = self.power_stale(&state);
        state["pace"] = json!(self.power_pace()?);
        state["stale"] = json!(stale);
        state["power_state"] = json!(power_state(&state, stale));
        let reason = self.power_allows(Cost::Heavy, false).err().map(|e| e.code());
        state["heavy_allowed"] = json!(reason.is_none());
        state["reason"] = json!(reason);
        Ok(state)
    }

    fn power_stale(&self, state: &Value) -> bool {
        state["declared"] != true && state["reported_ms"].as_i64().is_none_or(|t| self.clock.now_ms() - t > STALE_MS)
    }

    /// `ac`, `battery` or `unknown` right now; stored on each job when it runs.
    pub fn power_state_now(&self) -> Result<&'static str> {
        let state: Value = self.power_meta(STATE_KEY)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let stale = self.power_stale(&state);
        Ok(power_state(&state, stale))
    }

    /// May work of this cost spend power now? The only such decision in the repository. A manual run
    /// (「现在处理」) always may; otherwise:
    /// - 手动 (manual pace): every automatic job waits (`waiting_manual`);
    /// - serious or critical thermal state: heavy work waits in every pace (`waiting_thermal`);
    /// - 省电, or 随时 with Low Power Mode on: heavy work waits on battery or unknown power (`waiting_power`).
    ///
    /// `Cost::ByModel` that was not resolved to a provider counts as heavy.
    pub fn power_allows(&self, cost: Cost, manual: bool) -> Result<()> {
        if manual {
            return Ok(());
        }
        let gate = self.power_gate()?;
        let reason = gate.all.or(if cost == Cost::Light { None } else { gate.heavy });
        match reason {
            Some(r) => Err(TimelineError::Denied(r.into())),
            None => Ok(()),
        }
    }

    /// The gate as two answers, read once per worker round.
    pub fn power_gate(&self) -> Result<PowerGate> {
        let pace = self.power_pace()?;
        if pace == "manual" {
            return Ok(PowerGate { all: Some("waiting_manual"), heavy: Some("waiting_manual") });
        }
        let state: Value = self.power_meta(STATE_KEY)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let stale = self.power_stale(&state);
        let fresh = |k: &str| if stale { Value::Null } else { state[k].clone() };
        if ["serious", "critical"].contains(&fresh("thermal").as_str().unwrap_or("")) {
            return Ok(PowerGate { all: None, heavy: Some("waiting_thermal") });
        }
        let saver = pace == "saver" || fresh("low_power_mode") == true;
        if saver && power_state(&state, stale) != "ac" {
            return Ok(PowerGate { all: None, heavy: Some("waiting_power") });
        }
        Ok(PowerGate::default())
    }

    /// `POST /local/power`: `report`, `status`, `set_pace`, and `allows` for the platform layer
    /// (`{"kind":"app.ocr","manual":false,"local":true}` → `{"allowed":bool,"reason":...}`).
    pub fn power_command(&self, v: &Value) -> Result<Value> {
        match v["action"].as_str().unwrap_or("status") {
            "report" => self.power_report(v),
            "status" => self.power_status(),
            "set_pace" => self.power_set_pace(v["pace"].as_str().unwrap_or("")),
            "allows" => {
                let kind = v["kind"].as_str().filter(|k| crate::job_priority::class_of(k).is_some())
                    .ok_or_else(|| TimelineError::InvalidInput("未登记的后台作业种类".into()))?;
                let cost = resolve(crate::job_priority::job_cost(kind), v["local"].as_bool());
                let result = self.power_allows(cost, v["manual"] == true);
                Ok(json!({"allowed": result.is_ok(), "reason": result.err().map(|e| e.code())}))
            }
            _ => Err(TimelineError::InvalidInput("未知电源操作".into())),
        }
    }

    /// Whether the model assigned to `role` runs on this Mac (loopback provider); None when no model is set.
    pub fn role_model_is_local(&self, role: &str) -> Option<bool> {
        let c = self.model_config().ok()?;
        let a = c["roles"].as_array()?.iter().find(|a| a["role"] == role && a["model"].is_object())?;
        let p = c["providers"].as_array()?.iter().find(|p| p["provider_id"] == a["model"]["provider_id"])?;
        Some(crate::feature_settings::is_loopback(p["base_url"].as_str().unwrap_or("")))
    }

    /// 「现在处理」: marks waiting jobs (by `id`, or every queued job of `input_ref` and its recording parts) as
    /// manual and due now. Manual only passes the power gate; authorization, sources and daily limits still apply.
    pub fn job_run_now(&self, v: &Value) -> Result<Value> {
        let now = self.clock.now_ms();
        let mut ids: Vec<String> = vec![];
        if let Some(id) = v["id"].as_str().filter(|s| !s.is_empty()) {
            ids.push(id.into());
        } else if let Some(input) = v["input_ref"].as_str().filter(|s| !s.is_empty()) {
            let mut inputs = vec![input.to_string()];
            inputs.extend(self.media_part_ids(input).unwrap_or_default());
            for input in inputs {
                ids.extend(self.db.query("SELECT job_id FROM jobs WHERE input_ref=? AND state='queued'", &[text(input)], |r| r.text(0))?);
            }
        } else {
            return Err(TimelineError::InvalidInput("缺少任务或输入".into()));
        }
        let mut marked = 0;
        for id in &ids {
            self.db.execute("UPDATE jobs SET manual=1,next_run_ms=NULL,error_code=NULL,updated_ms=? WHERE job_id=? AND state='queued'", &[int(now), text(id)])?;
            marked += self.db.changes();
        }
        Ok(json!({"marked": marked}))
    }
}

/// The cost a call actually has once its provider is known: a model on this Mac is heavy, a remote one light.
pub fn resolve(cost: Cost, local_model: Option<bool>) -> Cost {
    match (cost, local_model) {
        (Cost::ByModel, Some(true)) => Cost::Heavy,
        (Cost::ByModel, Some(false)) => Cost::Light,
        (c, _) => c,
    }
}

fn power_state(state: &Value, stale: bool) -> &'static str {
    if stale {
        return "unknown";
    }
    match state["external_power"].as_bool() {
        Some(true) => "ac",
        Some(false) => "battery",
        None => "unknown",
    }
}
