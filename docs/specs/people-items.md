# 人物与条目

**中文** · [English](people-items.en.md)

**状态**：部分实现。人物归一、LLM 人物判断、条目抽取与状态流转、说话人标注、备忘均已有代码和测试；少数佐证规则、拼音候选、条目自动批准等未实现。

本文规定人物（别名、归并、候选、对外形态）、说话人标注、条目（日程、提醒、承诺）和备忘（关于人和事的事实）的行为。不规定：条目抽取与人物判断所用模型调用的调度、预算、重试（见 [models.md](models.md)）；整理层的对话与消息（见 [curation.md](curation.md)）；`person`、`items` 工具的参数与输出字段（见 [query.md](query.md)）；承诺提醒技能（见 [workflows.md](workflows.md)）。

<a id="overview"></a>

## 1. 概览

整理层的消息（[curation.md](curation.md)）带发送者。人物层把「谁发的」归成稳定的人；条目层从对话里抽出有时间和状态的约定；备忘记录关于人和事的事实。三者都在 Rust 核心的 `candid-store` 里，全部写入经核心。

| 部分 | 存放 | 谁产生 | 调模型 |
|---|---|---|---|
| 人物 | `entities`（`kind=person`）、`entity_aliases`、`corrections`、`messages.sender_entity_id` | 规则并查集；LLM 判断只补候选与佐证后的合并 | 只有人物判断（`summary` 角色，后台） |
| 条目 | `items`、`item_changes`、`item_sources` | `summary` 角色抽取出的操作，经规则检查后应用 | 抽取一次调用 |
| 备忘 | `memories`（`kind=fact`，`status=candidate`）、`memory_sources` | 抽取输出里的 `memo` | 同条目抽取 |
| 说话人 | `corrections(target_type='speaker')`、`voice` 别名 | 语音服务匹配 + 你的指定 | 不调 |

<a id="decided-rules"></a>

## 2. 总则

- LLM 判「同一人」且至少一条规则佐证才自动合并；只有 LLM 判断时作为候选进「待你处理」。自动合并可一键撤销，撤销记为「不是同一人」。
- 日程、提醒、承诺放 `items` 表（有时间和状态，会写进日历，不整表重算）。备忘放 `memories`（`kind=fact`）并挂到人。
- 已写入日历或提醒的条目被聊天改期或取消，只生成 update/delete 提案，等你批准；候选阶段的条目仍自动更新。
- 不新增模型角色：人物判断与条目抽取用 `summary`。AI 默认只读整理层和理解层。
- 声纹识别出的说话人和人物实体是两件事：自动匹配只是候选关联，你指定后才是确认的关联。

<a id="people"></a>

## 3. 人物

<a id="alias-identity"></a>

### 3.1 别名是稳定身份

`entity_id` 在重算时会变，别名不会，所以**用户纠正挂在别名上**，不挂在 `entity_id` 上。别名键写作 `kind:alias`（`handle:wechat/张三`、`email:<地址或哈希>`、`name:张三`、`voice:<id>`），固定一个「我」的实体（`ppl-self`，键 `self`），所有 `actor=self` 的消息都挂它。代码创建的人物 id 以 `ppl-` 开头；声纹库等别处创建的人物沿用，不会被删。

| 来源 | alias_kind | alias 形式 |
|---|---|---|
| 私聊会话名、群聊发送人 | `handle` | `wechat/张三`、`qq/张三`（带应用前缀，规范化：去首尾空白、全角转半角、去表情、转小写） |
| 邮箱 | `email` | 小写地址 |
| 邮件显示名、录音里你起的名字 | `name` | 规范化名字 |
| 声纹 | `voice` | 语音服务的人员编号 |

邮件显示名自带地址（形如「张三 ZHANG San - 20250001@example.edu」，由课程系统等共用发件地址代发）时按名字而不是发件地址建键，避免一个地址后面的多人被合并。「说话人 N」「unknown」等通用说话人名不建键。声纹库里你命名为「我」的人就是你自己。

<a id="merge-rules"></a>

### 3.2 归并规则（按顺序）

