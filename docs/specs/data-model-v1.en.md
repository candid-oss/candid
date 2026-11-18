# Data model v1: layers, evidence tiers and storage budget

[中文](data-model-v1.md) · **English**

**Status:** design settled, validation pending (section 13). Once accepted, this document replaces sections 2.2 (data root layout), 4 (data structures) and 9 (migrating existing data) of [Timeline library v1](timeline-v1.en.md). Query tools, connectors and access rules stay in Timeline library v1; the data organization pipeline stays in [Agent platform v1](agent-platform-v1.en.md).

This document fixes only the **skeleton**: layers, identity, provenance, evidence tiers, how deletion and compaction propagate, the storage budget and how the model is extended. Dedup thresholds, automatic cleanup rules and encoder settings are policy: they can change at any time, and the schema only guarantees it can express them.

<a id="current"></a>

## 1. Where the space goes today

Measured on 2026-11-18 on a migrated library covering 94 days (2026-08-17 to 11-18), 6.8 GB in total:

| Item | Size | Problem |
|---|---|---|
| Screenshot originals (JPEG) | 14,610 files, 4.6 GB, 328 KB average, about 1900×1230 | About 70% of all space; about 1,240 per day on capture days |
| Screenshots in the old desktop ledger | 481 MB | Duplicate copies kept after the switch to direct writes |
| `store_meta` | 256 MB | Screen episode inputs (`screen.input`) and revisions (`screen.revision`), 1,015 each and equal in size; a key-value config table used as a document store |
| `episode_evidence` | 114 MB | Evidence selectors (`selector_json`) average 12.6 KB, up to 113 KB, because they copy the text |
| `source_cursors` | 112 MB | One AI session's sync cursor reaches 6.5 MB |
| Derived JSON originals | 19,739 files, 435 MB | Whole recognition results stored as files |
| Log-like tables | 157k access log rows, 152k change rows, 143k AI session entries | Append-only, no retention |
| Recordings (WAV) | 54 files, 140 MB | 2.6 MB per file on average, uncompressed |

What already works: paragraph dedup. 96,852 text references point at only 39,606 stored paragraphs, about 2.4× smaller.

Ten tables are empty or nearly empty: `memories`, `memory_sources`, `entity_mentions`, `digests`, `proposals`, `record_audit`, `record_revisions`, `record_selected`, `ingest_conflicts`, `entities` (2 rows).

> In short: the space problem is screenshot encoding and duplicate storage; the database problem is large content stored where it does not belong. Both are solved with structure and policy, without giving up traceability.

<a id="features"></a>

## 2. Initial features and the data they need

The first version does the first five well; the last two only get room reserved:

| Feature | Typical question | Data needed |
|---|---|---|
| Recall and search | "Did I talk about the budget with X last week?" | Records, paragraph full-text index, people |
| Context for agents | "What am I working on, what do I care about lately?" | Episode summaries, current state, a small curated profile |
| Commitments and to-dos | "What did I promise to whom?" | Commitments extracted from episodes, with sources and handling state |
| Daily review | "What did I do today?" | Episodes, time statistics |
| People and project pages | "Everything with X" | Entities, aliases, mentions |
| Profile and preferences (reserved) | "How do I like replies written?" | Long-term memory with a high evidence bar |
| Proactive suggestions (reserved) | "What should I be reminded of now?" | All of the above plus attention signals |

These features need largely the same data: records with provenance, episodes, entities and extractions. The schema is designed around those, not around individual features.

<a id="principles"></a>

## 3. Principles

1. **Records may be reduced, but every reduction leaves a trace.** Originals and records can be downgraded or deleted by rule, provided we record what was reduced, by which rule, when, and how much was saved. Derived content and memories that cite it stay valid; their source is marked "original compressed to text" or "original deleted".
2. **Dedup happens at three points**: at capture, nothing duplicate is written (unchanged frame, no new text); at ingest, content addressing merges identical bytes and identical paragraphs; during later cleanup, near-duplicates are merged (adjacent similar screenshots, near-duplicate paragraph families).
3. **Rules are data, not schema.** The schema provides the hooks (tiers, fidelity, reduction log, usage accounting); concrete policies (when to downgrade, what thresholds) are editable policy rows executed by the core's cleanup job.
4. **Your marks and corrections beat any automatic rule.** Content you pinned and evidence your corrections point at are never touched by automatic cleanup.
5. **Derived content can be deleted and recomputed; recomputation never overwrites your corrections.**
6. **Large content is stored once.** Text goes into the paragraph table, binaries into the blob store; everything else stores references and ranges, never copies.

