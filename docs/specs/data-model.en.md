# Data Model: Layers, Evidence Tiers and Storage Budget

[中文](data-model.md) · **English**

**Status**: Partly implemented. Layering, content-addressed dedup, WebP screenshots, AAC recordings, compressed derived JSON, the screenshot budget and the v8 to v10 slimming are implemented. Evidence tiers, reduction records, usage statistics, the policy table and log retention are not implemented.

This document defines the **skeleton and policy** of the data: layers, evidence tiers, the fidelity ladder, dedup, the storage budget, space-saving storage, how deletion and compaction affect each other, and the upgrade path from v8. It does not define concrete tables and fields (see the [Storage spec](storage.en.md)), the curated-layer workflows (see the [Curation spec](curation.en.md)) or the query interface (see the [Query spec](query.en.md)). Measurements of the current state and verification results are in [Storage measurements](../research/storage-measurements.md).

Dedup thresholds, automatic curation rules and encoding parameters are policy and can change at any time. The schema only guarantees that it can express them.

<a id="overview"></a>

## 1. Overview

Data is stored in five layers. The higher a layer, the easier it is to discard and recompute. Most space goes to the original screenshots and recordings, so governance focuses on how originals are encoded, deduplicated and downgraded by tier, and on making every reduction leave a trace without breaking references from the upper layers.

| Layer | Examples | Governance | Status |
|---|---|---|---|
| Original | Screenshots, recordings, attachment bytes | Dedup by content hash; WebP/AAC encoding; downgrade by budget | Partly implemented |
| Record | Window switches, emails, what you said to an AI | Dedup at capture; logs cleaned by retention period | Partly implemented |
| Derived | OCR, transcripts, episodes, extractions | Can be deleted and recomputed | Implemented |
| Memory | People, facts, commitments | Has revisions; links to evidence | Implemented |
| Correction | Rename, merge and delete decisions | Stored separately; recomputation does not overwrite them | Implemented |

