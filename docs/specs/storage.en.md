# Storage

[中文](storage.md) · **English**

**Status**: Partly implemented. The service, data root, events, threads, blobs, text chunks, search index, jobs, changes and deletion are implemented, and the schema version is 13. Vector search and portable package export are not implemented.

This spec defines the **storage skeleton** of the timeline library: who holds the database, what is in the data root, the common conventions, and the duties and invariants of sources, events, threads, blobs, text chunks, the search index, jobs, changes and deletion. It does not define the curated layer and derived layer (see [Curation](curation.en.md)), model providers and usage (see [Models](models.en.md)), clients and audit (see [Query interface](query.en.md)), connectors and the ingest protocol (see [Connectors](connectors.en.md)), or layering and the storage budget (see [Data model](data-model.en.md)).

<a id="overview"></a>

## 1. Overview

The timeline library is the only store of facts in Candid. Every source is written as an event of one common structure through the ingest interface. Blobs are stored by content hash, text is deduplicated by chunk, and AI-derived content is stored in layers that can be deleted and recomputed.

- Only one timeline service process holds the database. The app, MCP, the command line, Pi and connectors all go through the interface and never open SQLite directly.
- Events have one common structure. All AI calls are recorded in the same usage table.
- The data layer does not depend on location. The "data root" locates all data, and the directory still works as a whole after it is moved.

| Layer | Tables | This spec |
|---|---|---|
| Sources | `sources`, `source_cursors` | Section 4 |
| Facts | `events`, `event_kinds`, `ingest_conflicts`, `threads` | Sections 5–7 |
| Blobs and text | `blobs`, `event_blobs`, `text_chunks`, `event_text` | Sections 8, 9 |
| Search projection | `text_fts`, `meta_fts` | Section 10 |
| Operation | `jobs`, `changes`, `tombstones`, `store_meta` | Section 11 |
| Derived, models, access | `episodes`, `memories`, `entities`, `ai_*`, `access_log`, etc. | See Curation, Models, Query interface |

For terms, see the [glossary](../glossary.md). The table schema follows `core/crates/candid-store/src/schema.rs`. This spec only describes duties, key fields, invariants and rules.

<a id="deployment"></a>

## 2. Service and data root

### 2.1 Components

The app reaches the `timeline.sqlite` and `blobs/` in the data root through the local channel, connectors through the ingest interface, and local HTTP `/v1` through the service. The MCP shell and the `candid` command line are thin HTTP clients. External AI connects through MCP.

- **Timeline service**: the only component that reads and writes the database and the blob directory. On the desktop it is an independent process of the Rust core ([ADR-024](../decisions/024-rust-core.en.md)), namely the helper process `Contents/Helpers/candid serve` inside the app bundle. The app starts and monitors it. It listens on loopback HTTP and on a local channel used only by the app. If the interface stays the same, it can also run on a NAS.
- **Loopback HTTP**: each start picks a free port and writes it to `endpoint.json` in the run directory (`CANDID_RUN_DIR`, default `~/Library/Application Support/Candid/run`). The file contains no secrets. The first version listens only on `127.0.0.1`. Remote access must add authentication and TLS.
- **MCP shell and command line**: both are the same `candid`. They are thin HTTP clients. They do not open the database or read the blob directory.
- **Single writer**: `.lock` in the data root holds an exclusive `flock`. A second process opening the same directory is refused (`Locked`).
- **Location independence**: if the data root is moved elsewhere, or a service is started on another host with the same directory, the data is still complete and usable.

> SQLite cannot be opened remotely by another machine over an SMB/NFS share (file locks are not reliable). When data is on a NAS, the service must run on the NAS host, and the Mac uses HTTP instead.

### 2.2 Data root layout

Default location: `~/Library/Application Support/Candid/timeline/`.

```text
<root>/
  store.json                  # identity and version of this library, see below
  timeline.sqlite             # and -wal, -shm
  blobs/sha256/<first 2 chars>/<full hash>.<extension>
  staging/                    # ingest staging; cleared at startup (see 2.3)
  exports/                    # export output directory
  logs/                       # rolling diagnostic logs, no body text
  .lock                       # write lock
```

`store.json`:

| Field | Type | Description |
|---|---|---|
| `store_id` | UUID | Identity of this library; never changes after creation |
| `schema_version` | integer | Same as `PRAGMA user_version`; a library with a newer version refuses writes from an older service (`NewerSchema`) |
| `created_at` | RFC 3339 | Time the library was created |
| `created_by` | string | Program and version that created the library, for example `candid/0.10.0` |
| `layout` | string | Fixed as `timeline-root/1`; any other value is refused |

