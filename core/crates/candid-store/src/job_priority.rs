//! The one register of background work: which kinds exist, who runs them, what they cost, which goes first, and
//! which ones stop mattering after a while.
//!
//! One small table drives the pending-job query, the worker's order, the stale-job cleanup, the power gate
//! ([`crate::power`]) and the energy ledger, so a new background job kind is one line here and everything else
//! follows. A kind that appears in code without a line here fails the `every_job_kind_is_registered` test.
//!
//! P0 is the interactive lane (`complete()`, compose, Pi): it never queues here. Background lanes are P1..P3;
//! within one priority the oldest job goes first.

/// How much power one run takes; the power gate decides from this alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// Always runs a model on this Mac (speech recognition, the speaker pass): waits on battery in 省电.
    Heavy,
    /// No model, or a remote one: plain CPU work such as projection, compaction or image encoding.
    Light,
    /// Calls a model through the role's provider: heavy when the provider is on this Mac (loopback), light when
    /// remote. Unresolved (no provider known yet) counts as heavy.
    ByModel,
}

/// Who executes jobs of a kind. Only `Core` kinds are picked up by the core's background worker; the others are
/// registered so the gate, the waiting reasons and the ledger cover them too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner {
    /// The core's background worker (`workflow_prepare` → model → `workflow_complete`).
    Core,
    /// The macOS app (kinds start with `app.`); it asks the core's gate before running.
    App,
    /// Pi runs it after claiming it from the automation queue.
    Agent,
}

/// A job kind (exact) or kind family (prefix ending in `.`), its priority, cost, runner and useful age.
pub struct JobClass {
    pub kind: &'static str,
    pub priority: u8,
    /// Queued jobs older than this are abandoned with `expired`.
    pub max_age_ms: Option<i64>,
    pub cost: Cost,
    pub runner: Runner,
}

const DAY: i64 = 86_400_000;

const fn core(kind: &'static str, priority: u8, max_age_ms: Option<i64>, cost: Cost) -> JobClass {
    JobClass { kind, priority, max_age_ms, cost, runner: Runner::Core }
}

const fn app(kind: &'static str, cost: Cost) -> JobClass {
    JobClass { kind, priority: 3, max_age_ms: None, cost, runner: Runner::App }
}

pub const JOB_CLASSES: &[JobClass] = &[
    // P1: feeds everything downstream (conversations, items), so it goes first.
    core("screen.excerpt", 1, Some(DAY), Cost::ByModel),
    // Entry extraction is never dropped for age: over the daily limit it waits, and a later job covers more messages.
    core("items.extract", 1, None, Cost::ByModel),
    // P2: recording flows. The voice service runs on this Mac, so recognition and the speaker pass are always heavy.
    core("speech.recognize", 2, None, Cost::Heavy),
    core("workflow.speakers", 2, None, Cost::Heavy),
    // Titles, summaries and the other fixed steps use the role's model.
    core("workflow.", 2, None, Cost::ByModel),
    // P3: can wait for a quiet moment or the next day.
    core("screen.summarize", 3, None, Cost::ByModel),
    // Batches are re-checked against the current candidates when they run, so a long wait does no harm.
    core("people.judge", 3, None, Cost::ByModel),
    // Automatic skill runs: queued by the core, claimed and run by Pi with the agent_chat model.
    JobClass { kind: "agent.skill", priority: 3, max_age_ms: None, cost: Cost::ByModel, runner: Runner::Agent },
    // The app's own background work on screenshots; it asks the gate (`POST /local/power {"action":"allows"}`)
    // before each automatic run. Capture itself (and its inline WebP encoding) is not background work.
    // System OCR (Vision framework) of a captured or stored screenshot: a fraction of a second, no large model.
    app("app.ocr", Cost::Light),
    // Screenshot analysis or OCR through the 「图像理解」/「文字识别」 role's model.
    app("app.vision", Cost::ByModel),
];

/// Priority used for a kind the table does not know. Such jobs are not picked up by the worker at all.
pub const DEFAULT_PRIORITY: u8 = 2;

pub fn class_of(kind: &str) -> Option<&'static JobClass> {
    // Exact entries win over a family prefix (`workflow.speakers` before `workflow.`).
    JOB_CLASSES.iter().find(|c| c.kind == kind).or_else(|| JOB_CLASSES.iter().find(|c| c.kind.ends_with('.') && kind.starts_with(c.kind)))
}

/// 1 (first) to 3 (last) for background kinds.
pub fn job_priority(kind: &str) -> u8 {
    class_of(kind).map_or(DEFAULT_PRIORITY, |c| c.priority)
}

/// Registered cost of a kind; an unknown kind counts as heavy (the gate stays conservative).
pub fn job_cost(kind: &str) -> Cost {
    class_of(kind).map_or(Cost::Heavy, |c| c.cost)
}

