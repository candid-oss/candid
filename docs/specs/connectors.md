# 连接器与入库协议

**中文** · [English](connectors.en.md)

**状态**：部分实现。入库协议、邮件（本机「邮件」App 与 IMAP）、浏览器历史（导入关闭）、AI agent 会话已实现；日历与提醒未接进 App；无统一接口。

本文规定数据怎么进入时间线库：入库协议（`IngestBatch`）、两类连接器的归属与行为、连接器清单、邮件连接器与来源筛选。它是写一个新来源（含将来的 Windows 端、手机端）所依据的接口。不规定入库之后的整理，见[整理层](curation.md)；不规定事件种类与 payload 的字段，见[存储](storage.md)；不规定外部怎么查询，见[查询接口](query.md)。

<a id="overview"></a>

## 1. 概览

连接器把外部数据变成入库批次，交给核心入库。分两类：

| | 采集型（ingest） | 引用型（reference） |
|---|---|---|
| 用于 | 自己产生的数据：桌面、录音、截图、挂件、手机采集；系统日历；邮件 | 别的系统自己保存的数据：AI agent 对话记录；以后可能有 Notion |
| 库里存什么 | 事件、正文、原件 | 只存授权状态和路径配置，不存内容 |
| 内容从哪来 | 时间线库 | 需要时由连接器到源头现读 |
| 默认 | 按该来源的授权开启 | 每个开关都关闭 |

