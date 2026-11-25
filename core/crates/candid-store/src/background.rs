//! What wakes background work, and when it is next due.
//!
//! Every background step (a scan that queues jobs, the job queue itself, cleanup) is a [`Trigger`]: the tables, or
//! keys of `store_meta` / `background_revisions`, that it reads and others write. The connection counts writes in
//! memory ([`crate::db::WriteCounters`]), so checking "did anything I read change" costs no SQL. A step that depends
//! on the clock (a chat counts as finished after ten quiet minutes, a daily skill, a retry delay) says when it is next
//! due instead of being polled. The background thread sleeps until something is written or the earliest due time.
//!
//! Adding a scan: add one [`Scan`] to [`SCANS`] with what it reads, and have it put `next_due_ms` in its result when
//! time alone can make it do something. Never add a timer or a polling loop (the idle test and
//! `script/check_no_polling.py` fail on both).
use crate::{db::{int, text, RowExt}, Result, TimelineStore};
use serde_json::{json, Value};
use std::collections::HashMap;

/// A background step and what it reads. Each entry of `reads` is a table name (`events`) or a key prefix of a keyed
/// table (`store_meta:features.`, `background_revisions:jobs`).
#[derive(Clone, Copy, Debug)]
pub struct Trigger {
    pub name: &'static str,
    pub reads: &'static [&'static str],
}

/// Model, feature, source and voice settings and media evidence: the `workflows` revision, maintained by triggers.
const SETTINGS: &str = "background_revisions:workflows";

/// A store scan run by the background worker: queues jobs or keeps a derived layer current.
pub struct Scan {
    pub trigger: Trigger,
    /// Returns `changed: true` when it queued something, and `next_due_ms` when the clock alone can give it work.
    pub run: fn(&TimelineStore) -> Result<Value>,
}

/// Background scans in pipeline order (each feeds the ones after it).
pub const SCANS: &[Scan] = &[
    Scan {
        trigger: Trigger { name: "workflow_scan", reads: &[SETTINGS] },
        run: |s| s.workflow_scan().map(|_| json!({})),
    },
    Scan {
        trigger: Trigger { name: "curated_sync", reads: &["events", "event_text", "sources", "background_revisions:media_projection"] },
        run: TimelineStore::curated_sync,
    },
    Scan {
        trigger: Trigger { name: "people_sync", reads: &["messages", "conversations", "corrections", "entities", "entity_aliases", "store_meta:people."] },
        run: TimelineStore::people_sync,
    },
    Scan {
        trigger: Trigger { name: "people_judge_scan", reads: &[SETTINGS, "store_meta:people.", "jobs"] },
        run: TimelineStore::people_judge_scan,
    },
    Scan {
        trigger: Trigger { name: "excerpt_scan", reads: &[SETTINGS, "events", "sources", "excerpt_sources", "store_meta:excerpt."] },
        run: TimelineStore::excerpt_scan,
    },
    Scan {
        trigger: Trigger { name: "items_scan", reads: &[SETTINGS, "messages", "conversations", "message_sources", "items", "jobs", "store_meta:items."] },
        run: TimelineStore::items_scan,
    },
    Scan {
        trigger: Trigger { name: "screen_scan", reads: &[SETTINGS, "events", "event_text", "sources", "messages", "excerpts", "corrections", "episodes"] },
        run: TimelineStore::screen_scan,
    },
    Scan {
        trigger: Trigger { name: "automation_scan", reads: &[SETTINGS, "jobs", "events", "episodes", "media_objects", "media_parts", "store_meta:automation."] },
        run: |s| {
            s.automation_scan()?;
            Ok(json!({"next_due_ms": s.automation_next_due()?}))
        },
    },
    Scan {
        trigger: Trigger { name: "expire_stale_jobs", reads: &["jobs"] },
        run: |s| {
            let n = s.expire_stale_jobs()?;
            Ok(json!({"changed": n > 0, "next_due_ms": s.next_job_expiry()?}))
        },
    },
];

/// Remembers, per step, the write revision it last ran at and when the clock next gives it work.
#[derive(Default, Debug)]
pub struct Gate {
    seen: HashMap<&'static str, (u64, Option<i64>)>,
}

impl Gate {
    /// The step's current revision: its reads plus `extra` (a counter kept outside the database, such as the model
    /// authorization). Every part only grows, so the sum moves exactly when one of them does.
    pub fn stamp(store: &TimelineStore, trigger: &Trigger, extra: u64) -> u64 {
        store.write_revision(trigger.reads).wrapping_add(extra)
    }

    /// True when the step never ran, something it reads was written since, or its due time has come.
    pub fn is_due(&self, trigger: &Trigger, stamp: u64, now_ms: i64) -> bool {
        match self.seen.get(trigger.name) {
            None => true,
            Some((seen, due)) => *seen != stamp || due.is_some_and(|d| d <= now_ms),
        }
    }

