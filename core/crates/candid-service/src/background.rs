//! The background thread (`candid-workflows`): asleep until something happens, never on a fixed beat.
//!
//! It wakes when the library is written (every write, whichever path made it, through the store's write listener),
//! when [`Wake::wake`] is called for something outside the library (model authorization, power), or when the earliest
//! clock deadline of a step comes (a retry delay, a lease, a chat that counts as finished after ten quiet minutes, the
//! next local day for daily limits). A 10-minute fallback covers anything missed. Each wake runs only the steps whose
//! inputs changed ([`candid_store::background::Gate`]); with nothing changed a wake costs no SQL.
//!
//! New background work never brings its own timer: it becomes a step with the tables it reads and, when time matters,
//! a due time. `script/check_no_polling.py` and the idle test in `tests/idle.rs` hold the line.
use crate::router::{lock, SharedStore};
use candid_store::background::{Gate, Trigger};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Longest sleep with nothing due: the safety net, not the schedule.
pub const FALLBACK: Duration = Duration::from_secs(10 * 60);
/// Passes in a row before the thread sleeps anyway (each pass runs only what changed, so this is never reached when
/// steps settle; it bounds a step that keeps writing).
const MAX_PASSES: usize = 64;

/// Wakes the background thread. Cheap to call from any thread, any number of times.
#[derive(Default)]
pub struct Wake {
    pending: Mutex<bool>,
    signal: Condvar,
    /// Moves when something outside the library changed (authorization, power, an explicit request): the job queue
    /// is looked at again even though no table changed.
    outside: AtomicU64,
    wakes: AtomicU64,
}

impl Wake {
    /// Something changed: run whatever is due.
    pub fn wake(&self) {
        *self.pending.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.signal.notify_all();
    }

    /// Something outside the library changed (power state, authorization): check waiting jobs again, then run.
    pub fn invalidate(&self) {
        self.outside.fetch_add(1, Ordering::SeqCst);
        self.wake();
    }

    pub fn outside_revision(&self) -> u64 {
        self.outside.load(Ordering::SeqCst)
    }

    /// Times the thread came out of a sleep (for the idle test and diagnostics).
    pub fn wake_count(&self) -> u64 {
        self.wakes.load(Ordering::Relaxed)
    }

    /// Forgets wake requests made so far: the pass about to run sees their changes anyway.
    fn take(&self) {
        *self.pending.lock().unwrap_or_else(|p| p.into_inner()) = false;
    }

    /// Sleeps until woken, the timeout passes, or `stop` is set.
    pub fn wait(&self, timeout: Duration, stop: &AtomicBool) {
        let deadline = std::time::Instant::now() + timeout;
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        while !*pending && !stop.load(Ordering::SeqCst) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            pending = self.signal.wait_timeout(pending, left).unwrap_or_else(|p| p.into_inner()).0;
        }
        *pending = false;
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }
}

/// Model, feature, source and voice settings, and media evidence (the trigger-maintained `workflows` revision).
const SETTINGS: &str = "background_revisions:workflows";
/// The job queue: job rows, settings that decide whether a job may run, and the power state.
pub const JOB_QUEUE: Trigger = Trigger { name: "job_queue", reads: &["jobs", "job_checkpoints", SETTINGS, "sources", "store_meta:power."] };
/// Removing speaker vectors of deleted recordings from the voice service.
pub const VOICE_CLEANUP: Trigger = Trigger { name: "voice_cleanup", reads: &["store_meta:voice"] };
/// Re-encoding recording parts once imports and jobs have been quiet for the grace period.
pub const AUDIO_COMPACTION: Trigger = Trigger { name: "audio_compaction", reads: &["media_parts", "media_objects", "jobs"] };

/// Everything the background thread runs, and when each part is next due.
#[derive(Default)]
pub struct Background {
    gate: Gate,
    json: crate::json_compaction::Converter,
    compactor: crate::audio_compaction::Compactor,
}

impl Background {
    /// Runs what is due; returns true when more is due right away (a job ran, a step's own writes need one more
    /// look, the one-time conversion has more). `outside` is [`Wake::outside_revision`].
    pub fn pass(&mut self, store: &SharedStore, outside: u64, on_start: &dyn Fn()) -> bool {
        let Some(_lane) = crate::workflows::Execution::acquire(store, crate::workflows::Lane::Background) else {
            return false;
        };
        let mut again = false;
        if !self.json.finished() {
            self.json.tick(store);
            again |= !self.json.finished();
        }
        self.voice_cleanup(store);
        {
            let s = lock(store);
            if self.gate.run_scans(&s) {
                on_start();
            }
        }
        let queue = {
            let s = lock(store);
            let stamp = Gate::stamp(&s, &JOB_QUEUE, outside.wrapping_add(crate::workflows::authorization_revision()));
            self.gate.is_due(&JOB_QUEUE, stamp, s.clock.now_ms()).then_some(stamp)
        };
        if let Some(stamp) = queue {
            let outcome = crate::workflows::run_queue(store, on_start);
            let s = lock(store);
            let mut due = s.next_workflow_deadline().ok().flatten();
            if outcome.budget_blocked {
                let midnight = s.next_local_midnight();
                due = Some(due.map_or(midnight, |d| d.min(midnight)));
            }
            self.gate.ran(&JOB_QUEUE, stamp, due);
            again |= outcome.ran;
        }
        self.audio_compaction(store);
        again || self.due_now(store, outside)
    }

