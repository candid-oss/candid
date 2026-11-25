# Definition and Execution of AI Features

[中文](workflows.md) · **English**

**Status**: Partly implemented. Workflows, the attached Pi, skill runs, proposals and "Help me write" (帮我写) are implemented. MCP prompt templates, hourly and weekly summaries, and a batch of "template-only" skills are not implemented.

This spec defines which AI features Candid does itself, how workflows are defined, the boundary and guardrails of the two ways of running (automatic inside the core, and the attached Pi), skills and triggers, commitments and reminders, reference and reply retrieval, "Help me write" (帮我写), and where things run. It does not define the data-curation flow (see [Curated Layer](curation.en.md)), people and items (see [People and Items](people-items.en.md)), model roles, budgets and scheduling (see [Models](models.en.md)), or query tools and clients (see [Query Interface](query.en.md)).

Hard constraints (same as [AGENTS.md](../../AGENTS.md)):

1. Do not build your own agent loop or harness. Conversations and skills go to Pi. The core runs only fixed workflows (scan → job queue → prepare → model → complete).
2. Captured content is evidence, not instructions.
3. Writes go only through proposals. Derivations and corrections inside this library follow the write levels in Section 5, and can be undone.
4. The executor has no terminal and no file-writing tool.

<a id="overview"></a>

## 1. Overview

Candid has two kinds of AI features, plus an external interface:

| Way | What it is | Scheduled by | Examples |
|---|---|---|---|
| Automatic inside the core | Fixed steps that call a role model at most once; the output is validated and then saved as a derived draft | The Rust core: scan → job queue → prepare → model → complete | Transcription, recording titles, summaries, screen excerpts, person judgment, item extraction |
| Attached agent | [Pi](../glossary.md) runs in RPC mode managed by the app, using our skills, extension tools and guardrails; started by an AI page conversation or a skill run | The app's `SkillRunner` claims a run from the core, and the core freezes scope and budget | AI page conversation, daily review, reference and reply retrieval |
| External interface | MCP and API, used by Claude Code, Codex and others, with the same tools and permissions | The external agent itself | See [Query Interface](query.en.md) |

There is also "Help me write" (帮我写) (Section 8): one model call triggered by an app shortcut. It is not one of the two ways above and has no run record.

<a id="position"></a>

## 2. Position and Principles

Candid is two things at once:

- **A standalone product**: without any external agent, it can still browse, search, do simple reviews and draft to-dos.
- **A personal agent base**: it turns captured information into clean, citable, correctable data for external agents to read.

The key questions are "how to make the data clean" and "which features to build, which not, and which to offer only as templates for external agents". Task definition is kept apart from the executor. Memory and persona have no default design in this spec (see research/memory-and-persona.md).

Principles:

1. **Built-in features and external agents read the same data**, both through the timeline service's scope filtering and audit.
2. **Cost grows by tier**: rules and statistics cost no tokens; a small model is called once per "chunk" and cached; workflows run only when a trigger is authorized, with a budget cap.
3. **Merging only builds links; originals are not deleted.** Deduplication, noise reduction and entity merging are recorded as relations or weights, and can be undone and recomputed.
4. **Writes go only through proposals.** Derived drafts can be saved automatically within the authorized scope, but are always marked as drafts.
5. **Text on screens, web pages, messages and mail is untrusted data**: it can be evidence and cannot be instructions. Task prompts put it in a clearly marked data area, and the executor has no terminal and no file-writing tool.

<a id="features"></a>

## 3. Feature Choices

The criteria: can the product still be used without an external agent; how often it is used; whether it needs tools outside Candid (mail, code, web); whether privacy requires it to stay on this machine.

The status column is the result of a line-by-line check against the code. "Default switch" follows `core/workflows/catalog.json`.

