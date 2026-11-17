//! §4.11 deletion: tombstone first (hidden at once), cancel jobs, remove derived content and index rows, then
//! unreferenced chunks and blobs. Descendants are deleted with their parent.

use crate::db::{int, opt_text, placeholders, text, RowExt, P};
use crate::ingest::tombstone_key;
use crate::store::TimelineStore;
use crate::{Result, TimelineError};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::PathBuf;

const REASONS: &[&str] = &["user", "retention", "source_revoked", "import_rollback", "superseded", "source_deleted"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletionReport {
    pub events: usize,
    pub threads: usize,
    pub episodes: usize,
    pub memories: usize,
    /// Hour/day/week digests whose period covered a deleted event; regenerated on their next run.
    pub digests: usize,
    /// Entity mentions that pointed at a deleted event or episode (the entities themselves stay).
    pub mentions: usize,
    pub chunks: usize,
    pub blobs: usize,
    pub jobs_cancelled: usize,
}

fn chunks_of(ids: &[String]) -> impl Iterator<Item = &[String]> {
    ids.chunks(400)
}

fn params(ids: &[String]) -> Vec<P> {
    ids.iter().map(text).collect()
}

impl TimelineStore {
    pub fn delete_events(&self, ids: &[String], reason: &str) -> Result<DeletionReport> {
        if !REASONS.contains(&reason) {
            return Err(TimelineError::InvalidInput(format!("reason {reason}")));
        }
        let mut files = Vec::new();
        let report = self.db.transaction(|| self.delete_in_transaction(ids, reason, &mut files))?;
        for f in files {
            let _ = fs::remove_file(f);
        }
        Ok(report)
    }

    /// Deletes everything one source contributed, e.g. when access is revoked or a migration is rolled back.
    pub fn delete_source_data(&self, source_id: &str, reason: &str) -> Result<DeletionReport> {
        if !REASONS.contains(&reason) {
            return Err(TimelineError::InvalidInput(format!("reason {reason}")));
        }
        self.agent_file_stamps.borrow_mut().clear();
        let mut files = Vec::new();
        let report = self.db.transaction(|| {
            let ids = self.db.query("SELECT event_id FROM events WHERE source_id=?", &[text(source_id)], |r| r.text(0))?;
            let mut r = self.delete_in_transaction(&ids, reason, &mut files)?;
            self.db.execute("DELETE FROM source_cursors WHERE source_id=?", &[text(source_id)])?;
            self.db.execute("DELETE FROM agent_session_entries WHERE source_id=?", &[text(source_id)])?;
            self.db.execute("DELETE FROM agent_file_counts WHERE source_id=?", &[text(source_id)])?;
            let threads = self.db.query("SELECT thread_id FROM threads WHERE source_id=?", &[text(source_id)], |r| r.text(0))?;
            for t in threads {
                self.tombstone("thread", &t, reason)?;
                self.record_change("thread", &t, "delete")?;
                r.threads += 1;
            }
            self.db.execute("DELETE FROM threads WHERE source_id=?", &[text(source_id)])?;
            Ok(r)
        })?;
        for f in files {
            let _ = fs::remove_file(f);
        }
        Ok(report)
    }

    pub fn is_tombstoned(&self, entity: &str, id: &str) -> Result<bool> {
        Ok(self.db.scalar("SELECT COUNT(*) FROM tombstones WHERE entity=? AND entity_id=?", &[text(entity), text(id)])? > 0)
    }

    fn tombstone(&self, entity: &str, id: &str, reason: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO tombstones(entity,entity_id,deleted_ms,reason) VALUES(?,?,?,?)",
            &[text(entity), text(id), int(self.clock.now_ms()), text(reason)],
        )
    }

    pub(crate) fn delete_in_transaction(&self, requested: &[String], reason: &str, files: &mut Vec<PathBuf>) -> Result<DeletionReport> {
        let mut report = DeletionReport::default();
        // expand to descendants
        let mut all: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut frontier: Vec<String> = requested.to_vec();
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for chunk in chunks_of(&frontier) {
                let ph = placeholders(chunk.len());
                let found = self.db.query(&format!("SELECT event_id FROM events WHERE event_id IN ({ph})"), &params(chunk), |r| r.text(0))?;
                let kids = self.db.query(&format!("SELECT event_id FROM events WHERE parent_event_id IN ({ph})"), &params(chunk), |r| r.text(0))?;
                for id in found.into_iter().chain(kids) {
                    if seen.insert(id.clone()) {
                        all.push(id.clone());
                        next.push(id);
                    }
                }
            }
            frontier = next;
        }
        if all.is_empty() {
            return Ok(report);
        }
        let mut chunk_ids: BTreeSet<i64> = BTreeSet::new();
        let mut shas: BTreeSet<String> = BTreeSet::new();
        let mut thread_ids: BTreeSet<String> = BTreeSet::new();
        for chunk in chunks_of(&all) {
            let ph = placeholders(chunk.len());
            let p = params(chunk);
            // 1. tombstones first (events and their source keys)
            let rows = self.db.query(
                &format!("SELECT event_id,source_id,source_key,thread_id,rowid,title,app_name,url FROM events WHERE event_id IN ({ph})"),
                &p,
                |r| (r.text(0), r.text(1), r.text(2), r.opt_text(3), r.int(4), r.opt_text(5), r.opt_text(6), r.opt_text(7)),
            )?;
            for r in &rows {
                self.tombstone("event", &r.0, reason)?;
                self.tombstone("event_key", &tombstone_key(&r.1, &r.2), reason)?;
                if let Some(t) = &r.3 {
                    thread_ids.insert(t.clone());
                }
            }
            // Samples a text event absorbed (merge.rs): a late resend must not bring them back either.
            for (source, key) in self.db.query(
                &format!("SELECT e.source_id, k.value FROM events e, json_each(e.payload_json,'$.merge.keys') k WHERE e.event_id IN ({ph})"),
                &p,
                |r| (r.text(0), r.text(1)),
            )? {
                self.tombstone("event_key", &tombstone_key(&source, &key), reason)?;
            }
            // 2. cancel jobs that reference these events
            let mut job_params = vec![int(self.clock.now_ms())];
            job_params.extend(p.iter().cloned());
            self.db.execute(
                &format!("UPDATE jobs SET state='cancelled', updated_ms=? WHERE state IN ('queued','running') AND input_ref IN ({ph})"),
                &job_params,
            )?;
            report.jobs_cancelled += self.db.changes() as usize;
            // 3. derived content and index entries
            let episodes = self.db.query(&format!("SELECT DISTINCT episode_id FROM episode_evidence WHERE event_id IN ({ph})"), &p, |r| r.text(0))?;
            for ep in episodes {
                self.tombstone("episode", &ep, reason)?;
                self.record_change("episode", &ep, "delete")?;
                self.db.execute("DELETE FROM episodes WHERE episode_id=?", &[text(&ep)])?;
                self.db.execute("DELETE FROM entity_mentions WHERE ref_type='episode' AND ref_id=?", &[text(&ep)])?;
                report.mentions += self.db.changes() as usize;
                report.episodes += 1;
            }
            let memories = self.db.query(
                &format!("SELECT DISTINCT memory_id FROM memory_sources WHERE ref_type='event' AND ref_id IN ({ph})"),
                &p,
                |r| r.text(0),
            )?;
            for m in memories {
                self.tombstone("memory", &m, reason)?;
                self.record_change("memory", &m, "delete")?;
                self.db.execute("DELETE FROM memories WHERE memory_id=?", &[text(&m)])?;
                report.memories += 1;
            }
            self.db.execute(&format!("DELETE FROM entity_mentions WHERE ref_type='event' AND ref_id IN ({ph})"), &p)?;
            report.mentions += self.db.changes() as usize;
            let digests = self.db.query(
                &format!("SELECT DISTINCT d.digest_id FROM digests d JOIN events e ON d.start_ms <= COALESCE(e.end_ms, e.start_ms) AND d.end_ms > e.start_ms WHERE e.event_id IN ({ph})"),
                &p,
                |r| r.text(0),
            )?;
            for d in digests {
                self.tombstone("digest", &d, reason)?;
                self.record_change("digest", &d, "delete")?;
                self.db.execute("DELETE FROM digests WHERE digest_id=?", &[text(&d)])?;
                report.digests += 1;
            }
            for r in &rows {
                self.db.execute(
                    "INSERT INTO meta_fts(meta_fts,rowid,title,app_name,url) VALUES('delete',?,?,?,?)",
                    &[int(r.4), opt_text(r.5.clone()), opt_text(r.6.clone()), opt_text(r.7.clone())],
                )?;
            }
            chunk_ids.extend(self.db.query(&format!("SELECT DISTINCT chunk_id FROM event_text WHERE event_id IN ({ph})"), &p, |r| r.int(0))?);
            shas.extend(self.db.query(&format!("SELECT DISTINCT sha256 FROM event_blobs WHERE event_id IN ({ph})"), &p, |r| r.text(0))?);
        }
        // children before parents so the parent FK never dangles mid-statement
        self.db.execute("PRAGMA defer_foreign_keys=ON", &[])?;
        for chunk in chunks_of(&all) {
            self.db.execute(&format!("DELETE FROM events WHERE event_id IN ({})", placeholders(chunk.len())), &params(chunk))?;
        }
        report.events = all.len();
        for id in &all {
            self.record_change("event", id, "delete")?;
        }
        // 4. unreferenced chunks, blobs, empty threads
        for id in chunk_ids {
            if self.db.scalar("SELECT COUNT(*) FROM event_text WHERE chunk_id=?", &[int(id)])? == 0 {
                if let Some(t) = self.db.first("SELECT text FROM text_chunks WHERE chunk_id=?", &[int(id)], |r| r.text(0))? {
                    self.db.execute("INSERT INTO text_fts(text_fts,rowid,text) VALUES('delete',?,?)", &[int(id), text(t)])?;
                    self.db.execute("DELETE FROM text_chunks WHERE chunk_id=?", &[int(id)])?;
                    report.chunks += 1;
                }
            }
        }
        for sha in shas {
            if self.db.scalar("SELECT COUNT(*) FROM event_blobs WHERE sha256=?", &[text(&sha)])? == 0 {
                if let Some(rel) = self.db.first("SELECT rel_path FROM blobs WHERE sha256=?", &[text(&sha)], |r| r.text(0))? {
                    self.db.execute("DELETE FROM blobs WHERE sha256=?", &[text(&sha)])?;
                    files.push(self.root.join(rel));
                    self.record_change("blob", &sha, "delete")?;
                    report.blobs += 1;
                }
            }
        }
        for t in thread_ids {
            if self.db.scalar("SELECT COUNT(*) FROM events WHERE thread_id=?", &[text(&t)])? == 0 {
                self.tombstone("thread", &t, reason)?;
                self.record_change("thread", &t, "delete")?;
                self.db.execute("DELETE FROM threads WHERE thread_id=?", &[text(&t)])?;
                report.threads += 1;
            }
        }
        Ok(report)
    }
}