    /// The earliest clock deadline of any step, in the store clock's milliseconds.
    pub fn next_due(&self) -> Option<i64> {
        self.gate.next_due()
    }

    /// Whether some step is due right now (something it reads was written since it last ran, or its time came).
    pub fn due_now(&self, store: &SharedStore, outside: u64) -> bool {
        let s = lock(store);
        let now = s.clock.now_ms();
        self.gate.scans_due(&s)
            || self.gate.is_due(&JOB_QUEUE, Gate::stamp(&s, &JOB_QUEUE, outside.wrapping_add(crate::workflows::authorization_revision())), now)
            || self.gate.is_due(&VOICE_CLEANUP, Gate::stamp(&s, &VOICE_CLEANUP, 0), now)
    }

    /// Runs when voice settings or the forget list change; while something could not be removed (voice service
    /// offline), tries again after the fallback interval.
    fn voice_cleanup(&mut self, store: &SharedStore) {
        let stamp = {
            let s = lock(store);
            let stamp = Gate::stamp(&s, &VOICE_CLEANUP, 0);
            if !self.gate.is_due(&VOICE_CLEANUP, stamp, s.clock.now_ms()) {
                return;
            }
            stamp
        };
        crate::voiceprints::reconcile(store);
        let s = lock(store);
        let left = s.voice_refs_to_forget(None).is_ok_and(|v| !v.is_empty()) || s.voiceprint_cleanup_candidates().is_ok_and(|v| !v.is_empty());
        let retry = left.then(|| s.clock.now_ms() + FALLBACK.as_millis() as i64);
        self.gate.ran(&VOICE_CLEANUP, stamp, retry);
    }

    /// Recording parts are re-encoded after imports and jobs have been quiet for [`crate::audio_compaction::GRACE`]:
    /// every change restarts the quiet period, and the step runs once it ends (and once at start).
    fn audio_compaction(&mut self, store: &SharedStore) {
        let (stamp, now) = {
            let s = lock(store);
            (Gate::stamp(&s, &AUDIO_COMPACTION, 0), s.clock.now_ms())
        };
        let grace = crate::audio_compaction::GRACE.as_millis() as i64;
        match self.gate.last(&AUDIO_COMPACTION) {
            Some((seen, _)) if seen != stamp => self.gate.ran(&AUDIO_COMPACTION, stamp, Some(now + grace)),
            Some((_, Some(due))) if due <= now => {
                self.compactor.tick(store);
                let after = Gate::stamp(&lock(store), &AUDIO_COMPACTION, 0);
                self.gate.ran(&AUDIO_COMPACTION, after, None);
            }
            Some(_) => {}
            None => {
                self.compactor.tick(store);
                let after = Gate::stamp(&lock(store), &AUDIO_COMPACTION, 0);
                self.gate.ran(&AUDIO_COMPACTION, after, None);
            }
        }
    }
}

/// How the loop sleeps. The service waits on [`Wake`]; the idle test moves a virtual clock instead.
pub trait Sleep {
    /// Sleeps at most `timeout`; returns false when the loop should end.
    fn sleep(&mut self, timeout: Duration) -> bool;
}

/// The background loop: passes until nothing is due, then sleeps until woken, the next deadline, or the fallback.
/// `announce` tells the App that jobs moved (it must not wake this loop).
pub fn run(background: &mut Background, store: &SharedStore, wake: &Wake, sleeper: &mut dyn Sleep, announce: &dyn Fn()) {
    loop {
        let jobs_before = lock(store).write_revision(&["jobs"]);
        let mut passes = 0;
        loop {
            wake.take();
            passes += 1;
            if !background.pass(store, wake.outside_revision(), announce) || passes >= MAX_PASSES {
                break;
            }
        }
        if passes >= MAX_PASSES {
            eprintln!("后台：连续 {MAX_PASSES} 轮仍有待办，先休息");
        }
        if lock(store).write_revision(&["jobs"]) != jobs_before {
            announce();
        }
        let timeout = sleep_for(background, store, passes >= MAX_PASSES);
        if !sleeper.sleep(timeout) {
            return;
        }
    }
}

/// Until the earliest deadline, at most [`FALLBACK`]; at least a second after a run of passes that did not settle.
fn sleep_for(background: &Background, store: &SharedStore, unsettled: bool) -> Duration {
    let now = lock(store).clock.now_ms();
    let fallback = FALLBACK.as_millis() as i64;
    let mut ms = background.next_due().map_or(fallback, |d| (d - now).clamp(0, fallback));
    if unsettled {
        ms = ms.max(1000);
    }
    Duration::from_millis(ms as u64)
}

/// The service's sleep: on the wake signal, until stopped.
pub struct WakeSleep<'a> {
    pub wake: &'a Wake,
    pub stop: &'a AtomicBool,
}

impl Sleep for WakeSleep<'_> {
    fn sleep(&mut self, timeout: Duration) -> bool {
        if self.stop.load(Ordering::SeqCst) {
            return false;
        }
        self.wake.wait(timeout, self.stop);
        !self.stop.load(Ordering::SeqCst)
    }
}