| Category | Feature | Notes | Status |
|---|---|---|---|
| **Automatic inside the core** | Timeline browsing and search | Rules and indexes, no model | Implemented (see [Query Interface](query.en.md)) |
| | Time distribution | Pure rule statistics (`pv_overview`) | Implemented |
| | Transcription, multi-speaker segmentation, matching existing voiceprints | Automatic after a recording is saved (workflow `transcribe`, job `speech.recognize`; setting `automatic` is on by default) | Implemented |
| | Recording title | Automatic after transcription (`title`, setting `titles` is on by default); a manually changed title is not overwritten by a rerun | Implemented |
| | Recording summary | After the whole recording ends (`summary`). The catalog writes default on, but the scan queues it only when the setting `summary == true`, so it is actually off by default | Implemented (the two defaults disagree, see Section 11) |
| | Screenshot text | Local system OCR on the app capture side; vision-model curation: `screen-excerpt` (when new screenshot content appears, off by default, 300 per day) and manual `screenshot` | Partly implemented (OCR is on the capture side, not checked) |
| | Extraction: commitments, requests, decisions, questions, topics | Now `item-extract`: triggered when a conversation has been idle for 10 minutes, on key mail, and after a recording ends (off by default, 150 per day); plus manual `decisions` and `tags`. The old wording "once per ended episode" no longer holds | Implemented |
| | Screen episode curation | `screen-episodes`, after an episode ends (off by default, 60 per day) | Implemented |
| | Commitment candidate → reminder proposal | Section 6 | Implemented |
| | Speaker and person merging | `people-judge`, when a new alias appears (off by default, 50 per day), see [People and Items](people-items.en.md) | Implemented |
| | Hourly and weekly summaries | `hourly-weekly`; the catalog registers `implemented: false` | Not implemented |
| **Attached agent** | Converse in the app and operate on content | Find, organize, change titles, mark references; writes inside this library can be undone, and actions outside the app go through proposals | Implemented |
| | Daily review | Skill `daily-review`; scheduled hook, saved as a day-level draft; push is off by default | Implemented (automatic triggering is limited by the quality gate, Section 5.4) |
| | Reference and reply retrieval | Skill `references-replies`; returns original text with sources and a reply draft, and does not send for you | Implemented |
| | Ask about your day (Q&A with citations) | AI page conversation; external AI uses the same tools through the external interface | Implemented |
| | Meeting review, project history, commitment tracking | Skills `meeting-review`, `project-history`, `commitment-tracker`, all manual | Implemented |
| **External interface (MCP/API)** | People and project pages | Based on people and items, see [People and Items](people-items.en.md) | Implemented (`person` tool; project page unverified) |
| | Proposal approval | For the app only; any request with a token is refused (Section 6) | Implemented |
| | Usage, privacy settings | The `usage` tool exists; the external control surface for privacy settings | Partly implemented (privacy settings unverified) |
| **Template-only, left to external agents** | Standup and weekly report, project handover, workflow discovery and automation suggestions, skill capture, deep research, writing with context, meeting preparation, bookkeeping follow-up | They need other tools or personal formats, or involve heavy reasoning or large personal differences. The repository has no such templates | Not implemented |
| | MCP prompt templates (the prompts primitive) | The goal is that an external agent attached to `candid mcp` can list and call them; the code has no prompts implementation | Not implemented |
| **Not built** | A general chat assistant | Not competing with general AI products | — |
| | Directly running external actions (send messages, change the calendar) | Only proposals are made, and the app runs them after you approve | — |
| | An agent with a terminal or code writing | The attached Pi turns off its own tools and has only our extension tools | — |
| | Monitoring-style metrics such as focus scores | Product position | — |
| | Calling the vision model on every screenshot by default | Cost; `screen-excerpt` is off by default, and when on it triggers on "new content", not on every screenshot | — |
| | Video export | Unrelated to the base | — |
| **Deferred** | Browser history import, direct Exchange | See [Connectors](connectors.en.md) | — |
| **To research** | Memory and persona profile, "what you care about", when to interrupt proactively | See research/memory-and-persona.md; the daily review push is a decided exception | In design |

<a id="tasks"></a>

## 4. The Minimal Workflow Framework

Only fixed steps are provided, not a loop where the model picks tools. Features are switched on as needed. Sorting, grouping, deduplication and statistics prefer rules. New models are not downloaded automatically and do not change authorized destinations.

The workflow catalog is in `core/workflows/` and is loaded with the release (compiled into the core, `include_str!`):

| File | Content |
|---|---|
| `catalog.json` (`candid-workflows/1`) | `workflows`: versioned workflow definitions; `features`: the feature catalog, registering core features and skills; the settings page, the AI page and the external interface all read it |
| `prompts.json` (`candid-prompts/1`) | Prompt templates for workflows: `title`, `summary`, `decisions`, `tags`, `screenshot`, `daily` |
| `outputs.schema.json` (`candid-workflow-output/1`) | Output types: `draft/1` (`text` 1–16000 characters, `source_revision`, `producer`, `prompt_version`, `job_id`), `candid-voice-result/1`, and others |

The run logic stays in the store and service modules, not in the catalog. User switches, role mapping and budgets are separate configuration (`workflows.settings/1` in `store_meta`, and feature settings); they do not change the released definitions. The external AI's `agents/skills/` is maintained separately and is not an internal directory of executable scripts.

