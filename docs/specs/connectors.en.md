# Connectors and the Ingest Protocol

[中文](connectors.md) · **English**

**Status**: Partly implemented. The ingest protocol, mail (the local 「邮件」 (Mail) app and IMAP), browser history (import turned off) and AI agent sessions are implemented. Calendar and reminders are not connected to the app. There is no unified interface.

This spec defines how data enters the timeline library: the ingest protocol (`IngestBatch`), where the two kinds of connectors belong and how they behave, the connector list, the mail connector, and source filtering. It is the interface to follow when writing a new source (including a future Windows app or phone app). It does not define curation after ingest (see [Curation](curation.en.md)), the fields of event kinds and payloads (see [Storage](storage.en.md)), or how outsiders query (see [Query interface](query.en.md)).

<a id="overview"></a>

## 1. Overview

A connector turns external data into ingest batches and hands them to the core for ingest. There are two kinds:

| | Ingest connector (ingest) | Reference connector (reference) |
|---|---|---|
| Used for | Data the system produces itself: desktop, recordings, screenshots, pendant, phone capture; system calendar; mail | Data that other systems keep themselves: AI agent conversation records; possibly Notion later |
| What the library stores | Events, body text, blobs | Only the authorization state and path config, no content |
| Where content comes from | The timeline library | Read live from the source by the connector when needed |
| Default | On, according to the authorization of that source | Every switch is off |