<a id="layers"></a>

## 4. Layers

| Layer | Contents | Mutability |
|---|---|---|
| **Originals** | Bytes of screenshots, recordings, attachments, stored by content hash | Bytes never change; may be replaced by a smaller representation (section 6) |
| **Records** | Facts reported by sources: window switches, received mail, what you sent to an AI | Content never changes; time range can only be extended (when merging repeats); can be deleted by rule |
| **Derived** | OCR, transcripts, episodes, extractions, entity candidates, indexes | Can be deleted and recomputed as a whole; must state producer, model, prompt version, confidence |
| **Memory** | Consolidated people, facts, preferences, commitments | Revision history; each item links to evidence in lower layers |
| **Corrections** | Your renames, merges, marks, deletion decisions | Stored separately, still applied after recomputation |

> OCR and transcripts are derived, not records: a model produces them from originals and they can be recomputed with another model. The screen episode inputs in `store_meta` today sit in the wrong layer.

<a id="tiers"></a>

## 5. Evidence tiers

Every record and every original has a **retention tier** that decides the order of automatic cleanup:

| Tier | Meaning | Automatic cleanup |
|---|---|---|
| `pinned` | Marked by you, or directly referenced by one of your corrections | Never |
| `key` | Used as primary evidence by an episode or extraction | After `normal`; how far down is set by policy |
| `normal` | Ordinary records | Downgraded by policy |
| `ephemeral` | Repeated frames with nothing new, screenshots that are mostly interface chrome, merged-away near-duplicates | First to go |

- Tiers are computed by rules and you can override them; an override is a correction, so recomputation keeps it.
- Tiers rise when something starts referencing an item and can fall back when the reference goes away.
- **Memories do not have to map to specific text or screenshots.** A key fact is usually distilled from many episodes and processes; its evidence is derived content such as episodes and extractions, not a particular image. So memories do not pin originals, and compressing or deleting an original does not invalidate a memory. The evidence chain only needs to reach the layer the memory actually relies on.
- Tiers also apply to originals: when several records reference the same screenshot, the highest tier wins.

<a id="fidelity"></a>

## 6. Fidelity ladder and reduction log

Each type of original has a fidelity ladder; automatic cleanup only moves down it:

| Type | Ladder (high to low) |
|---|---|
| Screenshots | WebP at ingest (section 9) → thumbnail → text only (OCR kept) → record metadata only |
| Recordings | WAV for processing (deleted afterwards) → AAC (section 9) → transcript only → record metadata only |
| Attachments, documents | Original → extracted text only → metadata only |
| Derived JSON | Moved into tables and the file deleted; anything that must stay is stored compressed |

Each downgrade is written to the **reduction log** (`reductions`): target, old fidelity, new fidelity, rule id, time, bytes before and after. Reductions cannot be undone, so they must be **previewable**: how much space will be freed, which items are affected, which citations will become "text only".

<a id="dedup"></a>

## 7. Dedup

| When | How | Schema support |
|---|---|---|
| Capture | If the frame matches the previous one (perceptual hash distance within threshold), store no new image; extend the previous record's time range and count the repeat | Records have `end_ms` and `repeat_count`; extending is allowed, changing content is not |
| Ingest | Identical bytes stored once (content hash); identical paragraphs stored once (paragraph hash, implemented) | `blobs` keyed by hash; `text_chunks` unique on normalized hash |
| Ingest | Near-duplicate paragraphs grouped into families (SimHash, rules in Agent platform 2.2) | Paragraph family table `chunk_families` |
| Later cleanup | Adjacent similar screenshots merged into one record with a representative frame; merged-away items drop to `ephemeral` and are cleaned up by policy | Originals carry a perceptual hash `phash`; the reduction log records the merge |

Near-duplicate merging keeps the floor from Agent platform 2.2: changes in numbers, negations and code count as new content and are never merged away.

