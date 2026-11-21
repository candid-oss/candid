# 数据层与 AI 处理规划 v1：人物、条目、整理层来源、模型调用

> 工作规划，不是规格。2026-11-21 与用户讨论后确定。接在 [AI 功能 v1 计划](ai-v1-plan.md) 的「AI 可读层」和 M2/M3 之后，替代其中 M4「人物」一条和 M1「模型调用」一节的粗略写法。原则不变：先打通再优化；不新增模型角色（复用 `vision`、`summary`、`agent_chat`，以及已存在的 `episode`、`title`）；质量靠样例回放加人工看，不建严格评估集；AI 默认只读整理层和理解层。

## 0. 已拍板的决定

| 议题 | 决定 |
|---|---|
| LLM 判断的人物合并何时自动生效 | LLM 判「同一人」**且**至少一条规则佐证才自动合并；只有 LLM 判断时作为候选进「待你处理」。自动合并可一键撤销，撤销记为「不是同一人」 |
| 条目存哪里 | 新建 `items` 表放日程、提醒、承诺（有时间和状态，会写进日历，不整表重算）；备忘（关于人和事的事实）放 `memories`（`kind=fact`）并挂到人 |
| 已写入日历/提醒的条目被聊天改期或取消 | 只生成 update/delete 提案，等你批准；候选阶段的条目仍自动更新 |
| 实施顺序 | 调度地基 → 不调模型的来源 → 人物规则层 → 条目抽取 → LLM 人物候选 → 活动块 |

## 1. 人物层（实体归一）

### 1.1 别名是稳定身份

entity_id 在重算时会变，别名不会。所以**用户纠正挂在别名上**，不挂在 entity_id 上。沿用现有 `alias_kind`，不改表结构：

| 来源 | alias_kind | alias 形式 |
|---|---|---|
| 私聊会话名、群聊发送人 | `handle` | `wechat/张三`、`qq/张三`（带应用前缀，规范化：去首尾空白、全角转半角、去表情） |
| 邮箱 | `email` | 小写地址 |
| 邮件显示名、录音里你起的名字 | `name` | 规范化名字 |
| 声纹 | `voice` | 语音服务的人员编号（已有） |

固定一个「我」的实体，所有 `actor=self` 的消息都挂它。

### 1.2 归并规则（按顺序）

1. **规则直接归并**（`source=rule`，自动生效）：同一邮箱；同一声纹编号；同一应用里同名的私聊会话；同一应用里群聊发送人与私聊会话名规范化后完全相同。
2. **你的纠正**（硬约束）：合并、拆分、改名、拒绝写进 `corrections(target_type='entity')`，`value_json` 记别名键。「不是同一人」是一条不能连接的边。
3. **规则否决**（不合并，也不送 LLM）：同一个群里两个名字都发过言；两个不同邮箱且显示名不同；你标过「不是同一人」的组合。
4. **LLM 判断**（`summary` 角色）：剩下值得问的组合——跨应用同名、邮件显示名与聊天名（「San Zhang」/「张三」）、备注名与昵称、录音说话人名与联系人。
5. **LLM 结果生效条件**：判「同一人」且至少一条佐证——共同出现在同一线程或会议、邮箱前缀与名字拼音吻合、同一时段不同来源提到同一件事——自动合并（`source=model`）；否则作为候选。「不同」也记下，避免重复问；「不确定」30 天内不再问。

重算流程：规则并查集 → 套用纠正（合并边、禁止边、改名）→ 套用缓存的 LLM 结论 → 回填 `messages.sender_entity_id` 和条目的对方。重算不调模型，只读缓存。

### 1.3 候选对怎么来

v1 只用规则找值得问的组合：完全同名（跨应用）、一方包含另一方、邮件显示名等于某个聊天名、录音说话人名等于某个别名。拼音和模糊匹配以后再加。每批最多 10 对，每对给两边的别名、出现过的应用和群、最近几条消息的话题；输出 `same | different | unsure` 和一句理由。

### 1.4 对外

- `person` 工具：人物卡 = 别名、最近往来（消息）、未结条目、备忘。
- 「待你处理」加「可能是同一人」卡片：合并 / 不是同一人。
- 声纹库与人物页合并的方向不变（基座 §2.6）。

## 2. 条目层

### 2.1 存储（时间线库 v12）

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