The AI agent connector is a reference connector. But once "Include in activity" (「纳入活动」) is turned on, it writes your sent messages and work summaries under the ingest protocol. See [AI agent conversations](#agent-connectors).

**Ownership rules** (see the [architecture map](../architecture.md)):

1. Connectors that read files or use the network belong in the core (`core/crates/candid-connectors/`; agent sessions are in `candid-store/src/agents/`). Connectors that call system interfaces belong in the platform layer (on Mac, screen, recording, calendar and reminders are in `apps/macos/`).
2. All writes go through the ingest interface `TimelineStore::ingest` (the platform layer uses the local channel `POST /local/ingest`). A connector does not touch the database directly. For the two exceptions, see [How it looks in code](#shape).
3. Credentials do not enter the library. The library stores only `secret_ref`. Passwords are in the Keychain, and the app puts them into the core process memory.
4. The connector is one of the three extension seams.

<a id="ingest-connectors"></a>

## 2. Ingest connector protocol

Every ingest connector implements the abilities below, and the service schedules and runs it. A connector may write only through the ingest interface and may not write the database directly.

| Ability | Description |
|---|---|
| `describe()` | kind, display name, permissions needed, the event kinds and thread kinds it can produce, default `policy_json` |
| `authorize()` | Guides the user through system authorization or OAuth. Credentials go into the Keychain, and only `secret_ref` is returned |
| `backfill(range)` | Backfills history. The user chooses the range, and it can be interrupted and resumed |
| `sync(cursor)` | Incremental sync. Returns a batch of `IngestBatch` and a new cursor |
| `revoke()` | Stops syncing, and keeps or deletes that source's data as the user chooses |

`IngestBatch` (`on_missing_parent` is optional. With `detach`, an event whose parent is neither in this batch nor in the library is ingested as usual, with no parent link. This is for parent records in app capture that were blocked by the privacy gate or already deleted by the user. When it is absent or `error`, the whole batch fails):

```json
{
  "source_id": "…",
  "cursor_after": {"stream": "calendar:<calendar ID>", "cursor": "…"},
  "threads": [{"source_key": "…", "kind": "calendar_series", "title": "…", "started_ms": 0}],
  "events":  [{"source_key": "…", "kind": "calendar.event", "start_ms": 0, "end_ms": 0, "time_basis": "source",
               "thread_key": "…", "payload_version": 1, "payload": {},
               "text": [{"field": "subject", "content": "…"}],
               "blobs": [{"staging_path": "…", "sha256": "…", "role": "original"}]}]
}
```

**Batch rules**:

- **A batch completes in one database transaction**: it writes threads, events, chunks, blob references and change records, and last advances the cursor. If it fails midway, the whole batch rolls back and is retried from the original cursor next time. `(source_id, source_key)` guarantees no duplicates.
- **Idempotence and conflicts**: the same `(source_id, source_key)` with the same content is a duplicate and counts in `duplicates`. Different content goes into the conflict area (`ingest_conflicts`) and does not overwrite what is already ingested. A deleted key has a tombstone, so a late resend does not bring it back.
- **Source state**: only sources that are `active` or `error` can be written to. Others are refused.
- **Source policy**: `policy_json` (`store_text`, `store_media`, `agent_visible`, `retention_days`, `exclude`) takes effect at ingest. Events that match `exclude.apps`, `exclude.threads` or `exclude.patterns` (case-insensitive containment match on title, URL and text) are not ingested.
- **Source deletion and metadata revision**: a batch can carry `removed_source_keys` (records deleted in the source or left only in the trash or spam folder; they are deleted in the same transaction with a `source_deleted` tombstone, and a resend does not bring them back; unknown keys are ignored) and `payload_revisions` (`[{source_key, payload}]`, which change only the payload of an already ingested event, for example when a mail moved to another mailbox; the old payload is kept as a revision snapshot, the payload carries `revision`, and body text, time and identity do not change). A change in body text is still handled as a conflict. Both commit in the same transaction as the events and the cursor. In one batch, a key that is added and then removed ends up removed.
- **Typing merge**: when a batch carries `merge_text: true`, `screen.text` is merged. See the [next section](#text-merge).
- **Batch validation** (if any item fails, the whole batch fails with `InvalidInput`; the full enum values follow the constants at the top of `ingest.rs`): `source_key` is not empty; the event kind is registered, the payload is an object, `payload_version` is 1, and the required fields are complete; `time_basis` ∈ `observed`, `source`, `imported`, `estimated`, and it must be `imported` when there is no `start_ms`, and `end_ms` is not earlier than `start_ms`; `actor` ∈ `self`, `other`, `agent`, `system`, `unknown`; `sensitivity` ∈ `normal`, `sensitive`, `protected`; the text field, the thread `kind` and the blob `role` (`original`, `preview`, `derived`) must be within the registered values; `sha256` is 64 lowercase hex characters. Blob files are first put in the staging directory, and the core moves them into `blobs/` at ingest.
- **Return value**: the counts (`inserted`, `duplicates`, `conflicts`, `merged`, `removed`, `revised`, etc.) and a map of `source_key → event_id`.

Field-level definitions follow `candid-store/src/types.rs`. For event kinds and payloads, see [Storage](storage.en.md).

**Every ingest connector must be tested with synthetic samples**: normal sync, repeated sync, retry after interruption, refusal on an unknown format, and no more reading after revoke.

<a id="shape"></a>

## 3. How it looks in code

The spec protocol is "five abilities per connector". The code has no unified trait. There are four forms, depending on where the code lives:

| Where | Form |
|---|---|
| Rust `candid-connectors` (mail, browser history) | A set of functions dispatched by `kind`: `describe()`, `list()`, `command()` (dispatched by `action`), `sync_all()` (sync now) and `sync_due()` (one background round, skipping sources in backoff). Sync does "plan (holding the lock) → read (without the lock) → commit (holding the lock, one transaction together with the cursor)" inside, and does not return batches to the caller |
| Rust `candid-store/src/agents/` (AI agent sessions) | One source `agent.transcript`, three agents (`codex`, `claude_code`, `pi`). `agent_call` dispatches by `describe / detect / sessions / read / search / usage`, and `sync_agents` does incremental ingest |
| Swift `TimelineConnectors` (calendar, reminders) | Has a `Connector` protocol with all five abilities (`describe / authorize / backfill(range, cursors) / sync(cursors) / revoke`). The caller commits the batches it produces |
| Swift desktop capture (`DesktopCore`) | Not a `Connector`: events are first written to a local outbox, and `DesktopDelivery` submits up to 256 per batch through `POST /local/ingest`, with `on_missing_parent: "detach"` and `merge_text: true` |

**Differences from the spec protocol**:

- `backfill(range)` has no separate entry on the Rust side. Mail and browser history limit the first read range with the config `backfill_days` (default 90). The Swift calendar connector has `backfill(range:)`.
- `authorize()` on the Rust side is the `authorize` command: it registers the source and probes permissions (without reading content). Credentials are first put into core memory by the app with the `secrets` command.
- `revoke()` is the `revoke` command: the source is set to `revoked`. With `delete_stored: true`, that source's events are also deleted with the reason `source_revoked`. Authorizing again reuses the original source ID and the history already ingested.
- Exception one: manual mail level changes and re-levelling (`set_level`, `relevel`) change the payload and text of already ingested events directly in one transaction through `apply_mail_levels` (the old payload is likewise kept as a revision snapshot), without going through `ingest`.
- Exception two: when the mail scope changes, `update` resets the cursor with an empty batch that carries only the cursor, so mail is scanned again.

**Connector management interface** (offered only on the app's local channel; requests with a token and loopback HTTP are always refused): `GET /local/connectors` lists sources, their state and missing authorizations (`needs`: `full_disk_access`, `credentials`, `unsupported_format`). `POST /local/connectors` `action`: `describe`, `list`, `authorize`, `update`, `revoke` (`delete_stored` optional), `dry_run` (reports counts only, usable before authorization), `sync`, `secrets` (the app reads them from the Keychain and puts them into the core process memory, not on disk), `mail_scope`, `mail_recent`, `mail_body`, `set_level`, `relevel`. A failure returns only a status code and a bounded error code, with no source content. There are also `POST /local/ingest` (one `IngestBatch`) and `POST /local/sources` (register a source). A `mac.desktop` source returns 409 `cutover_pending` before the switch to direct write.

**Sync scheduling**: a source that fails repeatedly backs off by 30 seconds, then 1, 2, 4 … minutes, up to 30 minutes (in process memory, so it retries at once after a restart). An explicit sync always tries at once. When the error code changes, it is reported only once.

<a id="text-merge"></a>

## 4. Typing merge (`merge_text`)

The rules and thresholds are provisional. When a batch carries `"merge_text": true` (turned on for Mac desktop capture), a `screen.text` is merged into the most recent `screen.text` of the same source, and produces no new event, if both have the same app, the same window title, the same focus element (`focused_role` and `focused_label`), the same capture run and activity, and the gap is no more than `gap_ms`:

- Body text and selection are identical line by line: only extend the `end_ms` of that event;
- Almost empty (fewer than `min_chars` characters, or less than `min_ratio` of that event's text), or `text_status` is not in `read_statuses`: no event is produced, and the merging is not interrupted;
- The line overlap (number of distinct common lines ÷ the larger line count of the two sides) is at least `min_line_overlap`: that event stores the new full text instead (the old chunks are removed from the index), `end_ms` is extended, and `payload.merge.revisions` is incremented;
- Otherwise a new event is created as usual.

The `source_key` of a merged sample is recorded in `payload.merge.keys` (the most recent 200 are kept), and `payload.merge.samples` counts them. A resend is a duplicate. Screenshots whose parent is such a sample attach to the merged event. When the merged event is deleted, these keys get tombstones together. The change writes one `update` in `changes`.

The thresholds are in `store_meta.text_merge`, with the default `{"gap_ms": 120000, "min_line_overlap": 0.6, "min_chars": 2, "min_ratio": 0.1, "read_statuses": ["captured","partial","ok"]}`. Old data (records imported from the desktop ledger) is not merged. Folding in the query and app layers is not implemented (plan to be decided).

<a id="agent-connectors"></a>

## 5. Reference connector: AI agent conversations

**Principle**: **what you say is ingested; the AI's replies are only referenced.** The AI's replies, tool output and reasoning stay in each agent's own files, and are read live on demand only within the scope the user has authorized. After "Include in activity" is turned on, the messages you send, session start and end, and work summaries are written to the timeline library. The timeline's "AI interaction" (「AI 交互」) row, search and curation use them.

**One connector per agent, three switches authorized separately, all off by default** (`AgentConsent`; turning on "Include in search" or "Include in activity" without "Read" is refused, with the error `read_required`):

| Switch | When on | When off |
|---|---|---|
| Read (「读取」) | When you or an authorized client opens a session, the connector reads that session live | The connector reads no file of that agent (it only checks whether the directory exists, for the "discovered" hint) |
| Include in search (「纳入搜索」) | `search` queries that agent's conversations live, and merges the results with results in the library (needs "Read" first) | Search does not touch that agent |
| Include in activity (「纳入活动」) | Incremental sync: the messages you send are written as `ai.prompt`, each AI round of work as `ai.work`, and the session as an `agent_session` thread (needs "Read" first) | No event is written. When turned off, it asks whether to delete what was already written |

`policy_json` of a `kind='agent.transcript'` source (fields and defaults follow `AgentConsent`):

```jsonc
{
  "version": 1,
  "agent": "codex",                       // codex | claude_code | pi (the AGENTS constant in code)
  "read": false,
  "search": false,
  "activity": false,                      // include in activity: write ai.prompt / ai.work
  "prompt_max_chars": null,               // configurable; the engineering default is no limit. 4000 chars is still a pending proposal
  "paths": ["~/.codex/sessions"],         // default paths, which the user can change
  "exclude": {"projects": [], "sessions": []},   // exclude by project directory or session
  "content": {"user": true, "assistant": true,
              "tool_calls": "name_only",  // none | name_only
              "tool_output": false, "reasoning": false}
}
```

**Connector interface** (all read-only; except for `sync`, results are not stored on disk):

| Ability | Description | Switch needed |
|---|---|---|
| `describe()` | agent ID, display name, default paths, supported format versions | — |
| `detect()` | Whether the default path exists and whether the format version is recognized. It looks only at directories and file headers, and does not read conversation content | — |
| `sessions(range, project?)` | Lists the sessions in the time range: reference, start and end time, project directory, title (if the source file has one), number of turns, usage total | Read |
| `read(ref, cursor?, limit)` | Reads the messages of one session in order: time, role, text (filtered and truncated by the `content` settings), tool names | Read |
| `search(q, range, project?, limit, deadline_ms)` | First narrows to the relevant session files by time and project, and then searches. When a limit is exceeded, it returns partial results marked `limited` | Read + Include in search |
| `usage(range)` | Computes that agent's token usage live | Read |
| `sync(cursor)` | Reads incrementally by session file and offset, and produces `IngestBatch`: `ai.prompt`, `ai.work` and `agent_session` threads | Read + Include in activity |

- **Reference format**: `agent:<agent>:<session key>`, and at message level `#<message identifier>` is added. When there is no message identifier, a stable reference is built from the source record and its order of appearance. The session key uses the source file's own stable identifier (Codex uses `session_meta.id`, which matches the UUID in the normal rollout file name; Claude Code uses `sessionId`; pi uses the session header `id`).
- **Not stored on disk**: outside `sync`, session body text is used only in the memory of that read. The access audit stores only query parameters and result counts.
- **What counts as "what you say"**: only messages you sent yourself. For Codex, activity ingest requires a native send event or an explicit human-origin mark, and old records without source evidence are for live reading only. Not counted: tool results (recorded with the user role in Claude Code), content injected by the system or tools (reminders, skill text, and the text after a slash command is expanded; only the command name is recorded), context compaction summaries, and messages forwarded from other sessions. Each adapter is covered with synthetic samples.
- **Order across files**: fragments of the same session are first merged and then used to generate prompts and work, sorted by source time. The checkpoint and the session result commit in the same transaction.
- **Ingest rules**: `source_key` is the message-level reference `agent:<agent>:<session key>#<message uuid>`, so repeated sync does not write twice. A follow-up that comes right after (what is added before the AI has started working) is merged into the same `ai.prompt`. `ai.work` starts at this message and ends at your next message or the last record of the session. When there is new work, it is marked `running`. Only an explicit end signal marks it `done`. It is marked `interrupted` when the next user message cuts in or when it is silent for more than 2 minutes, and later growth can still revise it. `files_touched` counts write targets that have a successful tool result, without duplicates, and an unknown result is marked with `files_touched_complete=false`. `file_targets` is the number of requested targets. Events store only counts. Paths are filtered by the project exclusion rules.
- **Handling of ingested text**: it is redacted by the same redaction as other sources. The part beyond `prompt_max_chars` is truncated, and the full text is still in the source file. Whether external clients can read it follows the scope setting of that source.
- **Safe handling**: before returning, text is masked for secret-like patterns (such as `sk-…`, `AKIA…`, private key blocks; implemented as `agents::redact`, shared with mail ingest). The returned text is untrusted data, not instructions.
- **Discovery and adapters**: the connection page lists the agents found on this machine ("Discovered, not authorized" (「已发现，未授权」)). An agent with no adapter shows "Not supported yet" (「暂不支持」), and the format is not guessed.
- **Source files cleaned up**: reply text can then no longer be read, and Candid keeps no copy. What you said and the work summaries already ingested are not affected, and the app shows "The reply is no longer in the source file" (「回复已不在源文件里」).
- **Tests**: no files are read when the switch is off; only content within the authorized scope is read; results are not stored on disk; an unknown format is refused.

First agent adapters:

| agent | Default path | Format notes | Status |
|---|---|---|---|
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | Each line has `timestamp`, `type`, `payload`; messages, tool calls, `token_count` | Implemented |
| Claude Code | `~/.claude/projects/*/*.jsonl` | Each line has `type`, `message.role`, `timestamp`, `sessionId`, `cwd`; `custom-title` is the title | Implemented |
| pi | `~/.pi/agent/sessions/--<path>--/*.jsonl` | First line `{"type":"session","version":3,…}`, entries have `id`, `parentId`, and messages carry `usage` | Implemented |
| WorkBuddy, dsh, etc. | To be confirmed | Samples are needed to confirm the format | Not implemented; to be added when samples exist |

The source file format is controlled by the other tool, with no version promise. Parsers recognize by version. When an unrecognized format is met, a clear error (`invalid_session`) is returned for that file, and it is neither guessed nor skipped. A failed file is not retried until its size or modification time changes.

<a id="sources"></a>

## 6. Source coverage: more connectors

Commitments, requests, people and projects mostly appear in mail and messages. Screen capture alone would miss content that was never opened and the thread structure. So these sources are **ingest** connectors: body text is ingested and goes into the deduplication, entity and extraction flow of the [curated layer](curation.en.md). Large or fast-changing content such as attachments and remote documents is still handled by reference, and read live from the source when needed.

All new connectors follow these rules:

- Each connector and each account is authorized separately and can be revoked at any time. After revoke, nothing more is read, and content already ingested is kept or deleted as you choose.
- Content others sent to you goes through the same entry redaction. External clients get metadata only by default, and body text needs a separate scope. Current implementation: mail gets secret-pattern masking before ingest (the same rule as AI sessions; other rules such as card numbers are not done, see [Curation](curation.en.md)). Mail sources have `agent_visible=false`, so external clients cannot see them at all by default, and access must be granted by source ID in their scope. A "metadata only" tiered view is not implemented.
- They are implemented under the [ingest connector protocol](#ingest-connectors) and tested with synthetic samples.

<a id="connector-table"></a>

## 7. Ingest connector list

This table is the only place where the connector list is maintained. The order is the suggested order of connecting.

| Order | Connector | Source | Mapping | Default policy | Status |
|---|---|---|---|---|---|
| 1 | Mac desktop (`mac.desktop`) | Desktop capture written directly (through `POST /local/ingest`); for the mirror and switch of the old ledger see [Data location and migration](../data-migration.md) | Desktop events such as screen, app and window | Keep the current per-app authorization | Implemented |
| 2 | Library (`media.import`) | The old Hub `media_timeline` and recognition results (read-only mirror, `livesync.rs`) | `audio.recording`, `image.capture`, `speech.segment`, `image.text` | Keep blobs | Implemented |
| 3 | System calendar and reminders (`calendar.system`, `reminders.system`) | EventKit, read-only | `calendar.event`, `reminder.item`; recurring events use a `calendar_series` thread | Keep title and time; notes are not kept by default | Partly implemented: the Swift connector and tests exist, but the app has no sync that calls it (unverified), so no data is ingested |
| 4 | Mail: the local 「邮件」 (Mail) app (`mail.apple`) | The Envelope Index and `.emlx` under `~/Library/Mail/V10`, read-only; needs Full Disk Access (「完全磁盘访问」) | `mail.message`, thread `mail_thread`; for attachments only name, type and size are recorded | Body text is ingested after redaction; external clients cannot see it by default and must be granted by source ID | Implemented (verified with synthetic samples); real read-only trial run (unverified) |
| 5 | Mail: own IMAP login (`mail.imap`) | email-lib (the pimalaya library behind himalaya); one source per account, passwords only through `secret_ref` | Same as above | Same as above; Gmail goes through app-specific passwords first | Implemented (verified with a local fake IMAP server); real accounts (unverified); OAuth not implemented |
| 6 | Browser history (`browser.history`) | The local history databases of Safari and Chrome, copied to a private temporary directory and then read-only | `browser.visit`: URL (credentials, query string and fragment removed), title, visit time, count | Authorization, revoke and dry run (counts only) work; **no import** (`import_enabled` cannot be set to true, and sync is always skipped) | Partly implemented: import off |
| 7 | Phone | Android companion app | `image.capture`, `audio.recording`; notifications and usage stats later | Authorized separately per kind of source | Not implemented |
| 8 | Instant messaging | iMessage (local database, needs Full Disk Access), Feishu or Slack (open APIs); WeChat local data is encrypted, so it is not considered for now | Conversations, sender, time, text; threads formed by conversation | Group chats and direct chats authorized separately | In design |
| 9 | Notes and selected folders | Local note directories such as Notes and Obsidian; document directories you choose | Note body text is ingested; other files record metadata only and are read by reference | Read only the chosen directories, never scan the whole disk | In design |
| 10 | Work services | Open APIs or remote MCP of GitHub, Notion, Feishu docs and so on | Only "what you took part in" is ingested as events, and content is by reference | Each query is counted separately in audit | In design |

AI agent sessions (reference connector) are described [above](#agent-connectors) and are not in this table. The full registry of source kinds is `SOURCE_KINDS` in `candid-store/src/schema.rs`. Calendar entries get changed at the source. Whether new content under the same `source_key` is handled as a revision or goes into the conflict area needs a rule before connecting (to be decided).

<a id="mail"></a>

## 8. Mail connector

The mail connector is implemented by Candid itself. The main route reads the data of the local 「邮件」 (Mail) app, and there is also an IMAP login adapter. It does not depend on a third-party mail MCP, and it does not implement a separate login for each mail provider. Reasons: each provider's official interface needs its own OAuth app registration with strict review; a unified mail API service is a cloud service; and the community Apple Mail MCP projects have no incremental sync or threads. After mail is ingested, external agents query it through Candid's own MCP, with scope filtering and audit. For the survey, see the [research notes](../research/references.md).

The mail connector is implemented in the Rust core ([ADR-024](../decisions/024-rust-core.en.md)). Parsing, threading, cleaning and filtering exist in one copy only. "Where to read mail from" is a replaceable adapter:

| Order | Adapter | Platform | Description | Status |
|---|---|---|---|---|
| 1 | Local data of the system 「邮件」 (Mail) app (`mail.apple`) | Mac | Main route. It accepts only `V10` with a matching index schema and signature, and refuses every other version as a whole (`unknown_mail_version` / `unknown_mail_schema`). The real version is to be confirmed by a read-only trial run | Implemented |
| 2 | Thunderbird local data | Mac, Windows, Linux | Thunderbird handles OAuth itself. Exchange uses the EWS interface that Microsoft is phasing out, to be checked before connecting. An offline copy must be kept, and the storage format is decided after testing | Not implemented |
| 3 | Own login (`mail.imap`) | All | Reads with only EXAMINE, UID SEARCH and BODY.PEEK, and does not change flags or move anything. When a mail leaves a mailbox, it is looked up by Message-ID in other folders (such as Gmail "All Mail"), and a deletion is recorded only if it still cannot be found. Transport must be `tls` or `starttls`, and `none` is allowed only on loopback addresses. Gmail uses an app-specific password, or an OAuth app registered by the user. It does not borrow the OAuth app of other software. One-click login in Candid's name needs Google and Microsoft review, to be assessed on demand | Implemented |
| — | MailKit live extension | Mac | Called by the 「邮件」 (Mail) app when new mail arrives, and can get the body text. It covers new mail only. To serve as a supplement to row 1 once testing shows it works | Not implemented |

**Data sources (adapter 1)**:

- Index: `~/Library/Mail/V<n>/MailData/Envelope Index` (SQLite). It is opened read-only and unchanged. The schema is recognized at runtime, and only verified versions are accepted. If it is not recognized, it is refused as a whole and the connection page shows a hint. It is never read by guessing.
- Body text: the `.emlx` files under each mailbox directory. The structure is a first line with the byte count, the RFC 822 original, and an attribute table at the end. `.partial.emlx` has only part downloaded. It is ingested with the content that exists and marked incomplete (`body_status=partial`). Body text that the Mail app has not downloaded is not read, and a download is not triggered.
- Permission: needs "Full Disk Access". Without it, the source shows as not authorized (error code `full_disk_access_required`), retries with backoff, and reports only when the error code changes.

**Sync**:

- The mail row number and modification info in the index are the cursor (stream name `mail`), and reading is incremental, at most `batch_limit` mails per batch (default 200, max 2000).
- Each round compares whether ingested mail is still there. Deletions and moves are recorded as tombstones or new revisions, and nothing is lost silently. Mail that is left only in the trash or spam, or completely deleted, is deleted with `source_deleted` and leaves a tombstone (so restoring it from the trash later does not ingest it again). A change of mailbox (such as archiving) is a payload revision of the same event.
- Mail whose body text is not downloaded stores only metadata from the index (subject, sender and time) (`body_status=not_downloaded`), and the body text is not filled in automatically even if it is downloaded later (to be decided).
- `source_key` uses `mid:<Message-ID>`. When there is no Message-ID, it uses a combination of account, mailbox and row number. The same mail in several mailboxes is ingested only once, and `payload.mailboxes` records the mailboxes it is in.
- Read-only: it does not change flags, move or delete anything, and does not call the Mail app's scripting interface.

**Parsing** (MIME uses a mature Rust library and is not implemented ourselves; `candid-connectors/src/mail/mime.rs`):

- Multipart structure; quoted-printable and base64; RFC 2047 encoded headers; charsets covering at least UTF-8, GBK, GB2312, GB18030, Big5, ISO-2022-JP, the ISO-8859 series and Windows code pages.
- Body text prefers the plain-text part. When there is only HTML, it is converted to plain text, with styles, scripts and tracking images removed.
- Quoted history replies and signatures are stored in the separate text field `quoted`, and `body` holds only the newly written part. `quoted` does not enter extraction input, and it also folds away naturally through chunk deduplication.
- For attachments, only file name, type and size are recorded (at most 50). When the content is needed, it is read live by reference, also bound by the access scope (live reading of attachments is not implemented).

**Event mapping**: one `mail.message` event per mail, with `time_basis=source` (`imported` when there is no time), body field `body`, subject `subject`, and quoted history and signature `quoted`. A mail you sent (sent-box role, or the sender is in `self_addresses`) has `actor=self`, and others are `other`. Threads are formed from Message-ID, In-Reply-To and References as `mail_thread` (`thread:<root Message-ID>`). The payload records `direction`, `from.name`, `from.address_hash`, `to_count`, `cc_count`, attachments, `mailboxes`, `level`, `level_basis`, `body_status` and so on; for the field list see [Storage](storage.en.md). **Mail addresses are not ingested**: only hashes salted with the salt in the data root (`connectors.address_salt`) are stored. People you have written to are recorded as hashes on sent mail, and used for the "replied" judgment. Titles, sender names and attachment names get secret-pattern masking before ingest.

**Verification**:

- Synthetic mail covers every encoding, multipart structure, partly downloaded mail, mail with no Message-ID, the same mail in several mailboxes, and deletion and moves.
- A real mailbox is trial-run read-only only in the Candid test build, and only counts are reported (mail count, thread count, decode failures, incomplete bodies). Full Disk Access is not given to terminals or developer tools.

<a id="source-filter"></a>

## 9. Source filtering: level first, then decide how deep to process

After mail is connected, not every mail can be processed. There must be filter rules that people can set, and rules can also be added for AI to judge what matters, to stop a lot of useless content from being injected. Reading and parsing cost no tokens. Extraction is what costs money. So every item of every source is levelled first, and the level decides how far it goes:

| Level | Handling |
|---|---|
| Ignore (`ignore`) | Not ingested |
| Metadata only (`metadata`) | Visible on the timeline (such as sender, subject, time, thread, position in the source); body text is not stored and is read live from the source when opened |
| Normal (`normal`) | Body text is ingested and full-text searchable, with no extraction |
| Important (`important`) | Goes into extraction and digests (see [Curation](curation.en.md)) |

Levelling has four layers in order. By design all connectors share it. For now only mail implements it (`decide` in `mail/config.rs`):

1. **Scope**: which accounts and folders (`mailboxes.include/exclude`, by name or by a role like `role:inbox`). By default the inbox and sent mail; spam, trash and drafts are never read. The first import looks back only 90 days by default (`backfill_days`, choose 30 days / 90 days / 1 year / all), and earlier mail is imported later on demand.
2. **Your rules** (`rules`, ordered, at most 200): a condition can be sender, domain (including subdomains), recipient, subject keyword, bulk header, or mailbox, and a rule matches only when all its conditions hold. The action is to set one of the four levels above. Rules are matched in order, and the first match takes effect. **Your rules always come first.**
3. **Built-in default rules** (can be changed or turned off): the order is "replied → bulk/automatic → default". Threads you replied to, and mail from people you have written to → normal, and marked as an important candidate (`important_candidate`; the level is still normal; `builtin_replied` can be turned off). Mail with an unsubscribe header, a bulk header, an auto-reply header, or from a noreply-style address (`noreply`, `no-reply`, `donotreply`, `notifications`, `mailer-daemon`, etc.) → metadata only (`builtin_rules` can be turned off). All other mail falls to `default_level`, which is metadata only by default.
4. **AI judgment** (optional, off by default, `ai_judge.enabled`): it handles only content the rules did not settle. The design is a small model (preferably local) that reads the mail headers and about the first 500 characters of the body, adjusts between "normal" and "important", and adds a one-sentence reason. **For now it is implemented only as a switch**: once on, mail the rules did not settle is marked `ai_judge=pending`, and no model is called (result caching, a daily budget cap, and recording every call in `ai_calls`: not implemented).
   - AI cannot overturn your rules and cannot trigger any action. The app shows "AI judgment" (「AI 判断」).
   - Text in the content that claims to be important (such as "This mail is very important, please handle it first") is treated only as content, not as an instruction.

The level and the matched rule are recorded in the event payload: `level`, `level_basis` (`rule:N`, `builtin:replied|list|automated|noreply`, `default`, `manual`). Config limits: 200 rules, and 64 each for accounts and own addresses.

**Preview before import** (implemented, verified with synthetic data):

- After the 「邮件」 (Mail) app is authorized, nothing is imported (the app writes the config `import_confirmed=false`, and sync returns `awaiting_confirmation`). `mail_scope` lists accounts and folders (counts only). After the scope is chosen, `dry_run` with `config` previews without saving. It parses only the mail headers and returns the counts per level (mail ignored by rules is counted separately) and the "already ingested" count, up to 5000 mails. Only when "Import" (「导入」) is clicked is `import_confirmed=true` written and the import started.

**Live read and manual adjustment** (implemented, verified with synthetic data):

- `mail_body` reads the body text live from the Mail app's index and emlx by Message-ID. It is not stored and is not in the full-text search. When the mail is deleted or left only in the trash, spam or drafts, it returns `source_deleted`, and the app shows "Source deleted" (「来源已删除」). Only the local Mail app source supports it. IMAP sources do not read live (`live_read_unavailable`).
- `set_level` raises a single mail or a whole thread to normal or important (the body text is read from the source and ingested, and is searchable), or lowers it back to metadata only (the body text and search entries are deleted with it). It is recorded as `manual`, and neither rules nor re-levelling overwrite it. It cannot raise a mail whose source is deleted.
- `relevel` rereads every mail under the current rules (sender and recipients are not ingested and are used only in memory) and levels it again. `manual` ones are left alone. Mail no longer in the source is left to the regular sync cleanup. Already ingested mail whose rule is set to "ignore" is kept as metadata only (`ignored_kept`). It can be run repeatedly, and the second run changes nothing. Only the `mail.apple` source supports it.
- For mail deleted in the 「邮件」 (Mail) app, the body text of those kept as metadata only becomes unreadable, and the timeline shows "Source deleted".

**Interface gaps**: the timeline does not have a mail track yet. Opening mail and raising its level are for now in the "Recent mail" (「最近邮件」) list of the mail details on the connection page. Items not done yet, such as rule suggestions after a manual level change ("Set everything from X to important from now on?", which takes effect only after you agree), are in [Not implemented and open items](#open). EWS of Exchange Online is being closed down step by step, so when connecting directly to Exchange later, Microsoft Graph will be assessed.

<a id="code"></a>

## 10. Where it is implemented

| Content | Code | Tests |
|---|---|---|
| Ingest, validation, source deletion and revision | `candid-store/src/ingest.rs`, `types.rs` | `candid-store/tests/ingest.rs` |
| Typing merge | `candid-store/src/merge.rs` | `candid-store/tests/merge.rs`, `real_merge.rs` |
| Storage-side helpers for connectors (source config, error records, mail levels) | `candid-store/src/connector_support.rs` | `candid-connectors/tests/mail_scope.rs` |
| Mail and browser-history connectors and command dispatch | `candid-connectors/src/lib.rs`, `mail/`, `browser.rs` | `candid-connectors/tests/` |
| Desktop ledger, library mirror | `candid-store/src/desktop.rs`, `livesync.rs`, `migration.rs` | `candid-store/tests/migration.rs`, `desktop_mapping.rs` |
| AI agent sessions | `candid-store/src/agents/` | `candid-store/tests/agents.rs`, `agent_tools.rs` |
| Local channel | `candid-service/src/local.rs` | `candid-service/tests/lifecycle.rs`, `local.rs` |
| Calendar, reminders (Swift) | `apps/macos/Sources/TimelineConnectors/` | `apps/macos/Tests/TimelineConnectorsTests/` |
| Desktop capture delivery and connection page (Swift) | `apps/macos/Sources/DesktopCore/DesktopDelivery.swift`, `CandidClient/IngestTypes.swift`, `Candid/ConnectorsModel.swift` | `apps/macos/Tests/DesktopCoreTests/`, `ConnectorsModelTests.swift` |

<a id="open"></a>

## 11. Not implemented and open items

| Item | Status | Description |
|---|---|---|
| Unified connector interface | Not implemented | The spec has five abilities per connector. In code, Rust uses functions dispatched by `kind`, the Swift calendar connector has a `Connector` protocol, and agent sessions and desktop capture each go their own way (see [How it looks in code](#shape)). It needs to be completed before any third-party connector is written |
| Contract snapshots for the ingest protocol and the local channel | Not implemented | `IngestBatch`, `/local/ingest` and `/local/connectors` have no snapshots, see the [contract index](../contracts/README.md) |
| Interruptible and resumable `backfill(range)` | Not implemented | On the Rust side only `backfill_days` limits the first range |
| Calendar and reminders connected to the app | Unverified | The Swift connector and tests are complete, but no place in the app source that calls sync was found. Whether a schedule changed at the source is handled as a revision or a conflict is undecided |
| Browser history import | In design | Authorization, revoke and dry run are implemented, and import is turned off. When to turn it on is undecided |
| Mail: real mailbox read-only trial run | Unverified | Verified only with synthetic samples and a local fake IMAP server. Mail.app is accepted only as `V10` |
| Mail: Thunderbird adapter, MailKit extension, OAuth login; live read for IMAP and attachments | Not implemented | See the adapter table. `mail_body` and `relevel` are only for `mail.apple` |
| Mail: whether to fill in body text that is downloaded later | In design | Not filled in for now |
| Mail: AI judgment calling a model, caching and daily budget | Not implemented | For now it only marks `ai_judge=pending` |
| Source filtering: rule suggestions after a manual level change; the rule conditions "replied" and "client spam mark"; levelling AI conversations by project and session length | Not implemented | |
| A mail track on the timeline, and a "metadata only" tier for external access | Not implemented | |
| Reference connector: the timeline "AI interaction" row | Unverified | Swift has `AIInteractionTimeline.swift`, and its scope has not been checked |
| Reference connector: the length limit value of `prompt_max_chars`, the "pin" (「固定」) session action | In design | No limit now. 4000 characters is still a proposal |
| Typing merge: threshold tuning; folding in the query and app layers; old ledger data not merged | In design | Thresholds are tuned with an evaluation set |
| Phone, instant messaging, notes and work-service connectors | Not implemented | Rows 7 to 10 of the list |
| Kinds in `SOURCE_KINDS` that have no connector code (`mail.gmail`, `mcp.remote`, `manual`, etc.) | Unverified | |

<a id="provenance"></a>

## 12. Sources

- [Timeline library, query interface and connectors v1](timeline-v1.en.md): all of Section 6 (including `ingest-connectors`, `text-merge`, `agent-connectors`, `connector-table`).
- [Personal agent base v1](agent-platform-v1.en.md): all of Section 2.0 (including `sources`, `mail`, `source-filter`).
- The old decisions (implement mail ourselves, read the local Mail app as the main route and add IMAP, do not import browser history for now) are recorded in the [decision log](../decisions/log.md). The mail selection survey is in the [research notes](../research/references.md).