The status of each part is marked in its own section. The summary is in [Not implemented and open items](#open).

<a id="features"></a>

## 2. Early features and the data they need

The first version builds the first five features solidly. The last two only get a place reserved:

| Feature | Typical question | Data needed |
|---|---|---|
| Recall and search | "Did I talk about the budget with so-and-so last week?" | Records, full-text index over chunks, people |
| Context for agents | "What am I working on, what have I cared about lately?" | Episode summaries, current state, a small and precise profile |
| Commitments and to-dos | "What did I promise to whom?" | Commitments extracted from episodes, with source and handling status |
| Daily review | "What did I do today?" | Episodes, time statistics |
| People and project pages | "All my dealings with so-and-so" | Entities, aliases, mentions |
| Profile and preferences (place reserved) | "What kind of reply do I like?" | Long-term memory, high evidence bar |
| Proactive suggestions (place reserved) | "What should I be reminded of now?" | All the layers above plus attention signals |

These features need largely the same data: records with source, episodes, entities and extraction results. The schema is designed around these things, not around tables for single features.

<a id="principles"></a>

## 3. Principles

1. **Records can be reduced, but every reduction leaves a trace.** Originals and records may be downgraded or deleted by rule, provided that we write down what was reduced, by which rule, when and how much space it saved. Derived content and memory that reference it remain valid. Their source is only marked "original compressed to text" or "original deleted". (Trace not implemented: the reduction record table does not exist, see section 6.)
2. **Dedup happens at three points.** At capture, do not write duplicates (the screen did not change, the text has nothing new). At ingest, merge by content address (the same bytes or the same text are stored only once). In later curation, merge near-duplicates (adjacent near-identical screenshots, families of near-identical chunks). (See section 7 for the status of each point.)
3. **Rules are data, not schema.** The schema provides the hooks for tiers, fidelity, reduction records and usage statistics. The concrete policy (how long before downgrading, which thresholds) is stored as editable policy rows, executed by the core's curation jobs. (Not implemented: the current thresholds are code constants.)
4. **Your marks and corrections take priority over any automatic rule.** Content you pinned and evidence your corrections point to directly are not touched by automatic curation. (Not implemented: there is no pinning mechanism, and the screenshot budget orders only by age and by whether text already exists.)
5. **Derived content can be deleted and recomputed. Your corrections are not overwritten by recomputation.** (Implemented: `corrections` is its own table, and deletion cleans derived content through the tombstone flow.)
6. **Large content is stored once.** Body text goes into the chunk table and binary data into the original store. Everywhere else stores only references and ranges, not copies. (Implemented, see sections 7 and 9.)

<a id="layers"></a>

## 4. Layers

| Layer | Content | Mutability |
|---|---|---|
| **Original** | Bytes of screenshots, recordings and attachments, stored by content hash | Bytes do not change; can be replaced by a more compact representation (see section 6) |
| **Record** | Facts reported by a source: window switches, received emails, what you said to an AI | Content does not change; the time range can only be extended (when duplicates merge); can be deleted by rule |
| **Derived** | OCR, transcripts, episodes, extraction results, entity candidates, indexes | Can be deleted as a whole and recomputed; must state the generator, model, prompt version and confidence |
| **Memory** | Consolidated people, facts, preferences, commitments | Has revision records; each one links to evidence in the lower layers |
| **Correction** | Your rename, merge, mark and delete decisions | Stored separately; still in effect after recomputation |

OCR and transcripts are derived, not records: they are generated from originals by a model and can be recomputed with another model. The input of screen episodes is still kept under the `screen.input/…` keys of `store_meta` (already slimmed down to references, see section 12). It has not yet moved into tables of the derived layer (not implemented).

<a id="tiers"></a>

## 5. Evidence tiers (not implemented)

Every record and every original has a **retention tier**. It decides the order and the floor for automatic curation. The code has no `events.tier`, `tier_by` or `blobs.tier`, and no curation job that runs by tier.

| Tier | Meaning | Automatic curation |
|---|---|---|
| `pinned` | Marked by you by hand, or directly pointed to by your correction | Not touched |
| `key` | Used as main evidence by an episode or an extraction result | Handled after normal ones; the floor is set by policy |
| `normal` | Ordinary records | Downgraded by policy |
| `ephemeral` | Repeated frames with nothing new, screenshots that are mostly fixed UI text, near-duplicates that were merged away | Cleaned first |

- Tiers are computed by rule. You can override them by hand. A manual override is recorded as a correction and recomputation does not change it.
- When references change, the tier moves up with them. When the references are gone, it can fall back.
- **Memory does not need to map to specific text or screenshots.** A key fact is often induced from many episodes and many processes. Its evidence is derived content such as episodes and extraction results, not one image. So memory does not pin originals, and compressing or deleting an original does not make the memory invalid. The evidence chain only needs to lead down to the layer the memory actually relied on. Key evidence has no fixed floor.
- Tiers also apply to the originals of records: if one screenshot is referenced by several records, the highest tier wins.

<a id="fidelity"></a>

## 6. Fidelity ladder and reduction records

Originals have a fidelity ladder by type. Automatic curation only moves down the ladder:

| Type | Ladder (high to low) | Status |
|---|---|---|
| Screenshot | WebP at ingest (see section 9) → thumbnail → text only (OCR result kept) → record metadata only | WebP implemented; "text only" implemented (screenshot budget); thumbnail not implemented; metadata only not implemented |
| Recording | WAV for processing (deleted once processed) → AAC (see section 9) → transcript only → record metadata only | WAV to AAC implemented; transcript only and metadata only not implemented |
| Attachment, document | Original → extracted text only → metadata only | Not implemented |
| Derived JSON | Delete the file after loading into tables; keep what must be kept in compressed form | Compressed storage implemented; deleting the file after loading into tables not implemented |

Every downgrade is written to a **reduction record** (`reductions`): the object, the old fidelity, the new fidelity, the rule number, the time, and the byte counts before and after. A reduction cannot be undone, so before running it there must be a **dry run**: how much space it frees, which items it affects, and which references become "text only".

Reduction records and the dry run are not implemented (the `reductions` table does not exist). The existing "text only" downgrade is run by `screenshot_budget.rs`. It deletes the `original` of the screenshot and keeps the event, the OCR text, the AX evidence and the metadata. The freed bytes and the number of screenshots are accumulated in `store_meta` (`screenshot_budget.freed_bytes`, `screenshot_budget.freed_screenshots`) and shown in the app statistics. They are not recorded one by one.

<a id="dedup"></a>

## 7. Dedup

| When | What is done | What the schema must support | Status |
|---|---|---|---|
| At capture | If the screen equals the previous one (perceptual hash distance within the threshold), do not store a new image. Extend the time range of the previous record and count it. | Records have `end_ms` and `repeat_count`, and may be extended without changing content | Partly implemented: the app's sampling policy skips reads whose content signature did not change (`DesktopContentSignature`), and continuous typing is merged by rule and counted in `payload.merge.samples`. Perceptual-hash dedup of screenshots and the `repeat_count` column are not implemented. `events.end_ms` exists. |
| At ingest | The same bytes are stored once (content hash). The same text is stored once (chunk hash). | `blobs` is keyed by hash; `text_chunks` is unique by normalized hash | Implemented (`blobs.sha256` primary key, `text_chunks.chunk_hash` unique) |
| At ingest | Near-identical chunks are grouped into families (SimHash, rules in the [Curation spec](curation.en.md)) | A chunk family table `chunk_families` | Partly implemented: the screen-text projection groups lines into families with SimHash (`screen_cleanup.rs`). There is no `chunk_families` table. |
| In later curation | Adjacent near-identical screenshots are merged into one record and only the representative frame is kept. Merged ones are downgraded to `ephemeral` and cleaned by policy. | Originals have a perceptual hash `phash`; reduction records state the merge relation | Not implemented (`blobs` has no `phash`; `screen_excerpt.rs` uses thumbnail differences to decide which screenshots are sent for excerpting, and does not merge or free storage) |

Floor for near-duplicate merging: a change in numbers, negation or code counts as new content and must not be merged away (rules are in the [Curation spec](curation.en.md)).

<a id="budget"></a>

## 8. Storage budget

- **You set the limit.** There is a total limit, which can be split by category (screenshots, recordings, text and database). You decide it entirely, and there is no preset value. The interface gives a reference based on measured compression: based on your recent capture volume, about how many days of screenshots and how many hours of recordings a given limit can keep. (Partly implemented: only the screenshot category exists. The app passes the target byte count (about 90% of the limit) to the core through `POST /local/desktop/free-space`. Per-category limits and the days reference are unverified.)
- **Usage statistics.** A usage summary table is kept by "category × retention tier × date". The interface and curation jobs read it directly and do not scan the whole library each time. (Not implemented: `storage_usage` does not exist.)
- **When over the limit.** Curation jobs pick items to downgrade in the order "tier low to high, time old to new, fidelity high to low", until usage is back under the limit. Pinned items are not touched. (Partly implemented: the screenshot budget starts from the oldest and downgrades screenshots that already have text first. Those from the last 7 days (constant `SCREENSHOT_KEEP_RECENT_MS`) are not touched. There are no tiers and no pinning.)
- **Dry run first, then execute.** When automatic curation fires for the first time, or a single run has a large effect, you get a dry-run explanation first. You can set it to run automatically afterward. (Not implemented.)
- **Policy is stored as data.** Retention days, thresholds and encoding parameters are policy rows. Changing policy needs no schema change and no migration. (Not implemented: `retention_policies` does not exist; these are code constants now.)

<a id="storage"></a>

## 9. Space-saving storage

The values were set after measuring on real data. The raw data is in [Storage measurements](../research/storage-measurements.md).

1. **Screenshots are stored as WebP (quality about 60)** (implemented). On 12 real screenshots, WebP at quality 60 averages about 122 KB, about one third of JPEG. OCR character recall is within the same range of variation as JPEG at quality 40, HEIC and AVIF. Why WebP: fast to encode; Windows and browsers can show it directly; HEIC needs an extension on Windows; AVIF is slow to encode.
   - OCR runs on the original picture before compression. The stored image is mainly for people to look at, and for recognition again later if needed. The app hands the raw pixels to `candid encode-webp`, and the core statically links libwebp to encode.
   - Lower the resolution further for secondary screens and non-focused windows (unverified).
2. **Screenshot dedup**: perceptual-hash dedup at capture, plus later merging of adjacent near-identical screenshots (not implemented, see section 7).
3. **Screenshots downgrade by age (on by default)**: the ephemeral tier is cleaned after 7 days; the normal tier is downgraded to thumbnails after 30 days and to text only after 90 days; the key tier is downgraded to thumbnails after 90 days. The days are policy and can be changed or turned off in settings. When the storage limit is exceeded, the budget in section 8 takes precedence. (Not implemented: there is no age-based automatic downgrade. Only the budget downgrade when over the limit exists.)
4. **Recordings**:
   - Current implementation: a new Mac recording is first written as PCM WAV and split into segments. Transcription and speaker analysis read the WAV. After processing, the core replaces the original with AAC (`audio/mp4`, mono, 48 kbps, with the sample rate of the segment itself and not lower than 16 kHz) and deletes the WAV. Encoding uses the system `afconvert`, runs outside the storage lock, and replacing the original is one atomic swap. Only new segments imported with `compact_to` are handled (implemented).
   - Design goal: the Mac writes AAC while recording (segmented MP4, finalized once per second) and no longer writes WAV first. After an unexpected exit, everything before the last unfinalized fragment can be recovered. On disk it is split into 10-minute parts, which the user does not see. (Partly implemented: the app already shows signs of incrementally written segmented AAC. Writing while recording and the 10-minute parts are unverified.)
   - Why AAC (`.m4a`): both Mac and Windows have a built-in encoder. The transcription model resamples to 16 kHz mono before recognition, so a higher sample rate is of no use. 48 kbps is about one fifteenth of WAV. MP3 is not chosen (worst at low bitrates). Opus is smaller, but on Mac it needs a bundled encoder library.
   - **Release condition**: for the same recording before and after compression, the transcript is basically the same, and the voiceprint match score drops by no more than a threshold. Voiceprint recognition itself is of low reliability, so its baseline must be made reliable first, otherwise this condition cannot discriminate. (Unverified.)
   - Old WAV segments are converted to AAC by a background conversion (implemented, see section 12).
5. **Database slimming**:
   - `store_meta` holds only small configuration. The input and revisions of screen episodes go into tables of the derived layer, no longer stored twice. (Partly implemented: revisions are deleted and the input is slimmed down to references. The input is still in `store_meta` and has not moved into derived tables. The `settings` table is not built, and `store_meta` still doubles as configuration.)
   - Evidence locators store only the chunk number and the character range, not the body text. (Implemented: `screen-selector/2`.)
   - Sync progress stores only the position, not the content. (Partly implemented: for AI session entries there are two checkpoint tables, `agent_session_entries` and `agent_file_counts`. Moving out the old inline progress and path-hash dedup are unverified.)
   - Derived JSON is stored in compressed form (implemented): raw DEFLATE (RFC 1951), the blob mime is `application/json+deflate`, the suffix is `.json.deflate`, and `sha256` is the hash of the compressed bytes. New writes are compressed at once, and old ones are converted in batches in the background (`json_blobs.rs`, and `json_compaction.rs` in the service layer). The "zstd" mentioned in the old design was not adopted. Loading derived JSON into tables is not implemented.
6. **Log retention**: the change stream is compacted after all subscribers have acknowledged it. Access logs older than N days are summarized into per-day, per-client statistics. AI session entries are cleaned by the retention setting of the source. (Not implemented: `changes` and `access_log` only grow. The `sources.retention_days` field exists, and the cleaning logic is unverified.)
7. **Retiring the old storage**: keep only one data root. The old desktop ledger and the old media library are deleted after conversion, together with the matching migration and reconciliation code in the core. (Partly implemented: the ledger has been moved out of the library on the author's machine. The migration and switch-over code in `migration.rs` and `desktop.rs` is still there, and the old media tables in `durable.rs` are still created.)
8. **WAL limit**: SQLite `journal_size_limit` is set to 64 MB, and the WAL is truncated when the library is opened and closed (implemented, `store.rs`).

<a id="tables"></a>

## 10. Table list (skeleton)

Only the tables, key fields and status are listed. The field-level list is whatever `core/crates/candid-store/src/schema.rs` says, and the description is in the [Storage spec](storage.en.md). The current `SCHEMA_VERSION` is 12.

| Group | Tables | Design requirement | Status |
|---|---|---|---|
| Sources and records | `sources`, `source_cursors`, `threads` | Sources and authorization; sync position stores only the position and has a size limit; meeting, conversation and email threads | Implemented (the cursor size limit is unverified) |
| | `events` | Add extendable `end_ms`, `repeat_count`, `tier`, `tier_by` | Partly implemented: `end_ms` exists; `repeat_count`, `tier`, `tier_by` not implemented |
| Originals and text | `blobs` | Keyed by `sha256`; add `fidelity`, `codec`, `phash`, `replaced_by`, `tier` | Partly implemented: `sha256` key, `mime_type`, `rel_path`; the other new columns are not implemented, and encodings are told apart by mime and suffix |
| | `event_blobs`, `text_chunks`, `event_text`, full-text index | References to the current representation; chunks and references; rebuildable projections | Implemented |
| | `chunk_families` | Near-identical chunk families | Not implemented |
| Derived and memory | `episodes`, `entities`, `entity_aliases`, `entity_mentions`, `memories`, `corrections` | Revision model; people and projects; long-term memory | Implemented |
| | `extractions` | Commitments, requests, decisions, questions, topics, with handling status | Not implemented: the curated layer now carries items in the `items` family of tables, see the [People and items spec](people-items.en.md) |
| | `evidence` | One evidence link table, replacing the separate evidence tables | Not implemented: `episode_evidence` and `memory_sources` are still used |
| Storage governance | `retention_policies`, `reductions`, `storage_usage` | Policy rows, reduction records, usage summary | Not implemented |
| Runs, access and audit | `jobs`, `job_checkpoints`, `changes`, `tombstones`, `clients`, `access_log`, `proposals`, `ai_providers`, `ai_models`, `ai_roles`, `ai_calls` | Kept as is; add the `compacted` tombstone reason; add a retention period to the access log | Implemented; the `compacted` reason and the retention period are not implemented |
| | `settings` | Small configuration, replacing the configuration use of `store_meta` | Not implemented |

**To be deleted**: tables used by old migrations such as `media_migrations`; unused tables such as `record_audit`, `record_revisions`, `record_selected` and `digests`, to be added again from this skeleton when really needed. (Not implemented: `schema.rs` and `durable.rs` still create these tables.)

<a id="deletion"></a>

## 11. How deletion and compaction affect each other

1. Before deleting or compacting, compute the affected set: content derived from the target, memories and corrections that reference it, and index entries that would become invalid. The dry run and the execution use the same computation. (Unverified: the deletion flow computes the affected set, but there is no dry-run entry point.)
2. **Delete**: write a tombstone and hide the content from queries and agents at once; cancel related jobs; delete derived content and indexes; clean chunks and originals that nobody references (`delete.rs`). (Implemented.)
3. **Compact**: write no tombstone, write a reduction record; replace the original with the representation one level lower; mark evidence links that reference it as "compacted", and the references stay valid. (Partly implemented: replacing the original exists, and file deletion in the screenshot budget follows the order of point 2, deleting rows in a transaction first and deleting files after commit. Reduction records and the "compacted" mark are not implemented.)
4. A late job checks tombstones and reduction records before committing. Content that was deleted or compacted is not regenerated or restored. (Partly implemented: the tombstone check exists. The reduction record check is not implemented along with `reductions`.)

<a id="migration"></a>

## 12. Upgrading from v8

A released version may carry a v8 library, so the core's transactional upgrade chain is kept (v8 → v9 → …). Each step runs automatically when the library is opened and rolls back on failure (`schema_upgrade.rs`, test `candid-store/tests/schema_upgrade.rs`). The current version is 12. v11 and v12 belong to the curated layer and the item tables and are outside this document.

| Version | Content | Status |
|---|---|---|
| v9 | Delete the write-only revisions of screen episodes; episode input no longer stores the original text and is rebuilt from the chunk table when used | Implemented (test `v8_upgrade_stores_large_content_once`) |
| v10 | Episode input and evidence locators no longer repeat the event ID and the chunk family hash on every line; evidence locators change to `screen-selector/2`, and the query interface restores the old format when reading | Implemented (the upgrade code is on the same chain; a dedicated v10 test is unverified) |
| One-time conversions | Convert existing screenshots to WebP, compress derived JSON (raw DEFLATE), convert old WAV recordings to AAC, WAL limit | Implemented: background, in batches, resumable, idempotent (`library_conversion.rs`, `json_blobs.rs`, `media_compaction.rs`). Tests cover screenshot conversion (`tests/library_conversion.rs`) and AAC (`tests/audio_compaction.rs`). Derived JSON conversion has only unit tests. |
| Old desktop ledger | Moved out of the library after a byte-by-byte check shows no difference | Done on the author's machine; the matching migration and reconciliation code is still to be deleted (not implemented) |

Measured space changes from the upgrades are in [Storage measurements](../research/storage-measurements.md). Steps not yet done: turn the evidence embedded in `screen.text` records into references (this is part of the record itself, so first confirm it does not affect the record fingerprint); store the path and hash of AI session entries only once; the tables and curation jobs for tiers, fidelity, reduction records and usage statistics. These are listed in the open-items table below.

<a id="code"></a>

## 13. Where it is implemented

| Content | Code | Tests |
|---|---|---|
| Table structure, upgrade chain, WAL limit | `core/crates/candid-store/src/schema.rs`, `schema_upgrade.rs`, `store.rs` | `candid-store/tests/schema_upgrade.rs` |
| WebP encoding, screenshot thumbnails | `candid-store/src/webp.rs`, `candid-cli` (`encode-webp`); app side `apps/macos/Sources/DesktopPlatform/DesktopWebPEncoder.swift` | Unit tests in `webp.rs`, `candid-cli/tests/cli.rs` |
| One-time conversions (JPEG to WebP, WAV to AAC) | `candid-store/src/library_conversion.rs`, `candid-service/src/library_conversion.rs` | `candid-store/tests/library_conversion.rs` |
| Derived JSON compression | `candid-store/src/json_blobs.rs`, `candid-service/src/json_compaction.rs` | Unit tests in `json_blobs.rs` |
| Recording compression | `candid-store/src/media_compaction.rs`, `candid-service/src/audio_compaction.rs` | `candid-service/tests/audio_compaction.rs` |
| Screenshot budget | `candid-store/src/screenshot_budget.rs`; entry point `/local/desktop/free-space` in `candid-service/src/local.rs` | `candid-store/tests/screenshot_budget.rs` |
| Near-duplicate merging of screen text | `candid-store/src/screen_cleanup.rs` | `candid-store/tests/screen_pipeline.rs` |
| Deletion order | `candid-store/src/delete.rs` | `candid-store/tests/records.rs` and others (exact files unverified) |
| Content signature at capture | `apps/macos/Sources/DesktopCore/DesktopSamplingPolicy.swift` | Unverified |

<a id="open"></a>

## 14. Not implemented and open items

| Item | Status | Note |
|---|---|---|
| Evidence tiers (`tier`, `tier_by`, pinning) | Not implemented | Section 5 |
| Reduction records `reductions` and dry run | Not implemented | Sections 6 and 11 |
| Usage summary `storage_usage`, per-category limits and the interface reference | Not implemented | Section 8 |
| Policy rows `retention_policies` | Not implemented | Thresholds are code constants now |
| Age-based screenshot downgrade, the thumbnail level | Not implemented | Section 9, point 3 |
| Screenshot perceptual hash (`phash`), `repeat_count`, later merging of near-identical screenshots | Not implemented | Section 7 |
| `chunk_families` table | Not implemented | The existing SimHash is used only in the screen-text projection |
| `evidence` unified evidence table, `extractions` table, `settings` table | Not implemented | Section 10 |
| Tombstone reason `compacted`, the "compacted" reference mark after compaction | Not implemented | Section 11 |
| Log retention (change-stream compaction, access-log summary, AI session entry cleaning) | Not implemented | Section 9, point 6 |
| Move the input of screen episodes into derived tables | Not implemented | Sections 4 and 9 |
| Turn evidence embedded in `screen.text` records into references | Not implemented | First confirm it does not affect the record fingerprint |
| Store the path and hash of AI session entries once; move out old inline progress | Unverified | Measured: moving it out line by line directly would grow the table from 82 MB to 221 MB, so the duplicates must be removed first |
| Delete the old migration and reconciliation code and the old media tables | Not implemented | Section 9, point 7 |
| Write AAC while recording, 10-minute parts | Unverified | Section 9, point 4 |
| Audio compression release condition (transcript and voiceprint comparison) | Unverified | The voiceprint baseline must be made reliable first |
| Lower resolution for secondary screens and non-focused windows | Unverified | |
| Real effect of not storing duplicate screenshots at capture | Unverified | The gain is not counted in the existing space estimate |
| Delete the file after derived JSON is loaded into tables | Not implemented | |
| Verification method: for each feature in section 2, pick two or three real questions and run them; compare space before and after conversion; check dedup and near-duplicate merging for wrong merges with an evaluation set | To do | The evaluation set is in the [Curation spec](curation.en.md) |
| Feature gap: whether the derived flows for episode summaries, extraction and the entity layer are connected end to end | Unverified | At the time of the old verification, episode generation had stopped, and extraction and entities had almost no data. The curated layer has been rebuilt since, so this needs to be verified again. |

<a id="sources"></a>

## 15. Sources

- Sections 2 to 12 of the old document `docs/specs/data-model-v1.md` (section 1, the current-state measurements, and section 13, the verification results, went to [Storage measurements](../research/storage-measurements.md); the open items of section 13 were merged into the table above).
- The five items "already decided" in section 13 (you set the storage limit, memory does not pin originals, recordings are stored as AAC, screenshots are stored as WebP, age-based downgrade is on by default) are written into sections 5, 8 and 9 of this document. The dated record is in the [Decision log](../decisions/log.md).
- This document replaces the parts of the old [Timeline library spec](timeline-v1.md) (data root layout, data structures, and migration of existing data) that overlap with storage governance. Table structure details are in the [Storage spec](storage.en.md).