    /// Records a run. Pass the stamp taken *before* running, so the step's own writes make it run once more and
    /// settle: a step that finds nothing to do must write nothing.
    pub fn ran(&mut self, trigger: &Trigger, stamp: u64, next_due_ms: Option<i64>) {
        self.seen.insert(trigger.name, (stamp, next_due_ms));
    }

    /// The stamp and due time recorded at the step's last run.
    pub fn last(&self, trigger: &Trigger) -> Option<(u64, Option<i64>)> {
        self.seen.get(trigger.name).copied()
    }

    /// The earliest time any step is due by the clock alone.
    pub fn next_due(&self) -> Option<i64> {
        self.seen.values().filter_map(|(_, d)| *d).min()
    }

    /// Runs the scans that are due, in order; returns whether any queued something.
    pub fn run_scans(&mut self, store: &TimelineStore) -> bool {
        let mut changed = false;
        for scan in SCANS {
            let stamp = Self::stamp(store, &scan.trigger, 0);
            let now = store.clock.now_ms();
            if !self.is_due(&scan.trigger, stamp, now) {
                continue;
            }
            let due = match (scan.run)(store) {
                Ok(v) => {
                    changed |= v["changed"] == true;
                    v["next_due_ms"].as_i64()
                }
                // A failing scan is tried again when what it reads changes, and at the latest after the fallback.
                Err(e) => {
                    eprintln!("后台：{} 失败 {}", scan.trigger.name, e.code());
                    Some(now + RETRY_FAILED_MS)
                }
            };
            self.ran(&scan.trigger, stamp, due);
        }
        changed
    }

    /// Whether any scan is due right now (something it reads was written, or its time came).
    pub fn scans_due(&self, store: &TimelineStore) -> bool {
        let now = store.clock.now_ms();
        SCANS.iter().any(|s| self.is_due(&s.trigger, Self::stamp(store, &s.trigger, 0), now))
    }
}

/// How long a scan that failed waits before it is tried again with nothing changed.
pub const RETRY_FAILED_MS: i64 = 10 * 60_000;

impl TimelineStore {
    /// See [`crate::db::Db::write_revision`].
    pub fn write_revision(&self, reads: &[&str]) -> u64 {
        self.db.write_revision(reads)
    }

    /// Calls `f(table)` after every write to the library (see [`crate::db::WriteCounters::set_listener`]).
    pub fn on_write(&self, f: impl Fn(&str) + Send + Sync + 'static) -> bool {
        self.db.writes.set_listener(f)
    }

    /// Rows written through this store's connection since it opened (the idle test asserts it does not move).
    pub fn write_count(&self) -> u64 {
        self.db.writes.total()
    }

    /// SQL statements run since the store opened, reads included (the idle test asserts it does not move).
    pub fn statement_count(&self) -> u64 {
        self.db.writes.statements()
    }

    /// Per-table write counts since the store opened, for tests and diagnostics.
    pub fn write_counts(&self) -> std::collections::BTreeMap<String, u64> {
        self.db.write_revision(&[]);
        self.db.writes.snapshot()
    }

    /// When the next queued job passes its kind's useful age ([`TimelineStore::expire_stale_jobs`]).
    pub fn next_job_expiry(&self) -> Result<Option<i64>> {
        let mut next: Option<i64> = None;
        for c in crate::job_priority::JOB_CLASSES {
            let Some(age) = c.max_age_ms else { continue };
            let pattern = if c.kind.ends_with('.') { format!("{}*", c.kind) } else { c.kind.to_string() };
            let oldest = self.db.first("SELECT MIN(created_ms) FROM jobs WHERE state='queued' AND kind GLOB ?", &[text(pattern)], |r| r.opt_int(0))?.flatten();
            if let Some(t) = oldest {
                next = Some(next.map_or(t + age, |n| n.min(t + age)));
            }
        }
        Ok(next)
    }

    /// The start of the next local day: daily limits and model budgets reset then.
    pub fn next_local_midnight(&self) -> i64 {
        let zone = &self.clock.zone;
        let today = zone.local(self.clock.now_ms()).date();
        zone.day_start(today.succ_opt().unwrap_or(today))
    }