The loop: pick saved inputs → follow the versioned fixed steps → rules or a role model → validate structure and sources → save a new revision → update indexes and processing state. In code this maps to `workflow_scan` (queue) → `workflow_prepare` (assemble an immutable request; the caller releases the database lock before sending HTTP) → model call → `workflow_reply_allowed` (check configuration, source revision and lease) → `workflow_complete` (draft and job are written in one transaction, and a replay is written only once).

**Features in the catalog** (`features`, 17 in total; `job_kind` is the kind in the job queue):

| id | Way | Trigger | Role (catalog value) | Job kind | Default |
|---|---|---|---|---|---|
| `transcribe` | Core | After a recording is saved | diarization | `speech.recognize` | On, 100 per day |
| `title` | Core | After transcription is done | title | `workflow.title` | On, 100 per day |
| `summary` | Core | After the whole recording ends | summary | `workflow.summary` | On in the catalog; the scan actually requires setting `summary == true` |
| `decisions` | Core | Manual | summary | `workflow.decisions` | On |
| `tags` | Core | Manual | title | `workflow.tags` | On |
| `screenshot` | Core | Manual | vision | `workflow.screenshot` | On |
| `daily` | Core | Manual | summary | `workflow.daily` | On |
| `screen-episodes` | Core | After an episode ends | episode | `screen.summarize` | Off, 60 per day, remote not allowed |
| `screen-excerpt` | Core | When a screenshot has new content | vision | `screen.excerpt` | Off, 300 per day, remote not allowed |
| `people-judge` | Core | When a new alias appears | summary | `people.judge` | Off, 50 per day, remote not allowed |
| `item-extract` | Core | Conversation idle for 10 minutes, key mail, recording ends | summary | `items.extract` | Off, 150 per day, remote not allowed |
| `hourly-weekly` | Core | Scheduled | summary | — | Not implemented |
| `daily-review`, `references-replies`, `commitment-tracker`, `meeting-review`, `project-history` | Agent | See Section 5.3 | agent_chat | `agent.skill` | See Section 5.3 |

`workflows` also has six versioned definitions: `recording` (transcribe → title → optional summary, triggered by `sealed_audio`), `conversation`, `decisions`, `tags`, `screenshot` and `daily`, each declaring its output type. The role names in the catalog (asr, diarization, title, summary, vision, episode) do not match the repository rule "use only `vision`, `summary`, `agent_chat`". The authoritative definition of roles is in [Models](models.en.md); see Section 11.

The first workflow wired end to end is "continuous recording → transcription → title → optional summary": transcription can run incrementally once a segment is saved, and finishes when the whole recording ends; multi-speaker segmentation and existing voiceprint matching are added by switch, and unknown identities stay anonymous; a usable title is generated from the time first, and the model title is a draft; if the summary is off or fails, the original audio, transcript and playback are not blocked. Adding a workflow needs only a definition, an output validator, a prompt template and a synthetic acceptance sample. When the existing model adapters are enough, no new scheduler, storage or UI state machine is added.

Scheduling, cost, idempotence and recovery (queue, lease, budget, cancel, `ai_calls`) are in [Models](models.en.md).

<a id="tasks-provenance"></a>

### 4.1 Sources, Permissions and Revisions

- Events record what happened, derived records explain it, and human corrections record the user's choices; the original text is not overwritten. Samples and character ranges use half-open intervals.
- Model output must have a locatable source. A citation out of range, a missing source or a wrong type is refused for publication (`workflow_complete` checks `source_revision`, and a mismatch returns `source_changed`).
- The effective permission is the intersection of the input sources. It is checked when choosing inputs, when sending to the model, when committing and on external queries. A source revision removes the old derived record from the default index and schedules a new revision. Deletion or revocation cancels related queued jobs, rejects late results, and invalidates derived records, indexes and caches. A rollback cannot restore text whose permission was revoked or that was deleted.
- External AI is read-only by default. Authorized internal draft writes do not grant external write permission.
- Which sources a feature can read is the intersection of the feature setting and the source grant. Whether it can use a remote model is the strictest of the feature setting, the source grant and the role destination. A model service on a local loopback address needs no per-source approval, whether it is marked local or self-hosted.
- Old revisions of derived results: keep the latest one, the ones you confirmed or changed, and the ones cited by a proposal; other old drafts are cleaned up after 30 days (unverified).

<a id="skills-triggers"></a>

## 5. Two Ways: Automatic in the Core and the Attached Agent

The AI page stays in the app, where you can talk to the AI and operate on content. We do not build the agent ourselves: we attach an external Pi, with our own interface, extension and strict guardrails, and we do not use pi-agent-core to build a loop.

<a id="pi-attach"></a>