1. **规则直接归并**（`source=rule`，自动生效）：同一邮箱；同一声纹编号；同一应用里同名的私聊会话；同一应用里群聊发送人与私聊会话名规范化后完全相同。
2. **你的纠正**（硬约束）：合并、拆分、改名、拒绝写进 `corrections(target_type='entity')`，`value_json` 记别名键。「不是同一人」是一条不能连接的边。
3. **规则否决**（不合并，也不送 LLM）：同一个群里两个名字都发过言；两个不同邮箱且显示名不同；你标过「不是同一人」的组合。
4. **LLM 判断**（`summary` 角色）：剩下值得问的组合，见 [3.3](#candidates)。
5. **LLM 结果生效条件**：判「同一人」且至少一条佐证，自动合并（`source=model`）；否则作为候选。「不同」也记下，避免重复问；「不确定」30 天内不再问（`UNSURE_MS`）。

佐证条件：共同出现在同一线程或会议；邮箱前缀与名字拼音吻合；同一时段不同来源提到同一件事。代码实现前两条里的「同一邮件线程或私聊」和「邮箱前缀与名字吻合」（拼音表只覆盖常见姓氏和名字用字，不认识的字等于没有佐证）；会议共现、同时段提到同一件事（未实现）。

你的纠正动作（`corrections`，`target_id` 为别名键）：

| action | target_id | value_json |
|---|---|---|
| `merge` | 键 A | `{"with": 键 B}` |
| `split`（不是同一人） | 键 A | `{"from": 键 B}` |
| `rename` | 该人任一键 | `{"name": "…"}` |
| `reject` / `confirm` | 该人任一键 | `{}` |

重算流程：规则并查集（以声纹库人物为种子）→ 套用纠正（合并边、禁止边、改名）→ 套用缓存的 LLM 结论 → 回填 `messages.sender_entity_id` 和条目的对方。重算不调模型，只读缓存。别名新增、纠正或结论变化、声纹库人物变化才触发重算；只有已知别名的新消息只回填 `sender_entity_id`。

<a id="candidates"></a>

### 3.3 候选对怎么来、怎么问

只用规则找值得问的组合：完全同名（跨应用，`same_name`）、一方包含另一方（`contains`）、邮件显示名等于某个聊天名（`mail_name`）、录音说话人名等于某个别名（`speaker_name`），另外词相同顺序不同或带备注的名字按完全匹配处理，不按包含。被规则否决、已被你纠正、已在同一人里、已有未过期回答的组合不再问。拼音和模糊匹配做候选（未实现；拼音只用于上面的佐证）。

| 项 | 值 | 位置 |
|---|---|---|
| 每次调用最多对数 | 10（`BATCH`） | `people/judge.rs` |
| 每次扫描最多排队批数 | 5 | `people/judge.rs` |
| 每对给模型的材料 | 两边别名、出现过的应用和群、每边最近 4 条消息（每条截 80 字）的话题 | `people/judge.rs` |
| 输出 | `same | different | unsure` 加一句理由 | `core/prompts/people-judge.md` |
| 缓存键 | 两边别名集合哈希 + 提示词版本与文本 + 模型；任一变化重新问 | `PeopleVerdict.cache_key` |
| 功能项 | `people-judge`（`summary`，默认关闭，默认 50 次/天，不许远端），作业 `people.judge`，最多重试 3 次 | `core/workflows/catalog.json` |
| 触发 | 出现新别名且有候选对时，后台排队 | |

调度、预算、重试的统一规则见 [models.md](models.md)。

<a id="people-external"></a>

### 3.4 对外形态

- `person` 工具：人物卡 = 别名、最近往来（消息）、未结条目（`open_items`）、备忘（`memos`）、该人内部的自动合并。条目和备忘按别名键查，所以人物重算后仍指向同一人。参数与字段见 [query.md](query.md)。
- App 的人物纠正入口（合并、拆分、改名、拒绝、确认）与「待你处理」里的「可能是同一人」卡片：`candidates`（LLM 判同一人但缺佐证，选合并或不是同一人）与 `auto_merged`（自动合并，可撤销）。两者都只读缓存的结论。Swift 侧有 `PeopleCandidatesModel.swift`。
- 声纹库与人物页合并的方向不变，见 [3.6](#speaker-labeling)。

<a id="entities"></a>

### 3.5 实体层

人、项目、组织、地点、网站、应用都作为实体，和事件、片段分开存。

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

- **归一顺序**：精确别名（邮箱、账号、域名、bundle id）；规范化后的名字加上下文规则（同一线程、同一会议）；仍然模糊时才问小模型，结果标为候选。人物的具体归并规则以 [3.2](#merge-rules) 为准；项目、组织、地点等非人物实体的归一没有对应实现（未验证）。
- **你的纠正优先**：合并、拆分、改名、拒绝都写进 `corrections`；重新生成派生内容后继续生效。
- 声纹识别出的说话人和人物实体是两件事：自动匹配只是候选关联；你在录音里指定「这是谁」后才是确认的关联，记为别名 `alias_kind = voice`（对应语音服务的声纹人员编号）并写入 `corrections`。
- `entity_mentions` 随人物重算会被级联删除，所以条目和备忘不挂在它上面，而挂在别名键上。

<a id="speaker-labeling"></a>

### 3.6 说话人标注

1. 录音详情的说话人列表里，每个说话人是四种状态之一：已认出（自动匹配，标「自动」，可改）、可能是某人（等你点一下）、你指定的、匿名（默认，不需要处理）。
   - 每个说话人显示**匹配度**：和此人样本中最像一段的相似度（语音服务的余弦相似度换算成百分比），并显示和第二名的差距。
   - 判定沿用语音服务的阈值：不低于匹配阈值且领先第二名足够多为「自动」；略低于阈值或领先不足为「可能是」；其余匿名。

     | 常量 | 值 | 位置 |
     |---|---|---|
     | `MATCH_THRESHOLD` | 0.65 | `speakers.rs`，与语音服务 `speaker_store.py`、`media_worker.py` 的默认值一致 |
     | `MATCH_MARGIN` | 0.05 | 同上 |
     | `MAYBE_BELOW` | 0.10（低于阈值多少以内仍提示「可能是」） | `speakers.rs` |

     阈值以语音服务为准，界面不另设。
   - 匹配度和判定一起存进说话人标注，换模型或改阈值后可以重算；你指定的不受重算影响。
2. 点一个说话人选「这是谁」：已有的人、新建一个人，或保持匿名（`label_speaker`）。
3. 你的每次指定都成为这个人的样本：取这个说话人最清楚的 3 段（语音服务 `sample_limit` 取 1–3，默认 3）；每人最多 20 份、超过只留最近的（未验证，没有在代码里找到 20 的上限）；只有确认过的才进样本。第一次录音时请你认一次自己（未验证）。
4. 声纹库只剩一张人员列表（`speaker_people`：名字、声纹编号、最近指定时间；样本数和「容易混」的对象由语音服务提供，未验证）：可以改名、合并、删除人，逐条删除样本或按条件清理（只留最近 N 条、删掉某次录音来的样本）。
5. 声纹向量和样本音频仍在语音服务；改名、合并、删除从核心发起并同步（`voice_library_changed`）。删除一个人时连同样本删除，已有转写里的说话人回到匿名。
6. **整条录音整理**：物理分段和模型分块只在内部使用，一次录音先统一匿名说话人，再匹配已有声纹库；没有已登记人物也可用。短句证据不足时保留未定归属，不按文件标签新增人物；可选合并强度（`conservative | balanced | merge_more`）和人数上限（0–20）属于高级设置，默认 `balanced`、0。原文、位置、原始识别及人工命名保留，来源撤回、删除和配置变化对投影与声纹引用同样生效。
7. **自动匹配**：新录音默认自动转写并分说话人；每段对话里每个说话人的向量留在语音服务、转写只带引用。你确认一个人并存下样本，或在人员列表里改名、合并、删除后，所有录音按当前声纹库重新匹配，不用重新转写；结果单独保存，不改转写本身。删除录音时一并清理它的向量。
8. 录音消息的 `sender` 为显示名，没有名字时为「说话人 N」；`sender_entity_id` 只在已确认或自动匹配、且有人物实体时填。媒体投影 revision 变化时重算，你改说话人后自动跟随。

<a id="items"></a>

## 4. 条目

<a id="items-storage"></a>

### 4.1 存储（时间线库 v12）

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

DDL 在 `curated.rs`。触发器：`item_sources` 删掉某个变更的最后一条依据，这条 `item_changes` 删除；被删的是 `op='create'` 那行，整条 item 连同其余变更一起删除（来源删了条目就消失）。没有依据的变更（`by=user`、`by=calendar`）不受影响。

<a id="item-extract"></a>

### 4.2 抽取（`summary` 角色，后台通道）

功能项 `item-extract`（默认关闭，默认 150 次/天，`summary`，不许远端），作业 `items.extract`（优先级 P1），提示词 `core/prompts/item-extract.md`（`<数据根>/prompts/` 下同名文件可覆盖）。

- **触发**（只靠规则，`items_scan`）：聊天会话 10 分钟没有新消息（`CHAT_IDLE_MS`）且该会话没有排队中的 `screen.excerpt`；录音转写（`meeting` 会话）静默 1 分钟（`MEETING_SETTLE_MS`）；邮件只有事件 payload 的 `level=important` 才入队。开启功能后的第一次扫描记下基线，只处理之后到达的消息。同一会话的摘录任务未清空前不触发它的抽取。
- **输入**：新消息 + 前 20 条上下文（每条带发出时间和星期几，单条截 500 字）；与这个对方之间未结的条目（≤15 条，带 item_id）；最近 30 天被你忽略的条目；当前日期。受保护事件（`sensitivity='protected'`）背后的消息不读。
- **游标**：每个会话一个（`items.cursor/1:<会话>`），一次最多 40 条新消息；超限或排队期间累积的消息合成更大的批次。
- **缓存键**：输入消息 ID 集合 + 未结条目快照 + 提示词版本与文本 + 模型。每个会话最近一次原始回复存 `items.raw/1:<会话>`，用于对比提示词。
- **输出是操作**：`create | update(item_id) | done(item_id) | cancel(item_id) | memo`，每个带 `when_text`、ISO 时间、依据的消息序号。只抽明确的约定和事实；「后来没再出现」不当作完成。依据消息不在输入里的操作被拒绝，其余照常。
- **对方**：会话里除自己外只有一个 `sender_entity_id` 时取这个人，条目的 `counterpart` 存他的别名键（优先 handle，其次 email、name、voice）；查询时展开成这个人的全部别名键。群聊（多人）没有对方，靠会话和 [4.3](#item-dedup) 的「时间相同 + 标题相似」去重。
- **时间**：以依据消息的发出时间为基准解析。模型给带时区的 ISO（无偏移按本机时区，只有日期算全天）；早于依据消息一天以上的时间丢弃——日程因此整条丢弃，提醒和承诺保持无日期；`update` 里这样的时间字段跳过。没有具体时间的提醒保持无日期。
- **备忘**：见 [第 5 节](#memos)。
- 失败重试、预算超限的统一规则见 [models.md](models.md)；条目抽取不丢，超限时把时间窗拉长合成更大批次。

<a id="item-dedup"></a>

### 4.3 去重与跨来源合并（规则）

| 规则 | 值 |
|---|---|
| 同一会话内 | 把未结条目给模型看，模型输出 update 而不是 create |
| 跨来源，有对方 | 同一对方、开始时间相差 30 分钟内（`MERGE_WINDOW_MS`）→ 同一条，追加来源（`op=merge`） |
| 跨来源，无对方 | 开始时间相同，且标题三元组相似度 ≥ 0.5（`SAME_TITLE`） |
| 被忽略过的 | 与你忽略过（`review=rejected`、`state=cancelled`）的条目命中同一合并条件、且在 30 天内（`IGNORED_DAYS`）的 `create` 直接丢弃 |

<a id="item-states"></a>

### 4.4 状态流转

| 当前 | 模型的 update / done / cancel | 你的操作 |
|---|---|---|
| 候选 | 直接生效 | 确认（→ 提案 → EventKit）、修改、忽略 |
| 已确认、已写入日历或提醒 | **只生成提案**（`update_calendar_event`、`delete_calendar_event`、`complete_reminder` 等），批准后才改 | 直接改，记为 `by=user` |

- 已确认且有 `external_ref` 的条目，提案 payload 带 `candid_item` 和 `item_set`；App 回报执行后条目才按回执变化（`by=calendar`）；同一条目同类待批提案只留一份最新的。已确认但还没写入日历的条目跟随模型。
- 已过结束时间的日程由规则标 `expired`（没有结束时间按开始后 1 小时算，`DEFAULT_EVENT_MS`）；7 天没处理的候选自动收起为 `expired`（`CANDIDATE_DAYS`，仍可用 `state=expired` 找回）。每次读取条目时跑一次（`items_maintain`），写一条 `op=expire`、`by=rule` 的变更。
- 你改过的字段写进 `user_fields`（可记录的字段：`title`、`when_text`、`start`、`end`、`all_day`），模型以后的 update 跳过这些字段。
- 自动批准提案不做（未实现，也不在计划内）。

<a id="item-presentation"></a>

### 4.5 呈现

AI 页「待你处理」分日程、提醒、备忘三组，加「可能是同一人」与待批准提案。出口：

- `items` 工具：默认只返回 open，可按 `state`（`open | done | cancelled | expired | all`）、`kind`、`owner` 过滤；`review=rejected` 的不返回；只返回至少一条依据消息对访问方可见的条目。已进 MCP 目录、Pi 工具清单。
- `person` 工具的 `open_items` 和 `memos`（需要整理层读取范围）。
- App 专用 `/local/agenda`：`today` 带 `items`（候选）和 `memos`；动作 `item`（confirm / ignore / done / edit）和 `memo`（confirm / ignore）。承诺（`memories.kind=commitment`）的提醒提案流程见 [workflows.md](workflows.md)。Swift 侧有 `AIAgenda.swift`。

回放：`excerpt_replay extract select|apply`、`entities`（示例程序，只用于真实库的副本）。

<a id="memos"></a>

## 5. 备忘

备忘是关于人和事的事实，存 `memories`（`kind=fact`、`status=candidate`），`memory_sources` 指向依据消息背后的事件。所属的人用别名键，存在 `store_meta` 的 `memo.person/1:<memory_id>`（不用 `entity_mentions`，它随人物重算被级联删除）。你确认或忽略（`/local/agenda` 的 `memo` 动作）之前只是候选。备忘出现在 `person` 工具的 `memos` 和「待你处理」的备忘组。

`memories` 的 `kind` 枚举为 `fact | preference | judgment | commitment | person | project | decision | question | reference`，`status` 枚举为 `candidate | confirmed | edited | rejected | expired | superseded`；条目抽取只写 `fact`。

<a id="code"></a>

## 6. 实现位置

| 内容 | 代码 | 测试 |
|---|---|---|
| 别名键、并查集、纠正、重算、回填 | `core/crates/candid-store/src/people.rs` | `core/crates/candid-store/tests/people.rs` |
| 候选对、佐证、LLM 结论、候选列表 | `src/people/judge.rs`、`src/people/pinyin.rs`、`core/prompts/people-judge.md` | `tests/people_judge.rs` |
| `person` 工具、App 纠正与候选入口 | `src/query/people.rs` | `tests/people.rs`、`tests/people_judge.rs` |
| 条目：操作应用、去重、状态、过期 | `src/items.rs`、`src/curated.rs`（DDL） | `tests/items.rs` |
| 条目抽取：触发、输入、作业 | `src/item_extract.rs`、`core/prompts/item-extract.md` | `tests/items.rs` |
| `items` 工具 | `src/query/items.rs` | `tests/items.rs` |
| `/local/agenda`（条目、备忘、提案） | `src/query/agenda.rs` | `tests/agenda.rs`、`tests/items.rs` |
| 说话人标注、声纹库同步 | `src/speakers.rs`、`src/voiceprints.rs` | `tests/speakers.rs` |
| 整条录音说话人整理 | `src/recording_speakers.rs`、`services/voice/` | `tests/speakers.rs`、`services/voice/tests` |
| 功能项（默认关闭、上限、不许远端） | `core/workflows/catalog.json` | `tests/items.rs`、`tests/people_judge.rs` |
| 全流程 | `core/fixtures/lifecycle/` | `core/crates/candid-service/tests/lifecycle.rs` |
| 界面 | `apps/macos/Sources/Candid/PeopleCandidatesModel.swift`、`AIAgenda.swift` | 存在性已核对，行为未验证 |

表结构见 `schema.rs`、`schema_upgrade.rs`（`SCHEMA_VERSION = 12`）。

<a id="open"></a>

## 7. 未实现与待定

| 项 | 状态 | 说明 |
|---|---|---|
| 佐证：同一会议共现 | 未实现 | 代码只看共同的邮件线程和近似私聊 |
| 佐证：同一时段不同来源提到同一件事 | 未实现 | |
| 拼音与模糊匹配做候选对 | 未实现 | 拼音目前只用于邮箱前缀佐证；其余模糊算法不在这一版 |
| 每人最多 20 份声纹样本、第一次录音请你认一次自己 | 未验证 | 核心里没有找到 20 的上限，样本在语音服务 |
| 声纹库人员列表的样本数、「容易混」 | 未验证 | 核心只返回名字、编号、最近指定时间 |
| 非人物实体（项目、组织、地点、网站、应用）的归一 | 未验证 | 表有 `kind`，没有找到对应归并代码 |
| 条目自动批准 | 未实现 | 暂不做 |
| 人物属性推断（关系、职位） | 未实现 | 不在这一版 |
| 群聊成员表的完整建模 | 未实现 | 不在这一版 |
| 向量检索 | 未实现 | 不在这一版 |
| 两个功能的每日上限（50、150） | 设计中 | 跑一周后按 `ai_calls` 的实际用量修正 |

每项在 [roadmap.md](../roadmap.md) 应有对应条目。

<a id="sources"></a>

## 8. 来源

- [数据层规划 v1](../plans/data-layer-v1.md) 的 §0（已拍板的决定中人物、条目部分）、§1 人物层、§2 条目层；决定本身记入 [决定流水](../decisions/log.md)。
- [agent-platform-v1](agent-platform-v1.md) 的 §2.6 实体层与说话人标注（锚点 `entities`、`speaker-labeling` 原样保留）。
- [AI 功能 v1 计划](../plans/ai-v1-plan.md) 的 §5（M4）里仍成立的规则；其第 2、4 条已被数据层规划的决定取代，不再保留。
