# Personal agent platform v1: data organization, feature scope and tasks

[中文](agent-platform-v1.md) · **English**

**Status**: AI features come in two kinds — simple automatic processing inside the core, and the attached Pi (conversations on the AI page or hook-started runs, using our skills, extension and guardrails) — plus MCP/API for external agents (section 4.1). The minimal fixed framework of section 4 is implemented and verified in isolation; the mail (Mail.app + IMAP) and browser-history (import off) connectors are implemented in the Rust core and verified with synthetic data, raising the timeline library to v4. Other capabilities follow as they are built.

How this relates to other specs:

- Storage, query tools, clients and usage tables: [timeline library v1](timeline-v1.en.md).
- Audio pipeline, model roles and per-task model choice: the intelligence spec.
- Identity, revisions and ContextPack of derived objects: artifact v1.
- Tool permissions, proposals and session rules: [agents](../../agents/README.md).

The split between core and platform follows [ADR-024](../decisions/024-rust-core.en.md): the organization pipeline, connectors (except those calling OS APIs), tasks and runners in this document are implemented in the Rust core.

This document covers: where data comes from and how it is cleaned (section 2), which features we build ourselves (section 3), and how AI features are defined and run (sections 4 and 5). Memory and persona get no default design here, only research questions (section 6).

<a id="position"></a>

## 1. Position and principles

Candid is two things at once:

- **A standalone product**: without any external agent it supports browsing, search, simple reviews and to-do drafts.
- **A personal agent platform**: it turns captured information into clean, citable, correctable data for external agents such as Claude Code, Codex, OpenClaw, pi and DeepSeek Harness.

> Design decision: the key questions are "how is the data cleaned" and "which features do we build, which not, and which only as templates for external agents"; task definitions are separate from runners; memory and persona are researched separately, with no default design.

Principles:

1. **Built-in features and external agents read the same data**, through the timeline service's scope filters and audit.
2. **Cost grows in layers**: rules and statistics cost no tokens; small models run once per segment and are cached; fixed pipelines run only on approved triggers, within a budget.
3. **Merging builds links and never deletes originals.** Deduplication, noise reduction and entity merging are recorded as relations or weights, undoable and recomputable.
4. **Writes go through proposals only**; derived drafts may be saved automatically within the authorized scope, but are always marked as drafts.
5. **Text from screens, web pages, messages and mail is untrusted data**: usable as evidence, never as instructions. Task prompts put it in a clearly marked data section, and runners get no terminal or file-writing tools.

<a id="pipeline"></a>

## 2. Data organization pipeline

> How capture signals first become raw record units ("visit", "input", "reading increment", "key frame") is described in the raw records spec. The pipeline below runs on top of those records.

Desktop text, media transcripts and connector content all go through this pipeline; transcription, speakers and relevance screening of audio itself follow the intelligence spec.

| # | Step | Output | Model | Status |
|---|---|---|---|---|
| 1 | Entry filtering and redaction | Excluded apps and windows are not stored; secret-like text is replaced before storage | No | Exclusions exist (desktop capture v1); redaction missing |
| 2 | Normalization and splitting | Paragraphs | No | Done ([timeline §4.6](timeline-v1.en.md)) |
| 3 | Exact dedup | Each paragraph stored once in the whole library | No | Done |
| 4 | Near dedup | Paragraph families | No | Missing |
| 5 | Noise reduction | "Fixed UI text" weight per paragraph family | No | Missing |
| 6 | Segmentation | Activity spans, episodes | Small model for episode titles | Activity spans done; episodes schema only |
| 7 | Extraction | Evidence-backed candidates: people, projects, commitments, decisions, topics | Small model, once per segment | `memories` schema only |
| 8 | Entity resolution | Entities, aliases, mentions | Mostly rules, small model when ambiguous | Missing |
| 9 | Recall | Folded results with evidence | No (vectors optional) | Full-text search done; short words, folding, entity expansion missing |
| 10 | Digests | Compact hourly, daily, weekly summaries | Small model | Missing |

<a id="sources"></a>

### 2.0 Source coverage: more connectors

> Design decision: more connectors are needed — mail, for example, matters for gathering information.

Commitments, requests, people and projects mostly appear in mail and messages; screen capture alone misses what was never opened and has no complete thread structure. These sources are therefore **ingest** connectors: text is stored and goes through the dedup, entity and extraction steps below. Large or fast-changing content such as attachments and remote documents stays by reference and is read live from the source.

> Design decision: reading Mail.app's local data stays the main route for mail, with an **additional** IMAP login adapter using the Rust libraries behind himalaya (pimalaya / email-lib); the browser-history connector is built now but **does not import yet**.

