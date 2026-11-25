# Query Interface and Access Control

[中文](query.md) · **English**

**Status**: Partly implemented. The 22 external tools, the four access methods, scope filtering, audit and proposals are implemented. A separate `raw` grant for the curated layer, time-limited media URLs and proposal expiry are not implemented.

This spec defines how external AI and the app read the timeline: the tool list, shared rules, access methods and authentication, clients and scope, audit (access_log), and proposals.
It does not define how the data the tools read is produced (see [Curated Layer](curation.en.md) and [Storage](storage.en.md)). It does not define who runs an approved proposal or the workflows (see [Workflows](workflows.en.md)). It does not define connectors (see [Connectors](connectors.en.md)).
Tool parameters and descriptions follow the snapshots and are not copied here: [`core/fixtures/contract/mcp-tools.json`](../../core/fixtures/contract/mcp-tools.json), [`mcp-initialize.json`](../../core/fixtures/contract/mcp-initialize.json), [`candid-tools.txt`](../../core/fixtures/contract/candid-tools.txt).

<a id="overview"></a>

## 1. Overview

The timeline service exposes one set of **tools** (22). Tool names and parameters are identical across the four access methods. Each call first resolves the client, then filters in SQL by the client's scope, then ranks and cuts snippets, and finally writes one audit record.