### 5.1 How Pi Is Attached

- Version: `@earendil-works/pi-coding-agent` 1.5.2, locked in `agents/pi-extension/package-lock.json`. Node and Pi are not bundled with the app: they are installed on demand in "Settings → Components". If the system already has Node 22.19+ it is used directly; otherwise the official Node 24.22.2 is downloaded and checked against a fixed SHA-256, and Pi is installed from the lock file. When Pi is upgraded, the parameters and isolation below must be checked again.
- It runs in RPC mode (`--mode rpc`, a JSON-lines protocol over standard input and output) as a background process managed by the app. The chat interface is drawn by the AI page itself.
- Start parameters (`apps/macos/Sources/Candid/PiAgent.swift`; `agents/pi-extension/test/e2e.mjs` uses the same set): `--no-builtin-tools --no-extensions -e <extension> --tools 'pv_*' --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline --system-prompt <system prompt> --provider candid --model agent_chat`. `--no-extensions` also turns off the built-in extensions; an explicit `-e` still loads.
- Isolation: the environment gets only `HOME` and `PI_CODING_AGENT_DIR` (pointing to `pi/` under the app data directory), `PI_OFFLINE`, `PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY=0`, and the core address and token for this run. It does not inherit the user's environment. The working directory is an empty directory, and project trust is always refused.
- Model: the extension registers one provider, `candid` (OpenAI compatible), pointing to a random port and random path the extension opens on local loopback. Each request is forwarded to the core's `POST /local/pi/model`. The core sends it with the frozen `agent_chat` model and the key from the keychain, and records it in `ai_calls` (`client = pi`). Neither Pi nor the model gets the provider key. The core does not stream upstream (`stream: false`), and the extension replays the whole answer to Pi once as SSE, so text does not appear character by character. Only OpenAI-compatible (`openai-chat`) and Ollama interfaces are connected now. A cloud model needs remote use to be allowed for this run.
- Each run is started by the app with `POST /local/pi/start` and freezes: the model (the role defaults to `agent_chat`), the scope (`sources`, `agents`, maximum days back, default 90), the write levels `writes`, the proposal kinds `propose_kinds` and the budget. Budget defaults and limits: model calls 12 (1–200), tool calls 40 (1–500), output tokens 16000 (256–400000), bytes per request 900000. The core issues a token for the dedicated client `candid-pi`; after `POST /local/pi/stop` the token is invalid. Going over budget stops the run, and a crash and restart does not reset the budget. Only one run is allowed at a time (otherwise `agent_busy`).
- A conversation run can refresh permission and model scope, but must bring back the bounded history of the current conversation and the local time. When freezing a run the app attaches the mail capability state (not connected, not authorized, not in scope): it says plainly how to restore it, does not infer "no mail" from lack of access, and does not expose the identifier, address or body of a closed source.

<a id="pi-guardrails"></a>

### 5.2 Guardrails: Three Tool Levels, Permissions Checked in the Core