已在时间线库 v12 落地（`curated.rs` 的 DDL）。与草案相比只加了 `all_day` 的默认值、`conversation_id` 的外键（对话删了置空）和索引。触发器：`item_sources` 删掉某个变更的最后一条依据，这条 `item_changes` 删除；被删的是 `op='create'` 那行，整条 item 连同其余变更一起删除。没有依据的变更（`by=user`、`by=calendar`）不受影响。


### 2.2 抽取（`summary` 角色，后台通道）

- **触发**：一个会话 10 分钟没有新消息，并且这个窗口没有在排队的摘录任务；重点级别的邮件到达；录音结束。
- **输入**：新消息 + 前 20 条上下文（每条带发出时间和星期几）；与这个对方之间未结的条目（≤15 条，带 item_id）；最近 30 天被你忽略的条目；当前日期。
- **输出是操作**：`create | update(item_id) | done(item_id) | cancel(item_id) | memo`，每个带 `when_text`、ISO 时间、依据的消息序号。只抽明确的约定和事实；「后来没再出现」不当作完成。
- **时间**：以依据消息的发出时间为基准解析。规则检查：早于依据消息一天以上的时间丢弃；没有具体时间的提醒保持无日期。
- **备忘**：`memo` 写成 `memories(kind=fact, status=candidate)`，`memory_sources` 指向依据事件，挂到对方实体。

### 2.3 去重与跨来源合并（规则）

- 同一会话内：靠把未结条目给模型看，模型输出 update 而不是 create。
- 跨来源：同一对方、开始时间相差 30 分钟内 → 同一条，追加来源（`op=merge`）；没有对方时要求时间相同并且标题三元组相似度 ≥ 0.5。
- 与你忽略过的条目命中同一合并条件的 `create` 直接丢弃。

### 2.4 状态流转

| 当前 | 模型的 update / done / cancel | 你的操作 |
|---|---|---|
| 候选 | 直接生效 | 确认（→ 提案 → EventKit）、修改、忽略 |
| 已确认、已写入日历或提醒 | **只生成提案**（`update_calendar_event`、`delete_calendar_event`、`complete_reminder` 等），批准后才改 | 直接改，记为 `by=user` |

- 已过结束时间的日程由规则标 `expired`；7 天没处理的候选自动收起（`expired`，可找回）。
- 你改过的字段写进 `user_fields`，模型以后的 update 跳过这些字段。

### 2.5 呈现

AI 页「待你处理」分日程、提醒、备忘三栏，加「可能是同一人」与待批准提案。`items` 工具对外提供（默认只返回 open 的）。

**实现状态（步 4，2026-11-21）**：代码在 `candid-store` 的 `item_extract.rs`（触发、输入、作业）和 `items.rs`（操作应用、去重、状态、提案联动），提示词 `core/prompts/item-extract.md`，功能目录项 `item-extract`（默认关闭，150/天，`summary`，不许远端）。几处具体取舍：
- 触发：聊天空闲 10 分钟且该会话没有排队中的 `screen.excerpt`；录音转写（`meeting` 会话）静默 1 分钟；邮件只有事件 payload 的 `level=important` 才入队。开启功能后的第一次扫描记下基线，只处理之后到达的消息。每个会话一个游标（`items.cursor/1:<会话>`），一次最多 40 条新消息，超限或排队期间累积的消息合成更大的批次。
- 对方：会话里除自己外只有一个 `sender_entity_id` 时取这个人，条目的 `counterpart` 存他的别名键（优先 handle，其次 email、name、voice）；查询时展开成这个人的全部别名键，所以人物重算、合并后仍指向同一人。群聊（多人）没有对方，靠会话和「时间相同 + 标题三元组相似度 ≥ 0.5」去重。
- 时间：模型给带时区的 ISO（无偏移按本机时区，只有日期算全天）；早于依据消息一天以上的时间丢弃——日程因此整条丢弃，提醒/承诺保持无日期；`update` 里这样的时间字段跳过。
- 已确认且有 `external_ref` 的条目，模型的 update/done/cancel 只生成 `update_*`/`complete_reminder`/`delete_*` 提案（payload 带 `candid_item` 和 `item_set`），App 回报执行后条目才按回执变化（`by=calendar`）；同一条目同类待批提案只留一份最新的。
- 备忘：`memories(kind=fact, status=candidate)`，来源是依据消息背后的事件；所属的人是别名键，存在 `store_meta` 的 `memo.person/1:<memory_id>`（`entity_mentions` 随人物重算会被级联删除，不适合）。
- 用户忽略（`review=rejected`、`state=cancelled`）后 30 天内相似的 `create` 丢弃；用户在界面改过的字段进 `user_fields`，模型不再覆盖。
- 出口：`items` 工具（默认只返回 open；MCP 目录、Pi 工具清单、`pv_items`）、`person` 工具的 `open_items` 和 `memos`、`/local/agenda` 的 `today` 带 `items`（候选）和 `memos`，动作 `item`（confirm/ignore/done/edit）和 `memo`（confirm/ignore）；AI 页「待你处理」分日程、提醒、备忘三组。
- 回放：`excerpt_replay extract select|apply`。