Rules:

- On creation, `store.json` is written last. A directory with only one of `store.json` and `timeline.sqlite` is refused and is not rebuilt automatically.
- The database uses WAL, `synchronous=FULL`, foreign keys, `busy_timeout=5000`, and a single write connection. `journal_size_limit` is 64 MiB, and the WAL is truncated once after opening.
- API keys, OAuth tokens and access tokens are **not** put in the data root in plain text. Secrets are kept in the host system's secret store (the Keychain on Mac). The library only stores references.

### 2.3 Startup recovery and integrity

- **Startup recovery**: `staging/` holds only uncommitted batches and is cleared at startup. Files under `blobs/` that no `blobs` row refers to (moved in, transaction not committed) are deleted. Paths that have a row but a missing file are written to the recovery report and are not repaired automatically.
- **Integrity check** (on demand): `integrity_check`, foreign keys, `text_fts` consistency, `user_version` matching `store.json`, and every blob existing with the right size. Optionally, recompute the hash of every blob and update `verified_ms`.
- **Whole-library migration**: stop the service, copy the whole data root as is (or use the SQLite backup interface), and start at the new location. The service checks `store.json` and the database version. For backup, restore and changing location, see [Data location and migration](../data-migration.md).

<a id="conventions"></a>

## 3. Common conventions