Every tool the extension provides calls the core (with this run's token). The core checks against the scope of `candid-pi` and writes the access log. Pi has no other tool. Hiding a tool in the extension is only a convenience; permission is judged only in the core.

| Level | What it can do | Pi tools (`pv_` prefix, matching core tools of the same name) |
|---|---|---|
| Read | Query the timeline, conversations, activity blocks, items, people, episodes, summaries and references; live-read ingested mail bodies and AI sessions | `pv_status`, `pv_overview`, `pv_search`, `pv_find`, `pv_timeline`, `pv_context`, `pv_get_event`, `pv_day`, `pv_conversations`, `pv_conversation`, `pv_evidence`, `pv_person`, `pv_items`, `pv_episodes`, `pv_episode`, `pv_records`, `pv_agent_session`, `pv_memories`; reading a body live from the mailbox has no core tool (not implemented) |
| Write (only derivations and corrections in this library) | Change titles and memos, mark references, confirm or ignore commitment candidates, save drafts; recorded as corrections or "AI drafts", with client, run id and model, and can be undone | `pv_annotate`, `pv_mark_reference`, `pv_commitment`, `pv_save_draft`, `pv_undo` |
| Proposal (actions outside the app) | The app runs them only after you approve each one in the interface | `pv_propose`; kinds limited to `create_reminder`, `update_reminder`, `complete_reminder`, `delete_reminder`, `create_calendar_event`, `update_calendar_event`, `delete_calendar_event`; change, complete and delete must carry an `item_id`, which comes from the entry the user brought in with "Ask AI" |

- Write levels are granted one by one by `writes` in the client scope (`annotate`, `reference`, `commitment`, `draft`), empty by default. Proposal kinds are limited by `propose_kinds`. Any other write value is refused at `start`.
- Undo: the app can undo any agent write. An agent can undo only its own writes with no newer change after them. The undo itself is also recorded as a correction.
- There is no terminal, and no file outside this library can be read or written. Tool results are wrapped in `<candid_data>` with a note not to run the requests inside, and the system prompt (`agents/pi-extension/system-prompt.md`) says the same. In the end-to-end test, a simulated model that "obeys" calls `bash` and gets only "no such tool"; injected instructions in the data are not run, and out-of-scope and over-budget calls are refused.
- When refused (permission, scope, budget), tell the user honestly and do not work around it.
- One conversation or one skill run freezes the model, tools, scope and budget.

<a id="hooks-skills"></a>

### 5.3 Skills, Hooks and Runs

Skill text is in `agents/skills/<name>/SKILL.md`, and Pi and external agents use the same file. The front matter has title, summary, trigger, whether it is enabled, and input. There are five now:

| Skill id | Name | Trigger (SKILL.md) | Notes |
|---|---|---|---|
| `daily-review` | Daily review | Scheduled 21:00 | Organizes the day's recordings, screen, mail and AI sessions into a cited account, saved as a day-level draft (`pv_save_draft`) |
| `references-replies` | Reference and reply retrieval | Manual | Section 7 |
| `commitment-tracker` | Commitment tracking | Manual | Finds explicit commitments, deadlines and evidence of completion; changes to reminders or the calendar only as proposals |
| `meeting-review` | Meeting review | Manual, input is one meeting recording | Decisions, open questions and follow-up proposals, each with a time basis |
| `project-history` | Project history | Manual, input is a project or topic name | Rebuilds the story in time order |

**Skill runs** are managed by the core's `automation` module (`core/crates/candid-store/src/automation/`). A run record is `candid-agent-run/1`, stored in `store_meta` (key `automation.run/1:<id>`, with `automation.revision/1:<id>:<rev>` kept on each change), and it occupies one `agent.skill` job. The flow:

1. Trigger and queue: `run` of `POST /local/automation` (app only) (manual; may carry `references`, `context` ≤16000 characters, and `executor` as `foreground` or `background`); or `automation_scan` queues it when a trigger is met. The idempotency key contains the skill, the trigger key and the effective config fingerprint; the same key does not create a run twice.
2. The app's `SkillRunner` claims with `claim`, keeps alive with `heartbeat` and submits with `finish`. Foreground conversation has priority, and background runs wait until the conversation is idle.
3. After the claim, the core freezes the run's scope and budget with `automation_pi_config` and calls `/local/pi/start` (with `automation_id` and the attempt number). The write levels of a skill run are fixed to `draft`, `reference`, `commitment`, `annotate`, and **the proposal kinds are empty**: a skill run cannot propose.
4. Each tool call is recorded as a step by `automation_audit_tool`, with the events it cited (source and revision fingerprint).
5. States: `queued`, `running`, `done`, `failed`, `cancelled`; `cancel` and `retry` (only failed, cancelled) are available. The run record's sources, model, steps, result and interruption state are stored with versions.

Each skill has one policy `SkillPolicy` (`automation.config/1`, saved by the `config` action):

| Field | Default | Value range |
|---|---|---|
| `enabled` | On | |
| `automatic` (automatic trigger) | Off; only `daily-review` counts as on when the quality gate passes and the user has not changed it | |
| `trigger` | `manual` (`daily` for `daily-review`) | `manual`, `daily`, `recording_sealed`, `episode_ended` |
| `time` / `timezone` | `21:00` / `Asia/Singapore` | `%H:%M`; a valid time zone name |
| `notify` | Off | Send a system notification when done; nothing is pushed when there is nothing to report |
| `sources` | `*` | List of source ids; intersected with source grants, excluding revoked sources and sources AI is not allowed to read |
| `role` | `agent_chat` | `agent_chat`, `summary`, `episode`, `title` |
| `allow_remote` | Off | Remote also needs the source listed in the global `remote_sources` (`*` not allowed); loopback addresses are not limited |
| `daily_runs` | 3 | 0–100; going over returns `daily_run_budget` |
| `max_model_calls` / `max_tool_calls` / `max_output_tokens` | 12 / 40 / 16000 | 1–200 / 1–500 / 256–400000 |

A run record freezes the config fingerprint, the model config fingerprint, the source-policy snapshot and the cited revisions. If the config, model, source policy or a cited event changes, `automation_validate` refuses; queued and running jobs are cancelled (`configuration_changed`, `source_or_configuration_changed`), and a run whose lease has expired is marked `interrupted`. Saving a config also cancels in-flight runs that became invalid. The status endpoint hides the result and steps of an invalid run and returns only a result preview (512 characters).

**Hooks**: `daily` (local time reaches the hour and it has not run that day; one idempotency key per skill per day), `recording_sealed` (a recording sealed within the past 24 hours with the transcript ready; one key per recording), `episode_ended` (a draft episode that ended more than 10 minutes ago; one key per episode), and manual. A hook starts one Pi skill run. The triggers of in-core workflows are in the feature table in Section 4.

<a id="quality-gate"></a>

### 5.4 Quality Gate for Automatic Triggers

A skill run with a non-manual trigger must meet all of these: the skill is enabled, `automatic` is true, and the quality report passes; otherwise queuing is refused (`waiting_quality`). The quality report (`candid-context-evaluation/1`, submitted by the `quality` action) must match this library's `store_id`, the pipeline `screen-cleanup/1` and the current policy revision (a change of sources or model config invalidates it), and must meet:

| Metric | Threshold (`automation_quality`) |
|---|---|
| Number of queries | ≥ 30 |
| Retrieval recall | relevant hits / relevant total ≥ 0.8 |
| Noise-reduction spot-check accuracy | ≥ 0.9 |
| Summary spot-check | checks > 0 and fabrication errors = 0 |
| Each point can be clicked back to its source | `grounded = true` |

The evaluation set itself is in [Curated Layer](curation.en.md). The AI page conversation can be used as soon as it is connected, and every answer carries citations. Until the gate is met, skills started by hooks can only be run manually.

<a id="ai-page"></a>

### 5.5 The AI Page and the Agenda

The switches for models, features and skills in the settings page are in [Models](models.en.md). The AI page is where you use AI in the app, and it has no switches:

- One page with two columns: the left column is "Today", from top to bottom the schedule (system calendar, read-only), reminders (today's and overdue ones from the system Reminders, plus candidates the AI proposed), "Needs your attention" (待你处理) (pending proposals, speakers and reference candidates waiting for your confirmation), and review (the latest daily review); the right column is the conversation, and any entry in the left column can be brought in as context with "Ask AI". When the window is narrower than 900pt it becomes a top switch.
- The schedule and reminders are read through EventKit and are not ingested; each time the page opens it fetches today's and overdue ones. When you create, change, complete or delete reminders and events yourself, no proposal is used (deletion asks once more). Pi can make the same changes only as proposals, and approving a deletion proposal also asks once more.
- A skill or a long operation turns into progress plus "Cancel" after you click it, and cancelling stops this round.
- How the conversation looks: one line above the answer says what it looked up, and can be expanded to see each call. Citations are written as `[[event_id]]`, shown in the interface as clickable citations that jump to the timeline. Typing `@` can cite a record. Conversations exist only on this machine, and can be searched and deleted.
- Every AI output shows who produced it (rule, model or Pi), which records it is based on, and when. Each can be changed, confirmed or rejected; changes are written to `corrections` and still apply after a recompute.

The interface behavior above exists on the Swift side (`AISkills.swift`, `AutomationModel.swift` and others); the exact form of each item has not been checked one by one (unverified).

<a id="commitments"></a>

## 6. Commitments and Reminders

1. Extraction (`item-extract`, see [People and Items](people-items.en.md)) produces candidates such as commitments, requests and deadlines from conversations, mail and recordings. Candidates can also be written by Pi's `pv_commitment`, stored as `memories` (`kind = commitment`, candidate state).
2. Candidates appear in the daily review and in the AI page's "Needs your attention" (待你处理), where you can confirm, edit or ignore them.
3. After confirmation a proposal is created (`create_reminder` or `create_calendar_event`). After you approve each one, the app writes it to Reminders or the calendar through the system interface, and the receipt is written back to the proposal. There is no automatic approval for now.
4. Your handling is fed back to the next extraction. "It did not appear again" is not treated as done (unverified).

`POST /local/agenda` (`candid-service/src/agenda.rs`): for the app only; any request with a token gets 403 `internal_only`, so Pi and external clients cannot approve their own proposals.

| Action | Effect |
|---|---|
| `today` (may carry `since_ms`) | Pending and recently handled proposals, commitment candidates, the latest daily review |
| `proposal` (`decision`: `approve`, `reject`, `executed`, `failed`; may carry `receipt`, `error`) | Advance a proposal: `pending → approved → executed \| failed`; a failure can be retried or rejected; `approve` is valid only for proposals that are `pending` or `failed` and of an executable kind; the approval is recorded in the core first and then written to the system, and the receipt (`receipt.item_id`) or the failure reason is written back to the proposal |
| `commitment` (`decision`: `confirm`, `ignore`; may carry `due`) | Decide a candidate; `confirm` creates a `create_reminder` proposal |

The executable kinds are `AGENDA_EXECUTABLE_KINDS` (seven, the same as the proposal level in Section 5.2). "Add to reminders" on the AI page = confirm the candidate + create the proposal + approve.

<a id="references-skill"></a>

## 7. Reference and Reply Retrieval

- On mail, files, episodes and messages you sent, you can "save as reference", optionally with a tag (such as "leave reply" or "expense process"). It is stored as `memories` (`kind = reference`, confirmed since you created it) and keeps only the reference and tags; the original text stays where it is. What Pi marks with `pv_mark_reference` is a candidate and needs your confirmation.
- Mail you sent and text you revised many times are listed automatically as reference candidates, and count only after you confirm (automatic listing unverified).
- The skill `references-replies` retrieves references and related episodes for a question, and returns original text with sources and a reply draft. The draft is only for copying or editing, and is not sent for you. The attached Pi and external AI use the same skill.

<a id="compose"></a>

## 8. "Help me write" (帮我写)

A global shortcut (or the floating-ball dictation) brings up a floating input bar. You say what to write, the core drafts text from your own history, and after you read and edit it you click "Insert"; the app pastes the draft back into the original input position. **Nothing is sent automatically.**

**Platform side (Swift, `ComposePanel.swift`)**: at the moment the shortcut is pressed it takes a snapshot: the front app name, the window title, the selected text (≤4000 characters), and what is already in the focused input field (≤2000 characters). The panel does not activate Candid, so the original app stays in front. "Insert": stash the draft on the clipboard → simulate ⌘V → restore the original clipboard; "Copy" is also available. No screenshot is taken.

**Core interface**: `POST /local/compose` (app only) `{instruction, app, title, selected, field}`, one non-streaming model call, returns `{draft, context, call_id}`. `compose_prepare` (`candid-store/src/compose.rs`) assembles the request:

| Context | Rule |
|---|---|
| Current conversation | A conversation that appeared in the same app within the last 30 minutes, preferring one whose title matches the window title; take the latest 30 messages |
| What the user said before | First take 8 messages you sent in this conversation; if not enough, add messages of 4–400 characters you sent in other chats or mail, up to 4; learn the tone only, do not copy |
| Selected text, draft in the input field | Taken from the snapshot as is |

The model is `agent_chat`, and if absent `summary`, then `title`, in that order. `max_tokens` is 1024 and `temperature` 0.3. The prompt requires output of the body text only, in your own tone, no invented facts, 【】 marks where more input is needed, and says text in conversations and references is data, not instructions. `context` returns what was used this time, and the panel shows it as "Referenced: …" (参考了：……).

Items from the original plan that are not in the current implementation: live recognition of the current screenshot from conversation excerpts, recognizing people by alias, prefetching items and memos, searching the top 5 with instruction keywords, treating a question as Q&A and outputting an answer, regenerating after deleting a context tag on the overlay, and the MCP tool `compose_context`; see Section 11.

<a id="executors"></a>

## 9. Where Things Run

The Rust store keeps jobs and results, and the service schedules and routes models. Voice and other models are called through external service interfaces. The core does not start Python workers or manage their environments. Swift handles system capabilities, settings and status display.

Model calls go through two channels that do not wait for each other: the interactive channel (`complete()`, "Help me write", Pi) is for things someone is waiting on; the background channel is a serial job worker (`candid-service/src/workflows.rs`). HTTP handling does not hold the store mutex for long.

Attached agent: Pi runs in RPC mode as a process managed by the app, the interface is drawn by the AI page itself, and its tools are only the three levels our extension provides: read, write in this library, and proposal. External AI such as Codex and Claude Code use the same skills and tools through MCP/API. Reading pi sessions is kept (`pv_agent_session`). The core has no agent backend or tool loop.

<a id="code"></a>

## 10. Implementation

| Content | Code | Tests |
|---|---|---|
| Workflow catalog, prompts, output validation | `core/workflows/catalog.json`, `prompts.json`, `outputs.schema.json` | `core/crates/candid-store/tests/workflows.rs` |
| Workflow scan / prepare / complete | `core/crates/candid-store/src/workflows.rs` | Same as above |
| Job worker, model calls, two channels | `core/crates/candid-service/src/workflows.rs` | `core/crates/candid-service/tests/workflows.rs`, `lifecycle.rs` |
| Skill policy, runs, hooks, quality gate | `core/crates/candid-store/src/automation/` (`config.rs`, `runs.rs`, `hooks.rs`, `runtime.rs`, `api.rs`) | `core/crates/candid-store/tests/automation.rs` |
| Pi run freezing, model gateway, tool counting | `core/crates/candid-service/src/pi.rs` | `core/crates/candid-service/tests/pi.rs`, `agents/pi-extension/test/e2e.mjs` |
| Agenda and proposals | `core/crates/candid-service/src/agenda.rs`, `core/crates/candid-store/src/query/agenda.rs` | `core/crates/candid-service/tests/agenda.rs` |
| "Help me write" | `core/crates/candid-store/src/compose.rs`, `core/crates/candid-service/src/local.rs` (`/local/compose`) | `core/crates/candid-store/tests/screen_excerpt.rs` (`compose_uses_the_on_screen_conversation_and_own_wording`) |
| Pi extension, system prompt, skill text | `agents/pi-extension/index.ts`, `system-prompt.md`, `agents/skills/*/SKILL.md` | `agents/pi-extension/test/e2e.mjs` |
| App: Pi process, skill runs, settings, Help me write | `apps/macos/Sources/Candid/PiAgent.swift`, `SkillRunner.swift`, `AISkills.swift`, `AutomationModel.swift`, `WorkflowControlModel.swift`, `ComposePanel.swift` | `apps/macos/script/test.sh` |

<a id="open"></a>

## 11. Not Implemented and Open Items

| Item | Status | Notes |
|---|---|---|
| Hourly and weekly summaries (`hourly-weekly`) | Not implemented | The catalog registers `implemented: false` |
| MCP prompt templates (the prompts primitive) | Not implemented | Goal: an external agent attached to `candid mcp` can list and call them; templates only reference query tools and carry no execution permission |
| A batch of "template-only" skills (standup and weekly report, etc.) | Not implemented | The repository has no such templates |
| A core tool to live-read mail bodies from the mailbox | Not implemented | Only ingested mail can be read now |
| Pi answers are not streamed character by character | Partly implemented | The core does not stream upstream, and the extension replays the whole answer |
| Inconsistent summary default | In design | `catalog.json` writes `summary` default on, but `workflow_scan` actually requires setting `summary == true` |
| Catalog role names vs. the three-role rule | In design | The catalog and code use roles such as asr, diarization, title and episode, while the repository rule allows only `vision`, `summary`, `agent_chat`; this belongs to [Models](models.en.md) |
| Inconsistent description of the `daily-review` trigger | In design | SKILL.md says scheduled 21:00 and `enabled: false`; the catalog says "daily 21:00 · manual only"; the policy default is `enabled` on, with automatic triggering waiting for the quality gate |
| Whether the quality gate thresholds are final | Unverified | The original text called the thresholds proposals, to be fixed after a baseline run; the code already enforces 0.8, 0.9 and 30 |
| Planned enhancements of "Help me write" | Not implemented | See the end of Section 8: live screenshot recognition, prefetching people and items, keyword retrieval, screen Q&A, regenerating after deleting a tag, `compose_context` |
| 30-day cleanup of old derived revisions | Unverified | No matching cleanup job was found |
| Default and push of daily review notifications | Partly implemented | `notify` is off by default; the system notification is sent by the app after it reads the `notifications_only` state; not checked item by item |
| UI details such as the AI page's two columns, conversation history and `@` citation | Unverified | The Swift side has matching files; not checked item by item |
| Model statistics card, thumbnails, extra-capability reasoning tests | Not implemented | Listed as not added in the original progress note |

<a id="sources"></a>

## 12. Sources

- The old `agent-platform-v1.md`: Section 1 (`#position`), Section 3 (`#features`), Section 4 (`#tasks`, including "sources, permissions and revisions"; "scheduling, cost and recovery" went to [Models](models.en.md)), Section 4.1 (`#skills-triggers`), Section 4.3 (`#commitments`), Section 4.4 (`#references-skill`), Section 5 (`#executors`). The settings part of Section 4.2 (`#ai-settings`) went to [Models](models.en.md), and the AI page part is in Section 5.5 of this spec.
- Section 7 of the old `timeline-v1.md`: the decision to cancel the built-in assistant went to the [Decision Log](../decisions/log.md), and the current rules are merged into Sections 5 and 9.
- Section 4 (M3) of the old `ai-v1-plan.md`: merged into Section 8 and rewritten to match the current state.
- Dated decision sentences (including "Design decision:" blockquotes) went to the [Decision Log](../decisions/log.md); implementation progress went to the [Changelog](../changelog.md).