## 3. 整理层补齐的来源

| 来源 | 做法 | 模型 |
|---|---|---|
| 录音说话人 | `media_recognitions` 对话结果里的标签 → `messages.sender`，通过 `voice` 别名填 `sender_entity_id`；你改说话人后重投影 | 不调 |
| AI 会话 | `ai.prompt` 事件 → `conversations.kind='agent'`（v12 放宽 CHECK），你写的提示词为 self 消息；AI 回答按引用型连接器的设计不入库，经 `evidence` 现读 | 不调 |
| 浏览器等 AX 能读到正文的应用 | `screen.text` 中 AX 主内容区（网页区域）的文字，按 (应用, URL 或标题) 和访问时段（间隔 > 30 分钟算新一段）合并为 `excerpts`，段落去重复用 `screen_cleanup`；AX 读不到正文的应用仍走截图摘录 | 不调 |
| 活动块 | 规则切时段（5 分钟间隔，沿用现有屏幕片段）→ 一句话概括，**只读该时段的消息、摘录和应用时长统计**，不读原始 OCR；配 `day` 工具 | `episode` 角色，每块 1 次 |

**实现状态（v12，2026-11-21）**：三项都已接入 `curated_sync`。录音说话人在媒体投影 revision 变化时重算（你改说话人、重新匹配后自动跟随）；`sender` 为显示名，没有名字时为「说话人 N」，`sender_entity_id` 只在已确认或自动匹配、且有人物实体时填。AI 会话每个 session 一个 `agent` 对话，id 即 `session_ref`。AX 摘录只收浏览器和文档类应用（见 `curated_screen.rs` 的 `AX_APPS`）；AI 聊天站点与应用、无痕窗口不收（AI 回答不入整理层）；微信、QQ 等聊天客户端的 AX 文本是气泡列表，仍走截图摘录。

前三项先在本机真实库副本上确认数据形态（尤其 AX 是否带网页区域、`ai.prompt` 是否在采），再写投影。

## 4. 模型调用规划

### 4.1 调用清单

| # | 调用 | 角色 | 触发 | 粒度 | 估算/天 | 默认上限/天 | 缓存键 | 优先级 |
|---|---|---|---|---|---|---|---|---|
| 1 | 帮我写、屏幕问答 | agent_chat | 快捷键 | 1 次 | 5–30 | 不限 | 不缓存 | P0 交互 |
| 2 | 帮我写时的当前帧摘录 | vision | 同上 | 1 帧 | 同上 | 不限 | 与 #4 共用 | P0 交互 |
| 3 | AI 页 Pi 对话、手动技能 | agent_chat | 用户 | 多步 | 按运行预算 | 每次运行冻结 | — | P0 交互 |
| 4 | 截图摘录 | vision | 新关键帧 | 1 帧 | 150–300 | 300（已有） | 图像哈希 + OCR 哈希 + 提示词版本 + 模型 | P1 后台 |
| 5 | 条目抽取 | summary | 会话空闲 10 分钟、重点邮件、录音结束 | 每会话一批 | 30–80 | 150 | 输入消息 ID 集合 + 未结条目快照 + 提示词版本 + 模型 | P1 后台 |
| 6 | 录音标题、摘要（现有） | title / summary | 录音结束 | 每段录音 | 0–5 | 现有 | 现有 | P2 后台 |
| 7 | 人物判断 | summary | 新别名有候选对 | ≤10 对一批 | 0–10（首次回填约 20 批） | 50 | 两边别名集合哈希 + 提示词版本 + 模型 | P3 后台 |
| 8 | 活动块概括 | episode | 活动块结束 | 每块 | 20–40 | 60 | 输入哈希 + 提示词版本 + 模型 | P3 后台 |
| 9 | 日回顾（现有技能） | agent_chat | 21:00 或手动 | 1 次运行 | 0–1 | 运行预算 | — | P2 |

