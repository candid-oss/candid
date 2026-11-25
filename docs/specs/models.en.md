# Model Calls

[中文](models.md) · **English**

**Status**: Partly implemented. The core's model providers, usage accounting, two lanes, budgets and retries are implemented. Two things are not yet done: reducing roles to three, and keeping the platform app from calling models directly.

This document defines how the core calls models: model providers and roles, accounting for each call, the call list and dependency order, scheduling, budgets, failure and retry, and settings and management. It does not define the prompts and output of each call (see [Curated layer](curation.en.md) and [People and items](people-items.en.md)). It does not define Pi conversations and skills (see [Workflows](workflows.en.md)). It does not define query interfaces other than the local channel (see [Query interface](query.en.md)).

<a id="overview"></a>

## 1. Overview

<a id="rules"></a>

### 1.1 Rules

1. **There are only three model roles**: `vision` (screenshot excerpts), `summary` (activity blocks, extraction, person judgment), `agent_chat` (replies and Q&A). Do not add roles. If quality is not enough, change the model instead of adding a role. (Repository rule, see [AGENTS.md](../../AGENTS.md).)
2. **Only the core calls models.** When the platform app (the Mac App) needs a model, it asks the core to call it. The core handles keys, budgets, authorization and accounting.
3. **Every call writes one `ai_calls` row.** Prompt and reply text are not saved.
4. **Never switch to a remote model silently.** When a model is missing, authorization is missing, or the budget is exceeded, the job stays in a waiting state with the reason written down.
5. **Do not rerun models.** A background call checks the cache key first. If the input, prompt and model have not changed, it is not called again.
6. **No network requests the core does not know about.** The user configures model addresses in settings. Whether a request goes through the system proxy is decided by the network proxy setting (`proxy.rs`, see [Where it is implemented](#code); this is a network setting, not a model proxy).

<a id="lanes"></a>

### 1.2 Two lanes

Model calls in the core run on two lanes. They do not wait for each other, and each runs one call at a time (`Lane` in `core/crates/candid-service/src/workflows.rs`):

| Lane | Who uses it | Features |
|---|---|---|
| Interactive lane | `POST /local/models/complete`, `POST /local/compose` (「帮我写」, Help Me Write) | A person is waiting. It does not enter the job queue. No retry. It waits at most 5 seconds (50 ms × 100 tries) for the lane, and if it cannot get it, it returns "model busy" |
| Background lane | The only consumer of the job queue (`Worker`) | Takes jobs one at a time by priority. On failure it retries as described in section 6. The thread is set to system background QoS (`scheduling.rs`) |

Pi's model gateway `POST /local/pi/model` does not use either lane. It freezes the model and budget per "run" (section 7.2), does not enter the job queue, and records accounting with `agent_call_start` (`client = pi`). Only one run exists at a time.

<a id="providers"></a>

## 2. Model providers and models

<a id="ai-usage"></a>

### 2.1 Configuration and tables

One model configuration (version 1: `providers`, `models`, `roles`) is saved as a whole in `store_meta` as `models.config/1`. On save it is also written to the three tables `ai_providers`, `ai_models` and `ai_roles` (`model_command` in `candid-store/src/workflows.rs`). The configuration is what is read. The three tables mirror it, for queries and foreign keys. Checks on save:

- Only version 1 and known fields are accepted. `secret_ref` must be a keychain reference (it must not start with `sk-` and must be at most 80 characters).
- `base_url` must be `http(s)://` and must not contain `@`, `?` or `#`.
- `headers_json` must not contain credential headers such as `authorization`, `x-api-key` or `cookie`.
- When the configuration changes, queued and running `workflow.*` and `speech.recording` jobs are cancelled (`configuration_changed`).

The key is handed to the core only by the app, through `POST /local/models/authorize`, into core process memory (`workflows::authorize`). It is not written to disk. When the core sends a request, it gets the key by `secret_ref`.

| Table | Content | Notes |
|---|---|---|
| `ai_providers` | `provider_id`, name, `api`, `base_url`, `secret_ref`, `destination`, `enabled`, `headers_json` | `api` is one of `openai-chat`, `openai-responses`, `anthropic-messages`, `gemini`, `ollama`, `other`. `destination` is one of `on_device`, `self_hosted`, `cloud` |
| `ai_models` | The provider's model name, `modalities_json`, `cost_json` (price per million tokens, currency, source, date) | The table also has capability-probe columns such as `context_window` and `supports_tools`. The core does not write them on save (not implemented) |
| `ai_roles` | Mapping from role to model, `fallback_json`, `limits_json` | `limits_json` reads `max_input_tokens`, `max_output_tokens`, `daily_budget`, `timeout_ms`. The core does not read `fallback_json` (not implemented) |
| `ai_calls` | One row per call, see 2.3 | |

Interfaces supported for real calls: `openai-chat` and `ollama` (with `/v1` added) use `/chat/completions`. `anthropic-messages` uses `/v1/messages`. Any other `api` value returns `unsupported_model_api`. Pi currently connects only to `openai-chat` and `ollama`.

<a id="destination"></a>

### 2.2 Destination and authorization

- A provider with `destination` `on_device` must have a local loopback address (`127.0.0.1`, `localhost`, `[::1]`). Otherwise the job waits for authorization.
- A model service on the local loopback needs no per-source approval, whether it is marked on-device or self-hosted (it is the user's own choice).
- For a provider at any other address, each "data source × model provider" pair needs one explicit approval (`workflows.approval/1:<source>:<source id>`; the value is a hash of that source's configuration, and the approval is void if the configuration changes).
- Whether a feature may use a remote model is decided by the strictest of the feature setting `allow_remote` and the source authorization. A loopback address is not limited by `allow_remote` (`feature_check`).

<a id="calls"></a>

### 2.3 Usage accounting

`ai_calls` records every model call the core makes (fields are in `schema.rs`):

- `feature` is the feature id (the `id` in the catalog, such as `screen-excerpt` or `title`), `compose`, or the `agent_chat` / automation id of a Pi run. `client` is `app`, `pi` or `worker`.
- A row is written when the call starts, with status preset to `error`, `error_code = interrupted_unknown_charge` and `cost_source = unknown`. When the call ends it is rewritten as `ok`, `error`, `cancelled` or `timeout`. If the process dies midway, the row honestly remains as "may have been charged".
- At start, the model's current price is saved as a snapshot in `models.call-prices/1:<call_id>`. Later changes to the price table do not rewrite history.
- Cost: when the provider returns the actual charge, record `provider`. When it is estimated from the price table and token counts, record `estimated`. When neither exists, record `unknown`. **Never record zero**, and leave the currency empty.
- If, when the reply returns, the source has changed, permission has been revoked or the configuration has changed, the call is recorded as `cancelled` (`source_or_configuration_changed`) and the result is discarded.
- When a background call ends, only `input_tokens` and `output_tokens` are written back (the `cache_read_tokens` and `cache_write_tokens` columns exist, but the background path does not write them back; not implemented).
- Usage of external agents such as Codex and Claude is not ingested. When the `usage` tool is called with `include_external=true` and that agent is authorized for reading, the connector computes it from the source files and states the source.
- Old usage records from outside are imported idempotently by `call_id` through `import_usage` of `model_command`.

<a id="roles"></a>

## 3. Roles

Each of the three roles covers one kind of call:

| Role | Use |
|---|---|
| `vision` | Screenshot excerpts; manual screenshot text curation |
| `summary` | Activity block summaries, item extraction, person judgment, recording summaries, decisions and to-dos, daily review |
| `agent_chat` | 「帮我写」 (Help Me Write), Pi conversations and skills (writing the daily review, reply search, commitment follow-up, and so on) |

**A feature takes only its own role. A text feature, when its role has no model, borrows a text model configured for another role, and uses its own prompt.** (The core's lookup order, checked.)

| Call | Lookup order |
|---|---|
| Screenshot excerpt, manual screenshot curation | `vision` |
| Item extraction, person judgment | `summary` → `title` |
| Activity block summary | `episode` → `summary` → `title` |
| Recording summary, decisions and to-dos, daily review | `summary` → `title` → `vision` |
| Recording title | `title` → `summary` → `vision` |
| 「帮我写」 (Help Me Write) | `agent_chat` → `summary` → `title` |
| Pi | The role in the request, `agent_chat` by default; if missing, it waits |

Borrowing does not change role settings. Settings only need `vision`, `summary` and `agent_chat`. A local machine can efficiently run only one model at a time. It is advised to set `vision` and `summary` to the same multimodal model that can handle text, so the local service does not swap models back and forth (a setting suggestion, not a rule).

### 3.1 Current state that breaks the three-role rule

The code and the app still use the role names below, left over from earlier designs. They are not a precedent for adding roles. They are debt to be reduced (see [Not implemented and undecided](#open)):

| Role name | Where it is used | Current state |
|---|---|---|
| `title` | Recording titles, label drafts; fallback borrowing by other text features | Used by the core; should be merged into `summary` |
| `episode` | Activity block summary (`screen-episodes` in the catalog) | Used by the core; should be merged into `summary` |
| `asr`, `diarization` | Recording transcription (`quick` uses `asr`, `conversation` uses `diarization`). The request goes to the voice service's `/voice`, and the core validates the result | Used by the core. The voice service is an external service, not a "model role". It should become a separate voice service setting (in design) |
| `ocr`, `relevance`, `embedding` | Appear only in the app's role catalog | The core never calls them (for the app-side use of `ocr`, see 7.3). They should be removed |

<a id="dispatch"></a>

## 4. Call list

The table below follows `core/workflows/catalog.json` (the feature catalog, the single source of truth) and the module code. "Default limit" is the number of calls per day, and can be changed on the settings page. The three background features in the table are off by default (`default_enabled: false`) and do not allow remote models.

<a id="call-list"></a>

| Feature (catalog id / job type) | Role | Trigger | Input | Output | Default limit/day | Priority |
|---|---|---|---|---|---|---|
| 「帮我写」 (Help Me Write) (`/local/compose`) | `agent_chat` | Shortcut key | Current app, selected text, recent conversation text; no images (the planned "current frame excerpt" is not implemented) | A draft text | Not counted against a feature limit | Interactive |
| Pi conversation, manual skills | `agent_chat` | User, daily-review hook | Conversation and tool results | Conversation reply | Frozen per run, see 7.2 | Interactive |
| Screenshot excerpt (`screen-excerpt`, `screen.excerpt`) | `vision` | New content appears in a screenshot | Screenshot (scaled to WebP) plus window, URL and OCR reference; up to 3 chat frames merged | JSON: kind, title, messages, excerpt; up to 4096 tokens | 300 | P1 background |
| Item extraction (`item-extract`, `items.extract`) | `summary` | A chat conversation is idle for 10 minutes; a recording ends; key mail (unverified) | New messages (at most 40 each time, 20 messages of earlier context, at most 15 open items, each cut to 500 characters) | JSON, up to 2048 tokens, one retry on bad JSON | 150 | P1 background |
| Recording transcription (`transcribe`, `speech.recording`) | `asr` / `diarization` (voice service) | After a recording ends | The whole recording in 10-minute windows (1-minute overlap), one call per window with checkpoints saved | Timestamped transcript and speakers; the core validates the source hash and producer | 100 | P2 background |
| Recording title, summary, decisions and to-dos, tags, daily review (`title`, `summary`, `decisions`, `tags`, `daily`, `workflow.*`) | See section 3 | Title: automatic after transcription; summary: after a recording ends (turned on in settings); the rest are manual | Transcript text (rejected if over 32000 bytes) | A draft, written to the derived layer | 100 each | P2 background |
| Screenshot text curation (`screenshot`, `workflow.screenshot`) | `vision` | Manual | One image (rejected if over 16 MiB) | A Markdown draft | 100 | P2 background |
| Person judgment (`people-judge`, `people.judge`) | `summary` | A new alias appears that has candidate pairs | At most 10 pairs per batch, at most 5 batches queued at once; at most 4 messages per side, 80 characters each | JSON, up to 2048 tokens, one retry on bad JSON | 50 | P3 background |
| Activity block summary (`screen-episodes`, `screen.summarize`) | `episode` → `summary` | An activity block ends | Excerpts in the block and foreground app statistics | JSON: title, summary, citations | 60 | P3 background |

A placeholder among background features: `hourly-weekly` (hourly/weekly digests) is marked `implemented: false` (not implemented) in the catalog.

Cache keys (if equal, the call is not made again; the job's unique key is `(kind, input_ref, config_version)`):

| Call | Cache key includes |
|---|---|
| Item extraction | The set of input message ids, the snapshot of open items, prompt version and text, model |
| Person judgment | The alias sets of both sides, prompt version and text, model. A pair answered "not sure" is not asked again for 30 days. The same pair is asked at most 3 times |
| Activity block summary | Input hash, prompt version and text, model |
| Screenshot excerpt | The job version includes prompt version and text, model configuration, feature setting. Queued only once per screenshot id (image-hash key unverified) |
| Recording calls | Model configuration, transcript text, hash of the content revision |

A prompt can be overridden by a file with the same name under `prompts/` in the library root. Otherwise the built-in version in `core/prompts/` is used. Default prompts for recording calls are in `core/workflows/prompts.json`.

<a id="order"></a>

## 5. Dependency order

```
OCR → key frame → screenshot excerpt
mail / recording transcript / AI session / AX excerpt (projection, no model) ─┴→ conversation, message, excerpt
  → rule-based person resolution (no model) → item extraction → item / memo
  → person judgment (async, does not block extraction; after a merge the item's counterpart follows through aliases)
  → activity block summary (waits until excerpts of that period are done; unverified)
```

For one conversation, its extraction is not triggered until its excerpt jobs are cleared (checked: the extraction scan skips conversations that still have excerpt jobs).

<a id="scheduling"></a>

## 6. Scheduling, budgets, failure and retry

<a id="scheduling-queue"></a>

### 6.1 Scheduling

- The background lane is the only consumer of the job queue. One work loop takes due jobs by "priority, then creation time", pre-checks them one by one, and claims and runs the first one that can run. Screenshot excerpts are ordered by screenshot capture time, so chat messages are appended in order.
- Priority table (`candid-store/src/job_priority.rs`, one row per job type): P1 `screen.excerpt`, `items.extract`; P2 `speech.recording`, `workflow.*`; P3 `screen.summarize`, `people.judge`. A job type not in the table is not taken by the background lane. Interactive calls (P0) do not enter the queue.
- The work loop does not spin. It reads the queue only when the version counter of jobs, workflow configuration or data changes, when authorization changes, or when the next due time arrives. There is no fixed period: the background thread is woken by ingest, job, power and settings changes (`Core::wake_background`); the legacy mirroring pass has a 10-minute fallback.
- The scans that enqueue jobs (recording, curated-layer sync, people, screenshot excerpts, items, activity blocks, automations) run before the queue is read, in the same loop round.
- Claiming uses a lease (180 seconds) and an attempt number. After a crash, expired tasks are claimed again in the next round. When a result is submitted, the attempt number, cancellation, source revision, permission and configuration hash are checked, so a late result cannot overwrite a newer one. Completed voice blocks have checkpoints, so a restart does not charge them again.
- Job states: `queued`, `running`, `done`, `failed`, `cancelled`. A waiting condition is not a separate state. It is `queued` plus an `error_code` (see 6.4). The same reason is logged only once and not written to the database again.

<a id="power"></a>

### 6.2 When to spend power

Any computation the user is not waiting for is background work: it runs only when spending power is allowed, runs once per input, never wakes without an event, and every run is accounted for (design and measurements: [power v1](../plans/power-v1.md), in Chinese).

- **One switch, Background Processing** (top of the AI settings; `power.pace/1` in `store_meta`): Anytime (`anytime`; still waits when the Mac is hot, and acts like Power Saver in Low Power Mode), Power Saver (`saver`, the default: on battery no on-device model runs, and work resumes when plugged in), Manual (`manual`: automatic jobs are still queued but all wait until Process Now). There are no battery percentages, plugged-in durations or batches, and no per-feature power options.
- **The platform reports, the core keeps**: the app reports through `POST /local/power` at launch, on every change (power, thermal state, Low Power Mode, wake) and whenever it reconnects to the core. A report holds until the next one, with no clock-based expiry. Before any report the state is unknown, and unknown counts as battery. Power events in the timeline stay evidence; the gate never reads them.
- **One decision**: every background job kind registers its cost in `JOB_CLASSES` in `job_priority.rs`: heavy (`Heavy`, on-device model), light (`Light`, remote model or no model), by model (`ByModel`, decided by the role's provider). `feature_check` asks `power_allows` as its fifth condition; while the gate is closed the queue does not pick held heavy jobs. The app's own background work (`app.ocr`, `app.vision`) is registered in the same table and asks the core before each automatic run. A kind that appears in code without being registered fails the `every_job_kind_is_registered` test.
- **Waiting reasons**: `waiting_power`, `waiting_thermal` and `waiting_manual`, shown like the waiting reasons in 6.4.
- **Process Now** marks a waiting job as manual and queues it. It passes only the power condition, never authorization, sources or daily limits. A transcription the user asks for is also a manual run.
- **Accounting**: each job records CPU milliseconds (core thread time plus the voice service's own `cpu_seconds`), wall milliseconds and the power state it ran on (`jobs.cpu_ms`, `wall_ms`, `power_state`). The `energy_summary` tool gives runs, CPU seconds, wall seconds and battery CPU seconds by day and job kind, plus what is waiting and why. The settings line and the hourly `core.log` line come from it; counts only.
- **No polling**: `candid-service/tests/no_polling.rs` scans core and app sources for sleeps in loops and repeating timers; anything not in `core/fixtures/polling/allowlist.txt` (each line with its reason) fails.

The background lane yielding while an interactive call runs: not implemented. The two lanes currently do not know about each other.

<a id="budget"></a>

### 6.3 Budget

| Item | Rule | Where it is stored |
|---|---|---|
| Daily call limit per feature | One for each catalog feature, changeable on the settings page (0 to 10000). Local and cloud calls both count, and failed calls count too. The day boundary is the UTC day. A job over the limit waits with `waiting_feature_budget`, and the remaining jobs of the same kind are skipped this round | `default_daily_calls` in `catalog.json`; `features.settings/1` in `store_meta`; `feature_budget()` counts `ai_calls` |
| Input limit per call | Role `max_input_tokens`. Over the limit is rejected as `input_limit` (the core compares by approximate request bytes) | `limits_json` |
| Output limit per call | Each step carries its own `max_tokens` (default 1024, at most 4096). Role `max_output_tokens` can only lower it | `limits_json` |
| Timeout | `timeout_ms`, default 120000, limited to 50 to 120000 milliseconds | `limits_json` |
| Daily money limit | Role `daily_budget`: if the cost already recorded for the same model that day, plus the reservation for this call (input bytes × input price + output limit × output price), exceeds the limit, the job waits with `waiting_budget`. If the price is missing, an existing call's cost is unknown, or currencies differ, it waits with `waiting_budget_cost_unknown` and does not treat it as zero. The money for voice and image input cannot be reserved, so if a money limit is set, they wait | `limits_json`; prices in `cost_json` |
| Each Pi run | Four items: model calls, tool calls, output tokens, request bytes. Frozen at start | See 7.2 |

The plan's "a separate global daily money limit for cloud, default 0" is not implemented. Today there is only the per-role `daily_budget`, and with none set there is no money limit. A feature's `allow_remote` defaults to `false` (`agent_chat` follows the user's configuration).

Handling after a limit is exceeded: a screenshot excerpt stays in the queue waiting for the next day's quota, and is voided (`expired`) after more than 24 hours in the queue. Item extraction, recording calls, person judgment and activity blocks do not expire by time. The job stays in the queue and runs again after the quota recovers (the next item extraction merges a batch that covers more messages).

<a id="retry"></a>

### 6.4 Failure and retry

| Case | Handling |
|---|---|
| Timeout (`timeout`), model offline (`offline`) | A background job backs off 30 seconds, 2 minutes, 10 minutes and tries again, for at most 4 attempts (3 retries), then `failed`. The reason is on the job and visible in the interface. Interactive calls are not retried |
| Other HTTP errors, transport errors | No retry, the job is `failed`. For an activity block, a deterministic refusal such as 400/401/403/404/422 blocks that configuration, to avoid charging again for every queued block (`waiting_model_request_rejected`) |
| A step that needs JSON (item extraction, person judgment) gets a reply with no JSON object | Ask once more, with the bad reply and "output only JSON that matches the structure" (`json_repair`). The second reply is judged by the step's own parser |
| Model missing or source not enabled | Wait with `waiting_model` |
| Key not handed to the core, source not approved | Wait with `waiting_authorization` |
| Over the feature limit / over the money limit | Wait with `waiting_feature_budget`, `waiting_budget`, `waiting_budget_cost_unknown` |
| Feature off, source not in the allowed range, remote not allowed | `feature_disabled`, `waiting_feature_source`, `waiting_feature_remote`. Turning it on again restores only the jobs cancelled by that switch |
| When the result returns, the source has changed, permission is revoked, or the configuration or feature setting has changed | Discard the result, record the call as `cancelled` |
| At pre-check, the input no longer holds (the source is gone, the needed transcript is missing, and so on) | Void it (`cancelled`, with a reason), no retry |

Never switch to a remote model silently. A timed-out or interrupted attempt does not fall back to another model. During retries, the `ai_calls` rows already written are kept honestly, one row per attempt.

<a id="ai-settings"></a>

## 7. Settings and management

<a id="settings-where"></a>

### 7.1 Where settings are

| Place | What it manages |
|---|---|
| Settings → Models | Providers, models, role assignment, fallback, budget, network proxy |
| Settings → AI features | One row per catalog feature: switch, which sources it reads, whether it may use a remote model, daily call limit, last run and result |
| Each source on the connections page | Whether this source can be read by AI, and whether it can be sent to a remote model |

The save format of "AI features" (`features.settings/1`, validated by the core): each feature has exactly four fields, `enabled`, `sources`, `allow_remote` and `daily_calls`. `sources` takes `import`, `mac_microphone`, `mac_system_audio`, `mac_region`. Screenshot excerpts, item extraction, person judgment and activity block summaries can also take `*` or a specific source id. Turning a feature off cancels its queued and running jobs (`feature_disabled`).

Rules:

- The sources a feature can read are the intersection of the feature setting and the source authorization. Whether it can use a remote model is decided by the strictest of the feature setting, the source authorization and the role's destination.
- Saving the configuration also cancels the affected pending jobs. Late replies are rejected by the lease and revision checks.
- Every AI output shows who produced it (rule, model or Pi), which records it is based on, and when. Each can be edited, confirmed or rejected. See [Curated layer](curation.en.md). For how the AI page presents conversation, schedule and skills, see [Workflows](workflows.en.md).

<a id="pi-gateway"></a>

### 7.2 Pi's model gateway and run budget

The app starts a run through `POST /local/pi/start`. The core resolves the model for the role, checks that the provider is enabled, the interface is supported and remote use is allowed, freezes the scope and budget, registers the dedicated client `candid-pi`, and returns a token. The Pi extension calls models only through `POST /local/pi/model`. The model, token limits and keys all belong to the core. Replies are not streamed.

| Budget item | Default | Range |
|---|---|---|
| `max_model_calls` | 12 | 1–200 |
| `max_tool_calls` | 40 | 1–500 |
| `max_output_tokens` | 16000 | 256–400000 |
| `max_request_bytes` | 900000 | 10000–1000000 |

When one is used up, the run is stopped (`budget_exhausted`). Only one run exists at a time (`agent_busy`). The budget of an automatic skill run (automation) is frozen from the automation configuration, and its sources and remote authorization are checked.

<a id="app-side"></a>

### 7.3 Model calls on the platform app

The rule is that the platform app does not call models directly. The current state differs (the first two items in Not implemented and undecided): the app's `AIClient` calls the provider directly for cloud speech transcription. The app also keeps its own copy of the model configuration and usage log, and syncs it into the core with `import_usage`. The other path, where the core calls through `POST /local/models/complete` on the app's behalf, is the target form.

<a id="code"></a>

## 8. Where it is implemented

| Content | Code | Tests |
|---|---|---|
| Model configuration, save checks, pre-checks, usage accounting, budget | `core/crates/candid-store/src/workflows.rs` | `core/crates/candid-store/tests/workflows.rs` |
| Feature settings, feature budget, source and remote checks | `core/crates/candid-store/src/feature_settings.rs` | Same as above |
| Job priority and expiry | `core/crates/candid-store/src/job_priority.rs`, `performance.rs` | `candid-store/tests/job_scheduling.rs`, `performance.rs` |
| Background lane, interactive lane, HTTP calls, checkpoints | `core/crates/candid-service/src/workflows.rs` | `candid-service/tests/workflows.rs`, `lifecycle.rs` |
| Retry and JSON repair | `core/crates/candid-service/src/retry.rs` | Unit tests in the same file |
| Background QoS | `core/crates/candid-service/src/scheduling.rs` | Unit tests in the same file |
| Local channel `/local/models…`, `/local/compose` | `core/crates/candid-service/src/local.rs` | `candid-service/tests/workflows.rs` |
| Pi runs and model gateway | `core/crates/candid-service/src/pi.rs` | `agents/pi-extension/test/e2e.mjs` |
| Network proxy setting (system or direct, not a model proxy) | `core/crates/candid-service/src/proxy.rs` | `candid-service/tests/proxy.rs` |
| Budget and sources of automatic skill runs | `core/crates/candid-store/src/automation/` | Unverified |
| Preparation and reply validation of each call | `screen_excerpt.rs`, `item_extract.rs`, `people/judge.rs`, `screen_pipeline.rs`, `compose.rs` in `candid-store` | `lifecycle.rs` (the model is a loopback stub, input and output are snapshotted) |
| Feature catalog, prompts, output schema | `core/workflows/catalog.json`, `prompts.json`, `outputs.schema.json`; `core/prompts/` | Snapshots in `core/fixtures/` |
| Table structure | `candid-store/src/schema.rs`, `schema_upgrade.rs` | `cargo test --workspace` |

<a id="open"></a>

## 9. Not implemented and undecided

| Item | Status | Notes |
|---|---|---|
| Reduce roles to three | Not implemented | The core and the app still use `title`, `episode`, `asr`, `diarization`, and the app catalog also has `ocr`, `relevance`, `embedding`. See 3.1 |
| The Mac App calls the provider directly from Swift for cloud speech transcription | Not implemented | It bypasses the core and does not enter `ai_calls`. This conflicts with "only the core calls models". See [Architecture](../architecture.md#debts) |
| The Swift side keeps its own model configuration and usage log, then syncs into the core | Not implemented | There should be only one copy, in the core. See [Architecture](../architecture.md#debts) |
| Background yields to interactive calls | Not implemented | The two lanes do not know about each other; the power gate is in 6.2 |
| Global daily money limit for cloud (default 0) | Not implemented | Today there is only the per-role `daily_budget` |
| 「帮我写」 (Help Me Write) "current frame excerpt" and image input for screen Q&A | Not implemented | 「帮我写」 currently reads text context only |
| Hourly / weekly digests (`hourly-weekly`) | Not implemented | Catalog placeholder |
| `ai_models` capability-probe columns, `fallback_json` | Not implemented | The columns exist, but the core neither writes nor reads them. Design constraint: `fallback_json` may fall back only to a provider with the same `destination` or a more local one, and `limits_json` includes per-call input and output token limits and the daily budget (old text, unverified) |
| Writing back cache tokens for background calls | Not implemented | The columns exist |
| Model statistics cards, small images, and extra capability-inference tests | Not implemented | Old documents say they were not added |
| Key mail triggering item extraction | Unverified | The trigger is written in the catalog, not checked item by item |
| Activity block summary waits until excerpts of that period are done | Unverified | |
| Sample comparison for each kind of call | Partly implemented | `candid-lab` and the `lifecycle` snapshots can compare changes in prompts or models. Whether the `excerpt_replay` subcommand from the old document is kept is unverified |
| Correct estimates and limits with one week of measured data | In design | Adjust by the actual counts in `ai_calls` |
| Old design for cancellation and concurrency: cancel is persisted first, then the external model request is cancelled; when the provider cannot cancel, the record may still be charged and late output is not published; a network failure is not assumed to be uncharged, and the provider's request status is checked first; resource concurrency has a unified limit and capture saving takes priority; two recordings with the same content do not merge idempotency keys | Unverified | From old Agent Platform v1 §4. The core already has `interrupted_unknown_charge` and discards late results (sections 5 and 6). The rest is not checked item by item |
| Automatic skills are off by default until the context evaluation threshold is met | Unverified | From the old feature catalog notes |

<a id="sources"></a>

## 10. Sources

- [Timeline v1 §4.9 "Model providers, roles and usage"](timeline-v1.en.md#ai-usage): section 2 and 3.1 (the table and accounting parts).
- [Data Layer v1 plan §4 "Model call planning"](../plans/data-layer-v1.md): section 4 (call list), section 5 (dependency order), sections 1.2 and 6.1 (scheduling), 6.3 (budget), 6.4 (failure and retry). The verification in §4.6 is folded into section 9.
- [Agent Platform v1 §4 "Scheduling, cost and recovery"](agent-platform-v1.en.md): section 6 and 2.3 (idempotency, lease, reservation and settlement, cancellation).
- [Agent Platform v1 §4.2 "Settings and management"](agent-platform-v1.en.md#ai-settings): section 7.1.