The connector list (order, sources, mapping, default policy and status) lives only in [timeline §6.3](timeline-v1.en.md#connector-table).

Every new connector follows these rules:

- Each connector and each account is authorized separately and can be revoked at any time; after revocation nothing more is read, and stored content is kept or deleted as you choose.
- Content others send you goes through the same entry redaction (2.1); external clients get metadata only by default, with text requiring a separate scope. Current implementation (engineering choice): mail text is masked for secret-like patterns before storage (the same rules as AI sessions; the other 2.1 rules such as card numbers are not done); mail sources have `agent_visible=false`, so external clients see nothing by default and must be granted the source ID in their scope — the "metadata only" tiered view is not implemented yet.
- Implemented per the ingest protocol in [timeline §6.1](timeline-v1.en.md), with synthetic tests for normal sync, repeated sync, interrupted retry, refusing unknown formats and revocation.

<a id="mail"></a>

#### 2.0.1 Mail connector

> Design decision: Candid implements the mail connector itself, mainly reading Mail.app's local data; no third-party mail MCP tools and no per-provider login implementations.
>
> Design decision: in addition to the main route, an IMAP login adapter based on himalaya (pimalaya / email-lib), with Gmail first through an app password.

**Status**: the Rust core's `candid-connectors` implements the shared pipeline and two adapters — Mail.app local data (adapter 1) and IMAP (adapter 3) — verified only with synthetic data and a local fake IMAP server; a read-only trial on real mailboxes, the Thunderbird adapter, the MailKit extension and OAuth login are not done. Usage and endpoints in the [core README](../../core/README.md).

**Why this way**:

References: [EagleFiler importing mail](https://c-command.com/eaglefiler/help/importing-mail-from-app), [DEVONthink and Full Disk Access](https://www.devontechnologies.com/support/faq/full-disk-access), [EmailEngine docs](https://learn.emailengine.app/docs), [Littlebird connecting tools](https://docs.littlebird.ai/learn/connect-your-tools/), [claude-connector-apple-mail](https://github.com/falconbradley/claude-connector-apple-mail), [MailKit MEMessageActionHandler](https://developer.apple.com/documentation/mailkit/memessageactionhandler).

- Official APIs per provider each need a registered OAuth app; mail-read scopes are strictly reviewed, and corporate Microsoft 365 tenants often block third-party apps. Cloud assistants (e.g. Littlebird) support only two or three providers.
- Unified mail API services (Nylas, EmailEngine) only hide provider differences behind a service; you still register OAuth apps, and they are cloud or paid self-hosted services, at odds with local, standalone operation.
- Mac archiving tools (EagleFiler, DEVONthink) read Mail.app's local files with Full Disk Access; Mail.app plug-ins are deprecated by Apple.
- Community Apple Mail MCP projects (claude-connector-apple-mail, apple-mail-mcp) target ad-hoc agent queries with search and read only — no incremental sync, deletion detection or threads. They run on Python or Node, so Full Disk Access would go to an interpreter or terminal, far too broad. They serve only as references for schema and file format; check licenses before borrowing ideas, never copy code.
- Once mail is stored, external agents can query it through Candid's own MCP, with scope filters and audit, without another mail MCP.

**Data sources**:

- Index: `~/Library/Mail/V<n>/MailData/Envelope Index` (SQLite), opened read-only and untouched; the schema is recognized at run time and only verified versions are accepted — anything else is refused as a whole with a hint on the Connections page, never read by guesswork.
- Bodies: `.emlx` files in each mailbox folder: a byte count on the first line, the RFC 822 message, and a trailing property list. `.partial.emlx` files are partially downloaded; their available content is stored and marked incomplete. Bodies Mail.app has not downloaded are not read, and no download is triggered.
- Permission: Candid needs Full Disk Access. Without it the connector shows as unauthorized — no retries, no error spam.

**Sync**:

- The index's message row IDs and modification info are the cursor for reading new mail incrementally.
- Each pass checks that stored messages still exist; deletions and moves become tombstones or new revisions, never silent losses. Implementation: messages only left in trash/spam or fully deleted are removed as `source_deleted` with a tombstone (restoring them from the trash later does not re-ingest them); moving to another mailbox (e.g. archiving) is a payload revision of the same event.
- Messages whose bodies Mail.app has not downloaded keep metadata only from the index (subject, sender, time; `body_status=not_downloaded`); bodies downloaded later are not added automatically (open question).
- `source_key` is the Message-ID; without one, a combination of account, mailbox and row ID. A message in several mailboxes (inbox and archive) is stored once, with the mailboxes it is in.
- Read-only: no flag changes, moves or deletions, and no use of Mail.app's scripting interface.

**Parsing**:

- MIME: multipart; quoted-printable and base64; encoded words in headers (RFC 2047); charsets converted by the system, covering at least UTF-8, GBK, GB2312, GB18030, Big5, ISO-2022-JP, the ISO-8859 family and Windows code pages.
- Plain-text parts first; HTML-only bodies are converted to text without styles, scripts or tracking images.
- Quoted history and signatures are marked separately and excluded from extraction input; they already exist in earlier messages of the thread and fold through paragraph dedup. Implementation: stored in a separate `quoted` text field (timeline v4), with only the newly written part in `body`.
- Attachments keep only file name, type and size; content is read live by reference when needed, under the same access scope.

**Event mapping**: one `mail.message` event per message, with `body` and `subject` text fields; threads `mail_thread` built from Message-ID, In-Reply-To and References. The concrete payload is in [timeline §4.3](timeline-v1.en.md).

**Storage adapters**:

The mail connector lives in the Rust core ([ADR-024](../decisions/024-rust-core.en.md)). Parsing, threading, cleaning, filtering and AI judgement exist once; "where mail is read from" is a replaceable adapter:

| Order | Adapter | Platforms | Notes |
|---|---|---|---|
| 1 | Mail.app local data | Mac | The main route above; **implemented**, accepting only `V10` with an index schema matching the synthetic signature; other versions are refused as a whole (`unknown_mail_version` / `unknown_mail_schema`); real versions to be confirmed by a read-only trial |
| 2 | Thunderbird local data | Mac, Windows, Linux | Thunderbird handles Gmail and Outlook OAuth itself; since 145 (November 2025) it supports Exchange mail natively through EWS, which Microsoft is retiring — reportedly Exchange Online stops third-party EWS access from October 2026, to be checked before integrating. Needs offline copies kept in Thunderbird; the local format (mbox or Maildir) to be measured |
| 3 | IMAP login | All | **Implemented**: email-lib 0.27, reading only with EXAMINE, UID SEARCH and BODY.PEEK, never changing flags or moving; when a message leaves a mailbox it is looked up by Message-ID in other folders (e.g. Gmail's All Mail) and only recorded as deleted if not found. Uses the Rust mail libraries behind himalaya (pimalaya). Gmail through an app password or the user's own OAuth app; never borrowing another program's OAuth client ID. One-click sign-in under Candid's name needs Google and Microsoft review, to be reconsidered on demand |
| — | MailKit live extension | Mac | Called by Mail.app when new mail arrives, with access to the body; covers only new mail. There are reports of the extension switch not appearing on recent macOS; a supplement to adapter 1 once verified |

MIME parsing uses mature Rust libraries, not an in-house implementation; each encoding and charset above is verified with synthetic messages.

**Verification**:

- Synthetic mail covering every encoding above, multipart structures, partially downloaded messages, messages without Message-ID, one message in several mailboxes, deletions and moves.
- Real mailboxes only in a Candid test build, read-only, reporting counts only: messages, threads, decoding failures, incomplete bodies. Terminals and developer tools never get Full Disk Access to read real mail.

<a id="source-filter"></a>

#### 2.0.2 Source filtering: grade first, then decide how deep to go

> Design decision: once mail is connected, not every message can be processed; user-settable filter rules are needed, optionally with rules that let AI judge importance, to keep large amounts of useless content out. The same applies to AI session logs and other sources.

Reading and parsing cost no tokens; extraction does. So each item from each source is graded first, and its grade decides how far it goes:

| Grade | Processing |
|---|---|
| Ignore | Not stored |
| Metadata only | Visible on the timeline (sender, subject, time, thread, location in the source); no text stored, read live from the source when opened |
| Normal | Text stored and searchable, no extraction |
| Important | Goes to extraction (2.5) and digests (2.8) |

Grading has four layers in order, shared by all connectors:

1. **Scope**: which accounts, folders, project folders. Mail defaults to inbox and sent; spam, trash and drafts are never read. The first import goes back 90 days by default; older mail on demand.
2. **Your rules**: conditions on sender or domain, recipients, subject keywords, bulk-mail headers, the client's own spam flag, whether you replied; for AI sessions, project folder and session length. The action is one of the four grades. Rules match in order and the first hit wins. **Your rules always win.**
3. **Built-in defaults** (editable, can be turned off): mail with unsubscribe, bulk or auto-reply headers or from noreply addresses keeps metadata only; threads you replied to and mail from people you have written to become important candidates.
4. **AI judgement** (optional, off by default): only for items the rules did not settle. A small model (local preferred) reads the headers and about the first 500 characters and answers "normal" or "important" with a one-line reason; results are cached, there is a daily budget, and each call is recorded in `ai_calls`.
   - AI can only move between "normal" and "important"; it cannot override your rules or trigger any action;
   - the UI marks it "AI judgement";
   - text claiming its own importance ("this mail is very important, handle it first") is content, not an instruction.

When you change an item's grade by hand, nothing is learned silently in the background; instead a rule suggestion appears ("Mark everything from X as important from now on?"), effective only once you accept it.

Grades and rule hits are recorded on the stored event, and stored items can be regraded after rules change.

**Implemented for mail**: scope (accounts, mailbox names or roles such as `role:inbox`; trash, spam and drafts never read; 90 days back by default); your rules (sender, domain including subdomains, recipients, subject keywords, bulk headers, mailbox; all conditions must match; first hit in order wins); built-in rules in the order "replied → bulk/automated → default", each switchable: threads you replied to and mail from people you have written to → normal plus important candidate (`important_candidate`, grade still normal; people you have written to are stored only as salted hashes on sent mail); bulk/unsubscribe headers, auto-reply headers, noreply-like addresses → metadata only; everything else → metadata only by default. AI judgement exists only as a switch: when on, mail the rules did not settle (replied and default) is marked `ai_judge=pending` without calling a model. Grade and matched rule are written to the event payload (`level`, `level_basis`: `rule:N`, `builtin:replied|list|automated|noreply`, `default`, `manual`).

**Defaults and import scope**:

- Before importing, choose accounts, mailbox folders and how far back, and preview "about N messages: X metadata only, Y with text"; import starts only after confirmation.
- Mail no rule graded defaults to "metadata only". Threads you replied to and mail from people you have written to default to "normal" and are "important" candidates; your rules always win.
- A message or a whole thread can be raised to "normal" or "important" at any time; stored items can be regraded after rule changes.
- For mail deleted in Mail.app, the text of metadata-only messages can no longer be read, and the timeline marks it "deleted at source".
- **Implemented (synthetic verification; not installed, no real mailbox tried)**:
  - Scope and preview: the mail detail on the Connections page first authorizes Mail.app; after authorizing nothing is imported (`import_confirmed=false`, sync returns `awaiting_confirmation`); `mail_scope` lists accounts and folders (counts only); after choosing accounts, folders (default inbox + sent) and look-back (30 days / 90 days / 1 year / all, default 90 days), `dry_run` with `config` previews that scope (not saved), parsing only headers, and returns counts per grade (metadata only / normal / important candidate, plus rule-ignored); "Import" writes `import_confirmed=true` and starts.
  - Live reads: `mail_body` reads a body live by Message-ID from Mail.app's index and emlx, not stored and not indexed; messages deleted or only left in trash/spam/drafts return `source_deleted` and the UI shows "deleted at source"; only Mail.app sources support this, IMAP sources do not read live. `set_level` raises a message or thread to normal/important (the text is then read from the source, stored and searchable) or lowers it to metadata only (text and index entries removed), recorded as `manual`, never overridden by rules or regrading; deleted-at-source messages cannot be raised.
  - Rule editor: ordered rules, reordered by drag or a "Move" menu, grade changed inline, conditions edited when unfolded (multiple conditions), deletion; the two built-in rules can be toggled, and the last row "all other mail" is fixed. Rules live in the configuration shared by all mail sources.
  - Regrading: `relevel` rereads each message under the current rules (sender and recipients are not stored, so they are read live and used only in memory) and grades it again; `manual` grades are untouched; messages no longer at the source are left to regular sync cleanup; stored mail matching an "ignore" rule stays as metadata only rather than being deleted (`ignored_kept`); raising adds text, lowering removes it; repeatable, a second run changes nothing. After a rule change the UI offers to regrade.
  - UI gaps: there is no mail lane on the timeline yet; opening and raising mail currently happen in the "Recent mail" list on the Connections page; rule suggestions after manual grading, a real-mailbox trial and IMAP live reads are not done.
- Exchange Online's EWS is being shut down from October 2026; direct Exchange support would be evaluated on Microsoft Graph.

<a id="redaction"></a>

### 2.1 Entry redaction

- **Always replaced**: API keys and access tokens (known prefixes, or high-entropy runs of at least 32 characters), private key blocks, card numbers passing the Luhn check. Replaced before storage with `‹redacted:type›`, with a redaction marker in `event_text`. Original screenshots are not altered and stay under the `media` scope.
- **Kept but masked externally**: phone numbers, email addresses, ID numbers and similar personal data are stored for your own search; external clients see them masked by default unless their scope explicitly allows it.
- Redaction rules are the same for every app; false positives are fixed by turning a rule off, not by per-app exceptions.

<a id="near-dup"></a>

### 2.2 Near dedup

What exact dedup cannot handle: OCR jitter, only a time or count changing, a paragraph with one or two characters edited.

1. **Normalize the original**: unify Unicode width and strip trailing whitespace, keeping digits, case, inner spaces and indentation.
2. **Fingerprint families**: short paragraphs from the same source and window within 10 minutes of each other, with 64-bit SimHash distance over character trigrams of at most 3, edit distance at most 10% and identical digit strings, can be recorded as a near family; original paragraphs and character spans are kept.
3. **Newness**: identical paragraphs in the same window within 30 minutes can reduce repeated model input; short replies carry the previous line as context. Real edits within a near family still count as new content — negations, numbers and code changes must not be erased.
4. **Bounds**: near comparison is limited to recent candidates and short paragraphs; long paragraphs only get exact duplicate checks. Every original occurrence and exact search hit is kept.

These are engineering choices that favour conservative change preservation; an early digit-skeleton approach is not used for production dedup. Parameters and resource bounds follow the core implementation; fixed evaluation is kept separate from real-quality release decisions.

Whether screenshots show "the same picture" follows the existing desktop capture rules; this section only covers text. Thresholds are set by measurement on the evaluation set (2.9).

<a id="boilerplate"></a>

### 2.3 Noise reduction: fixed UI text

Menus, sidebars, navigation and button text appear almost every time in the same app and crowd out summaries and search.

- Per "app + window type", the share of events in the last 7 days containing each paragraph family (document frequency) is computed. Families appearing in over 30% of events and at least 20 times are fixed UI text, with weight decreasing as the share grows.
- Isolated short paragraphs under 4 characters are down-weighted too.
- Only down-weighted, never deleted, and exact search hits are unaffected; you can mark a family "important" to undo the down-weighting (recorded as a correction).
- The rule is the same for every app; there is no per-app blacklist.

<a id="segments"></a>

### 2.4 Segmentation

- **Activity spans**: a rules projection, see desktop capture v1 §6.1.
- **Episodes**: adjacent activity spans merge when the gap is at most 5 minutes and they share entities or paragraph families; meetings, calls and recording sessions are their own episodes. Rules decide episode boundaries; a small model writes titles.
- Episodes enter extraction only after they "end": no new content for more than 10 minutes, or a meeting or recording that clearly ended.

Currently screen episodes are built per source, app and window within 5-minute gaps, with one model call after they end; cross-window merging by entity, structured extraction and entity resolution are still to come. Existing titles and summaries can be edited by hand, and rebuilding keeps the manual values.

<a id="extraction"></a>

### 2.5 Extraction

- **Unit**: one small-model call per finished episode, never per screenshot.
- **Input**: the episode's new, not down-weighted, high-attention paragraphs, cut by weight to the input limit (proposed 3,000 tokens), plus the episode's app, time and participants.
- **Output**: a fixed JSON structure with mentioned entities, explicit commitments and requests, decisions, open questions and topics, each with a selector back to the original text.
- **Rules**:
  - extract only what is explicitly stated;
  - "never came up again" does not mean done;
  - leave confidence empty without a calibration method.
- **Cache**: keyed by input hash, prompt version and model; hits are not recomputed. A model or prompt change creates a new revision, keeping old results and your corrections.
- **Storage**: results go to the derived layer as candidates (`status = candidate`). Commitments go to the task list (section 4), entities to the entity layer (2.6).

<a id="entities"></a>

### 2.6 Entity layer

People, projects, organizations, places, websites and apps are entities, stored apart from events and episodes. Structure (in timeline v5, where `kind` also has `topic` and `alias_kind` also has `voice`; see [timeline §4.8.1](timeline-v1.en.md#derived-next)):

```sql
CREATE TABLE entities (
  entity_id  TEXT PRIMARY KEY,
  kind       TEXT NOT NULL CHECK (kind IN ('person','project','org','place','site','app')),
  name       TEXT NOT NULL,
  status     TEXT NOT NULL CHECK (status IN ('candidate','confirmed','merged','rejected')),
  merged_into TEXT REFERENCES entities(entity_id),
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE entity_aliases (       -- names, emails, handles, domains, bundle ids
  entity_id  TEXT NOT NULL REFERENCES entities(entity_id) ON DELETE CASCADE,
  alias      TEXT NOT NULL,
  alias_kind TEXT NOT NULL CHECK (alias_kind IN ('name','email','handle','domain','bundle','other')),
  source     TEXT NOT NULL CHECK (source IN ('rule','model','user')),
  PRIMARY KEY (entity_id, alias, alias_kind)
) STRICT;
CREATE TABLE entity_mentions (
  entity_id  TEXT NOT NULL REFERENCES entities(entity_id) ON DELETE CASCADE,
  ref_type   TEXT NOT NULL CHECK (ref_type IN ('event','episode')),
  ref_id     TEXT NOT NULL, ref_rev INTEGER,
  selector_json TEXT,
  producer   TEXT NOT NULL, confidence REAL,
  PRIMARY KEY (entity_id, ref_type, ref_id)
) STRICT;
```

- **Resolution order**:
  1. exact aliases (email, handle, domain, bundle id);
  2. normalized names plus context rules (same thread, same meeting);
  3. only when still ambiguous, ask a small model, with the result marked as a candidate.
- **Your corrections win**: merges, splits, renames and rejections go to `corrections` and keep applying after derived content is regenerated.
- Speakers recognized by voiceprint and person entities are different things: automatic matches are only candidate links; a link is confirmed once you say "this is X" in a recording, recorded as an alias with `alias_kind = voice` (the voice service's voiceprint person ID) and in `corrections`.

<a id="speaker-labeling"></a>

**Speaker labelling**:

1. In a recording's speaker list, each speaker is in one of four states: recognized (automatic match, marked "auto", editable), maybe someone (waiting for your click), assigned by you, anonymous (the default, nothing to do).
   - Each speaker shows a **match score**: similarity to the closest sample of that person (the voice service's cosine similarity as a percentage), and the margin over the runner-up.
   - The decision uses the voice service's thresholds: at or above the match threshold with enough margin is "auto"; slightly below or with too little margin is "maybe"; everything else anonymous. Thresholds come from the voice service's configuration; the UI sets none of its own.
   - Scores and decisions are stored with the speaker labels and can be recomputed after a model or threshold change; your assignments are not affected.
2. Click a speaker and choose "who is this": an existing person, a new person, or keep anonymous.
3. Each assignment becomes samples for that person: the speaker's 3 clearest segments, at most 20 per person, keeping the most recent; only confirmed assignments become samples. On the first recording you are asked to identify yourself once.
4. The voiceprint library is just a list of people (name, sample count, last seen, and who they are easily confused with): rename, merge or delete people, delete samples one by one or clean up by condition (keep only the latest N, remove samples from a given recording).
5. Voiceprint vectors and sample audio stay in the voice service; renames, merges and deletions start from the core and sync. Deleting a person deletes their samples, and their speakers in existing transcripts return to anonymous.
6. **Whole-recording reconciliation**: physical segments and model chunks are internal; a recording first unifies its anonymous speakers, then matches the existing library, and works with no enrolled people at all. Short utterances with too little evidence stay unassigned rather than creating people from file labels; optional merge strength and speaker caps are advanced settings. Original text, positions, raw recognition and manual names are kept, and source withdrawal/deletion and configuration changes apply equally to projections and voiceprint references.
7. **Automatic matching**: new recordings are transcribed and separated into speakers automatically; each speaker's vector per conversation stays in the voice service and transcripts carry only references. When you confirm a person and save samples, or rename, merge or delete people, all recordings are rematched against the current library without retranscribing; the results are stored separately and never change the transcript itself. Deleting a recording also removes its vectors.

<a id="recall"></a>

### 2.7 Recall

1. **Parse filters**: time, app, source, person, project.
2. **Generate candidates**: full-text search over paragraphs and metadata; two-character Chinese words via one of the two approaches in timeline §4.7; expansion along entities (finding a person includes their aliases); vector search later.
3. **Rank**: BM25 plus recency and attention, multiplied by "1 − fixed-UI-text weight".
4. **Fold**: hits from the same paragraph family merge into one entry marked "seen N times" with first and last times; at most 3 hits per episode, for diversity.
5. **Return with evidence**: every hit leads back to its event and text span.

<a id="digests"></a>

### 2.8 Digests

Compact summaries for agents and the UI, at three levels: hour, day, week.

| Level | Contents | Cap (proposed) |
|---|---|---|
| Hour | Activity statistics, episode titles, newly seen entities | 300 tokens |
| Day | Episode list, time distribution, open commitments, main entities | 1,500 tokens |
| Week | Daily highlights, recurring projects and people, commitment changes | 3,000 tokens |

- Statistics come from rules; only the prose uses a small model.
- Cached by input hash; when underlying data changes (new events, your corrections) only the affected levels are recomputed.
- Exposed through a new query tool `digest` and MCP resources, with the same scope filtering as every other tool.
- The **daily review** is a skill on top of the day digest (section 4.1): rules and fixed pipelines prepare statistics and episodes, the attached agent writes it with the "daily review" skill, and the result is a draft revision of the day digest; when enabled it runs on schedule and notifies you. The former "recording daily review" pipeline merges into it.

<a id="evaluation"></a>

### 2.9 Evaluation set

Without an evaluation set the thresholds above are guesses, so it is built together with the first implementation.

- **Synthetic fixtures** live in the repository, covering OCR jitter, changing counts, fixed UI text, people sharing a name, mixed Chinese and English, and two-character words.
- A [synthetic record metrics fixture](../../core/fixtures/metrics/ideal-day.json) with independent fixed expectations is delivered (see the [core README](../../core/README.md)). It verifies that record metrics are measured correctly; the full evaluation of entities, recall, extraction and model usage below is still to come.
- **Samples you label** stay in the local data folder and never enter Git. Verification records hold only counts and metrics.
- **Metrics**:
  - storage dedup rate;
  - noise-reduction precision (manual spot checks);
  - search hit rate (recall@10 over at least 30 queries);
  - extraction precision and recall (labelled episodes);
  - tokens per day.

<a id="features"></a>

## 3. Feature scope

Criteria:

- whether the product still works without an external agent;
- how often it is used;
- whether it needs tools beyond Candid (mail, code, web);
- whether privacy requires it to stay on the machine.

| Category | Feature | Notes |
|---|---|---|
| **Automatic in the core** | Timeline browsing and search | Rules and indexes, no model |
| | Time distribution | Pure rule statistics |
| | Transcription, speaker separation, matching known voiceprints | Automatic after a recording is saved |
| | Titles and summaries of recordings and episodes | One small-model call per segment; your edits are never overwritten by reruns |
| | Screenshot text | Local system OCR after saving; a vision model on demand or per episode as fallback, within a daily budget |
| | Extraction: commitments, requests, decisions, questions, topics | Once per finished episode (2.5); the former "decision and to-do drafts" and "tag drafts" merge into it |
| | Commitments and to-dos → reminder proposals | Your handling of each item feeds the next run; writing to system reminders and calendars per 4.3 |
| | Merging speakers and people | Part of data organization (2.6) |
| **Attached agent (AI page or hook-started)** | Conversation and actions on content in the app | Find, organize, retitle, mark references; writes only within the library, actions outside the app through proposals (4.1) |
| | Daily review | Writes from day episodes and statistics, with citations; scheduled notification when enabled (2.8, 4.1) |
| | References and reply lookup | Material you marked, mail you sent and replies you refined; returns sourced originals and a reply draft, never sends |
| | Ask about your day (Q&A with citations) | Pi answers on the AI page; external AI uses the same skills and tools through the external interface |
| | Meeting review, project history, commitment tracking | Existing skill templates (`agents/skills/`) |
| **External interface (MCP/API)** | People and project pages | Based on the entity layer |
| | Proposal approval, usage, privacy settings | The platform's control plane |
| **Templates only, for external agents** | Stand-ups and weekly reports, project handovers | Need other tools or personal formats |
| | Workflow discovery and automation suggestions, skill distillation | Open-ended, infrequent |
| | Deep research over time, writing with context, meeting prep | Reasoning-heavy; external agents do it better |
| | Personal processes such as expense follow-up | Vary greatly between people |
| **Not built** | A general chat assistant | Not competing with general AI products |
| | Executing external actions directly (sending messages, editing calendars) | Proposals only; the app executes after your approval |
| | Agents with a terminal or code writing | Security boundary; the attached Pi has its own tools off and only our extension's tools |
| | Surveillance-style metrics such as focus scores | Product position |
| | A vision model on every screenshot by default | Cost; still available on demand |
| | Video export | Unrelated to the platform |
| **Deferred** | Browser history import, direct Exchange, details of MCP data supply | Deferred by design decision; MCP data supply is discussed with the memory layer |
| **Research** | Memory and persona, "what you care about" | Section 6 |
| | When to interrupt proactively | Section 6; daily review notifications are the decided exception |

Goal: templates are offered through MCP's prompts primitive (integration still to be verified), so an external agent connected to `candid mcp` can list and call them. Templates reference only query tools and digests and carry no execution rights.

<a id="tasks"></a>

## 4. Minimal framework for fixed pipelines

Only fixed steps, no loop in which a model picks tools. Features switch on as needed: transcription, speaker separation, matching known voiceprints, titles and summaries, screenshot text, extraction (commitments, requests, decisions, questions, topics), digest statistics. The existing "decision and to-do drafts" and "tag drafts" merge into extraction, and "recording daily review" into the day digest; writing the daily review and other skills are in 4.1. Sorting, grouping, dedup and statistics prefer rules. New models are never downloaded automatically, and approved destinations are never changed.

The pipeline catalog lives in `core/workflows/`: each pipeline has a versioned definition, prompt templates and output type references, loaded with the release; run logic stays in the existing store/service modules. User switches, role mappings and budgets are separate configuration and never modify released definitions. External AI skills in `agents/skills/` are maintained separately and are not an internal script directory. The catalog exists with the fixed processing implementation; current definitions, prompts and output validation are maintained there.

Loop: pick saved input → fixed steps by version → rules or a role model → validate structure and sources → save a new revision → update indexes and processing state.

| Shared object (logical design, not added fields) | Contents and owner |
|---|---|
| Source record | Stable occurrence identity, source version, time/sample span, original reference and authorization; owned by store |
| Recording and segments | One recording identity per start-to-stop; segments with sequence numbers, source sample spans, save confirmation and gaps; segments reuse existing originals |
| Pipeline definition | ID/version, input type, fixed steps, triggers, output validator, roles, budget; released with versions, never running arbitrary scripts |
| Runs and steps | Input revision snapshot, configuration version, step checkpoints, attempts, lease and cancel flag; Rust is the only scheduler |
| Model roles | asr, speaker, title, summary, ocr… mapped to provider/model/runtime/destination; the settings page edits the same configuration |
| Derived content and corrections | Artifact identity/revision, source selection spans, rule or model/prompt version, confidence; manual corrections stored separately and shown first |

The first end-to-end pipeline is "continuous recording → transcription → title → optional summary". Transcription runs incrementally once segments are saved and wraps up when the whole recording ends; overlapping windows align by source sample position, and genuinely repeated speech is kept. Speaker separation and voiceprint matching are added by switch, and unknown identities stay anonymous. A usable title from the time comes first, with the model title as a draft; titles edited by hand are never overwritten by reruns. A disabled or failed summary never blocks the audio, transcript or playback.

A pipeline definition only lists: the input recording's version, a transcribe step calling the asr role, a title step reading the transcript, and a summary step with its own switch. Each step declares its output type, maximum input/output, timeout and dependencies. Adding "tags" means adding one definition, output validator, prompt template and synthetic acceptance fixture; with sufficient model adapters there is no new scheduler, storage or UI state machine.

### Scheduling, cost and recovery

- Manual, sealed-event and scheduled triggers feed one queue; disabled pipelines are not queued. Durable jobs resume on start without rescanning the library for new ones.
- States: queued, running, waiting, succeeded, failed, cancelled, skipped. Waiting distinguishes missing model, not authorized and insufficient budget; never silently falling back to a remote model. Steps have start/end and errors, so the UI can explain where something is stuck.
- The idempotency key includes the source occurrence identity, input revision, pipeline version and effective configuration; two recordings with the same content are not merged. Step results and checkpoints commit transactionally, event delivery may retry, and index updates are deduplicated by revision.
- Claims use leases and attempt numbers; expired jobs are reclaimed after a crash. Commits check the attempt number, cancellation, source revision and permissions, so late results never overwrite newer ones. Completed steps are reused; failures retry only retryable steps.
- Per-call and daily limits on tokens, cost, duration and concurrency are enforced by the service; budget is reserved before a call and settled after. Unknown usage is recorded as unknown, never zero; after a network failure, billing is not assumed absent — the provider's request status is checked first.
- Cancellation is persisted first, then the external model request is cancelled and later steps blocked; when a provider cannot cancel, the possible charge is recorded and late output is not published. Resource concurrency has one global cap, with capture and saving first.
- Calls go to ai_calls, linked to the run/step, model version, destination, usage and errors; original text is never copied into logs. Settings migrations keep existing authorization; secrets are read only through the platform's secret interface.

### Sources, permissions and revisions

Uses the artifact public envelope; samples and characters use half-open spans. Events record what happened, derived content records interpretations, manual corrections record the user's choices; originals are never overwritten. Model output must have locatable sources; out-of-range citations, missing sources or wrong types are refused.

Effective permission is the intersection of the input sources, checked when choosing input, sending to a model, committing and answering external queries. A source revision takes older derived content out of the default index and schedules a new revision; deletion or revocation cancels related queued jobs, refuses late results and invalidates derived content, indexes and caches. History retention follows the deletion policy, and rolling back never restores revoked or deleted text. External AI is read-only by default; internally authorized draft writes grant no external write permission.

Schema and external model service protocol changes are submitted together with versioning, old-library reads, refusal of unknown versions, failure recovery and consumer verification; the v3 upgrade for media, queues and record revisions is implemented; fixed pipeline definitions, configuration and output each have their own versioned formats and are verified for compatibility per change.

<a id="skills-triggers"></a>

### 4.1 Two ways: automatic in the core, and the attached agent

> Design decision: the AI page stays, for conversations with AI and actions on content inside the app. We don't build our own agent; we attach the external Pi with our own UI, extension and strict guardrails (reading and writing only specific scopes), so there is no agent harness to build while users still get the capability; no custom loop on pi-agent-core. Features come in two kinds: simple features triggered automatically in the core, and an external agent invoked through hooks, skills and custom constraints. MCP/API is offered for external agents too, though most users will not use it.

| Way | What it is | Examples |
|---|---|---|
| **Automatic in the core** | Rules or fixed steps (possibly one role-model call), triggered by events; scheduling, budgets and recovery as above | Dedup and noise reduction, transcription, naming recordings, OCR, episode titles, extraction, grading mail |
| **Attached agent** | Pi talks with you on the AI page or is started by a hook; with our skills, extension tools and guardrails | Conversation and actions on content, daily review, references and replies, reminder proposals |
| Also: external interface | MCP and API for Claude Code, Codex and others; same tools and permissions | For advanced users |

**How Pi is attached**:

- Version: `@earendil-works/pi-coding-agent` 1.5.2 (locked in `agents/pi-extension/package-lock.json`). Node and Pi are not bundled: Settings → Components installs them on demand, reusing a system Node 22.19+ or downloading the official Node 24.22.2 checked against a pinned SHA-256, and installing Pi from the lockfile. Upgrading Pi requires rechecking the flags and isolation below.
- Runs in RPC mode (`--mode rpc`, a JSON-lines protocol over standard input/output, split on LF only) as a background process managed by the app; the chat UI is drawn entirely by the AI page.
- Verified launch flags: `--no-builtin-tools --no-extensions -e <our extension> --tools 'pv_*' --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline --system-prompt <our prompt> --provider candid --model agent_chat --session-dir <separate folder>`. Note that this version uses `--no-extensions` (plural), which also disables built-in extensions (MCP, codemode); an explicit `-e` still loads.
- Isolation: the environment carries only `HOME` and `PI_CODING_AGENT_DIR` (both pointing to `pi/` in the app's data folder), `PI_OFFLINE`, `PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY=0` and this run's core address and token, never the user's environment (keys, proxies, PATH). Control experiment: a fake `~/.pi/agent` with extensions and settings is loaded without isolation and not loaded with it. The working directory is empty and project trust is always refused.
- Model: the extension registers a single provider `candid` (OpenAI-compatible) pointing at a random port and path the extension opens on loopback; each request goes to the core's `POST /local/pi/model`, which sends it with the frozen `agent_chat` model and the key from the Keychain and records it in `ai_calls` (`client = pi`). Neither Pi nor the model ever gets a provider key. The core does not stream upstream; the extension replays the whole answer to Pi as SSE at once (text does not appear word by word yet). Only OpenAI-compatible and Ollama APIs are supported; cloud models must be allowed for the run.
- For each run the app calls `POST /local/pi/start` to freeze the model, scope (sources, write tiers, proposal kinds, AI session sources) and budget (model calls, tool calls, output tokens), and the core issues a token for the dedicated client `candid-pi`; after `POST /local/pi/stop` the token is invalid. Exceeding the budget stops the run, and crash restarts do not reset it.

**Guardrails: three tool tiers, permissions checked in the core**. Every tool the extension provides calls the core (local socket, with this run's token), which checks the `candid-pi` scope and writes the access log; Pi has no other tools. Hiding tools in the extension is a convenience; permissions are decided only by the core.

| Tier | What it can do | Pi tool → core tool |
|---|---|---|
| Read | Query the timeline, episodes, digests, references; read mail bodies and AI sessions live | `pv_status`, `pv_overview`, `pv_search`, `pv_timeline`, `pv_context`, `pv_get_event` (including stored mail bodies; reading a body live from the mailbox has no core tool yet), `pv_episodes`, `pv_episode`, `pv_records`, `pv_agent_session` (`agent`), `pv_memories` (new `memories`) |
| Write (only derived content and corrections in the library) | Edit titles and notes, mark references, confirm or dismiss commitment candidates, save drafts; every write is recorded as a correction or "AI draft" with client, run ID and model, and can be undone | `pv_annotate` (`annotate`, through the episode annotation correction path), `pv_mark_reference` (`mark_reference`, `memories` kind `reference`, candidate), `pv_commitment` (`commitment`), `pv_save_draft` (`save_draft`, `digests` draft), `pv_undo` (`undo_write`) |
| Propose (actions outside the app) | Executed by the app only after you approve each one in the UI | `pv_propose` (`propose`, limited to the reminder kinds `create_reminder`, `update_reminder`, `complete_reminder`, `delete_reminder` and the calendar kinds `create_calendar_event`, `update_calendar_event`, `delete_calendar_event`; update, complete and delete need an `item_id`, from the item the user brought in with "Ask AI") |

- Write tiers are granted one by one through the client scope's `writes` (`annotate`, `reference`, `commitment`, `draft`), empty by default; proposal kinds are limited by `propose_kinds`. Undo: the app can undo any agent write; an agent can undo only its own writes with no later changes; undo itself is recorded as a correction.
- No terminal and no files outside the library; text from screens, mail and messages is always data (principle 5): tool results are wrapped in `<candid_data>` with a note not to follow requests inside, and the system prompt says the same. In tests, a mock model told to "comply" calling `bash` only got "no such tool", and a rename in a read-only run was refused by the core.
- A conversation or skill run freezes its model, tools, scope and budget; skill run records are stored as versioned interaction-layer payloads with sources, model, steps, result and interruption state.

**Hooks and skills**:

- Hooks: scheduled (e.g. every day at 21:00), events (recording sealed, episode ended, day ended, mail graded important), manual. A hook starts either a core pipeline or a Pi run with a skill.
- Skill texts are maintained in `agents/skills/`, shared by Pi and external agents.
- Example: daily review = scheduled hook + daily review skill + Pi, with the result saved as a draft revision of the day digest; if you enable notifications, a system notification is sent when it finishes (off by default, 21:00 by default), and nothing is sent when there is nothing to report.

**Feature catalog**: `core/workflows/catalog.json` is the single feature catalog, registering core features, skills and hooks. Each entry has an ID, name, way, input (sources and layer), triggers, role, output artifact type, default switch, default daily budget and whether remote models are allowed. The settings page, AI page and external interface all read it.

**Context threshold**: on the evaluation set (2.9), search recall@10 of at least 0.8 (at least 30 queries), noise-reduction spot-check precision of at least 0.9, and every point of the day digest leading back to its source with no fabrication in spot checks (proposed values, to be fixed once a baseline exists). Conversations on the AI page are usable as soon as they connect, always with citations; hook-started skills (such as the daily review) are enabled by default only after the threshold is met, and can be run by hand before that.

<a id="ai-settings"></a>

### 4.2 Settings and management

| Place | Manages |
|---|---|
| Settings → Models | Providers, models, role assignment, fallbacks, global budget (exists) |
| Settings → AI features | One row per catalog entry: switch, hook and time, notification, sources read, role used, daily budget, last run and result |
| Each source on the Connections page | Whether AI may read this source and send it to remote models; mail scope and rules |

> Progress: model settings, the people entry, separate settings for fixed pipelines and skills, durable run records, hooks and notifications are connected. Skill runs persist across pages, can be cancelled/retried and their sources inspected; every call rechecks the frozen permissions and quota. Screen text increments, low-weight corrections, rule episodes and cited summaries are connected to the derived layer. Automatic skills still wait for a qualifying quality report; notifications are off by default.

- Which sources a feature may read is the intersection of feature settings and source authorization; whether it may use remote models is the strictest of feature settings, source authorization and role destination. Model services on this machine (loopback addresses), whether marked on-device or self-hosted, need no per-source approval (design decision: privacy rests on the user's own choice without excessive ceremony).
- Conversation runs may refresh permissions and model scope, but must carry the conversation's bounded history and local time; short follow-ups continue the topic. Mail tasks list `mail.message` directly and expand the originals, never substituting mail keywords seen on screen or app time. Ordinary answers do not volunteer capture gaps, and screen gaps are never taken to mean missing mail. The first page omits empty optional paging parameters; real cursors must be used with the same tool and filters.
- When freezing a run, the app includes mail capability status: when mail is not connected, not authorized or out of scope for this run, it says so with the way to fix it; lack of access is never taken to mean there is no mail, and closed sources' identifiers, addresses or text are never exposed.
- Design decision: agenda and reminders are managed by the agent; the manual create and edit forms are removed, keeping display, "Ask AI" and proposal approval.
- **The AI page is where AI is used in the app**, without switches. Two columns:
  - Left, "Today", top to bottom: calendar (system calendars, read-only), reminders (system Reminders due today or overdue, plus AI candidates in dashed boxes that can be "Added to Reminders" or dismissed), waiting for you (pending proposals, speakers and reference candidates to confirm), review (the latest daily review); a bottom row shows running tasks, which can be opened to cancel, retry or see why they failed;
  - Right, conversation: talk with Pi to find, organize, retitle and mark references; any item on the left can be "Asked about" and brought into the conversation as context; writes and proposals appear one by one in the conversation — writes within the library happen directly and can be undone, proposals outside the app need your approval; conversations stay on this machine, and "History" groups them by today/yesterday/earlier, searchable, each showing a title (the first question by default, renamable), time, a preview of the last answer and the number of cited records, deletable, restoring the cards when opened;
  - below 900 pt of window width the two columns become a "Today / Conversation" switch at the top;
  - the home timeline is for the past, the AI page for today and what's next; the calendar does not enter the timeline.
- **Reading and editing the agenda**: EventKit reads system calendars and reminders without storing them, fetching today's and overdue items each time the AI page opens; all calendars by default, excludable on the Connections page. Pi can only propose changes (4.3), and approving a deletion asks for confirmation again.
- **Skills and long operations**: "Skills" at the top of the AI page lists the skills the attached Pi can run (name, one-line description, trigger, enabled, run) and core pipelines; per-feature settings live in Settings → AI features. "Review today" runs the daily review skill (Pi reads every allowed source and saves a day draft with `pv_save_draft`), no longer only recordings. Reviews, briefings, running skills and other operations that may take a while turn into progress plus "Cancel" when clicked, and cancel aborts the round; everything clickable on the page is a button, capsule or icon button.
- **Presenting the conversation**: a line above Pi's answer says what it looked up ("Looked at 3 records · recording Nov 12 15:30, 2 mails", expandable to show each call and its parameters); while a call runs it shows "Searching for 'budget'…"; the body is rendered as Markdown; Pi writes citations as `[[event_id]]` per the system prompt, and the UI shows them as clickable "Recording Nov 10 10:14" that jump to that moment on the timeline, converting bare ids too; typing @ cites a record as context.
- The voiceprint library moves from the AI page to "People": first as a settings page, later to a dedicated people page.
- Every AI output shows who produced it (rules, a model or Pi), which records it relies on (clickable back to the source), and when; each can be edited, confirmed or rejected, with changes written to `corrections` and kept after recomputation.
- Old revisions of derived results: keep the latest, those you confirmed or edited, and those referenced by proposals; other old drafts are cleaned up after 30 days.

<a id="commitments"></a>

### 4.3 Commitments and reminders

1. Extraction after an episode ends (2.5) produces candidates for commitments, requests and deadlines, stored as `memories` (`kind = commitment`, candidate).
2. Candidates appear in the daily review and in "Waiting for you" on the AI page, to confirm, edit or dismiss.
3. Confirming creates a proposal (`create_reminder` or `create_calendar_event`); after you approve each one, the app writes to Reminders or Calendar through system APIs and the receipt goes back to the proposal. No automatic approval for now. Implementation: the app-only `POST /local/agenda` (always refused with a token) lists pending items and moves proposals through `pending → approved → executed | failed` (failures can be retried or rejected); approval is recorded in the core before writing to the system, and the receipt (`receipt.item_id`) or failure reason is written back; "Add to Reminders" on the AI page = confirm candidate + create proposal + approve.
4. Your handling feeds the next extraction; "never came up again" does not mean done.

<a id="references-skill"></a>

### 4.4 References and reply lookup

- Mail, files, episodes and messages you sent can be "saved as reference", optionally with tags ("leave reply", "expense process"); stored as `memories` (`kind = reference`, confirmed when you create it), keeping only references and tags while originals stay where they are.
- Mail you sent and text you revised several times automatically become reference candidates, counting only once you confirm.
- The skill "look up references and reply templates" searches references and related episodes for a question and returns sourced originals plus a reply draft; the draft is only for copying or editing, never sent. The attached Pi and external AI use the same skill.

<a id="executors"></a>

## 5. Where things run

The Rust store keeps jobs and results, and the service schedules and routes models; speech and other models are called through external service APIs; the app never starts a Python worker or manages its environment. Swift handles system capabilities, settings and status display. The attached agent (4.1): Pi runs in RPC mode as an app-managed process, with the UI drawn by the AI page and tools only in our extension's three tiers — read, in-library write and propose; Codex, Claude Code and other external AI use the same skills and tools through MCP/API, and reading pi sessions stays. The core has no agent backend or tool loop.

<a id="research"></a>

## 6. Research questions

This section lists only questions and comparison methods, no default implementation.

### 6.1 Memory and persona

Purpose first, then structure:

- **What is the profile for**: adjusting how answers are given, judging what is worth a reminder, giving advice, or serving as background for external agents? Different purposes need entirely different structures and evidence thresholds.
- **Layers**: facts, preferences, behaviour patterns, traits. How high is each layer's evidence threshold, how long is it valid, and who may write it?
- **Change**: past views, current views and model guesses are stored separately (already in the intelligence spec); how are conflicts surfaced to you?
- **Injection**: a resident core profile must be small and capped; everything else is retrieved on demand.
- **Consolidation and review**: periodically distil candidates into long-term memory and give you a reviewable change note; a conclusion settles only after appearing in at least two independent situations.
- **Forgetting**: deleting a memory or a source deletes what was derived from it, through derivation links, with a preview.
- **Permissions**: profile and persona inference are off by default and need your separate confirmation; local models preferred; external clients get a separate scope, closed by default.

Comparison method: run two or three candidate approaches offline on your data with local models — e.g. "structured records with evidence", "an editable markdown profile", "both combined" — let you judge which is more accurate and useful, then write the spec.

### 6.2 "What you care about"

Signals:

- frequency and duration of topics and entities;
- deliberate actions: marking, searching, revisiting, dwelling;
- the subjects of commitments and decisions.

To research: how to weight the signals, how they decay over time, and how to tell "passively seen" from "actively followed".

### 6.3 Proactive interruptions

What is worth a notification, how often at most, and how it fits with focus. Until then, task results appear only inside the app; the only exception is a hook you enabled notifications for (such as the daily review, 4.1), off by default.

<a id="references"></a>

## 7. References and trade-offs

| Source | Borrowed | Not adopted |
|---|---|---|
| screenpipe | Task file = configuration plus prompt; checklist items with stable IDs, checked against your handling each time; evidence rules such as "not seen again is not done"; compact activity summaries | Runners with a terminal, so permissions are not a real boundary; memory as a flat list of entries |
| MineContext | Context categories (entity, activity, intent, knowledge, procedure, state) as a reference for extraction output; image-similarity dedup with batching; an "is the information sufficient" check in the Q&A loop | A vision model on every screenshot; deleting old entries when merging; no user corrections |
| Mail access (Littlebird, Nylas, EmailEngine, EagleFiler, DEVONthink, community Apple Mail MCPs, MailKit) | Reading Mail.app's local data with Full Disk Access; opening the index read-only and recognizing its schema; MailKit as a live supplement for new mail | Per-provider OAuth; cloud or paid unified mail APIs; Mail.app plug-ins; third-party mail MCPs running on interpreters |
| OpenClaw | Readable and editable profile and memory; capped injected content; scheduled consolidation with review notes; isolating writes from questionable sources; staying quiet when there is nothing to report; running only in idle and active periods | Security resting only on prompt instructions; forgetting that cannot reach rewritten content |

<a id="plan"></a>

## 8. Implementation slices and acceptance

Complete and continuous recording comes first, and the fixed pipeline loop follows once the media foundation is ready; UI and external reads can proceed in parallel on existing interfaces. Entities and near dedup in section 2 are candidates and do not justify starting every feature at once (the mail connector is implemented); memory and persona remain research only.
