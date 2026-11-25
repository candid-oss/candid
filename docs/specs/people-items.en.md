# People and Items

[中文](people-items.md) · **English**

**Status**: Partly implemented. People resolution, LLM person judgment, item extraction and state transitions, speaker labeling, and memos all have code and tests. A few corroboration rules, pinyin candidates, and automatic approval of items are not implemented.

This spec defines the behavior of people (aliases, merging, candidates, external form), speaker labeling, items (schedule items, reminders, commitments), and memos (facts about people and things). It does not cover: scheduling, budget, and retries of the model calls used for item extraction and person judgment (see [models.en.md](models.en.md)); conversations and messages in the curated layer (see [curation.en.md](curation.en.md)); parameters and output fields of the `person` and `items` tools (see [query.en.md](query.en.md)); or the commitment-reminder skill (see [workflows.en.md](workflows.en.md)).

<a id="overview"></a>

## 1. Overview

Messages in the curated layer ([curation.en.md](curation.en.md)) carry a sender. The people layer resolves "who sent it" to a stable person. The item layer extracts agreements that have a time and a state from conversations. Memos record facts about people and things. All three live in `candid-store` in the Rust core, and every write goes through the core.

| Part | Stored in | Produced by | Model call |
|---|---|---|---|
| People | `entities` (`kind=person`), `entity_aliases`, `corrections`, `messages.sender_entity_id` | Rule-based union-find; LLM judgment only adds candidates and merges that have corroboration | Only person judgment (`summary` role, background) |
| Items | `items`, `item_changes`, `item_sources` | Operations extracted by the `summary` role, applied after rule checks | One extraction call |
| Memos | `memories` (`kind=fact`, `status=candidate`), `memory_sources` | The `memo` in the extraction output | Same as item extraction |
| Speakers | `corrections(target_type='speaker')`, `voice` aliases | Voice service matching + your assignment | None |

<a id="decided-rules"></a>

## 2. General rules

- An LLM verdict of "same person" plus at least one corroborating rule merges automatically. With only the LLM verdict, it becomes a candidate in "Needs your attention". An automatic merge can be undone in one click, and undoing records "not the same person".
- Schedule items, reminders, and commitments go in the `items` table (they have a time and a state, are written to the calendar, and are not recomputed wholesale). Memos go in `memories` (`kind=fact`) and attach to a person.
- When an item already written to the calendar or reminders is rescheduled or cancelled in chat, only an update/delete proposal is generated and waits for your approval. Items still in the candidate stage are still updated automatically.
- No new model role: person judgment and item extraction use `summary`. AI reads only the curated layer and the understanding layer by default.
- A speaker recognized by voiceprint and a person entity are two different things. An automatic match is only a candidate link. It becomes a confirmed link after you assign it.

<a id="people"></a>

## 3. People

<a id="alias-identity"></a>

### 3.1 Aliases are the stable identity

`entity_id` changes on recompute and aliases do not, so **user corrections attach to aliases**, not to `entity_id`. An alias key is written as `kind:alias` (`handle:wechat/张三`, `email:<address or hash>`, `name:张三`, `voice:<id>`). There is one fixed entity for "me" (`ppl-self`, key `self`), and every message with `actor=self` attaches to it. Person ids created by code start with `ppl-`. People created elsewhere, such as in the voiceprint library, keep their ids and are not deleted.

| Source | alias_kind | alias form |
|---|---|---|
| Direct-chat conversation name, group-chat sender | `handle` | `wechat/张三`, `qq/张三` (with app prefix, normalized: trim whitespace, full-width to half-width, strip emoji, lowercase) |
| Email address | `email` | Lowercase address |
| Email display name, a name you gave in a recording | `name` | Normalized name |
| Voiceprint | `voice` | Person id from the voice service |

When an email display name carries its own address (like "张三 ZHANG San - 20250001@example.edu", sent on behalf of people through a shared sender address such as a course system), the key is built from the name instead of the sender address. This avoids merging several people behind one address. Generic speaker names such as "说话人 N" (Speaker N) and "unknown" get no key. The person you named "我" (me) in the voiceprint library is you.

<a id="merge-rules"></a>

### 3.2 Merge rules (in order)