| Method | Who uses it | See |
|---|---|---|
| Loopback HTTP `POST /v1/<tool>` | External programs; the backend of the command line and the MCP shell | [Section 5](#access) |
| MCP (`candid mcp`, stdio) | External agents such as Claude Code and Codex | [Section 5](#access) |
| Command line `candid <tool>` | People and scripts | [Section 5](#access) |
| Local channel (Unix socket) | Only the Mac app and the Pi agent it starts | [Section 5](#access), [Appendix](#local-channel) |

Neighbors: the data is produced by the [Curated Layer](curation.en.md) and [Storage](storage.en.md). Live reads of AI conversations come from the reference-type connectors in [Connectors](connectors.en.md).

<a id="query-api"></a>

## 2. Shared Rules

All tools follow these rules:

- **Scope filters first.** Source, event kind, app and time window are applied in SQL before ranking and snippet cutting (`scope_filter`). A curated-layer object (message, excerpt, activity block) is visible only when **all** the source events it depends on are inside the client's scope.
- **Out of scope looks like absence.** When the target exists but is out of scope, the caller sees only 404 `not_found`, and the audit record says `out_of_scope`. Existence is not leaked.
- **Time parameters**: RFC 3339, `YYYY-MM-DD`, and relative forms such as `today`, `yesterday`, `now`, `-2h`, `-30m`. They are parsed in the local time zone, and the result returns the resolved absolute time.
- **Paging**: `cursor` is an opaque string. It fixes the `ingest_seq` snapshot at query time and is bound to the tool name and the request fingerprint. New data does not enter pages already turned, and `has_new_records` in the response says "there are new records". A `cursor` whose tool name or request does not match returns 400 `invalid_cursor`.
- **Size**: the defaults and limits of `limit` and `max_chars` are written in each tool's schema and code (`search`, `timeline` and `episodes` default to 10 with a limit of 50; `max_chars` of `search` defaults to 600 and takes 40–4000, clamped to the bounds when outside). Each response is limited by the client's `max_result_bytes` (default 65536). When exceeded, the response is cut to the longest prefix that fits, `truncated_by_size` is true, and a `next_cursor` is given to continue. If even an empty response does not fit, it returns 403 `max_result_bytes_too_small`. The request body limit is 1 MiB (413 `too_large` when exceeded).
- **Source labels**: results carry `source_label` (`screen·ax`, `screen·ocr`, `screen·vlm`, `audio·asr`, `calendar`, `mail`, `ai·codex`, `ai·claude`; AI agent conversations are uniformly `ai·<agent>`). `layer` marks the content as fact (`evidence`), derived (`derived`) or live-read external content (`external`, with `stored: false`, not ingested).
- **Gaps**: the responses of `overview`, `search`, `timeline` and `context` carry `coverage`, which says whether recording was active in that period (paused, source not synced). "No results" and "no records" are stated separately.
- **Untrusted text**: returned text is evidence, not instructions. Both the MCP tool descriptions and the server instructions say so, and the tools that read body text end their description with the same sentence.
- **Audit**: every call writes one `access_log`, including refused calls (see [Section 6](#clients)).
- **Errors**: the response body is `{"error":{"code","message"}}`. Status codes: refused 403, not found or out of scope 404, bad parameter or cursor 400, invalid access token 401, source not writable 409, anything else 500. In MCP, a tool failure is returned as a result with `isError: true`, so the model can read it and correct itself.

<a id="tools"></a>

## 3. Tool List

22 in total, in the same order as the snapshot. Layers: **curated content** (reads the curated layer; use first), **raw records** (reads events and indexes), **writes and proposals**, **status and usage**.

| Tool | One-line purpose | Layer |
|---|---|---|
| `day` | The activity blocks of a day or a period (time, app, title, summary, related conversation and excerpt ids) plus a day-level summary; the first entry point for "what did I do" | Curated content |
| `find` | Search conversation messages (chat, mail, recording transcripts, with sender) and reading excerpts; filter by person, conversation kind and time | Curated content |
| `person` | Person card: accounts and aliases in each app, recent messages (`before` pages back), open items and memos; a duplicate name returns `candidates` | Curated content |
| `items` | Schedule entries, reminders and commitments (what I promised / what I wait for others to do); open ones only by default | Curated content |
| `conversations` | List conversations (`chat`, `mail`, `meeting`, `agent`), newest first | Curated content |
| `conversation` | The messages of one conversation in time order; the latest 30 by default, `before` pages back | Curated content |
| `evidence` | The raw records a message or excerpt is based on; use only to check the source | Curated content (drill-down) |
| `source_query` | Check authorized sources by actual message time, or resolve a memo reference; paging and incomplete state must be kept | Curated content |
| `records` | Read checked record revisions; source, body and derived permissions are checked item by item, and a revision becomes invalid after deletion or revocation | Curated content |
| `agent` | Live-read authorized Codex, Claude Code and pi sessions without saving the reply; `action` is `describe`, `detect`, `sessions`, `read`, `search` or `usage` | Raw records (live read) |
| `search` | Search screen text, screenshot recognition, recording transcripts and titles by keyword and filters; results carry snippet highlights | Raw records |
| `overview` | Overview of a period: foreground time and switch counts per app, episodes, schedule and recordings, gaps; the entry point for broad questions | Raw records |
| `timeline` | List events or episodes in time order (`level=event` is the raw event stream and is noisy) | Raw records |
| `context` | What was going on before and after a moment (events, focus chunks, concurrent recordings and schedule) | Raw records |
| `get_event` | The full content of one event: chunks ranked by attention, its thread, neighboring events, and the episodes that cite it | Raw records |
| `episodes` | The list of curated episodes | Raw records |
| `episode` | The content and evidence of one episode | Raw records |
| `thread` | The content of one recording, conversation or mail thread in order | Raw records |
| `media` | Fetch a screenshot or recording blob (no access by default; needs the client's `media` scope) | Raw records |
| `status` | Whether this library and each source are healthy and recording; call it first when unsure the data is complete | Status and usage |
| `usage` | Number of AI calls, tokens and cost inside Candid (actual and estimated values are kept apart); with `include_external`, computes external agent usage live | Status and usage |
| `propose` | Submit a proposal; it only enters the pending list (the only write tool, MCP annotation `readOnlyHint: false`) | Writes and proposals |

Key points (behavior; details follow the snapshots and tests):

- `status` returns `store` (version, library size, event count, `ingest_seq`), `sources[]` (`last_sync` is the latest successful check; it advances even with no new data and does not advance on failure; `lag_seconds` is computed from it; `last_ingest` is the latest write of new data; when `status` is `error`, `last_error` gives a bounded error code), `capture` (whether recording happened in the last 10 minutes) and `agent` (whether workflows are available).
- The app time in `overview` is foreground time (seconds) and switch counts (coming back within 5 minutes counts as the same switch). `events` is the number of observation records, not the number of uses.
- `media` currently returns only base64 (when the size exceeds three quarters of half of `max_result_bytes`, it gives metadata only), and checks the blob hash before reading.
- Routing of `agent`: `search` queries and merges an agent live only when that agent's "include in search" is on and the client's `scope.agents` contains it. `get_event` and `thread` accept `agent:` references and read live when "read" is on. Live-read results are marked `layer: "external"` and `stored: false`, and enter neither the library nor the search index; `access_log` is still written. `overview` and `timeline` can include `ai.prompt` and `ai.work` that were ingested under authorization.
- Curated-layer tools do not return the full text of raw events by default. Use `evidence` to check the source and `get_event` to expand.
- The `attention` of `search` is `focus` (default; prefers the chunks the user was looking at) or `all`. `before` and `after` of `context` are in minutes, 5 each by default. The answer of `overview` is kept to about 300 tokens.
- The structure of one `search` hit (`SearchHit`, `core/crates/candid-store/src/query/types.rs`; `highlights` are character ranges `[start, end)` inside `snippet.text`; `ref` can be passed to `get_event` to expand; the example is synthetic data):

```json
{
  "event_id": "01933c2a-7f5e-7c8a-9a1b-2f6d0c1e4b55",
  "kind": "screen.text",
  "start": "2026-11-10T10:24:48+08:00",
  "app": {"bundle_id": "example.notes", "name": "Notes"},
  "title": "weekly overview",
  "source_label": "screen·ax",
  "layer": "evidence",
  "snippet": {"text": "…refine the performance model, what is the bar…", "highlights": [[10, 21]], "truncated": true},
  "attention": {"visible": true, "focus_distance": 2, "is_new": true},
  "ref": {"event_id": "01933c2a-…", "field": "body", "chunk_ord": 3, "char_range": [120, 168]}
}
```

<a id="call-order"></a>

## 4. Recommended Call Order

Written into the MCP server instructions (`instructions` of `initialize`) and each tool description, and consistent with Pi's `pv_*` tools:

1. For "what is there to do, what is scheduled" use `items`. For "what did I do that day" start with `day`. For who I talked to and what was said, use `conversations` and `conversation`.
2. To find specific content, start with `find`. If the curated layer has nothing, use `search`, then expand the raw records with `context` or `get_event`.
3. For app time and gaps, use `overview`.
4. The `event` level of `timeline` is the raw event stream and is not the entry point for "what did I do that day". Call `media` only when you really need to see the screen or hear the sound.
5. State the time and source in the answer. "No results" and "no records" are different; check `coverage` in the result.

<a id="access"></a>

## 5. Access Methods and Authentication

| Method | Address or command | Authentication |
|---|---|---|
| Loopback HTTP | `http://127.0.0.1:<port>/v1/<tool>`; accepts `POST` only and binds 127.0.0.1 only. The port is written to `endpoint.json` in the run directory (Mac: `~/Library/Application Support/Candid/run/`), not in the data root | One Bearer token per client. `Host` must be the local loopback address plus this port (otherwise 403 `bad_host`). Any browser request carrying an `Origin` header gets 403 `browser_origin` |
| MCP | `candid mcp` (stdio, newline-delimited JSON-RPC 2.0; protocol versions `2025-06-18`, `2025-03-26`, `2024-11-05`; line limit 4 MiB). The shell holds no data and forwards `tools/call` to loopback HTTP | Each MCP client is registered separately in `clients` with its own token and scope |
| Command line | `candid <tool> [--param value]` or `--json '{...}'`; `candid tools` lists the tools | The current user's local token (`cli.token` in the run directory, 0600); its scope matches the "local command line" client, which the core manages automatically and which cannot be revoked |
| Local channel | A Unix domain socket in the run directory (file 0600, directory 0700), HTTP/1.1, one request per connection (except change push) | Only the current user can connect. The app is an internal client (`transport='internal'`, no token). A request that carries a token is handled as the client of that token |

Rules:

- Paths such as `/local/…` (admin, change push, health check, ingest) and `/health` are always refused on loopback HTTP (403 `local_only`), whatever the token. The full list is in the [Appendix](#local-channel).
- `/v1/<tool>` on loopback HTTP and on the local channel runs the same code (`run_tool`), so scope filtering and audit are the same. Loopback HTTP accepts only the 22 external tools. The app's own `app_*` read tools and the agent write tools are not in the external catalog; calling them on the local channel with a token returns 403 `internal_only`.
- A token is shown only once at registration, and only its SHA-256 is stored. After a client is revoked its token fails at once (401), and a call by a known client after revocation writes `client_revoked`.
- Clients are registered in Settings → "External Access" (`POST /local/clients`): one token per tool, with the Claude Code command, the Codex config and generic JSON given together. The default scope at registration is in [Section 6](#clients).
- The environment variables `CANDID_TOKEN`, `CANDID_ENDPOINT` and `CANDID_RUN_DIR` override the token, address and run directory. The command line and the MCP shell connect according to the app's "network proxy" setting (system proxy by default, with a no-proxy option), and `CANDID_PROXY` can override it.
- Sending and receiving on a connection has an overall limit of 30 seconds, and unfinished requests are dropped. Request headers over 16 KiB or a request body over 1 MiB are refused.

<a id="clients"></a>

## 6. Clients, Scope and Audit

<a id="schema"></a>

### 6.1 Tables

The columns of the three tables (full definitions in `core/crates/candid-store/src/schema.rs`):

| Table | Columns |
|---|---|
| `clients` | `client_id`, `name` (such as "Claude Desktop", "Codex"), `transport` (`mcp`/`http`/`cli`/`internal`), `token_hash` (the token itself is shown once), `scope_json`, `created_ms`, `revoked_ms` |
| `access_log` | `log_id`, `at_ms`, `client_id`, `tool`, `params_json` (keywords can be omitted per the `store_meta` setting), `result_count`, `result_bytes`, `duration_ms`, `denied` (refusal reason code; empty on success); index `access_log_time(at_ms)` |
| `proposals` | `proposal_id`, `client_id`, `kind`, `payload_json`, `evidence_json` (default `[]`), `status` (`pending`/`approved`/`rejected`/`executed`/`failed`/`expired`), `created_ms`, `decided_ms`, `executed_ms`, `result_json` |

<a id="scope"></a>

### 6.2 Scope (`scope_json`)

The server filters **before** ranking and snippet cutting:

```jsonc
{
  "version": 1,
  "sources": ["*"],               // allowed source_id, or "*"
  "kinds": ["*"],
  "apps": {"allow": ["*"], "deny": ["com.apple.keychainaccess"]},
  "time": {"from": null, "to": null, "max_days_back": 90},
  "text": true,                   // whether body text is available; when false, metadata only
  "media": false,                 // whether screenshots and audio can be fetched
  "derived": true,                // whether episodes and memories can be read
  "agents": [],                   // agent connectors (agent_id list) readable through this client, empty by default; still bound by the user's grant for that connector
  "propose": true,                // whether proposals can be submitted
  "propose_kinds": [],            // proposal kinds that can be submitted; empty means no limit (the default; all older registrations are like this)
  "writes": [],                   // write levels inside this library: annotate, reference, commitment, draft; empty means read-only (the default). See Section 8
  "max_result_bytes": 65536
}
```

- The default scope used by the registration endpoint (`POST /local/clients`): the last 90 days (`max_days_back`), body text and derived content readable, screenshots and recordings not readable, **cannot submit proposals** (`propose: false`), and `agents` containing only the agents ticked at registration. The struct default `propose: true` is used only for tests and internal clients.
- A source is also bound by its own `agent_visible`: a source with `agent_visible=false` is hidden from external clients unless the scope lists it explicitly by source_id. The app (an internal client) is not bound by this. A revoked source is invisible to all clients.
- The `agent.transcript` source (AI conversations) additionally requires that the user's "read" grant for that agent is valid and that the client's `agents` contains that agent.

<a id="audit"></a>

### 6.3 Audit

- Every tool call writes one `access_log`: tool, parameters, result count and bytes, duration, and the refusal reason code. On success `denied` is empty.
- Refusal reason codes are stable error codes, for example `client_revoked`, `out_of_scope`, `text_scope`, `source_scope`, `kind_scope`, `app_scope`, `agent_scope`, `media_not_permitted`, `propose_not_permitted`, `propose_kind_not_permitted`, `evidence_out_of_scope`, `max_result_bytes_too_small`, `tool_not_permitted`.
- `access_log_params` in `store_meta` is `full` (default) or `redacted_q`. The latter records the keyword `q` in the parameters as `<redacted>`.
- When Pi calls a tool it has no right to use, the core adds a `tool_not_permitted` record with `audit_refusal`.
- A request with an invalid token or an unknown client is refused with 401 before the client is resolved. It has no `client_id` to attribute to, so no `access_log` is written.
- The client list in settings shows each client's last use time, calls in the last 7 days and refused calls (`GET /local/clients`).

<a id="proposals"></a>

## 7. Proposals

An external agent cannot directly change anything outside the app (create a reminder, a calendar event, and so on). It can only submit a proposal with `propose`, which runs only after the user approves it.

- `propose` needs scope `propose: true`, and `propose_kinds` must be empty or contain the `kind`. `kind` is 1–64 characters and `payload` must be a JSON object. Every entry of `evidence[]` (event ids, deduplicated, at most 100) must be inside the client's scope, otherwise 403 `evidence_out_of_scope`.
- On success it returns `proposal_id` and `status: "pending"` and adds one row to the `proposals` table. `propose` itself runs no action.
- `status` values: `pending`, `approved`, `rejected`, `executed`, `failed`, `expired`. Transitions are decided by the app through `POST /local/agenda` (`action: "proposal"`): `pending → approved → executed | failed`, `failed → approved` (retry), `pending | approved | failed → rejected`. Repeating the same decision returns the original proposal. Any request with a token is refused (403 `internal_only`), so external agents and Pi cannot approve their own proposals.
- The kinds the app can run (`AGENDA_EXECUTABLE_KINDS`): `create_reminder`, `update_reminder`, `complete_reminder`, `delete_reminder`, `create_calendar_event`, `update_calendar_event`, `delete_calendar_event`. Other kinds stay `pending`, waiting for another executor.
- The core also creates proposals itself (confirming a commitment candidate, or confirming an item writes a calendar or reminder proposal, with the app as client). The rules for items and commitments are in [People and Items](people-items.en.md).
- `expired` exists in the table constraint, but no code implements expiry (not implemented).

<a id="agent-tools"></a>

## 8. Local Agent Tools

The six tools below are not in the external catalog (not in the MCP list; loopback HTTP returns 404). Only the local channel provides them, for the app and the attached Pi: `memories` (read derived memories), `annotate` (change an episode title or memo), `mark_reference` (mark as a reference candidate), `commitment` (confirm or ignore a commitment candidate), `save_draft` (save a draft), `undo_write` (undo a write).

- Each write needs the matching level in the client's `scope.writes` (`annotate`, `reference`, `commitment`, `draft`). The records referenced are checked one by one to be in scope. Each write carries a "who wrote it" mark (client, run, model) and can be undone with `undo_write`. Like queries, writes produce an `access_log`.
- Writes stay inside this library. Anything to be done outside the app still goes through `propose`.
- Pi can call only these six tools, `propose` and 18 read tools (`PI_READ_TOOLS`), and is bound by a per-run tool-call budget; going over returns 429 `budget_exhausted`.
- The product behavior of these tools is in [Workflows](workflows.en.md).

<a id="local-channel"></a>

## 9. Appendix: Local Channel Endpoints

The local channel (`/local/…`, plus `/health` and `/v1/…`) is served only on the Unix socket. The list is compiled from `core/crates/candid-service/src/local.rs`, `pi.rs` and `agenda.rs`, and **has no snapshot guard yet** (see [Section 11](#open)). Unless stated otherwise, a request with a token returns 403 `internal_only` (external clients cannot use them).

| Path | Method | One-line purpose |
|---|---|---|
| `/v1/<22 tools>` | POST | Query tools; no token means the app, a token means that client |
| `/v1/app_*` (10: `app_person_correct`, `app_people_candidates`, `app_activities`, `app_activity`, `app_media`, `app_recognition`, `app_pending_recognition`, `app_ocr`, `app_retention_count`, `app_day_counts`) | POST | Read and write tools for the app's own use; internal clients only |
| `/v1/<agent write tools>` (6, see Section 8) | POST | Agent write tools; the app uses no token, Pi uses its own run token |
| `/health` | GET | Process, library and endpoint identity |
| `/local/status` | GET | Sync phase, event count, recent changes, issues, last check |
| `/local/events` | GET | Change push: newline-delimited JSON (`hello`, `status`, `data`, `clients`); the connection stays until one side closes; at most 8 subscribers |
| `/local/clients` | GET | Registered clients and recent use |
| `/local/clients` | POST | Register a client; returns a one-time token and the Claude Code, Codex and generic JSON configs |
| `/local/clients/revoke` | POST | Revoke a client (the "local command line" cannot be revoked, 409) |
| `/local/sync` | POST | Trigger one sync now (202) |
| `/local/verify` | POST | Check the desktop ledger against the timeline |
| `/local/sources` | GET / POST | List sources / register by kind and name (reuse if it exists) |
| `/local/ingest` | POST | Submit one `IngestBatch` (see [Connectors](connectors.en.md)); a desktop batch requires the direct-write cutover to be done (otherwise 409 `cutover_pending`) |
| `/local/desktop/cutover` | POST | Import the rest of the desktop ledger and switch to direct writes |
| `/local/desktop/legacy-preferences` | GET | Read the capture settings kept in the ledger |
| `/local/desktop/delete` | POST | Delete by activity or record id (tombstone) |
| `/local/desktop/retention` | POST | Delete desktop records before `before_ms` |
| `/local/desktop/free-space` | POST | Drop the oldest screenshot blobs to a target byte count |
| `/local/agents` | GET | Grant settings and stored counts of AI session sources |
| `/local/agents/consent` | POST | Save an agent's grant (can delete stored content at the same time) |
| `/local/agents/query` | POST | Live-read AI sessions as the app (same as the `agent` tool) |
| `/local/connectors` | GET / POST | Connector management (list, grant, update, revoke, sync, secrets, levels) |
| `/local/media` | POST | Media management commands (app) |
| `/local/jobs` | POST | Job management commands (app) |
| `/local/records` | POST | Record management; with a token only `action=query` is allowed |
| `/local/models` | POST | Model setting commands |
| `/local/models/authorize` | POST | Authorize one model call |
| `/local/models/complete` | POST | Run one model call |
| `/local/workflows` | POST | Workflow commands |
| `/local/compose` | POST | Generate a reply draft from the user's history |
| `/local/automation` | POST | Skill and automation management commands |
| `/local/screen` | POST | Screen-related commands |
| `/local/voiceprints` | POST | Voiceprint management commands |
| `/local/agenda` | POST | The "Today" column: decisions on pending proposals, commitment candidates, items and memos (see [Section 7](#proposals)) |
| `/local/pi/start`, `/local/pi/stop`, `/local/pi/idle` | POST | Start, stop and idle a Pi run |
| `/local/pi/run` | GET | Current run state |
| `/local/pi/model` | POST | Pi's model gateway: one chat completion with the model frozen for the run; accepts only Pi's run token |

There are 33 `/local/…` paths (including `/local/events`), plus the three groups `/health` and `/v1/…`. Known gap: the command-type endpoints (`/local/models`, `/local/workflows`, `/local/automation`, `/local/screen`, `/local/media`, `/local/jobs`, `/local/records`, `/local/voiceprints`, `/local/connectors`) dispatch by `action`, and the action lists are in their own code and not listed here.

<a id="code"></a>

## 10. Implementation

| Content | Code | Tests |
|---|---|---|
| Tool catalog, descriptions and schema | `core/crates/candid-service/src/catalog.rs` | `candid-service/tests/mcp_contract.rs`; snapshots `core/fixtures/contract/` |
| Dispatch by name | `core/crates/candid-store/src/query/dispatch.rs` | `candid-service/tests/server.rs` |
| Scope filtering, audit, cursor, size trimming | `core/crates/candid-store/src/query/core.rs` | `candid-store/tests/query.rs` |
| Curated-layer tools `day`, `find`, `person`, `items`, `conversations`, `conversation`, `evidence` | `query/day.rs`, `curated.rs`, `people.rs`, `items.rs` | `candid-service/tests/lifecycle.rs`; snapshots `core/fixtures/lifecycle/snapshots/mcp/` |
| Raw-record tools and `propose`, `media`, `usage`, `status` | `query/tools.rs`, `search.rs` | `candid-store/tests/query.rs` |
| `source_query`, `records` | `query/sources.rs`, `core/crates/candid-store/src/durable.rs` | `candid-service/tests/record_annotations.rs` (coverage unverified) |
| `agent` live read | `core/crates/candid-store/src/agents/access.rs`, `query/agent_tools.rs` (agent write tools) | `candid-store/tests/agents.rs`, `agent_tools.rs` |
| Proposal transitions | `query/agenda.rs`, `candid-service/src/agenda.rs` | `candid-store/tests/agenda.rs`, `candid-service/tests/agenda.rs` |
| Loopback HTTP, authentication | `candid-service/src/http.rs`, `router.rs` | `candid-service/tests/server.rs`, `tests/local.rs` |
| MCP shell | `candid-service/src/mcp.rs` | `tests/mcp_contract.rs`, `tests/server.rs` |
| Local channel | `candid-service/src/local.rs`, `pi.rs` | `tests/local.rs`, `tests/pi.rs` |
| Command line | `core/crates/candid-cli/src/main.rs` | `candid-cli/tests/contract.rs`, `cli.rs` |
| Run directory, token, endpoint file | `candid-service/src/endpoint.rs`, `serve.rs` | `tests/server.rs` |

<a id="open"></a>

## 11. Not Implemented and Open Items

| Item | Status | Notes |
|---|---|---|
| Snapshot guard for the `/local/` endpoint list | Not implemented | Only the 22 external tools have snapshots (`mcp-tools.json`, `candid-tools.txt`). The local channel paths, methods and the `action` of each command have no written contract, and Section 9 is compiled by hand from the code. A path-list snapshot and a routing test should be added |
| A separate `raw` grant for the curated layer | Not implemented | The design once said "only the curated layer is open by default; the raw layer needs a separate `raw` grant, and `evidence` needs it". `ClientScope` has no `raw` switch; `evidence` is bound by `text`, `derived` and the source scope like the other tools |
| Raw event IDs not exposed directly to AI | Not implemented | The design once said curated-layer objects carry only their own `id` and a source note. Today a message's `ref` is the id of the underlying event, and `evidence` returns event summaries |
| Demote the old raw tools to a capability under `evidence` | Not implemented | `search`, `timeline`, `context`, `get_event` and others are still in the external catalog; their priority is lowered only by the server instructions and the call order |
| `media` time-limited local URL, audio `range` parameter | Not implemented | Only base64 is returned today; add these before opening media access |
| Proposal `expired` status | Not implemented | The table constraint has it; no expiry logic |
| Registration path for `http` transport clients | Unverified | The table constraint allows `http`, but the registration endpoint creates only `mcp`; no path that creates an `http` client was found |
| Audit and test coverage of `records` | Unverified | `records` writes its own `access_log` in `durable.rs` (not through `audited`, no duration recorded); the tests that cover it were not checked one by one |
| "Interface extension" of `status` | In design | The old text said "processing status (whether workflows are available; interface extension to be done)"; it is now `agent.available/detail` |

Each item above has a matching entry in the [To-do list](../roadmap.md).

<a id="sources"></a>

## 12. Sources

- [Timeline Library, Query Interface and Connectors v1](timeline-v1.en.md): 4.10 (clients, audit and proposals), 5 (query interface, including 5.1–5.4).
- [AI v1 Plan](../plans/ai-v1-plan.md): in "AI readable layer", the section "Interface (MCP and Pi use the same set)" (the new generation of tools `find`, `conversations`, `conversation`, `day`, `person`, `items`, `evidence`).
- [Agent Platform v1](agent-platform-v1.en.md): 4.1 write levels (`scope.writes`).