估算基于 2026-11-20 的调研（微信约 9.5k 屏幕事件/周）；跑一周后按 `ai_calls` 的实际数修正上限。

### 4.2 依赖顺序

```
OCR → 关键帧 → 截图摘录(#4) ─┐
邮件 / 录音转写 / AI 会话 / AX 摘录（投影，不调模型）─┴→ messages / excerpts
  → 规则人物归一（不调模型）→ 条目抽取(#5) → items / 备忘
  → 人物判断(#7，异步，不阻塞抽取；合并后条目对方经别名自动跟随)
  → 活动块概括(#8，等该时段摘录处理完)
```

同一个会话：摘录任务未清空前不触发它的抽取。

### 4.3 调度

- 把现在唯一的执行槽（`candid-service/src/workflows.rs` 的 `Execution`）拆成**交互**和**后台**两条通道：交互调用不再等后台；后台串行，按上表优先级取任务，同级按时间先后。
- 本机模型同时只能高效跑一个：建议本机把 `vision` 与 `summary` 都配成 gemma-4-26B-A4B（它也能处理纯文字），避免 LM Studio 来回换模型；后台按角色成批取任务，减少切换。这是设置建议，不改角色。
- 后台在交互调用进行时让路（本机模型时），电池低于 20% 时暂停 P2/P3。

### 4.4 预算

- 每个功能一个每日调用上限（`catalog.json` 的 `default_daily_calls`，设置页可改），本地调用也计数。
- 云端另设全局每日金额上限，默认 0；功能默认 `allow_remote: false`，`agent_chat` 以用户配置为准。
- 超上限：截图摘录留到空闲时补做，超过 24 小时放弃；条目抽取不丢，把时间窗拉长合成更大的批次；人物判断、活动块顺延到第二天。

### 4.5 失败与重试

| 情况 | 处理 |
|---|---|
| 超时、模型离线 | 退避 30 秒、2 分钟、10 分钟，最多 3 次，然后 `failed` 并在界面显示原因 |
| 输出不是合法 JSON 或不符合结构 | 附「只输出符合结构的 JSON」重试一次，再失败放弃 |
| 缺模型、未授权、超预算 | 现有 `waiting_*` 状态，不静默改用远端 |
| 结果回来时源已改变或撤权 | 丢弃（现有 `workflow_reply_allowed`） |
| 引用越界（依据消息不在输入里） | 拒绝这条操作，其余照常 |

### 4.6 验证

每类调用各存一组本机样例（`excerpt_replay` 扩展子命令 `extract`、`entities`），改提示词或换模型后并排对比新旧输出，人工看。用量按功能看 `ai_calls`。

## 5. 实施顺序

| 步 | 内容 | 调模型 |
|---|---|---|
| 1 | 调度地基：两条通道、优先队列、每功能每日上限与超限策略、统一重试 | — |
| 2 | 不调模型的来源：录音说话人、AI 会话、AX 摘录；时间线库 v12（`conversations.kind` 加 `agent`，新建 `items`、`item_changes`、`item_sources`） | 否 |
| 3 | 人物规则层：别名生成、并查集、纠正约束、回填 `sender_entity_id`、`person` 工具、人物纠正界面 | 否 |
| 4 | 条目抽取：提示词、操作应用、去重合并、状态流转、「待你处理」三栏、提案联动、`items` 工具 | 是（#5） |
| 5 | LLM 人物候选：候选对生成、批量判断、佐证规则、「可能是同一人」卡片 | 是（#7） |
| 6 | 活动块概括与 `day` 工具 | 是（#8） |
| 7 | 优化：关键帧只裁变化区域、拼音候选、按实际用量调上限 | — |

每一步结束都在本机真实库副本上回放验证。

## 6. 不在这一版

人物属性推断（关系、职位）、条目的自动批准、群聊成员表的完整建模、向量检索、拼音以外的模糊匹配算法。