| Item | Convention |
|---|---|
| ID | Lowercase UUIDv7 text (sorted by time). A source's native ID is not the primary key; it is kept in `source_key` |
| Time | Integer milliseconds UTC, with field names ending in `_ms`. The `tz_offset_min` of the place where it happened is stored too. Ranges are half-open `[start_ms, end_ms)`. An empty `end_ms` means a point in time or not yet ended |
| Time basis | `time_basis`: `observed` (observed on this machine), `source` (reported by the source), `imported` (only the import time is known), `estimated` (inferred). There is also `time_uncertainty_ms` |
| JSON | `TEXT` columns, validated before writing. `*_json` columns have a version field or a fixed structure, and unknown fields are kept |
| Tables | Always `STRICT`. Foreign keys on, WAL mode, a single write connection |
| Enums | Written as `CHECK` constraints. When a value is added, the database version is raised and tests are added at the same time |
| Deletion | On deletion, a tombstone is written and derived content is cleaned up, see [Section 11](#jobs-changes-deletion). There is no "soft delete that can still be queried" |
| Revision | Fact events are not rewritten. A correction produces a new event or a new revision. The derived layer identifies items by `(id, rev)` |

<a id="schema"></a>

## 4. Schema overview and sources

The runtime schema follows the DDL in `core/crates/candid-store/src/schema.rs` (the main part) and in `durable.rs`, `performance.rs`, `agents/checkpoints.rs` and `curated.rs`. `core/fixtures/schema-v*.sql` are snapshots of old versions used by upgrade tests. The current schema version is `SCHEMA_VERSION = 13`, recorded in `PRAGMA user_version` and `store.json`.

### 4.0 Schema upgrades

- The upgrade chain v1 → … → v13 runs automatically when the library is opened. Each step is one transaction, and a failure rolls back the whole step. The database commit comes before the atomic replacement of `store.json`. `store_meta.schema.upgrade` (for example `11:12`) lets an interrupted replacement resume only that step. Any other version mismatch is refused.
- Tables whose columns or constraints change are rebuilt as "create new table, copy, drop old table, rename". The target DDL is taken from the current `schema.rs`.
- When a kind, an enum value or a table is added, the version is raised and an upgrade test is added at the same time (including failure rollback and resuming after interruption).

Version history: v2 adds `ai.prompt`, `ai.work` and `agent_event_revisions`; v3 the durable queue and media tables (`durable.rs`); v4 mail and browser-history source kinds, the `quoted` field, the `source_deleted` tombstone reason, and `browser.visit`; v5 the derived-layer `digests`, `entities`, `entity_aliases`, `entity_mentions` and `jobs.feature`; v6 query indexes and the `background_revisions` change counter; v7 the source `mac.system_audio`; v8 the two Agent session checkpoint tables and the media projection change counter; v9 removes the write-only screen-episode revisions, and episode inputs are stored as references; v10 changes evidence locators to `screen-selector/2` (restored on read); v11 the curated-layer tables (see [Curation](curation.en.md)); v13 the energy ledger on jobs (`cpu_ms`, `wall_ms`, `power_state`, `manual`); v12 the Agent session and item tables (see [Curation](curation.en.md), [People and items](people-items.en.md)).

After the v9 and v10 upgrades, `VACUUM` runs once. The old `ai.turn`, `ai_session` and the old source kinds `ai.codex` and `ai.claude_code` are kept only for compatibility. New connectors do not produce them.

<a id="source-tables"></a>

### 4.1 Sources and sync progress

`sources` registers sources and authorization. `source_cursors` keeps the incremental sync position.

| Table | Duty and key fields |
|---|---|
| `sources` | `source_id` (UUIDv7), `kind`, `display_name`, `device_id`, `account_ref` (account reference, no credentials), `secret_ref` (reference to a Keychain item), `config_json` (connector config, with `version`), `policy_json`, `status` (`active`/`paused`/`error`/`revoked`), `last_sync_ms`, `last_error` (a limited enum of error codes, no body text) |
| `source_cursors` | Primary key `(source_id, stream)`. `cursor` is opaque and **advances only after the ingest transaction commits**. It is deleted in cascade with the source |

Source kinds (the `CHECK` on `sources.kind`): `mac.desktop`, `mac.microphone`, `mac.system_audio`, `mac.region`, `media.import`, `device.pendant`, `phone.android`, `calendar.system`, `reminders.system`, `mail.imap`, `mail.gmail`, `mail.apple`, `browser.history`, `agent.transcript`, `mcp.remote`, `manual`, plus `ai.codex` and `ai.claude_code`, which are kept for compatibility.

`policy_json` (privacy and scope settings of ingest sources; for the settings of reference agent sources see [Connectors](connectors.en.md#agent-connectors)):

| Field | Meaning |
|---|---|
| `version` | Fixed at 1 |
| `store_text` | Whether to keep body text; when `false`, only metadata is kept |
| `store_media` | Whether to keep blobs (images, audio, attachments); default `false` |
| `agent_visible` | Whether it is visible to Agents by default; the client scope also matters |
| `retention_days` | Retention period for this source; `null` follows the global setting (unverified: no code that enforces it was found) |
| `exclude` | Three lists: `{apps, threads, patterns}` |

Rules:

- When `store_text=false`, events keep only metadata (no `event_text` is written). When `store_media=false`, blobs are not kept and the staging files are discarded. Events that match `exclude` (apps, threads, and patterns in the title, URL or body, case-insensitive) are skipped before ingest.
- When "include in activity" is off for an `agent.transcript` source, it keeps only the authorization state and path config, and produces no events, threads or text rows. When it is on, it produces only `ai.prompt`, `ai.work` and `agent_session` threads.
- The Agent durable checkpoints `agent_session_entries` (source, session, entry identity and category, and the admitted entries after redaction) and `agent_file_counts` (source, file, content hash, occurrence count) serve only recovery and deduplication. They inherit the source's authorization and deletion boundary, and they do not turn reply or tool output text into stored facts. The progress cursor, the changed entries and the admitted events commit in the same transaction. Old inline checkpoints are converted the next time that session syncs, and on failure the original state is kept.
- The media projection uses its own revision domain, `media_projection` (`background_revisions`). Only changes to media input, related authorization and annotations invalidate it.

<a id="events"></a>

## 5. Events: the only line of facts

`events` has one row per fact. It is the only main line of the timeline.

| Field | Description |
|---|---|
| `event_id` | UUIDv7 |
| `source_id`, `source_key` | The source and the source's native ID (or a durable import ID); `UNIQUE (source_id, source_key)` guarantees idempotence |
| `kind` | Foreign key to `event_kinds` |
| `start_ms`, `end_ms`, `tz_offset_min`, `time_basis`, `time_uncertainty_ms` | Time; `end_ms >= start_ms`; when `start_ms` is empty, `time_basis` must be `imported` |
| `app_bundle_id`, `app_name`, `title`, `url` | App and title; `url` has credentials, query string and fragment removed |
| `thread_id`, `parent_event_id` | The thread it belongs to; the parent event (for example, which focus observation a screenshot belongs to) |
| `actor` | `self`/`other`/`agent`/`system`/`unknown` |
| `sensitivity` | `normal`/`sensitive`/`protected` |
| `payload_version`, `payload_json` | Structure by kind, see [Section 6](#event-kinds); only `payload_version = 1` is supported now |
| `ingest_seq`, `ingested_ms` | `ingest_seq` increases monotonically and is unique. It is used for the change stream and paged snapshots (the counter is in `store_meta.ingest_seq_next`) |

Indexes are built on time, app, kind, thread, source, parent event and `source_key`. See `schema.rs` for the definitions.

Rules:

- **Sort key**: the timeline sorts by `(COALESCE(start_ms, ingested_ms), event_id)`. Events with unknown time are grouped separately in the app, and do not pose as an occurrence time.
- **Idempotence**: syncing the same source record again adds no row. When `source_key` is the same but the content differs, the new content goes into `ingest_conflicts` (primary key `(source_id, source_key, detected_ms)`, with `existing_event_id`, `incoming_json`, `resolved_ms`, `resolution`) and does not overwrite the existing event. Whether content is the same is decided by comparing the event's normalized fingerprint.
- **Protected content**: an event with `sensitivity='protected'` has metadata only. Its body text and blobs are never stored (for example password fields, or windows marked as protected).
- **Parent and child**: `parent_event_id` must resolve. A batch can declare `on_missing_parent=detach` to detach child events whose parent cannot be found.
- **Transaction**: one batch completes in one transaction: threads, events, chunks, blob references and change rows, and last the cursor advance. Any failure rolls back the whole batch (including the cursor) and cleans up newly created blob files, so resending is safe. For the ingest protocol, see [Connectors](connectors.en.md#ingest-connectors).

<a id="event-kinds"></a>

## 6. Event kinds and payloads

`event_kinds` is the registry table (`kind`, `layer`, `is_span`, `payload_version`, `description`). It is updated with the database version. The list in code is `KINDS` in `schema.rs`. `layer` is `evidence` or `perception` (perception is generated from evidence by jobs).

| kind | Layer | Duration | Source | payload fields |
|---|---|---|---|---|
| `app.focus` | evidence | yes | mac.desktop | `{reason}` |
| `window.focus` | evidence | yes | mac.desktop | `{window_token, coverage: "app"\|"window"}` |
| `screen.text` | evidence | no | mac.desktop | `{trigger, text_status, focused_role, focused_label, is_composing, reader}`. The body text is in `event_text` |
| `screen.shot` | evidence | no | mac.desktop, mac.region | `{trigger, pixel_w, pixel_h, scale, source_bounds, ax_evidence_ref?}`; the blob is linked through `event_blobs` |
| `marker` | evidence | no | any | `{label?, note?}` |
| `system.state` | evidence | no | mac.desktop | `{change: "power"\|"sleep"\|"wake"\|"lock"\|"unlock"\|"session", value}`; `change` is required |
| `coverage.gap` | evidence | yes | any | `{reason_code}` (paused, permission_denied, asleep, crashed, source_offline); required |
| `audio.recording` | evidence | yes | mac.microphone, mac.system_audio, device.pendant, media.import | `{codec, sample_rate, channels, duration_ms, …}` |
| `image.capture` | evidence | no | mac.region, phone.android, media.import | `{pixel_w, pixel_h, origin_filename?, exif_time?}` |
| `speech.segment` | perception | yes | ASR job | `{recording_event_id, sample_start, sample_end, speaker_label?, asr: {model_id, revision}, …}`; the text is in `event_text` |
| `image.text` | perception | no | OCR / VLM job | `{image_event_id, method: "ax"\|"ocr"\|"vlm", model_id?, regions_ref?}` |
| `calendar.event` | evidence | yes | calendar.system | `{calendar_id, calendar_name, all_day, location?, attendees: [{name?, email_hash?}], status, …}` |
| `reminder.item` | evidence | no | reminders.system | `{list_name, due_ms?, completed_ms?, priority?}` |
| `mail.message` | evidence | no | mail.apple, mail.imap | `{adapter, direction: "in"\|"out", from: {name?, address_hash?}, to_count, cc_count, attachments: [{name?, mime, size}], mailboxes, level, level_basis, body_status: "complete"\|"partial"\|"not_downloaded"\|"undecodable", quoted_chars, signature_chars, …}` (see the code for the full set); text `subject`, `body` (the newly written part), `quoted` (quoted history and signature, not used as extraction input); when `level=metadata`, only the subject is stored |
| `browser.visit` | evidence | no | browser.history | `{browser, visit_count}`; `url` has credentials, query string and fragment removed; registered, **import not enabled** |
| `note` | evidence | no | manual | `{format: "text"\|"markdown"}` |
| `ai.prompt` | evidence | no | agent.transcript | `{agent, session_ref, message_ref, project?, followups: n, truncated: bool, origin: "transcript"\|"screen"}`; the text you wrote is stored in `event_text` (field `prompt`), and follow-ups are merged into the same event |
| `ai.work` | evidence | yes | agent.transcript | `{agent, session_ref, prompt_event_id, status: "done"\|"running"\|"interrupted", assistant_messages, tool_calls, files_touched, tokens?}`; reply text is not stored |

- To add a kind, register it in `event_kinds`, provide payload validation (`records::validate_payload` and the required fields in `KINDS`) and tests, raise the database version, and then add a row to this table.
- Mail addresses and attendee mail addresses are stored only as salted hashes. Plain-text addresses appear only in person records that the user has authorized separately.
- For AI agent conversations, only the messages you send (`ai.prompt`) and the AI work summary (`ai.work`) are ingested. Reply text is not ingested. `ai.turn` is still registered but no longer used. `agent_event_revisions` keeps per-revision snapshots of received messages and statistics.

<a id="threads"></a>

## 7. Threads: meetings, conversations, mail threads

`threads` groups many events into one meeting, conversation or mail thread: `thread_id`, `source_id`, `source_key` (`UNIQUE (source_id, source_key)`), `kind`, `title`, `started_ms`, `ended_ms`, `participants_json`, `payload_json` (for example the source app of a meeting, or the labels of a mail thread). Index `threads_time(started_ms)`.

- `kind` values: `mail_thread`, `meeting`, `recording`, `calendar_series`, `chat`, `agent_session`, and `ai_session`, kept for compatibility.
- A thread is created or updated by `source_key`, and an `update` change is recorded only when fields change. An event points to its thread through `thread_key`, and pointing to a thread that does not exist fails the whole batch.
- A thread with no events left is cleaned up together with the deletion.

<a id="blobs"></a>

## 8. Blobs

| Table | Duty and key fields |
|---|---|
| `blobs` | Keyed by `sha256` (the hash of the bytes actually stored): `byte_count`, `mime_type`, `rel_path` (relative to the data root, generated by the service, unique), `pixel_w`, `pixel_h`, `duration_ms`, `created_ms`, `verified_ms` (the time the hash was last re-verified) |
| `event_blobs` | Primary key `(event_id, sha256, role)`; `role` is `original`/`preview`/`derived`; deleted in cascade when the event is deleted |

Rules:

- Two different occurrences can refer to the same bytes, and each keeps its own time and permissions. A blob that no event refers to is cleaned up in the deletion flow.
- At ingest, the caller puts the file in `staging/` and declares the hash. The service recomputes the hash and refuses on a mismatch (`BlobMismatch`). If it passes, the file is moved to `blobs/sha256/<first 2 chars>/<hash>.<extension>` (permissions 0600, fsync before the transaction commits). The same bytes are stored only once.
- Screenshot blobs are stored as WebP. Recordings are stored as AAC. `derived` JSON (screen AX evidence, recognition results) is stored compressed with raw DEFLATE, with mime `application/json+deflate` and suffix `.json.deflate`, and `sha256` is the hash of the compressed bytes. JPEG and WAV in old libraries are converted once in the background. For the fidelity ladder, downgrading and the storage budget, see [Data model](data-model.en.md).

<a id="text"></a>

## 9. Text: chunk deduplication and attention

| Table | Duty and key fields |
|---|---|
| `text_chunks` | Chunk body text, one copy per library: `chunk_id` (the rowid for the full-text index), `chunk_hash` (sha256 of the normalized text, unique), `text`, `char_count`, `lang` (`zh`/`en`/`mixed`, nullable), `first_seen_ms` |
| `event_text` | An event's references to chunks, primary key `(event_id, field, ord)`: `field` is `prompt`/`body`/`selection`/`title`/`transcript`/`ocr`/`vlm`/`message`/`subject`/`note`/`quoted`; `chunk_id`; attention marks `visible` (whether it is in the visible area, empty if unknown), `focus_distance` (tree distance to the focused element, 0 is the focus itself, empty if unknown), `is_new`; `char_start` (code point offset in the original concatenated text, used for locating references). Deleted in cascade when the event is deleted |

**Splitting and normalization**

1. The text is first normalized to Unicode NFC, line breaks are unified, trailing whitespace on lines is removed, runs of blank lines are folded into one, and leading and trailing blank lines are removed.
2. It is split by paragraph (blank line). A paragraph longer than 512 characters is split into sentences at sentence-ending punctuation (`。` `.` `！` `？` `!` `?` or a line break) and merged greedily, so that no chunk exceeds 512 characters. A very long sentence with no punctuation is cut hard at 512 characters. Offsets are Unicode scalar offsets in the normalized text.
3. `chunk_hash` is computed on the normalized chunk (`core/crates/candid-store/src/chunk.rs`, the same as the Swift implementation). The same piece of text is stored only once in the library.

**New-content decision (`is_new`)**: a `screen.text` event is compared with the chunk set of the previous `screen.text` event from the same source, the same app and the same title. Chunks that were not there last time are marked new. The chunks of all other events (speech, mail, AI conversations, etc.) are all marked new.

**Use**: the app and AI summaries read chunks that are "new, visible and near the focus (`focus_distance` ≤ 3)" first. The rest is background, and appears only when expanded or searched.

- When a long context repeats, only a reference row in `event_text` is added, and the body text is not written again. These rules are the same for all apps and are not tuned per app.

<a id="fts"></a>

## 10. Search index (rebuildable projection)

```sql
-- chunk body text index
CREATE VIRTUAL TABLE text_fts USING fts5(text, content='text_chunks', content_rowid='chunk_id', tokenize='trigram');
-- metadata index: title, app name, URL
CREATE VIRTUAL TABLE meta_fts USING fts5(title, app_name, url, content='', tokenize='trigram');
```

- The index can be rebuilt from `text_chunks` and `events` at any time, and is not a source of facts. It is written in the same transaction at ingest and cleaned up in the same transaction at deletion.
- **Chinese search**: trigram can directly match only queries of three characters or more. For two-character short words (such as 「预算」 "budget" and 「样稿」 "draft"), structured filters (time, app, source) narrow the range first, and then a limited literal match runs, with a scan limit of 5000 chunks (`SCAN_LIMIT` in `query/search.rs`). When the limit is exceeded, the result is marked `scan_limited`. Short words in title, app name and URL use literal containment matching.
- **Ranking**: on top of the BM25 score, recency and attention weights are added (chunks that are visible, near the focus and newly appeared get higher weight). The weights are in `store_meta.rank_weights`, with the default `{"visible":1.2,"near_focus":1.2,"is_new":1.3,"recency_half_life_days":14}`.
- Vector search (`chunk_vectors(chunk_id, model_id, dim, vector BLOB)`; changing the model needs a new index, and it is not mixed with old vectors) (not implemented).

<a id="jobs-changes-deletion"></a>

## 11. Jobs, changes and deletion

### 11.1 Jobs

`jobs`: `job_id`, `kind` (`asr`, `ocr`, `vlm`, `episode`, `fts_rebuild`, `connector_sync`, etc.), `input_ref` (an event ID, or the id and rev of derived content), `config_version` (the combined version of processor, model and prompt), `state` (`queued`/`running`/`done`/`failed`/`cancelled`), `attempts`, `next_run_ms`, `lease_until_ms`, `error_code`, `output_ref`, `feature` (the number of the feature it belongs to). `UNIQUE (kind, input_ref, config_version)` guarantees that the same input is queued only once under the same config. `job_checkpoints` keeps job checkpoints. For scheduling and retries, see [Models](models.en.md).

### 11.2 Change stream

`changes(seq, at_ms, entity, entity_id, op)`: `seq` auto-increments, and `op` is `insert`/`update`/`delete`. `entity` takes `event`, `thread`, `episode`, `memory`, `digest`, `source`, `blob`, etc. App refresh, sync and subscriptions all read it.

### 11.3 Tombstones

`tombstones(entity, entity_id, deleted_ms, reason)`, primary key `(entity, entity_id)`. `reason` takes: `user`, `retention`, `source_revoked`, `import_rollback`, `superseded` (after recognition or transcription is rerun, the old result is replaced by the new one), `source_deleted` (already deleted in the source). Besides `event`, there is also the `event_key` tombstone: its key is a token of `(source_id, source_key)`, so that a late resend (including samples already absorbed by text merging) cannot bring a deleted event back. Tombstones do not keep titles or body text.

### 11.4 `store_meta`

`store_meta(key, value)` holds only small config and small state (`ingest_seq_next`, `access_log_params`, `rank_weights`, `schema.upgrade`, the cursors of various flows, etc.). Large content is not put here.

### 11.5 Deletion

**Deletion order** (`delete.rs`, one transaction):

1. Write tombstones, and hide the content from queries and Agents at once. Deletion expands downward to child events.
2. Cancel queued or running jobs that refer to these events.
3. Delete the related derived content (episodes, memories that depend on these events, digests whose covered period contains these events, the matching person mentions) and index entries.
4. Clean up chunks that nobody refers to any more (including `text_fts` entries), blobs (rows and files; files are deleted only after the transaction commits), and threads that have no events left.

Rules:

- The deletion reason must be an enum value from Section 11.3. Deleting by source also clears that source's cursors, Agent checkpoints and threads.
- A late job must check the tombstones before it commits, and deleted content cannot be regenerated. Deletion does not keep titles or body text in tombstones.
- There is no "soft delete that can still be queried". For how deletion and compaction relate, see [Data model](data-model.en.md).

<a id="migration"></a>

## 12. Data migration rules

The existing old stores (the Mac desktop ledger and the Hub library) are read-only. Migration happens in the new library, and the old library is not deleted before verification passes. For migrations already completed, see [legacy-migration](../research/legacy-migration.md).

<a id="live-mirror"></a>

### 12.1 Incremental mirror and switching to direct write

Mac desktop capture writes directly into this library, and the original desktop ledger is read once at the switch. Neither side rewrites the other:

- **Desktop ledger (before the switch)**: each time, it rereads from 10 minutes before the last position. Records already ingested are skipped before the file is read. Records deleted in the ledger are deleted here too. Recognition results are told apart by completion time: after a rerun, the new result is ingested and the old result is deleted with `superseded`.
- **Hub library**: new entries are ingested. Entries deleted in the library are deleted together with their transcript chunks. When a transcription or image text changes, the whole group is replaced (`sync_hub_media`). The service's periodic loop now calls `migrate_media` (migration verification, old library kept). `sync_hub_media` is called only by tests (unverified whether a production path still exists).
- **Blobs**: on the same volume they are put into `blobs/` by hard link, and across volumes they are copied. The original files are not changed.
- **Verification**: every record in the ledger is either in this library with the same kind, or already deleted in this library (with a tombstone). Screenshots in the library refer to the blob with the same hash, and they are checked one by one at the first import, at the switch, and at a manual "re-verify". Extra records written directly after the switch are not counted.

**Switch** (`POST /local/desktop/cutover`):

1. Finish syncing the rest of the ledger increments (until a round adds nothing, at most 5 rounds);
2. Check the counts and the hash of every screenshot one by one;
3. If they pass, record `desktop.capture_mode = direct` and the switch record (`desktop.cutover`: time, duration, record count, screenshot count, verification result) in `store_meta`, and from then on the ledger is no longer mirrored;
4. If they do not pass, keep the original mode (keep mirroring). The reason appears in `cutover_error` of `/local/status`. "Re-verify" tries the switch once more.

Before the switch, the core refuses the app's desktop batches (409 `cutover_pending`), and the app keeps captures in its local pending-write queue. A new install with no ledger writes directly. The original ledger directory is kept, read-only.

**App reads**: the app is an internal client. It reads the desktop timeline (`app_activities`, `app_activity`, `app_media`, `app_day_counts`, etc.) through the same tool dispatch and the same audit. These tools are available to internal clients only on the local channel, and do not appear in the tool list or in MCP. For screenshots and recordings, only paths are passed, not bytes. `app_day_counts` returns only the count for each day (at most 400 days), with no content. Activity merging: screenshots and gaps follow the parent observation in the same run. Observations with an activity identifier are merged by (run, activity). For old records with no activity identifier, observations with the same run, the same app and the same title are linked. When reading one day, it reads 6 hours further back.

**Verification items**: counts match by kind; all blob hashes of screenshots match; spot checks against the timeline and search; running the migration again produces no new records; it can resume after an interruption.

<a id="code"></a>

## 13. Where it is implemented

| Content | Code (`core/crates/`) | Tests |
|---|---|---|
| Data root, `store.json`, write lock, startup recovery, integrity | `candid-store/src/store.rs`, `db.rs` | `candid-store/tests/store_root.rs` |
| Schema and kind registry | `candid-store/src/schema.rs`, `durable.rs`, `performance.rs`, `agents/checkpoints.rs` | `candid-store/tests/store_root.rs`, `schema_upgrade.rs` |
| Schema upgrades | `candid-store/src/schema_upgrade.rs` | `candid-store/tests/schema_upgrade.rs` |
| Ingest transaction, idempotence, conflicts, protected content | `candid-store/src/ingest.rs`, `records/` | `candid-store/tests/ingest.rs`, `records.rs`, `record_boundaries_acceptance.rs` |
| Chunk splitting and normalization | `candid-store/src/chunk.rs` | `candid-store/tests/ingest.rs`, unit tests in `chunk.rs` |
| Blobs, compression and conversion | `candid-store/src/ingest.rs`, `json_blobs.rs`, `webp.rs`, `library_conversion.rs` | `candid-store/tests/ingest.rs`, `library_conversion.rs` |
| Search index and ranking | `candid-store/src/query/search.rs`, `query/core.rs` | `candid-store/tests/query.rs` |
| Deletion and tombstones | `candid-store/src/delete.rs` | `candid-store/tests/ingest.rs`, `schema_upgrade.rs` |
| Service process, endpoint, sync loop | `candid-service/src/serve.rs`, `endpoint.rs`, `local.rs` | `candid-service/tests/server.rs`, `desktop.rs`, `local.rs` |
| Old store migration, mirror, switch | `candid-store/src/migration.rs`, `livesync.rs`, `desktop.rs` | `candid-store/tests/migration.rs`, `sync_status.rs`, `candid-service/tests/desktop.rs` |

<a id="open"></a>

## 14. Not implemented and open items

| Item | Status | Description |
|---|---|---|
| Enforcement of the source `retention_days` | Unverified | `policy_json` keeps this field, and no code that enforces it was found |
| Vector search `chunk_vectors` | Not implemented | See Section 10 |
| `origin: "screen"` of `ai.prompt` (screen fill-in) | Unverified | The payload convention has this value, and the logic that produces it has not been checked |
| `browser.visit` import | Not implemented | The kind is registered, and import is not enabled (see [Connectors](connectors.en.md)) |
| Levelling, fidelity, slim records, usage rollups, and the tables `chunk_families`, `evidence`, `retention_policies`, `settings`, etc. | Not implemented | The data model spec designs these tables, and the code does not have them; see [Data model](data-model.en.md) |
| Tombstone reason `compacted` | Not implemented | To be added together with the compaction feature |
| Portable package export and import | Not implemented | See below |
| Final Chinese search approach | Partly implemented | Now trigram plus limited literal matching over 5000 chunks. Whether to switch to a two-character split column is to be decided by comparing on a mixed Chinese-English question set |
| `focus_distance` threshold and ranking weights | In design | Defaults are in Section 10; to be compared against real windows |
| SQLite `synchronous` level and write cost | In design | Now `FULL`; to be measured over a long period |
| Removal of the old desktop ledger migration and reconciliation code | Not implemented | The code is still there |

**Export mapping** (not implemented): the plan is to export the library content as a portable package (scopes `agent_text`, `evidence_selected`, `full_owned`). The package can be read without this service and without any model. Import stages first and then validates, writes idempotently by `(source_id, source_key)` and `(artifact_id, revision)`, and puts conflicts in the conflict area without overwriting existing data.

| Content in this library | Form in the portable package |
|---|---|
| `events` and `event_text` | One `core.observation` family artifact per event (types such as `desktop.*`, `calendar.*`), with the body text restored in chunk order and a selector attached |
| `blobs` | `blobs/<sha256>`, with the hash and size listed in the manifest |
| `threads` | Separate artifacts; events point to them by reference |
| `episodes`, `memories`, `corrections` | Artifacts of the matching layer, keeping revision and source |
| `sources`, `clients`, `ai_*` config | Written to the config part of the manifest, with **no** keys or tokens |
| `ai_calls`, `access_log` | Exported only for the `full_owned` scope, in JSONL format |

<a id="sources"></a>

## 15. Sources

Content moved from the old documents (all in `docs/specs/`):

- [timeline-v1.en.md](timeline-v1.en.md): Section 1 "Engineering design", 2.1, 2.2, Section 3, the start of Section 4, 4.1–4.7, 4.11, Section 8 (export mapping), and the rules in Section 9 that still hold (9.1).
- [data-model-v1.en.md](data-model-v1.en.md): the implemented v9 and v10 upgrades in Section 12; the rest belongs to [Data model](data-model.en.md).
- Decisions: [ADR-024](../decisions/024-rust-core.en.md), [decision log](../decisions/log.md).
