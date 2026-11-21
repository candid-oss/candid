//! Which background jobs the worker runs first, and which ones stop mattering after a while.
//!
//! One small table drives the pending-job query, the worker's order and the stale-job cleanup, so a new job kind
//! (entry extraction, people judgement, activity summaries) is one line here.
//!
//! P0 is the interactive lane (`complete()`, compose, Pi): it never queues here. Background lanes are P1..P3;
//! within one priority the oldest job goes first.

/// A job kind (exact) or kind family (prefix ending in `.`), its priority, and how long a queued job stays useful.
pub struct JobClass {
    pub kind: &'static str,
    pub priority: u8,
    /// Queued jobs older than this are abandoned with `expired`.
    pub max_age_ms: Option<i64>,
}

const DAY: i64 = 86_400_000;

pub const JOB_CLASSES: &[JobClass] = &[
    // P1: feeds everything downstream (conversations, items), so it goes first.
    JobClass { kind: "screen.excerpt", priority: 1, max_age_ms: Some(DAY) },
    // Entry extraction is never dropped for age: over the daily limit it waits, and a later job covers more messages.
    JobClass { kind: "items.extract", priority: 1, max_age_ms: None },
    // P2: existing recording flows.
    JobClass { kind: "speech.recognize", priority: 2, max_age_ms: None },
    JobClass { kind: "workflow.", priority: 2, max_age_ms: None },
    // P3: can wait for a quiet moment or the next day.
    JobClass { kind: "screen.summarize", priority: 3, max_age_ms: None },
];

/// Priority used for a kind the table does not know. Such jobs are not picked up by the worker at all.
pub const DEFAULT_PRIORITY: u8 = 2;

fn class_of(kind: &str) -> Option<&'static JobClass> {
    JOB_CLASSES.iter().find(|c| if c.kind.ends_with('.') { kind.starts_with(c.kind) } else { kind == c.kind })
}

/// 1 (first) to 3 (last) for background kinds.
pub fn job_priority(kind: &str) -> u8 {
    class_of(kind).map_or(DEFAULT_PRIORITY, |c| c.priority)
}

pub fn is_background_kind(kind: &str) -> bool {
    class_of(kind).is_some()
}

/// SQL predicate on `kind` matching every background kind in the table.
pub(crate) fn kind_filter() -> String {
    let parts: Vec<String> = JOB_CLASSES
        .iter()
        .map(|c| if c.kind.ends_with('.') { format!("kind GLOB '{}*'", c.kind) } else { format!("kind='{}'", c.kind) })
        .collect();
    format!("({})", parts.join(" OR "))
}