    /// When time alone can make [`TimelineStore::automation_scan`] act: a daily skill's next run, a draft activity
    /// block turning ten minutes old, a recording's end time, or an agent run's lease running out.
    pub fn automation_next_due(&self) -> Result<Option<i64>> {
        let now = self.clock.now_ms();
        let mut next: Option<i64> = None;
        let mut take = |t: Option<i64>| {
            if let Some(t) = t.filter(|t| *t > now) {
                next = Some(next.map_or(t, |n: i64| n.min(t)));
            }
        };
        take(self.db.first("SELECT MIN(lease_until_ms) FROM jobs INDEXED BY jobs_pending WHERE kind='agent.skill' AND state IN ('queued','running') AND state='running'", &[], |r| r.opt_int(0))?.flatten());
        let c = self.automation_config()?;
        let skills: Vec<_> = c.skills.values().filter(|p| p.enabled && p.automatic).collect();
        for p in &skills {
            if p.trigger != "daily" {
                continue;
            }
            let (Some(zone), Ok(at)) = (crate::Zone::named(&p.timezone), chrono::NaiveTime::parse_from_str(&p.time, "%H:%M")) else { continue };
            let today = zone.local(now).date();
            let mut due = zone.instant(today.and_time(at));
            if due <= now {
                due = zone.instant(today.succ_opt().unwrap_or(today).and_time(at));
            }
            take(Some(due));
        }
        if skills.iter().any(|p| p.trigger == "episode_ended") {
            take(self.db.first("SELECT MIN(end_ms) FROM episodes WHERE status='draft' AND end_ms>?", &[int(now - 600_000)], |r| r.opt_int(0))?.flatten().map(|t| t + 600_000));
        }
        if skills.iter().any(|p| p.trigger == "recording_sealed") {
            take(self.db.first("SELECT MIN(end_ms) FROM events WHERE kind='audio.recording' AND end_ms>?", &[int(now)], |r| r.opt_int(0))?.flatten());
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn every_declared_read_names_a_real_table() {
        let store = TimelineStore::open(&fixtures::temp_root("bg-reads"), "candid/test", true, fixtures::test_clock()).unwrap();
        let tables: Vec<String> = store.db.query("SELECT name FROM sqlite_master WHERE type='table'", &[], |r| r.text(0)).unwrap();
        for scan in SCANS {
            for read in scan.trigger.reads {
                let table = read.split(':').next().unwrap();
                assert!(tables.iter().any(|t| t == table), "{} reads unknown table {table}", scan.trigger.name);
            }
        }
    }

    #[test]
    fn write_revisions_move_only_for_what_was_written() {
        let store = TimelineStore::open(&fixtures::temp_root("bg-revisions"), "candid/test", true, fixtures::test_clock()).unwrap();
        let settings = ["store_meta:features."];
        let (meta, jobs, all) = (store.write_revision(&settings), store.write_revision(&["jobs"]), store.write_count());
        store.db.execute("INSERT OR REPLACE INTO store_meta VALUES('people.synthetic/1','x')", &[]).unwrap();
        assert_eq!(store.write_revision(&settings), meta, "another key family does not move features");
        assert!(store.write_revision(&["store_meta:people."]) > 0);
        store.db.execute("INSERT OR REPLACE INTO store_meta VALUES('features.synthetic/1','x')", &[]).unwrap();
        assert!(store.write_revision(&settings) > meta);
        assert_eq!(store.write_revision(&["jobs"]), jobs);
        // A deleted key cannot be read back: every prefix sees it.
        let people = store.write_revision(&["store_meta:people."]);
        store.db.execute("DELETE FROM store_meta WHERE key='features.synthetic/1'", &[]).unwrap();
        assert!(store.write_revision(&["store_meta:people."]) > people);
        // An update that changes no row is not a write.
        let before = store.write_count();
        store.db.execute("UPDATE store_meta SET value='x' WHERE key='no such key'", &[]).unwrap();
        assert_eq!(store.write_count(), before);
        assert!(store.write_count() > all);
        // Trigger-maintained revisions count per domain.
        let w = store.write_revision(&[SETTINGS]);
        store.db.execute("INSERT OR REPLACE INTO store_meta VALUES('models.synthetic/1','x')", &[]).unwrap();
        assert!(store.write_revision(&[SETTINGS]) > w);
    }

    #[test]
    fn scans_and_deadlines_run_on_an_empty_library() {
        let store = TimelineStore::open(&fixtures::temp_root("bg-empty"), "candid/test", true, fixtures::test_clock()).unwrap();
        for scan in SCANS {
            (scan.run)(&store).unwrap_or_else(|e| panic!("{}: {}", scan.trigger.name, e.message()));
        }
        assert_eq!(store.automation_next_due().unwrap(), None);
        assert_eq!(store.next_job_expiry().unwrap(), None);
        assert!(store.next_local_midnight() > store.clock.now_ms());
    }

    #[test]
    fn gate_runs_a_step_once_per_change_and_at_its_due_time() {
        let store = TimelineStore::open(&fixtures::temp_root("bg-gate"), "candid/test", true, fixtures::test_clock()).unwrap();
        let t = Trigger { name: "synthetic", reads: &["store_meta:synthetic."] };
        let mut gate = Gate::default();
        let now = store.clock.now_ms();
        let s = Gate::stamp(&store, &t, 0);
        assert!(gate.is_due(&t, s, now), "never ran");
        gate.ran(&t, s, Some(now + 1000));
        assert!(!gate.is_due(&t, s, now));
        assert!(gate.is_due(&t, s, now + 1000), "due by the clock");
        assert_eq!(gate.next_due(), Some(now + 1000));
        assert!(gate.is_due(&t, Gate::stamp(&store, &t, 1), now), "an outside counter moved");
        store.db.execute("INSERT INTO store_meta VALUES('synthetic.key/1','x')", &[]).unwrap();
        assert!(gate.is_due(&t, Gate::stamp(&store, &t, 0), now), "something it reads was written");
    }
}