<a id="budget"></a>

## 8. Storage budget

- **You set the limit**: a total limit, optionally split by category (screenshots, recordings, text and database). The value is entirely yours; the UI shows guidance from measured compression, such as how many days of screenshots and hours of recordings a given limit holds at your recent capture rate.
- **Usage accounting**: a summary table by category × tier × day, read directly by the UI and the cleanup job instead of scanning the library.
- **When over the limit**: the cleanup job picks items to downgrade by tier (low to high), age (old to new) and fidelity (high to low) until usage is back under the limit; pinned items are never touched.
- **Preview before acting**: the first automatic cleanup, or any large one, produces a preview first; you can let later ones run automatically.
- **Policies are data**: retention days, thresholds and encoder settings are policy rows; changing them needs no schema change or migration.

<a id="storage"></a>

## 9. Space-saving storage

Engineering choices; all values are to be confirmed on real data:

1. **Screenshots are stored as WebP (quality about 60).** Measured on 2026-11-18 on 12 real screenshots (about 1920×1130):

   | Format | Average size | OCR character recall (mean / worst) |
   |---|---|---|
   | Current JPEG | 364 KB | baseline |
   | JPEG quality 40 | 185 KB | 94.5% / 77.0% |
   | **WebP quality 60** | **122 KB** | 92.0% / 82.4% |
   | HEIC quality 50 | 153 KB | 94.0% / 90.0% |
   | AVIF quality 50 | 124 KB | 92.0% / 85.1% |

   - The recall differences are within OCR's own variance, and the same text region cropped from each looks identical. The test recompressed existing JPEGs; encoding straight from the captured frame can only do better.
   - Why WebP: about a third of today's size, fast to encode, and displayed natively by Windows and browsers, so the future web UI and Windows app need no transcoding. HEIC needs an extension on Windows and is not supported by most browsers; AVIF is about the same size but slow to encode; JPEG is about 50% larger at the same clarity.
   - OCR runs on the raw frame before compression; the stored image is mainly for people to look at and for later re-recognition.
   - Secondary displays and unfocused windows can be stored at lower resolution.
2. **Screenshot dedup**: perceptual-hash dedup at capture, plus merging of adjacent near-duplicates later.
3. **Age-based screenshot downgrade (on by default)**: `ephemeral` removed after 7 days; `normal` to thumbnail after 30 days and text only after 90 days; `key` to thumbnail after 90 days. The days are policy and can be changed or turned off in Settings; when the storage limit is exceeded, the budget in section 8 takes precedence.
4. **Recordings**: WAV is kept only while processing, then deleted; the library stores a compressed format. Measured on a 30-second 48 kHz mono recording: WAV 2.9 MB, Opus 24 kbps 89 KB, AAC 32 kbps 128 KB, MP3 32 kbps 120 KB, all more than 20× smaller.

   - **AAC (`.m4a`), mono, 48 kbps, sample rate at least 16 kHz**: both macOS and Windows have built-in encoders, and it plays everywhere. Speech recognition models resample to 16 kHz mono anyway, so higher rates are wasted; 48 kbps leaves headroom over the tested 32 kbps and is about 1/15 of WAV.
   - Not MP3: weakest at low bitrates; at 32 kbps it cuts the band to about 8 kHz. Opus is smaller at the same clarity but needs an extra encoder library on macOS.
   - **Release gate**: for the same recording before and after compression, transcripts are essentially identical and voiceprint match scores drop by no more than a threshold. Voiceprint models are more sensitive to compression artifacts than transcription; but voiceprint recognition itself is not reliable enough yet, and its baseline has to be fixed before this gate means anything.
5. **Slimmer database**:
   - `store_meta` holds only small settings; screen episode inputs and revisions move into derived-layer tables, stored once.
   - Evidence selectors store paragraph ids and character ranges, never text.
   - Sync cursors store positions, never content.
   - Derived JSON goes into tables, or is stored zstd-compressed.
6. **Log retention**: the change feed is compacted once every subscriber has acknowledged it; access logs older than N days are rolled up into per-day, per-client statistics; AI session entries follow the source's retention setting.
7. **Retiring old storage**: one data root only; the old desktop ledger and the old media library are deleted after this conversion, and the core's migration and reconciliation code for them goes with them.