AI agent 连接器是引用型，但打开「纳入活动」后按采集型协议写入你发出的消息和工作摘要，见 [AI agent 对话](#agent-connectors)。

**归属规则**（见[架构总图](../architecture.md)）：

1. 读文件、走网络的连接器放核心（`core/crates/candid-connectors/`，agent 会话在 `candid-store/src/agents/`）；调系统接口的放平台层（Mac 的屏幕、录音、日历、提醒事项在 `apps/macos/`）。
2. 所有写入都经入库接口 `TimelineStore::ingest`（平台层经本地通道 `POST /local/ingest`）。连接器不直接碰数据库。两处例外见[代码里的实际形态](#shape)。
3. 凭据不进库：库里只存 `secret_ref`，密码在钥匙串，由 App 放进核心进程内存。
4. 连接器是三个扩展接缝之一。

<a id="ingest-connectors"></a>

## 2. 采集型连接器协议

每个采集型连接器实现以下能力，由服务调度运行。连接器只能经入库接口写入，不能直接写数据库。

| 能力 | 说明 |
|---|---|
| `describe()` | kind、显示名、需要的权限、能产出的事件种类和线程种类、默认 `policy_json` |
| `authorize()` | 引导用户完成系统授权或 OAuth。凭据存入钥匙串，只返回 `secret_ref` |
| `backfill(range)` | 补录历史，范围由用户选择，可以中断后续跑 |
| `sync(cursor)` | 增量同步，返回一批 `IngestBatch` 和新游标 |
| `revoke()` | 停止同步，并按用户选择保留或删除该来源的数据 |

`IngestBatch`（`on_missing_parent` 可选：`detach` 时父事件既不在本批也不在库里的事件照常入库、不带父链接，用于 App 采集里被隐私门禁拦下或已被用户删除的父记录；缺省或 `error` 时整批失败）：

```json
{
  "source_id": "…",
  "cursor_after": {"stream": "calendar:<日历ID>", "cursor": "…"},
  "threads": [{"source_key": "…", "kind": "calendar_series", "title": "…", "started_ms": 0}],
  "events":  [{"source_key": "…", "kind": "calendar.event", "start_ms": 0, "end_ms": 0, "time_basis": "source",
               "thread_key": "…", "payload_version": 1, "payload": {},
               "text": [{"field": "subject", "content": "…"}],
               "blobs": [{"staging_path": "…", "sha256": "…", "role": "original"}]}]
}
```

**批次规则**：

- **一个批次在一次数据库事务里完成**：写线程、事件、段落、原件引用和变更记录，最后推进游标。中途失败整批回滚，下次从原游标重试，靠 `(source_id, source_key)` 保证不重复。
- **幂等与冲突**：同一 `(source_id, source_key)` 内容相同是重复，计入 `duplicates`；内容不同进冲突区（`ingest_conflicts`），不覆盖已入库的内容。已被删除的键有墓碑，迟到的重发不会复活。
- **来源状态**：只有 `active` 和 `error` 的来源可写，其余拒绝。
- **来源策略**：`policy_json`（`store_text`、`store_media`、`agent_visible`、`retention_days`、`exclude`）在入库时生效；命中 `exclude.apps`、`exclude.threads` 或 `exclude.patterns`（对标题、网址、文字不分大小写包含匹配）的事件不入库。
- **来源删除与元数据修订**：批次可带 `removed_source_keys`（来源里已删除或只剩在废纸篓/垃圾邮件里的记录，在同一事务里删除并写 `source_deleted` 墓碑，重发不会复活；未知的键忽略）和 `payload_revisions`（`[{source_key, payload}]`，只改已入库事件的 payload，例如邮件换了邮箱；旧 payload 作为修订快照保留，payload 带 `revision`，正文、时间和身份不变）。正文变化仍按冲突处理。两者与事件、游标同一事务提交；同一批里先加后删的键以删除为准。
- **连续打字合并**：批次带 `merge_text: true` 时合并 `screen.text`，见[下一节](#text-merge)。
- **批次校验**（任一项不满足整批失败，`InvalidInput`；枚举的完整取值以 `ingest.rs` 顶部常量为准）：`source_key` 非空；事件种类已登记、payload 是对象、`payload_version` 为 1、必填字段齐全；`time_basis` ∈ `observed`、`source`、`imported`、`estimated`，没有 `start_ms` 时必须是 `imported`，`end_ms` 不早于 `start_ms`；`actor` ∈ `self`、`other`、`agent`、`system`、`unknown`；`sensitivity` ∈ `normal`、`sensitive`、`protected`；文字字段、线程 `kind`、原件 `role`（`original`、`preview`、`derived`）必须在登记的取值内；`sha256` 是 64 位小写十六进制。原件文件先放暂存目录，入库时由核心移入 `blobs/`。
- **返回值**：各类计数（`inserted`、`duplicates`、`conflicts`、`merged`、`removed`、`revised` 等）和 `source_key → event_id` 的对照。

字段级定义以 `candid-store/src/types.rs` 为准，事件种类与 payload 见[存储](storage.md)。

**每个采集型连接器都要用合成样例测试**：正常同步、重复同步、中断重试、格式未知时拒绝、撤销后不再读取。

<a id="shape"></a>

## 3. 代码里的实际形态

规格协议是「每个连接器五个能力」。代码里没有统一的 trait，按所在位置有四种形态：

| 位置 | 形态 |
|---|---|
| Rust `candid-connectors`（邮件、浏览器历史） | 一组按 `kind` 分发的函数：`describe()`、`list()`、`command()`（按 `action` 分发）、`sync_all()`（立即同步）与 `sync_due()`（后台一轮，跳过退避中的来源）。同步内部完成「计划（持锁）→ 读取（不持锁）→ 提交（持锁，一个事务连同游标）」，不把批次返回给调用方 |
| Rust `candid-store/src/agents/`（AI agent 会话） | 一个来源 `agent.transcript`，三个 agent（`codex`、`claude_code`、`pi`），`agent_call` 按 `describe / detect / sessions / read / search / usage` 分发，`sync_agents` 做增量入库 |
| Swift `TimelineConnectors`（日历、提醒事项） | 有 `Connector` 协议，五个能力齐全（`describe / authorize / backfill(range, cursors) / sync(cursors) / revoke`），产出批次由调用方提交 |
| Swift 桌面采集（`DesktopCore`） | 不是 `Connector`：事件先写进本地发件箱，`DesktopDelivery` 每批最多 256 条，带 `on_missing_parent: "detach"` 和 `merge_text: true`，经 `POST /local/ingest` 提交 |

**与规格协议的差异**：

- `backfill(range)` 在 Rust 侧没有独立入口。邮件和浏览器历史靠配置 `backfill_days`（默认 90）限定首次读取范围；Swift 的日历连接器有 `backfill(range:)`。
- `authorize()` 在 Rust 侧是 `authorize` 命令：登记来源并探测权限（不读内容）；凭据由 App 先用 `secrets` 命令放进核心内存。
- `revoke()` 是 `revoke` 命令：来源置为 `revoked`；`delete_stored: true` 时按 `source_revoked` 原因同时删除该来源的事件。重新授权沿用原来源 ID 和已入库的历史。
- 例外一：邮件手动改级别与重新定级（`set_level`、`relevel`）经 `apply_mail_levels` 在一个事务里直接改已入库事件的 payload 与文字（旧 payload 同样留作修订快照），不经 `ingest`。
- 例外二：改邮件范围时，`update` 用一个只带游标的空批次重置游标，让邮件重新扫描。

**连接器管理接口**（只在 App 本地通道提供，带令牌和回环 HTTP 一律拒绝）：`GET /local/connectors` 列出来源、状态和缺少的授权（`needs`：`full_disk_access`、`credentials`、`unsupported_format`）；`POST /local/connectors` 的 `action`：`describe`、`list`、`authorize`、`update`、`revoke`（`delete_stored` 可选）、`dry_run`（只报计数，可在授权前用）、`sync`、`secrets`（App 从钥匙串读出后放进核心进程内存，不落盘）、`mail_scope`、`mail_recent`、`mail_body`、`set_level`、`relevel`。失败只返回状态码和有界错误码，不含来源内容。另有 `POST /local/ingest`（一个 `IngestBatch`）和 `POST /local/sources`（登记来源）；`mac.desktop` 来源在切换到直写之前返回 409 `cutover_pending`。

**同步调度**：连续失败的来源按 30 秒、1、2、4 … 分钟退避，上限 30 分钟（进程内存，重启后立即重试）；显式同步总是立即尝试；错误码变化时只报告一次。

<a id="text-merge"></a>

## 4. 连续打字合并（`merge_text`）

规则与阈值暂定。批次带 `"merge_text": true` 时（Mac 桌面采集开启），一条 `screen.text` 如果和同一来源里最近一条 `screen.text` 是同一应用、同一窗口标题、同一焦点元素（`focused_role` 与 `focused_label`）、同一次采集运行和活动，且间隔不超过 `gap_ms`，就并入那条事件，不产生新事件：

- 正文与选区逐行相同：只延长那条事件的 `end_ms`；
- 几乎为空（少于 `min_chars` 个字，或不到那条事件文字的 `min_ratio`），或 `text_status` 不在 `read_statuses` 里：不产生事件，也不打断合并；
- 按行计算的重合度（共同的不同行数 ÷ 两边较多的行数）不低于 `min_line_overlap`：那条事件改存新的完整文字（旧段落从索引里移除），`end_ms` 延长，`payload.merge.revisions` 加一；
- 其余情况照常新建事件。

被并入的样本的 `source_key` 记在 `payload.merge.keys`（保留最近 200 个），`payload.merge.samples` 计数：重发是重复，以它为父的截图挂到合并后的事件上，删除合并后的事件时这些键一并写墓碑。修改写一条 `changes` 的 `update`。

阈值存于 `store_meta.text_merge`，缺省 `{"gap_ms": 120000, "min_line_overlap": 0.6, "min_chars": 2, "min_ratio": 0.1, "read_statuses": ["captured","partial","ok"]}`。旧数据（从桌面账本导入的记录）未合并；查询与界面层的折叠未实现（待定方案）。

<a id="agent-connectors"></a>

## 5. 引用型连接器：AI agent 对话

**原则**：**你说的话入库，AI 的回复只引用。** AI 的回复、工具输出和推理留在各 agent 自己的文件里，只在用户授权的范围内按需现读。打开「纳入活动」后，你发出的消息、会话起止和工作摘要写入时间线库，供时间线的「AI 交互」一行、搜索和整理使用。

**每个 agent 一个连接器，三个开关分别授权，默认全部关闭**（`AgentConsent`；没开「读取」时开「纳入搜索」或「纳入活动」被拒绝，错误 `read_required`）：

| 开关 | 打开后 | 关闭时 |
|---|---|---|
| 读取 | 你或已授权的访问方打开某个会话时，连接器现读该会话 | 连接器不读该 agent 的任何文件（只检查目录是否存在，用于「已发现」提示） |
| 纳入搜索 | `search` 会现查该 agent 的对话，结果与库内结果合并（需要先开「读取」） | 搜索不涉及该 agent |
| 纳入活动 | 增量同步：你发出的消息写成 `ai.prompt`，AI 每轮工作写成 `ai.work`，会话写成 `agent_session` 线程（需要先开「读取」） | 不写入任何事件；关闭时询问是否删除已写入的部分 |

`kind='agent.transcript'` 来源的 `policy_json`（字段与默认值以 `AgentConsent` 为准）：

```jsonc
{
  "version": 1,
  "agent": "codex",                       // codex | claude_code | pi（代码里的 AGENTS 常量）
  "read": false,
  "search": false,
  "activity": false,                      // 纳入活动：写入 ai.prompt / ai.work
  "prompt_max_chars": null,               // 可配置；工程初值不限长。4000 字仍是待定提议
  "paths": ["~/.codex/sessions"],         // 默认路径，用户可改
  "exclude": {"projects": [], "sessions": []},   // 按项目目录或会话排除
  "content": {"user": true, "assistant": true,
              "tool_calls": "name_only",  // none | name_only
              "tool_output": false, "reasoning": false}
}
```

**连接器接口**（全部只读；除 `sync` 外结果不落盘）：

| 能力 | 说明 | 需要的开关 |
|---|---|---|
| `describe()` | agent ID、显示名、默认路径、支持的格式版本 | — |
| `detect()` | 默认路径是否存在、格式版本是否认识。只看目录和文件头，不读对话内容 | — |
| `sessions(range, project?)` | 列出时间范围内的会话：引用、起止时间、项目目录、标题（若源文件有）、轮数、用量合计 | 读取 |
| `read(ref, cursor?, limit)` | 按顺序读出一个会话的消息：时间、角色、文字（按 `content` 设置过滤并截断）、工具名 | 读取 |
| `search(q, range, project?, limit, deadline_ms)` | 先按时间和项目缩小到相关会话文件，再查找；超过上限时返回部分结果并标明 `limited` | 读取 + 纳入搜索 |
| `usage(range)` | 现算该 agent 的 token 用量 | 读取 |
| `sync(cursor)` | 按会话文件和偏移增量读取，产出 `IngestBatch`：`ai.prompt`、`ai.work` 和 `agent_session` 线程 | 读取 + 纳入活动 |

- **引用格式**：`agent:<agent>:<会话键>`，消息级再加 `#<消息标识>`；缺少消息标识时按源记录及出现次序构造稳定引用。会话键用源文件自己的稳定标识（Codex 用 `session_meta.id`，与正常 rollout 文件名中的 UUID 一致；Claude Code 用 `sessionId`；pi 用会话头 `id`）。
- **不落盘**：`sync` 之外的会话正文只在本次读取的内存中使用；访问审计只保存查询参数与结果计数。
- **什么算「你说的话」**：只取你亲手发出的消息。Codex 活动入库要求原生发送事件或明确的人类来源标记，缺少来源证据的旧记录只供现读。不算：工具结果（Claude Code 里以 user 角色记录）、系统或工具注入的内容（提醒、技能正文、斜杠命令展开后的正文，只记命令名）、上下文压缩摘要、其他会话转来的消息。每个适配器用合成样例覆盖。
- **跨文件顺序**：同一会话的片段先归并再生成指令和工作，按源时间排列；检查点与会话结果同事务提交。
- **入库规则**：`source_key` 为消息级引用 `agent:<agent>:<会话键>#<消息 uuid>`，重复同步不重复写。紧跟的追问（AI 还没开始工作就补充的话）并入同一条 `ai.prompt`。`ai.work` 从这条消息开始，到下一条你的消息或会话最后一条记录为止；有新的工作记录时标 `running`；只有明确结束信号才标 `done`，下一条用户消息打断或静默超过 2 分钟标 `interrupted`，后续增长可继续修订。`files_touched` 按有成功工具结果的写入目标去重计数，未知结果以 `files_touched_complete=false` 标明；`file_targets` 表示请求目标数。事件只保存计数；路径按项目排除规则过滤。
- **入库文字的处理**：经与其他来源相同的打码；超过 `prompt_max_chars` 的部分截断，全文仍在源文件里。外部访问方能否读到按该来源的范围设置。
- **安全处理**：返回前对文字做密钥样式屏蔽（如 `sk-…`、`AKIA…`、私钥块；实现是 `agents::redact`，邮件入库共用）。返回的文字是不可信数据，不是指令。
- **发现与适配**：连接页列出本机发现的 agent（「已发现，未授权」）；没有适配器的显示「暂不支持」，不猜测格式。
- **源文件被清理**：回复正文就读不到了，Candid 不保留副本；已入库的你说的话和工作摘要不受影响，界面标明「回复已不在源文件里」。
- **测试**：开关关闭时不读文件、只读授权范围内的内容、结果不落盘、格式未知时拒绝。

首批 agent 适配器：

| agent | 默认路径 | 格式要点 | 状态 |
|---|---|---|---|
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | 每行 `timestamp`、`type`、`payload`；消息、工具调用、`token_count` | 已实现 |
| Claude Code | `~/.claude/projects/*/*.jsonl` | 每行 `type`、`message.role`、`timestamp`、`sessionId`、`cwd`；`custom-title` 作标题 | 已实现 |
| pi | `~/.pi/agent/sessions/--<路径>--/*.jsonl` | 首行 `{"type":"session","version":3,…}`，条目有 `id`、`parentId`，消息带 `usage` | 已实现 |
| WorkBuddy、dsh 等 | 待确认 | 待取样确认格式 | 未实现，有样本后再加 |

源文件格式由对方工具控制，没有版本承诺。解析器按版本识别，遇到不认识的格式时对该文件返回明确错误（`invalid_session`），不猜测不跳过；失败的文件在大小或修改时间变化前不重试。

<a id="sources"></a>

## 6. 来源覆盖：更多连接器

承诺、请求、人和项目大多出现在邮件和消息里，只靠屏幕采集会漏掉没打开过的内容和线程结构，所以这类来源作为**采集型**连接器：正文入库，进入[整理层](curation.md)的去重、实体、抽取流程。附件、远端文档这类体积大或变化快的内容仍按引用处理，用时到源头现读。

所有新连接器都遵守：

- 每个连接器、每个账号单独授权，可随时撤销；撤销后不再读取，已入库内容按你的选择保留或删除。
- 别人发给你的内容同样经过入口打码；对外部访问方默认只给元数据，正文需要单独的范围。当前实现：邮件入库前做密钥样式屏蔽（与 AI 会话同一规则，卡号等其余规则未做，见[整理层](curation.md)）；邮件来源 `agent_visible=false`，外部访问方默认整条看不到，须在其范围里按来源 ID 授予；「只给元数据」的分级视图未实现。
- 按[采集型协议](#ingest-connectors)实现并用合成样例测试。

<a id="connector-table"></a>

## 7. 采集型连接器清单

本表是连接器清单的唯一维护处。顺序是建议的接入顺序。

| 顺序 | 连接器 | 来源 | 映射 | 默认策略 | 状态 |
|---|---|---|---|---|---|
| 1 | Mac 桌面（`mac.desktop`） | 桌面采集直写（经 `POST /local/ingest`）；旧账本的镜像与切换见[数据位置与迁移](../data-migration.md) | 屏幕、应用、窗口等桌面事件 | 沿用当前按应用的授权 | 已实现 |
| 2 | 资料库（`media.import`） | 旧 Hub `media_timeline` 和识别结果（只读镜像，`livesync.rs`） | `audio.recording`、`image.capture`、`speech.segment`、`image.text` | 保存原件 | 已实现 |
| 3 | 系统日历与提醒事项（`calendar.system`、`reminders.system`） | EventKit，只读 | `calendar.event`、`reminder.item`；重复日程用 `calendar_series` 线程 | 保存标题和时间；备注默认不存 | 部分实现：Swift 连接器与测试有，App 里没有调用它的同步（未验证），所以没有数据入库 |
| 4 | 邮件：本机「邮件」App（`mail.apple`） | `~/Library/Mail/V10` 的 Envelope Index 与 `.emlx`，只读；需要完全磁盘访问 | `mail.message`，线程 `mail_thread`；附件只记名称、类型、大小 | 正文经打码入库；外部访问方默认看不到，须按来源 ID 单独授予 | 已实现（合成样例验证）；真实只读试运行（未验证） |
| 5 | 邮件：自己登录 IMAP（`mail.imap`） | email-lib（himalaya 背后的 pimalaya 库）；每个账户一个来源，密码只经 `secret_ref` | 同上 | 同上；Gmail 先走应用专用密码 | 已实现（本机假 IMAP 服务验证）；真实账户（未验证）；OAuth 未实现 |
| 6 | 浏览器历史（`browser.history`） | Safari、Chrome 本机历史库，复制到私有临时目录后只读 | `browser.visit`：网址（去凭据/查询串/片段）、标题、访问时间、次数 | 授权、撤销与试算（只报计数）可用；**不导入**（`import_enabled` 不能设为真，同步恒为跳过） | 部分实现：导入关闭 |
| 7 | 手机 | 安卓伴生 App | `image.capture`、`audio.recording`，以后加通知、使用统计 | 按每一类来源单独授权 | 未实现 |
| 8 | 即时消息 | iMessage（本机库，需完全磁盘访问）、飞书或 Slack（开放接口）；微信本机数据加密，暂不考虑 | 会话、发送人、时间、文字；按会话组成线程 | 群聊和单聊分开授权 | 设计中 |
| 9 | 笔记与选定文件夹 | 备忘录、Obsidian 等本地笔记目录；你选定的文档目录 | 笔记正文入库，其他文件只记元数据并按引用读取 | 只读选定目录，不扫描整个磁盘 | 设计中 |
| 10 | 工作服务 | GitHub、Notion、飞书文档等的开放接口或远端 MCP | 只把「你参与了什么」作为事件入库，内容按引用 | 每次查询单独计入审计 | 设计中 |

AI agent 会话（引用型）见[上文](#agent-connectors)，不在本表。来源种类的完整登记在 `candid-store/src/schema.rs` 的 `SOURCE_KINDS`。日历条目在源头会被修改：同一 `source_key` 的新内容按修订处理还是进入冲突区，接入前要定规则（待定）。

<a id="mail"></a>

## 8. 邮件连接器

邮件连接器由 Candid 自己实现，主路线读取本机「邮件」App 的数据，另有 IMAP 登录适配器。不依赖第三方邮件 MCP，也不为每家邮箱分别实现登录。原因：各家官方接口要分别注册 OAuth 应用且审核严，统一邮件接口服务是云服务，社区 Apple Mail MCP 项目没有增量同步与线程。邮件入库后，外部 agent 经 Candid 自己的 MCP 查询，带范围过滤和审计。调研见[研究记录](../research/references.md)。

邮件连接器在 Rust 核心里实现（[ADR-024](../decisions/024-rust-core.md)）。解析、组线程、清洗、筛选只有一份；「从哪里读邮件」是可替换的适配器：

| 顺序 | 适配器 | 平台 | 说明 | 状态 |
|---|---|---|---|---|
| 1 | 系统「邮件」App 本机数据（`mail.apple`） | Mac | 主路线。只认 `V10` 且索引表结构与签名一致，其他版本整体拒绝（`unknown_mail_version` / `unknown_mail_schema`），真实版本待只读试运行确认 | 已实现 |
| 2 | Thunderbird 本机数据 | Mac、Windows、Linux | Thunderbird 自己处理 OAuth；Exchange 走微软正在淘汰的 EWS 接口，接入前核实；需要保留离线副本，存储格式实测后确定 | 未实现 |
| 3 | 自己登录（`mail.imap`） | 全部 | 只用 EXAMINE、UID SEARCH、BODY.PEEK 读取，不改标记、不移动；邮箱移出后按 Message-ID 在其他文件夹（如 Gmail「所有邮件」）查找，仍找不到才记删除。传输必须是 `tls` 或 `starttls`，`none` 只允许回环地址。Gmail 用应用专用密码，或用户自己注册的 OAuth 应用；不借用其他软件的 OAuth 应用。以 Candid 名义的一键登录需通过 Google 和微软审核，按需求再评估 | 已实现 |
| — | MailKit 实时扩展 | Mac | 新邮件到达时由「邮件」App 调用，可拿到正文；只覆盖新邮件。实测可用后作为第 1 条的补充 | 未实现 |

**数据来源（适配器 1）**：

- 索引：`~/Library/Mail/V<n>/MailData/Envelope Index`（SQLite）。以只读、不改动的方式打开，运行时识别表结构，只认已验证过的版本；认不出就整体拒绝并在连接页提示，不按猜测读取。
- 正文：各邮箱目录下的 `.emlx` 文件，结构是首行字节数、RFC 822 原文、末尾的属性表。`.partial.emlx` 只下载了一部分，按已有内容入库并标记为不完整（`body_status=partial`）；邮件 App 没下载的正文不读取，也不触发下载。
- 权限：需要「完全磁盘访问」。没有权限时显示为未授权（错误码 `full_disk_access_required`），退避重试，错误码变化时才报告。

**同步**：

- 以索引里的邮件行号和修改信息作为游标（流名 `mail`）增量读取，每批至多 `batch_limit` 封（默认 200，上限 2000）。
- 每轮比对已入库邮件是否还在，删除和移动记为墓碑或新修订，不静默丢失：只剩在废纸篓/垃圾邮件里或已彻底删除的邮件按 `source_deleted` 删除并留墓碑（之后从废纸篓恢复也不会再入库）；换邮箱（如归档）是同一事件的 payload 修订。
- 没下载正文的邮件按索引里的主题、发件人和时间只存元数据（`body_status=not_downloaded`），之后下载了也不自动补正文（待定）。
- `source_key` 用 `mid:<Message-ID>`；没有 Message-ID 时用账户、邮箱和行号组合。同一封邮件在多个邮箱时只入库一次，`payload.mailboxes` 记录所在邮箱。
- 只读：不改标记、不移动、不删除，不调用邮件 App 的脚本接口。

**解析**（MIME 用成熟的 Rust 库，不自己实现；`candid-connectors/src/mail/mime.rs`）：

- 多段结构；quoted-printable 和 base64；RFC 2047 编码头；字符集至少覆盖 UTF-8、GBK、GB2312、GB18030、Big5、ISO-2022-JP、ISO-8859 系列和 Windows 代码页。
- 正文优先取纯文本部分；只有 HTML 时转成纯文本，去掉样式、脚本和跟踪图片。
- 引用的历史回复和签名存入独立文字字段 `quoted`，`body` 只放新写的部分；`quoted` 不进入抽取输入，也靠段落去重自然折叠。
- 附件只记文件名、类型和大小（最多 50 个）；需要内容时按引用现读，同样受访问范围约束（附件现读未实现）。

**事件映射**：每封邮件一个 `mail.message` 事件，`time_basis=source`（没有时间时 `imported`），正文字段 `body`、主题 `subject`、引用与签名 `quoted`；你发出的邮件（发件箱角色或发件人是 `self_addresses`）`actor=self`，其余 `other`。线程按 Message-ID、In-Reply-To 和 References 组成 `mail_thread`（`thread:<根 Message-ID>`）。payload 记录 `direction`、`from.name`、`from.address_hash`、`to_count`、`cc_count`、附件、`mailboxes`、`level`、`level_basis`、`body_status` 等，字段清单见[存储](storage.md)。**邮箱地址不入库**：只存以数据根目录里的盐（`connectors.address_salt`）加盐的哈希；你写过信的人以哈希记在已发邮件上，用于「回复过」判断。标题、发件人名、附件名入库前经密钥样式屏蔽。

**验证**：

- 合成邮件覆盖每一种编码、多段结构、只下载了一部分的邮件、没有 Message-ID 的邮件、同一封邮件在多个邮箱、删除和移动。
- 真实邮箱只在 Candid 测试版里只读试运行，只报计数（邮件数、线程数、解码失败数、不完整正文数）；不给终端或开发工具开完全磁盘访问。

<a id="source-filter"></a>

## 9. 来源筛选：先定级，再决定处理多深

接入邮件后不能每封都处理：要有人工可设置的筛选规则，也可以加规则让 AI 判断重点，防止注入大量无用内容。读取和解析本身不花 token，花钱的是抽取，所以每个来源的每一条内容先定级，级别决定它走多远：

| 级别 | 处理 |
|---|---|
| 忽略（`ignore`） | 不入库 |
| 只存元数据（`metadata`） | 时间线上可见（如发件人、主题、时间、线程、在来源里的位置），正文不存，打开时从来源现读 |
| 普通（`normal`） | 正文入库，可全文检索，不做抽取 |
| 重点（`important`） | 进入抽取和汇总（见[整理层](curation.md)） |

定级按顺序分四层，设计上所有连接器共用；当前只有邮件实现了（`mail/config.rs` 的 `decide`）：

1. **范围**：选哪些账户、文件夹（`mailboxes.include/exclude`，名称或 `role:inbox` 这样的角色）。默认收件箱和已发送；垃圾邮件、废纸篓、草稿永不读取。首次导入默认只回溯 90 天（`backfill_days`，可选 30 天 / 90 天 / 1 年 / 全部），更早的按需再导。
2. **你的规则**（`rules`，有序，至多 200 条）：条件可以是发件人、域名（含子域）、收件人、主题关键词、群发头、邮箱，多个条件同时满足才命中；动作是定成上面四个级别之一。按顺序匹配，第一条命中的生效。**你的规则永远优先。**
3. **内置默认规则**（可改、可关）：顺序「回复过 → 群发/自动 → 默认」。你回复过的线程、你写过信的人发来的邮件 → 普通，并标为重点候选（`important_candidate`，级别仍是普通；`builtin_replied` 可关）；带退订头、群发头、自动回复头或来自 noreply 类地址（`noreply`、`no-reply`、`donotreply`、`notifications`、`mailer-daemon` 等）的邮件 → 只存元数据（`builtin_rules` 可关）；其余邮件落到 `default_level`，默认只存元数据。
4. **AI 判断**（可选，默认关闭，`ai_judge.enabled`）：只处理规则没有定下来的内容，设计是小模型（优先本地）读邮件头和正文开头约 500 字，在「普通」和「重点」之间调整并附一句理由。**当前只实现为开关**：开启后规则未定的邮件标 `ai_judge=pending`，不调用模型（结果缓存、每天预算上限、每次调用记入 `ai_calls`：未实现）。
   - AI 不能推翻你的规则，不能触发任何动作；界面标明「AI 判断」；
   - 内容里自称重要的文字（如「此邮件非常重要，请优先处理」）只当作内容，不当作指令。

级别和命中规则记录在事件 payload：`level`、`level_basis`（`rule:N`、`builtin:replied|list|automated|noreply`、`default`、`manual`）。配置上限：规则 200 条，账户与自身地址各 64。

**导入前预览**（已实现，合成验证）：

- 授权「邮件」App 后不导入（App 写入配置 `import_confirmed=false`，同步返回 `awaiting_confirmation`）；`mail_scope` 列出账户与文件夹（只有计数）；选好范围后，`dry_run` 带 `config` 预览（不保存），只解析邮件头，返回各级别计数（被规则忽略的另计）和「已入库」数，上限 5000 封；点「导入」才写 `import_confirmed=true` 并开始。

**现读与手动调整**（已实现，合成验证）：

- `mail_body` 按 Message-ID 从「邮件」App 的索引和 emlx 现读正文，不存、不进全文检索；邮件已删除或只剩在废纸篓/垃圾邮件/草稿时返回 `source_deleted`，界面标「来源已删除」。仅本机邮件 App 来源支持，IMAP 来源不现读（`live_read_unavailable`）。
- `set_level` 把单封或整个线程升为普通/重点（正文从来源读入并入库，可检索）或降回只存元数据（正文与检索项随之删除），记为 `manual`，规则和重新定级都不覆盖；来源已删除时不能升级。
- `relevel` 按当前规则重读每封邮件（发件人、收件人不入库，只在内存里用）再定级；`manual` 的不动；来源里已没有的留给常规同步清理；规则设为「忽略」的已入库邮件保留为只存元数据（`ignored_kept`）；可重复运行，第二次不变。仅 `mail.apple` 来源支持。
- 「邮件」App 里删掉的邮件，只存元数据的那些正文随之不可读，时间线上标「来源已删除」。

**界面缺口**：时间线里还没有邮件轨道，打开邮件和升级目前在连接页邮件详情的「最近邮件」列表里。手动改级别后的规则建议（「以后来自 X 的都设为重点？」，你同意后才生效）等未做项见[未实现与待定](#open)。Exchange Online 的 EWS 逐步关闭，以后直连 Exchange 时按 Microsoft Graph 评估。

<a id="code"></a>

## 10. 实现位置

| 内容 | 代码 | 测试 |
|---|---|---|
| 入库、校验、来源删除与修订 | `candid-store/src/ingest.rs`、`types.rs` | `candid-store/tests/ingest.rs` |
| 连续打字合并 | `candid-store/src/merge.rs` | `candid-store/tests/merge.rs`、`real_merge.rs` |
| 连接器的存储侧辅助（来源配置、错误记录、邮件级别） | `candid-store/src/connector_support.rs` | `candid-connectors/tests/mail_scope.rs` |
| 邮件、浏览器历史连接器与命令分发 | `candid-connectors/src/lib.rs`、`mail/`、`browser.rs` | `candid-connectors/tests/` |
| 桌面账本、资料库镜像 | `candid-store/src/desktop.rs`、`livesync.rs`、`migration.rs` | `candid-store/tests/migration.rs`、`desktop_mapping.rs` |
| AI agent 会话 | `candid-store/src/agents/` | `candid-store/tests/agents.rs`、`agent_tools.rs` |
| 本地通道 | `candid-service/src/local.rs` | `candid-service/tests/lifecycle.rs`、`local.rs` |
| 日历、提醒事项（Swift） | `apps/macos/Sources/TimelineConnectors/` | `apps/macos/Tests/TimelineConnectorsTests/` |
| 桌面采集投递与连接页（Swift） | `apps/macos/Sources/DesktopCore/DesktopDelivery.swift`、`CandidClient/IngestTypes.swift`、`Candid/ConnectorsModel.swift` | `apps/macos/Tests/DesktopCoreTests/`、`ConnectorsModelTests.swift` |

<a id="open"></a>

## 11. 未实现与待定

| 项 | 状态 | 说明 |
|---|---|---|
| 统一的连接器接口 | 未实现 | 规格是每个连接器五个能力；代码里 Rust 是按 `kind` 分发的函数，Swift 日历连接器有 `Connector` 协议，agent 会话与桌面采集各走各的（见[代码里的实际形态](#shape)）。第三方连接器要写之前需要补齐 |
| 入库协议与本地通道的契约快照 | 未实现 | `IngestBatch` 与 `/local/ingest`、`/local/connectors` 没有快照，见[契约清单](../contracts/README.md) |
| `backfill(range)` 可中断续跑 | 未实现 | Rust 侧只有 `backfill_days` 限定首次范围 |
| 日历与提醒事项接入 App | 未验证 | Swift 连接器与测试齐全，但 App 源码里没找到调用同步的地方；源头被改的日程按修订还是冲突处理未定 |
| 浏览器历史导入 | 设计中 | 授权、撤销、试算已实现，导入被关闭；何时打开未定 |
| 邮件：真实邮箱只读试运行 | 未验证 | 只用合成样例和本机假 IMAP 服务验证；Mail.app 版本只认 `V10` |
| 邮件：Thunderbird 适配器、MailKit 扩展、OAuth 登录；IMAP 与附件现读 | 未实现 | 见适配器表；`mail_body`、`relevel` 仅 `mail.apple` |
| 邮件：没下载的正文之后下载是否补正文 | 设计中 | 目前不补 |
| 邮件：AI 判断调用模型、缓存与每日预算 | 未实现 | 目前只标 `ai_judge=pending` |
| 来源筛选：手动改级别后的规则建议；规则条件「回复过」「客户端垃圾标记」；AI 对话按项目与会话长度定级 | 未实现 | |
| 邮件在时间线上的轨道、「只给元数据」的外部访问分级 | 未实现 | |
| 引用型：时间线「AI 交互」一行 | 未验证 | Swift 有 `AIInteractionTimeline.swift`，未核对范围 |
| 引用型：`prompt_max_chars` 的限长值、「固定」会话操作 | 设计中 | 现不限长；4000 字仍是提议 |
| 连续打字合并：阈值调优；查询与界面层折叠；旧账本数据未合并 | 设计中 | 阈值由评估集调整 |
| 手机、即时消息、笔记、工作服务连接器 | 未实现 | 清单第 7 至 10 行 |
| `SOURCE_KINDS` 里无连接器代码的种类（`mail.gmail`、`mcp.remote`、`manual` 等） | 未验证 | |

<a id="provenance"></a>

## 12. 来源

- [时间线库、查询接口与连接器 v1](timeline-v1.md) 第 6 节全部（含 `ingest-connectors`、`text-merge`、`agent-connectors`、`connector-table`）。
- [个人 Agent 基座 v1](agent-platform-v1.md) 第 2.0 节全部（含 `sources`、`mail`、`source-filter`）。
- 旧决定（邮件自己实现、主路线读本机邮件 App 并加 IMAP、浏览器历史先不导入）记入[决定流水](../decisions/log.md)；邮件选型调研在[研究记录](../research/references.md)。
