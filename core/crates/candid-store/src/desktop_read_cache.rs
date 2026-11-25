//! Bounded App read cache. Unrelated AI/mail changes do not rebuild the desktop projection.
//! Authorisation and auditing still run for every request; the cache never crosses a source-policy snapshot.
use crate::{db::{int, placeholders, text, RowExt}, AppActivitiesResponse, AppActivityResponse, AppStats, Result, TimelineStore};

/// Everything the cached desktop answers are built from. While none of it is written, the cache is current without
/// asking SQLite anything (see [`crate::db::Db::write_revision`]).
const DESKTOP_READS: &[&str] = &["events", "event_blobs", "blobs", "sources", "corrections", "changes", "store_meta:screenshot_budget."];

#[derive(Default)]
pub(crate) struct DesktopReadCache {
    /// Write revision of [`DESKTOP_READS`] when the cache was last checked.
    rev: Option<u64>,
    seq: Option<i64>,
    sources: Vec<(String, String, String, bool)>,
    corrections: (i64, i64),
    pub stats: Option<AppStats>,
    pub activities: Option<(String, AppActivitiesResponse)>,
    pub detail: Option<(String, AppActivityResponse)>,
}

impl TimelineStore {
    /// True when nothing the cache was built from has been written since it was last checked: cached answers stand.
    pub(crate) fn desktop_read_cache_current(&self) -> bool {
        self.desktop_read_cache.borrow().rev == Some(self.write_revision(DESKTOP_READS))
    }

    pub(crate) fn refresh_desktop_read_cache(&self, ids: &[String]) -> Result<()> {
        // Taken before reading, so a write that lands meanwhile is seen next time.
        let rev = self.write_revision(DESKTOP_READS);
        if self.desktop_read_cache.borrow().rev == Some(rev) {
            return Ok(());
        }
        // An annotation may reference another source; include every policy, while ignoring sync/error churn.
        let sources = self.db.query("SELECT source_id,kind,policy_json,status='revoked' FROM sources ORDER BY source_id", &[], |r| (r.text(0), r.text(1), r.text(2), r.int(3) != 0))?;
        // Corrections are immutable, append-only revisions. Count also covers source deletion/cleanup.
        let corrections = self.db.first("SELECT COUNT(*),COALESCE(MAX(rowid),0) FROM corrections", &[], |r| (r.int(0), r.int(1)))?.unwrap_or_default();
        // Ingest sequence advances only for inserts; edits and deletions advance the change feed.
        let seq = self.db.scalar("SELECT COALESCE(MAX(seq),0) FROM changes", &[])?;
        let mut cache = self.desktop_read_cache.borrow_mut();
        let changed = match cache.seq {
            None => true,
            Some(previous) if sources != cache.sources || corrections != cache.corrections || seq < previous => true,
            Some(previous) if seq > previous && !ids.is_empty() => {
                let mut params = vec![int(previous)];
                params.extend(ids.iter().map(text));
                // Deleted event rows no longer carry a source: invalidate conservatively for every deletion.
                self.db.scalar(&format!("SELECT EXISTS(SELECT 1 FROM changes c LEFT JOIN events e ON c.entity='event' AND e.event_id=c.entity_id WHERE c.seq>? AND (c.entity='blob' OR (c.entity='event' AND (c.op='delete' OR e.source_id IN ({})))))", placeholders(ids.len())), &params)? != 0
            }
            _ => false,
        };
        if changed { cache.stats = None; cache.activities = None; cache.detail = None; }
        cache.rev = Some(rev);
        cache.seq = Some(seq);
        cache.sources = sources;
        cache.corrections = corrections;
        Ok(())
    }
}
