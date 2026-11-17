//! Bounded App read cache. Unrelated AI/mail changes do not rebuild the desktop projection.
//! Authorisation and auditing still run for every request; the cache never crosses a source-policy snapshot.
use crate::{db::{int, placeholders, text, RowExt}, AppActivitiesResponse, AppActivityResponse, AppStats, Result, TimelineStore};

#[derive(Default)]
pub(crate) struct DesktopReadCache {
    seq: Option<i64>,
    sources: Vec<(String, String, String, bool)>,
    corrections: (i64, i64),
    pub stats: Option<AppStats>,
    pub activities: Option<(String, AppActivitiesResponse)>,
    pub detail: Option<(String, AppActivityResponse)>,
}

impl TimelineStore {
    pub(crate) fn refresh_desktop_read_cache(&self, ids: &[String]) -> Result<()> {
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
        cache.seq = Some(seq);
        cache.sources = sources;
        cache.corrections = corrections;
        Ok(())
    }
}