1. **Direct rule merge** (`source=rule`, takes effect automatically): the same email address; the same voiceprint id; direct-chat conversations with the same name in the same app; a group-chat sender and a direct-chat conversation name in the same app that are identical after normalization.
2. **Your corrections** (hard constraints): merge, split, rename, and reject are written to `corrections(target_type='entity')`, with the alias keys in `value_json`. "Not the same person" is an edge that cannot be connected.
3. **Rule veto** (no merge, and not sent to the LLM): two names that both spoke in the same group; two different email addresses with different display names; any pair you marked "not the same person".
4. **LLM judgment** (`summary` role): the remaining pairs worth asking about, see [3.3](#candidates).
5. **When an LLM result takes effect**: a "same person" verdict with at least one corroboration merges automatically (`source=model`); otherwise it is a candidate. "Different" is also recorded to avoid asking again. "Unsure" is not asked again for 30 days (`UNSURE_MS`).

Corroboration conditions: appearing together in the same thread or meeting; the email prefix matches the name's pinyin; the same matter mentioned from different sources in the same period. The code implements the first two conditions in the form of "same email thread or direct chat" and "email prefix matches name" (the pinyin table covers only common surnames and given-name characters; an unknown character counts as no corroboration). Meeting co-occurrence and mentioning the same matter in the same period are (not implemented).

Your correction actions (`corrections`, `target_id` is an alias key):

| action | target_id | value_json |
|---|---|---|
| `merge` | key A | `{"with": key B}` |
| `split` (not the same person) | key A | `{"from": key B}` |
| `rename` | any key of that person | `{"name": "…"}` |
| `reject` / `confirm` | any key of that person | `{}` |

Recompute flow: rule union-find (seeded with people from the voiceprint library) → apply corrections (merge edges, forbidden edges, renames) → apply cached LLM verdicts → backfill `messages.sender_entity_id` and the counterpart of items. Recompute does not call a model and only reads the cache. It is triggered only by a new alias, a changed correction or verdict, or a change to people in the voiceprint library. A new message with only known aliases just backfills `sender_entity_id`.

<a id="candidates"></a>

### 3.3 How candidate pairs arise and how they are asked

Only rules find pairs worth asking about: exactly the same name (across apps, `same_name`); one name contains the other (`contains`); an email display name equals a chat name (`mail_name`); a recording speaker name equals an alias (`speaker_name`). In addition, names with the same words in a different order, or names with a note attached, are treated as exact matches, not as containment. Pairs that are vetoed by rules, already corrected by you, already in the same person, or already have an unexpired answer are not asked again. Pinyin and fuzzy matching as a source of candidates are (not implemented); pinyin is used only for the corroboration above.

| Item | Value | Location |
|---|---|---|
| Max pairs per call | 10 (`BATCH`) | `people/judge.rs` |
| Max batches queued per scan | 5 | `people/judge.rs` |
| Material given to the model per pair | Aliases on both sides, apps and groups they appeared in, topics of the latest 4 messages on each side (each cut to 80 characters) | `people/judge.rs` |
| Output | `same | different | unsure` plus a one-sentence reason | `core/prompts/people-judge.md` |
| Cache key | Hash of both alias sets + prompt version and text + model; any change asks again | `PeopleVerdict.cache_key` |
| Feature | `people-judge` (`summary`, off by default, 50 calls/day by default, no remote), job `people.judge`, at most 3 retries | `core/workflows/catalog.json` |
| Trigger | When a new alias appears and there are candidate pairs, queued in the background | |

The unified rules for scheduling, budget, and retries are in [models.en.md](models.en.md).

<a id="people-external"></a>

### 3.4 External form

- `person` tool: a person card = aliases, recent interactions (messages), open items (`open_items`), memos (`memos`), and automatic merges within that person. Items and memos are looked up by alias key, so they still point to the same person after a recompute. For parameters and fields see [query.en.md](query.en.md).
- The App's person correction entries (merge, split, rename, reject, confirm) and the "Maybe the same person" cards in "Needs your attention": `candidates` (the LLM judged same person but corroboration is missing; choose merge or not the same person) and `auto_merged` (merged automatically, can be undone). Both read only cached verdicts. On the Swift side there is `PeopleCandidatesModel.swift`.
- The direction of merging between the voiceprint library and the people page is unchanged, see [3.6](#speaker-labeling).

<a id="entities"></a>

### 3.5 Entity layer

People, projects, organizations, places, sites, and apps are all entities, stored separately from events and episodes.

```sql
CREATE TABLE entities (
  entity_id  TEXT PRIMARY KEY,
  kind       TEXT NOT NULL CHECK (kind IN ('person','project','org','place','site','app','topic')),
  name       TEXT NOT NULL,
  status     TEXT NOT NULL CHECK (status IN ('candidate','confirmed','merged','rejected')),
  merged_into TEXT REFERENCES entities(entity_id),
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE entity_aliases (       -- 名字、邮箱、账号、域名、bundle id、声纹
  entity_id  TEXT NOT NULL REFERENCES entities(entity_id) ON DELETE CASCADE,
  alias      TEXT NOT NULL,
  alias_kind TEXT NOT NULL CHECK (alias_kind IN ('name','email','handle','domain','bundle','voice','other')),
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

- **Resolution order**: exact alias (email, account, domain, bundle id); normalized name plus context rules (same thread, same meeting); only when still ambiguous, ask a small model, and mark the result as a candidate. The specific merge rules for people follow [3.2](#merge-rules). Resolution of non-person entities such as projects, organizations, and places has no corresponding implementation (unverified).
- **Your corrections come first**: merge, split, rename, and reject are all written to `corrections` and stay in effect after derived content is regenerated.
- A speaker recognized by voiceprint and a person entity are two different things. An automatic match is only a candidate link. After you assign "who this is" in a recording, it becomes a confirmed link, recorded as an alias with `alias_kind = voice` (the voiceprint person id of the voice service) and written to `corrections`.
- `entity_mentions` is cascade-deleted when people are recomputed, so items and memos do not attach to it. They attach to alias keys.

<a id="speaker-labeling"></a>

### 3.6 Speaker labeling

1. In the speaker list of a recording's detail view, each speaker is in one of four states: recognized (matched automatically, marked "自动" (Auto), can be changed), possibly someone (waiting for your click), assigned by you, or anonymous (the default, needs no action).
   - Each speaker shows a **match score**: the similarity to the closest of that person's samples (the voice service's cosine similarity converted to a percentage), and the gap to the runner-up.
   - The decision keeps the voice service's thresholds: at or above the match threshold and ahead of the runner-up by enough is "Auto"; slightly below the threshold or not far enough ahead is "Possibly"; the rest are anonymous.

     | Constant | Value | Location |
     |---|---|---|
     | `MATCH_THRESHOLD` | 0.65 | `speakers.rs`, same as the defaults in the voice service's `speaker_store.py` and `media_worker.py` |
     | `MATCH_MARGIN` | 0.05 | Same |
     | `MAYBE_BELOW` | 0.10 (how far below the threshold still shows "Possibly") | `speakers.rs` |

     The thresholds follow the voice service. The interface does not set its own.
   - The match score is stored together with the decision in the speaker label, so it can be recomputed after changing the model or the thresholds. Your assignments are not affected by recompute.
2. Click a speaker and choose "who this is": an existing person, a new person, or stay anonymous (`label_speaker`).
3. Each of your assignments becomes a sample for that person: the 3 clearest segments of that speaker (the voice service's `sample_limit` takes 1–3, default 3); at most 20 per person, keeping only the latest beyond that (unverified, no cap of 20 found in the code); only confirmed ones enter the samples. Asking you to identify yourself at the first recording is (unverified).
4. The voiceprint library is now just one person list (`speaker_people`: name, voiceprint id, time of latest assignment; sample counts and the "easily confused" counterparts are provided by the voice service, unverified). You can rename, merge, and delete people, delete samples one by one, or clean up by condition (keep only the latest N, delete samples that came from a given recording).
5. Voiceprint vectors and sample audio stay in the voice service. Rename, merge, and delete are initiated from the core and synchronized (`voice_library_changed`). Deleting a person deletes their samples too, and speakers in existing transcripts return to anonymous.
6. **Whole-recording cleanup**: physical segments and model chunks are used only internally. A recording first unifies its anonymous speakers, then matches against the existing voiceprint library; it also works with no registered people. When short utterances lack evidence, the attribution is left undecided, and no new person is created from file labels. The optional merge strength (`conservative | balanced | merge_more`) and speaker cap (0–20) are advanced settings, default `balanced` and 0. Original text, positions, raw recognition, and manual naming are kept. Source withdrawal, deletion, and configuration changes apply equally to the projection and to voiceprint references.
7. **Automatic matching**: new recordings are transcribed and split by speaker automatically by default. Each speaker's vector in each conversation stays in the voice service, and the transcript carries only references. After you confirm a person and save samples, or rename, merge, or delete in the people list, all recordings are re-matched against the current voiceprint library without re-transcribing. The result is saved separately and does not change the transcript itself. Deleting a recording also cleans up its vectors.
8. The `sender` of a recording message is the display name, or "说话人 N" (Speaker N) when there is no name. `sender_entity_id` is filled only when the match is confirmed or automatic and a person entity exists. It is recomputed when the media projection revision changes, and follows your speaker edits automatically.

<a id="items"></a>

## 4. Items

<a id="items-storage"></a>

### 4.1 Storage (timeline library v12)

```sql
CREATE TABLE items (
  item_id      TEXT PRIMARY KEY,
  kind         TEXT NOT NULL CHECK (kind IN ('event','reminder','commitment')),
  owner        TEXT NOT NULL CHECK (owner IN ('self','other')),   -- 我答应的 / 等别人做的
  title        TEXT NOT NULL,
  when_text    TEXT,                    -- 原话里的时间说法
  start_ms INTEGER, end_ms INTEGER,
  all_day      INTEGER NOT NULL DEFAULT 0,
  counterpart  TEXT,                    -- 对方别名键；人物合并后自动指向新实体
  conversation_id TEXT REFERENCES conversations(conversation_id) ON DELETE SET NULL,
  review       TEXT NOT NULL CHECK (review IN ('candidate','confirmed','rejected')),
  state        TEXT NOT NULL CHECK (state IN ('open','done','cancelled','expired')),
  user_fields  TEXT NOT NULL DEFAULT '[]',  -- 你改过的字段，模型不再覆盖
  external_ref TEXT,                    -- EventKit 标识（经提案写入后回填）
  producer TEXT NOT NULL, model_ref TEXT, prompt_version TEXT,
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL
) STRICT;                               -- 索引：items_open(state, review, start_ms)、items_conversation
CREATE TABLE item_changes (            -- 每次新增、更新、完成、作废一行，带依据
  change_id  TEXT PRIMARY KEY,
  item_id    TEXT NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  op         TEXT NOT NULL CHECK (op IN ('create','update','done','cancel','merge','expire','user')),
  fields_json TEXT NOT NULL,
  "by"       TEXT NOT NULL CHECK ("by" IN ('model','rule','user','calendar')),   -- 列名要加引号
  created_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE item_sources (
  change_id  TEXT NOT NULL REFERENCES item_changes(change_id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
  PRIMARY KEY (change_id, message_id)
) STRICT;
```

The DDL is in `curated.rs`. Trigger: when `item_sources` loses the last evidence of a change, that `item_changes` row is deleted. If the deleted row is the `op='create'` one, the whole item is deleted together with its other changes (when the source is deleted, the item disappears). Changes with no evidence (`by=user`, `by=calendar`) are not affected.

<a id="item-extract"></a>

### 4.2 Extraction (`summary` role, background channel)

Feature `item-extract` (off by default, 150 calls/day by default, `summary`, no remote), job `items.extract` (priority P1), prompt `core/prompts/item-extract.md` (a file of the same name under `<data root>/prompts/` overrides it).

- **Trigger** (rules only, `items_scan`): a chat conversation with no new message for 10 minutes (`CHAT_IDLE_MS`) and no queued `screen.excerpt` for that conversation; a recording transcript (`meeting` conversation) quiet for 1 minute (`MEETING_SETTLE_MS`); email is queued only when the event payload has `level=important`. The first scan after the feature is turned on records a baseline and handles only messages that arrive afterward. Extraction for a conversation is not triggered until its excerpt jobs are cleared.
- **Input**: new messages + the 20 preceding context messages (each with send time and day of week, each cut to 500 characters); open items with this counterpart (≤15, with item_id); items you ignored in the last 30 days; the current date. Messages behind protected events (`sensitivity='protected'`) are not read.
- **Cursor**: one per conversation (`items.cursor/1:<conversation>`), at most 40 new messages at a time. Messages beyond the limit, or accumulated while queued, are combined into a larger batch.
- **Cache key**: input message ID set + open-item snapshot + prompt version and text + model. The latest raw reply of each conversation is stored in `items.raw/1:<conversation>` for comparing prompts.
- **The output is operations**: `create | update(item_id) | done(item_id) | cancel(item_id) | memo`, each with `when_text`, an ISO time, and the sequence numbers of the evidence messages. Only explicit agreements and facts are extracted; "did not appear again later" is not treated as done. An operation whose evidence message is not in the input is rejected, and the others proceed as usual.
- **Counterpart**: when a conversation has only one `sender_entity_id` besides yourself, that person is used, and the item's `counterpart` stores their alias key (preferring handle, then email, name, voice). At query time it expands to all of that person's alias keys. A group chat (several people) has no counterpart and relies on the conversation and the "same time + similar title" dedup in [4.3](#item-dedup).
- **Time**: parsed relative to the send time of the evidence message. The model gives an ISO time with a time zone (no offset means the local time zone; a date alone means all day). A time more than a day earlier than the evidence message is discarded; for a schedule item the whole item is therefore discarded, while reminders and commitments stay undated. A time field like this in an `update` is skipped. A reminder with no specific time stays undated.
- **Memos**: see [Section 5](#memos).
- The unified rules for retries and budget overrun are in [models.en.md](models.en.md). Item extraction is not dropped; when over budget, the time window is widened and combined into larger batches.

<a id="item-dedup"></a>

### 4.3 Dedup and cross-source merging (rules)

| Rule | Value |
|---|---|
| Within the same conversation | Show open items to the model; the model outputs update instead of create |
| Cross-source, with a counterpart | Same counterpart and start times within 30 minutes (`MERGE_WINDOW_MS`) → the same item, append the source (`op=merge`) |
| Cross-source, no counterpart | Same start time, and title trigram similarity ≥ 0.5 (`SAME_TITLE`) |
| Previously ignored | A `create` that hits the same merge condition as an item you ignored (`review=rejected`, `state=cancelled`) within 30 days (`IGNORED_DAYS`) is dropped directly |

<a id="item-states"></a>

### 4.4 State transitions

| Current | Model's update / done / cancel | Your action |
|---|---|---|
| Candidate | Takes effect directly | Confirm (→ proposal → EventKit), edit, ignore |
| Confirmed, already written to the calendar or reminders | **Only generates a proposal** (`update_calendar_event`, `delete_calendar_event`, `complete_reminder`, etc.); changes only after approval | Edit directly, recorded as `by=user` |

- For a confirmed item with `external_ref`, the proposal payload carries `candid_item` and `item_set`. The item changes according to the receipt only after the App reports the execution (`by=calendar`). Only the latest pending proposal of the same type is kept for one item. A confirmed item not yet written to the calendar follows the model.
- An event past its end time is marked `expired` by rule (with no end time it counts as 1 hour after the start, `DEFAULT_EVENT_MS`). A candidate left unhandled for 7 days is automatically collapsed to `expired` (`CANDIDATE_DAYS`; it can still be found with `state=expired`). This runs once on every read of items (`items_maintain`) and writes a change with `op=expire`, `by=rule`.
- Fields you changed are written to `user_fields` (recordable fields: `title`, `when_text`, `start`, `end`, `all_day`), and later model updates skip those fields.
- Automatic approval of proposals is not done (not implemented, and not planned).

<a id="item-presentation"></a>

### 4.5 Presentation

The "Needs your attention" section of the AI page has three groups: schedule items, reminders, and memos, plus "Maybe the same person" and pending proposals. Outputs:

- `items` tool: returns only open items by default, and can filter by `state` (`open | done | cancelled | expired | all`), `kind`, and `owner`. Items with `review=rejected` are not returned. Only items with at least one evidence message visible to the client are returned. It is in the MCP catalog and the Pi tool list.
- `open_items` and `memos` of the `person` tool (require the curated-layer read scope).
- The App-only `/local/agenda`: `today` carries `items` (candidates) and `memos`; the actions are `item` (confirm / ignore / done / edit) and `memo` (confirm / ignore). For the reminder proposal flow of commitments (`memories.kind=commitment`), see [workflows.en.md](workflows.en.md). On the Swift side there is `AIAgenda.swift`.

Replay: `excerpt_replay extract select|apply`, `entities` (example programs, used only on copies of real libraries).

<a id="memos"></a>

## 5. Memos

A memo is a fact about a person or thing. It is stored in `memories` (`kind=fact`, `status=candidate`), and `memory_sources` points to the events behind the evidence messages. The person it belongs to is an alias key, stored in `store_meta` as `memo.person/1:<memory_id>` (not `entity_mentions`, which is cascade-deleted on person recompute). Until you confirm or ignore it (the `memo` action of `/local/agenda`), it is only a candidate. Memos appear in the `memos` of the `person` tool and in the memo group of "Needs your attention".

The `kind` enum of `memories` is `fact | preference | judgment | commitment | person | project | decision | question | reference`, and the `status` enum is `candidate | confirmed | edited | rejected | expired | superseded`. Item extraction writes only `fact`.

<a id="code"></a>

## 6. Implementation locations

| What | Code | Tests |
|---|---|---|
| Alias keys, union-find, corrections, recompute, backfill | `core/crates/candid-store/src/people.rs` | `core/crates/candid-store/tests/people.rs` |
| Candidate pairs, corroboration, LLM verdicts, candidate list | `src/people/judge.rs`, `src/people/pinyin.rs`, `core/prompts/people-judge.md` | `tests/people_judge.rs` |
| `person` tool, App correction and candidate entries | `src/query/people.rs` | `tests/people.rs`, `tests/people_judge.rs` |
| Items: applying operations, dedup, state, expiry | `src/items.rs`, `src/curated.rs` (DDL) | `tests/items.rs` |
| Item extraction: trigger, input, job | `src/item_extract.rs`, `core/prompts/item-extract.md` | `tests/items.rs` |
| `items` tool | `src/query/items.rs` | `tests/items.rs` |
| `/local/agenda` (items, memos, proposals) | `src/query/agenda.rs` | `tests/agenda.rs`, `tests/items.rs` |
| Speaker labeling, voiceprint library sync | `src/speakers.rs`, `src/voiceprints.rs` | `tests/speakers.rs` |
| Whole-recording speaker cleanup | `src/recording_speakers.rs`, `services/voice/` | `tests/speakers.rs`, `services/voice/tests` |
| Feature entries (off by default, limits, no remote) | `core/workflows/catalog.json` | `tests/items.rs`, `tests/people_judge.rs` |
| Full lifecycle | `core/fixtures/lifecycle/` | `core/crates/candid-service/tests/lifecycle.rs` |
| Interface | `apps/macos/Sources/Candid/PeopleCandidatesModel.swift`, `AIAgenda.swift` | Existence checked, behavior unverified |

For table definitions see `schema.rs` and `schema_upgrade.rs` (`SCHEMA_VERSION = 12`).

<a id="open"></a>

## 7. Not implemented and undecided

| Item | Status | Notes |
|---|---|---|
| Corroboration: co-occurrence in the same meeting | Not implemented | The code looks only at shared email threads and approximate direct chats |
| Corroboration: the same matter mentioned from different sources in the same period | Not implemented | |
| Pinyin and fuzzy matching as a source of candidate pairs | Not implemented | Pinyin is currently used only for email-prefix corroboration; other fuzzy algorithms are not in this version |
| At most 20 voiceprint samples per person; asking you to identify yourself at the first recording | Unverified | No cap of 20 found in the core; samples are in the voice service |
| Sample count and "easily confused" in the voiceprint library people list | Unverified | The core returns only name, id, and latest assignment time |
| Resolution of non-person entities (projects, organizations, places, sites, apps) | Unverified | The table has `kind`, but no corresponding merge code was found |
| Automatic approval of items | Not implemented | Not planned for now |
| Inferring person attributes (relationship, job title) | Not implemented | Not in this version |
| Full modeling of group-chat member tables | Not implemented | Not in this version |
| Vector search | Not implemented | Not in this version |
| Daily limits of the two features (50, 150) | In design | Adjust after a week of running, based on actual usage in `ai_calls` |

Each item should have a matching entry in [roadmap.md](../roadmap.md).

<a id="sources"></a>

## 8. Sources

- §0 (decisions already made on people and items), §1 people layer, and §2 item layer of the [Data layer plan v1](../plans/data-layer-v1.md). The decisions themselves are recorded in the [decision log](../decisions/log.md).
- §2.6 entity layer and speaker labeling of [agent-platform-v1](agent-platform-v1.md) (the anchors `entities` and `speaker-labeling` are kept as they are).
- The rules still in force in §5 (M4) of the [AI features v1 plan](../plans/ai-v1-plan.md). Its items 2 and 4 are superseded by decisions in the data layer plan and are no longer kept.