<a id="tables"></a>

## 10. Tables (skeleton)

Tables and key columns only; full DDL follows after review.

**Sources and records**

- `sources`: sources and authorization.
- `source_cursors`: sync positions only, with a size cap.
- `events`: records. New: extendable `end_ms`, `repeat_count`, `tier`, `tier_by` (rule or you).
- `threads`: meetings, conversations, mail threads.

**Originals and text**

- `blobs`: keyed by `sha256`. New: `fidelity`, `codec`, `phash`, `replaced_by` (the new representation after a downgrade), `tier`.
- `event_blobs`: record-to-original links, pointing at the current representation.
- `text_chunks`, `event_text`: paragraphs and references (unchanged).
- `chunk_families`: near-duplicate paragraph families.
- Full-text index: a rebuildable projection (unchanged).

**Derived and memory**

- `episodes`: episodes (same revision model).
- `extractions`: commitments, requests, decisions, questions, topics, with handling state.
- `entities`, `entity_aliases`, `entity_mentions`: people and projects.
- `memories`: long-term memory with a generic structure (kind, content, revision, status); detailed layering waits for research.
- `evidence`: one evidence link table. Derived content and memories point through it at lower layers (records, paragraph ranges, original ranges), with a role and a short selector. Replaces the separate evidence tables.
- `corrections`: your corrections.

**Storage governance**

- `retention_policies`: policy rows.
- `reductions`: reduction log.
- `storage_usage`: usage summary.

**Runtime, access and audit**

- `jobs`, `job_checkpoints`: processing jobs (unchanged), kept apart from data.
- `changes`, `tombstones`: change feed and tombstones (unchanged; new tombstone reason `compacted`).
- `clients`, `access_log`, `proposals`: clients, audit (with retention), proposals for external actions.
- `ai_providers`, `ai_models`, `ai_roles`, `ai_calls`: models and usage (unchanged).
- `settings`: small settings, replacing the config use of `store_meta`.

**Removed**: `media_migrations` and other tables used to migrate from Personavia; unused tables such as `record_audit`, `record_revisions`, `record_selected` and `digests`, to be added back on this skeleton when actually needed.

<a id="deletion"></a>

## 11. How deletion and compaction propagate

1. Before deleting or compacting, compute the affected set: content derived from the target, memories and corrections citing it, index entries that will go stale. Preview and execution use the same computation.
2. **Delete**: write a tombstone and hide the target from queries and agents at once; cancel related jobs; delete derived content and index entries; clean up paragraphs and originals nothing references any more (the deletion order of Timeline library v1).
3. **Compact**: no tombstone; write a reduction log entry; replace the original with the next lower representation; mark evidence links to it as "compacted", which keeps citations valid.
4. Late jobs check tombstones and the reduction log before committing, so deleted or compacted content is never regenerated or restored.

<a id="migration"></a>

## 12. Converting from v8

A one-time script outside the repository, like the Personavia migration: convert and verify a copy first, then swap. Main steps:

1. Move screen episode inputs and revisions out of `store_meta` into derived-layer tables, removing the duplicate.
2. Strip text copies from evidence selectors in favor of paragraph ids and ranges.
3. Keep only positions in sync cursors.
4. Verify hashes, then delete the old desktop ledger's screenshots.
5. Compute initial tiers for all records and originals.
6. Optional: re-encode existing screenshots in bulk (run separately, preview first).

<a id="validation"></a>

## 13. Validation and decisions

**Validation**: build the new structure on a copy of real data, then:

- for each feature in section 2, run two or three real questions as queries: can they be answered, how fast, are the sources right;
- compare space before and after, item by item;
- check dedup and near-duplicate merging against the evaluation set (Agent platform 2.9) for wrong merges.

**Decided** (2026-11-18):

1. The storage limit is set by the user; the UI gives guidance from measured compression, with no fixed default.
2. Memories do not pin originals; key evidence has no fixed floor (section 5).
3. Recordings are stored as AAC (mono, 48 kbps) and WAV is deleted after processing; release gate in section 9.
4. Screenshots are stored as WebP, quality about 60.
5. Age-based downgrade is on by default with conservative days (section 9); it can be changed or turned off in favor of limit-only cleanup.