/// Kinds the core's background worker picks up.
pub fn is_background_kind(kind: &str) -> bool {
    class_of(kind).is_some_and(|c| c.runner == Runner::Core)
}

fn glob(c: &JobClass) -> String {
    if c.kind.ends_with('.') { format!("kind GLOB '{}*'", c.kind) } else { format!("kind='{}'", c.kind) }
}

/// SQL predicate on `kind` matching every kind the core's worker runs.
pub(crate) fn kind_filter() -> String {
    let parts: Vec<String> = JOB_CLASSES.iter().filter(|c| c.runner == Runner::Core).map(glob).collect();
    format!("({})", parts.join(" OR "))
}

/// SQL predicate on `kind` matching the core kinds registered as always heavy.
pub(crate) fn heavy_filter() -> String {
    let parts: Vec<String> = JOB_CLASSES.iter().filter(|c| c.runner == Runner::Core && c.cost == Cost::Heavy).map(glob).collect();
    if parts.is_empty() { "0".into() } else { format!("({})", parts.join(" OR ")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !["target", "tests", "fixtures"].contains(&path.file_name().and_then(|n| n.to_str()).unwrap_or("")) { files(&path, ext, out); }
            } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                out.push(path);
            }
        }
    }

    /// String literals that follow `marker` in `text`, up to the closing quote.
    fn literals_after<'a>(text: &'a str, marker: &str) -> Vec<&'a str> {
        text.match_indices(marker).filter_map(|(i, _)| {
            let rest = &text[i + marker.len()..];
            let end = rest.find(['"', '\''])?;
            Some(&rest[..end])
        }).collect()
    }

    /// Every job kind written anywhere in the core (enqueue calls, `JOB_KIND` constants, direct inserts, the
    /// catalog) and every `app.` kind the macOS app asks the gate about must have a line in `JOB_CLASSES`.
    #[test]
    fn every_job_kind_is_registered() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut kinds: Vec<(String, String)> = vec![];
        let mut rust = vec![];
        files(&root.join("core/crates"), "rs", &mut rust);
        for path in rust.iter().filter(|p| !p.ends_with("job_priority.rs")) {
            let text = std::fs::read_to_string(path).unwrap();
            let name = path.display().to_string();
            for marker in ["\"action\":\"enqueue\",\"kind\":\"", "JOB_KIND: &str = \"", "INTO jobs(job_id,kind,input_ref,config_version,state,created_ms,updated_ms,feature) VALUES(?,'"] {
                for kind in literals_after(&text, marker) { kinds.push((kind.into(), name.clone())); }
            }
            // `format!("workflow.{step}")`: the steps come from the catalog below.
        }
        let catalog: serde_json::Value = serde_json::from_str(crate::workflows::CATALOG).unwrap();
        for f in catalog["features"].as_array().unwrap() {
            if let Some(kind) = f["job_kind"].as_str() { kinds.push((kind.into(), "workflows/catalog.json".into())); }
        }
        let mut swift = vec![];
        files(&root.join("apps/macos/Sources"), "swift", &mut swift);
        assert!(!swift.is_empty(), "the macOS sources are part of the check");
        // The app names its background work only through `enum AppBackgroundWork: String` (case x = "app.x").
        for path in &swift {
            let text = std::fs::read_to_string(path).unwrap();
            if !text.contains("enum AppBackgroundWork") { continue; }
            for kind in literals_after(&text, "= \"app.") { kinds.push((format!("app.{kind}"), path.display().to_string())); }
        }
        assert!(kinds.iter().any(|(k, _)| k == "screen.excerpt") && kinds.iter().any(|(k, _)| k.starts_with("app.")), "the scan found nothing: {kinds:?}");
        let missing: Vec<String> = kinds.iter().filter(|(k, _)| class_of(k).is_none()).map(|(k, f)| format!("{k} ({f})")).collect();
        assert!(missing.is_empty(), "job kinds without a line in JOB_CLASSES (add cost, priority and runner there):\n{}", missing.join("\n"));
        // App kinds are exactly the `app.` family, so the worker never picks them up.
        for c in JOB_CLASSES { assert_eq!(c.kind.starts_with("app."), c.runner == Runner::App, "{}", c.kind); }
    }

    #[test]
    fn exact_entries_win_and_unknown_kinds_are_heavy() {
        assert_eq!(job_cost("workflow.speakers"), Cost::Heavy);
        assert_eq!(job_cost("workflow.title"), Cost::ByModel);
        assert_eq!(job_cost("nothing.registered"), Cost::Heavy);
        assert!(!is_background_kind("app.ocr") && !is_background_kind("agent.skill") && is_background_kind("people.judge"));
    }
}
