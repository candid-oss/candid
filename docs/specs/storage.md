# 存储

**中文** · [English](storage.en.md)

**状态**：部分实现。服务、数据根目录、事件、线程、原件、文字段落、检索索引、作业、变更与删除均已实现，表结构版本为 12；向量检索、可迁移包导出等未实现。

本文规定时间线库的**存储骨架**：谁持有数据库、数据根目录里有什么、通用约定，以及来源、事件、线程、原件、文字段落、检索索引、作业、变更和删除的职责与不变量。不规定整理层与派生层（见[整理层](curation.md)）、模型来源与用量（见[模型](models.md)）、访问方与审计（见[查询接口](query.md)）、连接器与入库协议（见[连接器](connectors.md)）、分层与存储预算（见[数据模型](data-model.md)）。

<a id="overview"></a>

## 1. 概览

时间线库是 Candid 唯一的事实仓库：所有来源经入库接口写成统一结构的事件，原件按内容哈希存放，文字按段落去重，AI 派生内容分层存放并可删除重算。

- 只有一个时间线服务进程持有数据库；界面、MCP、命令行、Pi 和连接器都经接口访问，不直接打开 SQLite。
- 事件只有一种统一结构；所有 AI 调用记入同一张用量表。
- 数据层与位置无关：用「数据根目录」定位全部数据，目录整体移动后仍完整可用。

| 层 | 表 | 本文 |
|---|---|---|
| 来源 | `sources`、`source_cursors` | 第 4 节 |
| 事实 | `events`、`event_kinds`、`ingest_conflicts`、`threads` | 第 5–7 节 |
| 原件与文字 | `blobs`、`event_blobs`、`text_chunks`、`event_text` | 第 8、9 节 |
| 检索投影 | `text_fts`、`meta_fts` | 第 10 节 |
| 运行 | `jobs`、`changes`、`tombstones`、`store_meta` | 第 11 节 |
| 派生、模型、访问 | `episodes`、`memories`、`entities`、`ai_*`、`access_log` 等 | 见整理层、模型、查询接口 |

名词见[术语表](../glossary.md)。表结构以 `core/crates/candid-store/src/schema.rs` 为准，本文只写职责、关键字段、不变量和规则。

<a id="deployment"></a>

## 2. 服务与数据根目录

### 2.1 组件

界面经本地通道、连接器经入库接口、本地 HTTP `/v1` 经服务访问数据根目录里的 `timeline.sqlite` 与 `blobs/`；MCP 外壳和命令行 `candid` 只是 HTTP 的薄客户端，外部 AI 经 MCP 接入。

- **时间线服务**：唯一读写数据库和原件目录的组件。桌面端是 Rust 核心的独立进程（[ADR-024](../decisions/024-rust-core.md)），即 App 包内的辅助进程 `Contents/Helpers/candid serve`，由 App 启动和监控，同时监听回环 HTTP 和只供 App 使用的本地通道；接口不变时也可运行在 NAS 上。
- **回环 HTTP**：每次启动选一个空闲端口，写入运行目录（`CANDID_RUN_DIR`，默认 `~/Library/Application Support/Candid/run`）的 `endpoint.json`，文件不含密钥。首版只监听 `127.0.0.1`；远程访问必须加认证和 TLS。
- **MCP 外壳与命令行**：是同一个 `candid`，都是 HTTP 的薄客户端，不打开数据库，也不读原件目录。
- **单写者**：数据根目录里的 `.lock` 持有独占 `flock`；第二个进程打开同一目录被拒绝（`Locked`）。
- **位置无关**：数据根目录移到别处，或在另一台主机上用同一目录启动服务，数据都完整可用。

> SQLite 不能放在 SMB/NFS 共享上由另一台机器远程打开（文件锁不可靠）；数据放 NAS 时服务必须运行在 NAS 主机上，Mac 改用 HTTP 访问。

### 2.2 数据根目录布局

默认位置 `~/Library/Application Support/Candid/timeline/`。

```text
<root>/
  store.json                  # 本库身份与版本，见下
  timeline.sqlite             # 以及 -wal、-shm
  blobs/sha256/<前2位>/<完整hash>.<扩展名>
  staging/                    # 入库暂存；启动时清空（见 2.3）
  exports/                    # 导出产物目录
  logs/                       # 滚动诊断日志，不含正文
  .lock                       # 写锁
```

`store.json`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `store_id` | UUID | 本库身份，创建后不变 |
| `schema_version` | 整数 | 与 `PRAGMA user_version` 一致；版本更新的库拒绝被旧服务写入（`NewerSchema`） |
| `created_at` | RFC 3339 | 建库时间 |
| `created_by` | 字符串 | 创建该库的程序及版本，例如 `candid/0.10.0` |
| `layout` | 字符串 | 固定为 `timeline-root/1`；其他值拒绝打开 |

规则：

- 创建时 `store.json` 最后写入。只有 `store.json` 与 `timeline.sqlite` 之一存在的目录被拒绝，不自动重建。
- 数据库开启 WAL、`synchronous=FULL`、外键、`busy_timeout=5000`，单一写连接；`journal_size_limit` 64 MiB，打开后截断一次 WAL。
- API 密钥、OAuth 令牌和访问令牌原文**不**放进数据根目录。密钥存在宿主系统的密钥存储里（Mac 用钥匙串），库里只保存引用。

### 2.3 启动恢复与完整性

- **启动恢复**：`staging/` 只放未提交的批次，启动时清空；`blobs/` 下没有 `blobs` 行引用的文件（已移入、事务未提交）被删除；有行而文件缺失的路径记入恢复报告，不自动修复。
- **完整性检查**（按需）：`integrity_check`、外键、`text_fts` 一致性、`user_version` 与 `store.json` 一致、每个原件存在且大小一致；可选重算每个原件的 hash 并更新 `verified_ms`。
- **整库迁移**：停止服务后原样复制整个数据根目录（或用 SQLite 备份接口），在新位置启动；服务校验 `store.json` 与数据库版本。备份、恢复、换位置见[数据位置与迁移](../data-migration.md)。

<a id="conventions"></a>

## 3. 通用约定

| 项目 | 约定 |
|---|---|
| ID | 小写 UUIDv7 文本（按时间排序）。来源的原生 ID 不作主键，存在 `source_key` 中 |
| 时间 | 整数毫秒 UTC，字段名以 `_ms` 结尾。另存发生地的 `tz_offset_min`。区间为半开 `[start_ms, end_ms)`，`end_ms` 为空表示时间点或尚未结束 |
| 时间依据 | `time_basis`：`observed`（本机观察）、`source`（来源自报）、`imported`（只有导入时间）、`estimated`（推算）。另有 `time_uncertainty_ms` |
| JSON | `TEXT` 列，写入前校验。`*_json` 列有版本字段或固定结构，未知字段保留 |
| 表 | 一律 `STRICT`。开启外键，WAL 模式，单一写连接 |
| 枚举 | 写成 `CHECK` 约束；新增取值时同时升级数据库版本并补测试 |
| 删除 | 删除时写入墓碑并清理派生内容，见[第 11 节](#jobs-changes-deletion)。不做「软删除后仍可查询」 |
| 修订 | 事实事件不改写，更正时产生新事件或新修订。派生层用 `(id, rev)` 标识 |

<a id="schema"></a>

## 4. 数据结构总则与来源

运行表结构以 `core/crates/candid-store/src/schema.rs`（主体）及 `durable.rs`、`performance.rs`、`agents/checkpoints.rs`、`curated.rs` 里的 DDL 为准；`core/fixtures/schema-v*.sql` 是升级测试用的旧版快照。当前表结构版本 `SCHEMA_VERSION = 12`，记在 `PRAGMA user_version` 和 `store.json`。

### 4.0 表结构升级

- 升级链 v1 → … → v12 在打开库时自动执行，每一步一个事务，失败整体回滚；数据库提交先于 `store.json` 的原子替换，`store_meta.schema.upgrade`（如 `11:12`）让被中断的替换只续做那一步，其他版本不一致一律拒绝。
- 改列或约束的表按「建新表、复制、删旧表、改名」重建，目标 DDL 取自当前 `schema.rs`。
- 新增 kind、枚举取值或表时同时升版本并补升级测试（含失败回滚与中断续跑）。

版本沿革：v2 增加 `ai.prompt`、`ai.work` 与 `agent_event_revisions`；v3 持久队列与媒体表（`durable.rs`）；v4 邮件、浏览器历史来源种类、`quoted` 字段、`source_deleted` 墓碑原因、`browser.visit`；v5 派生层的 `digests`、`entities`、`entity_aliases`、`entity_mentions` 与 `jobs.feature`；v6 查询索引与 `background_revisions` 变更计数；v7 来源 `mac.system_audio`；v8 Agent 会话检查点两表与媒体投影变更计数；v9 删除只写不读的截屏片段修订、片段输入改存引用；v10 证据定位改为 `screen-selector/2`（读取时还原）；v11 整理层表（见[整理层](curation.md)）；v12 Agent 会话与条目表（见[整理层](curation.md)、[人物与条目](people-items.md)）。

v9、v10 升级后执行一次 `VACUUM`。旧 `ai.turn`、`ai_session` 与旧来源种类 `ai.codex`、`ai.claude_code` 仅为兼容保留，新连接器不产生。

<a id="source-tables"></a>

### 4.1 来源与同步进度

`sources` 登记来源与授权；`source_cursors` 保存增量同步位置。

| 表 | 职责与关键字段 |
|---|---|
| `sources` | `source_id`（UUIDv7）、`kind`、`display_name`、`device_id`、`account_ref`（账号引用，不含凭据）、`secret_ref`（钥匙串条目引用）、`config_json`（连接器配置，带 `version`）、`policy_json`、`status`（`active`/`paused`/`error`/`revoked`）、`last_sync_ms`、`last_error`（有限枚举错误代码，不含正文） |
| `source_cursors` | 主键 `(source_id, stream)`；`cursor` 不透明，**只在入库事务提交后推进**；随来源级联删除 |

来源种类（`sources.kind` 的 `CHECK`）：`mac.desktop`、`mac.microphone`、`mac.system_audio`、`mac.region`、`media.import`、`device.pendant`、`phone.android`、`calendar.system`、`reminders.system`、`mail.imap`、`mail.gmail`、`mail.apple`、`browser.history`、`agent.transcript`、`mcp.remote`、`manual`，另有兼容保留的 `ai.codex`、`ai.claude_code`。

`policy_json`（采集型来源的隐私与范围设置；引用型 agent 来源的设置见[连接器](connectors.md#agent-connectors)）：

| 字段 | 含义 |
|---|---|
| `version` | 固定 1 |
| `store_text` | 是否保存正文；`false` 时只保存元数据 |
| `store_media` | 是否保存原件（图片、音频、附件），默认 `false` |
| `agent_visible` | 默认是否对 Agent 可见；具体还要看客户端范围 |
| `retention_days` | 该来源的留存期，`null` 跟随全局设置（未验证：代码里没找到执行） |
| `exclude` | `{apps, threads, patterns}` 三个列表 |

规则：

- `store_text=false` 时事件只存元数据（不写 `event_text`）；`store_media=false` 时不保存原件，暂存文件丢弃。命中 `exclude`（应用、线程、标题/URL/正文里的模式，不区分大小写）的事件在入库前跳过。
- `agent.transcript` 来源在「纳入活动」关闭时只保存授权状态和路径配置，不产生任何事件、线程或文字行；打开后只产生 `ai.prompt`、`ai.work` 和 `agent_session` 线程。
- Agent 持久检查点 `agent_session_entries`（来源、会话、条目身份与类别、已脱敏的准入条目）和 `agent_file_counts`（来源、文件、内容 hash、出现次数）只服务恢复和去重，继承来源授权与删除边界，不把回复/工具输出正文变成已存事实。进度游标、变化条目与准入事件在同一事务提交；旧内联检查点在下次同步该会话时转换，失败保留原状态。
- 资料投影用独立的 `media_projection` 修订域（`background_revisions`），只有媒体输入、相关授权与标注变化使其失效。

<a id="events"></a>

## 5. 事件：唯一的事实主线

`events` 一行一条事实，是时间线的唯一主线。

| 字段 | 说明 |
|---|---|
| `event_id` | UUIDv7 |
| `source_id`、`source_key` | 来源与来源原生 ID（或持久化导入 ID）；`UNIQUE (source_id, source_key)` 保证幂等 |
| `kind` | 外键到 `event_kinds` |
| `start_ms`、`end_ms`、`tz_offset_min`、`time_basis`、`time_uncertainty_ms` | 时间；`end_ms >= start_ms`；`start_ms` 为空时 `time_basis` 必须是 `imported` |
| `app_bundle_id`、`app_name`、`title`、`url` | 应用与标题；`url` 已去掉凭据、查询串和片段 |
| `thread_id`、`parent_event_id` | 所属线程；父事件（例如截图属于哪次焦点观察） |
| `actor` | `self`/`other`/`agent`/`system`/`unknown` |
| `sensitivity` | `normal`/`sensitive`/`protected` |
| `payload_version`、`payload_json` | 按 kind 的结构，见[第 6 节](#event-kinds)；当前只支持 `payload_version = 1` |
| `ingest_seq`、`ingested_ms` | `ingest_seq` 单调递增且唯一，用于变更流和分页快照（计数器在 `store_meta.ingest_seq_next`） |

索引按时间、应用、种类、线程、来源、父事件和 `source_key` 建立，定义见 `schema.rs`。

规则：

- **排序键**：时间线按 `(COALESCE(start_ms, ingested_ms), event_id)` 排序。时间未知的事件在界面里单独分组，不冒充发生时间。
- **幂等**：重复同步同一条来源记录不会新增行。`source_key` 相同但内容不同时，新内容进入 `ingest_conflicts`（主键 `(source_id, source_key, detected_ms)`，含 `existing_event_id`、`incoming_json`、`resolved_ms`、`resolution`），不覆盖已有事件。内容相同与否用事件的规范化指纹比较。
- **受保护内容**：`sensitivity='protected'` 的事件只有元数据，永远不存正文和原件（例如密码框、标为受保护的窗口）。
- **父子**：`parent_event_id` 必须能解析；批次可声明 `on_missing_parent=detach` 摘离找不到父事件的子事件。
- **事务**：一个批次在一个事务里完成：线程、事件、段落、原件引用、变更行，最后推进游标。任何失败整批回滚（含游标）并清理新建的原件文件，所以重发是安全的。入库协议见[连接器](connectors.md#ingest-connectors)。

<a id="event-kinds"></a>

## 6. 事件种类与 payload

`event_kinds` 是登记表（`kind`、`layer`、`is_span`、`payload_version`、`description`），随数据库版本更新，代码里的清单是 `schema.rs` 的 `KINDS`。`layer` 为 `evidence`（证据）或 `perception`（感知，由作业从证据生成）。

| kind | 层 | 时长 | 来源 | payload 字段 |
|---|---|---|---|---|
| `app.focus` | 证据 | 有 | mac.desktop | `{reason}` |
| `window.focus` | 证据 | 有 | mac.desktop | `{window_token, coverage: "app"\|"window"}` |
| `screen.text` | 证据 | 无 | mac.desktop | `{trigger, text_status, focused_role, focused_label, is_composing, reader}`。正文在 `event_text` |
| `screen.shot` | 证据 | 无 | mac.desktop, mac.region | `{trigger, pixel_w, pixel_h, scale, source_bounds, ax_evidence_ref?}`，原件经 `event_blobs` 关联 |
| `marker` | 证据 | 无 | 任意 | `{label?, note?}` |
| `system.state` | 证据 | 无 | mac.desktop | `{change: "power"\|"sleep"\|"wake"\|"lock"\|"unlock"\|"session", value}`；必填 `change` |
| `coverage.gap` | 证据 | 有 | 任意 | `{reason_code}`（paused、permission_denied、asleep、crashed、source_offline）；必填 |
| `audio.recording` | 证据 | 有 | mac.microphone, mac.system_audio, device.pendant, media.import | `{codec, sample_rate, channels, duration_ms, …}` |
| `image.capture` | 证据 | 无 | mac.region, phone.android, media.import | `{pixel_w, pixel_h, origin_filename?, exif_time?}` |
| `speech.segment` | 感知 | 有 | ASR 作业 | `{recording_event_id, sample_start, sample_end, speaker_label?, asr: {model_id, revision}, …}`；文字在 `event_text` |
| `image.text` | 感知 | 无 | OCR / VLM 作业 | `{image_event_id, method: "ax"\|"ocr"\|"vlm", model_id?, regions_ref?}` |
| `calendar.event` | 证据 | 有 | calendar.system | `{calendar_id, calendar_name, all_day, location?, attendees: [{name?, email_hash?}], status, …}` |
| `reminder.item` | 证据 | 无 | reminders.system | `{list_name, due_ms?, completed_ms?, priority?}` |
| `mail.message` | 证据 | 无 | mail.apple, mail.imap | `{adapter, direction: "in"\|"out", from: {name?, address_hash?}, to_count, cc_count, attachments: [{name?, mime, size}], mailboxes, level, level_basis, body_status: "complete"\|"partial"\|"not_downloaded"\|"undecodable", quoted_chars, signature_chars, …}`（全集见代码）；文字 `subject`、`body`（新写的部分）、`quoted`（引用的历史和签名，不进抽取输入）；`level=metadata` 时只存主题 |
| `browser.visit` | 证据 | 无 | browser.history | `{browser, visit_count}`，`url` 去掉凭据、查询串和片段；已登记，**导入未开启** |
| `note` | 证据 | 无 | manual | `{format: "text"\|"markdown"}` |
| `ai.prompt` | 证据 | 无 | agent.transcript | `{agent, session_ref, message_ref, project?, followups: n, truncated: bool, origin: "transcript"\|"screen"}`；你写的文字存入 `event_text`（字段 `prompt`），追问并入同一事件 |
| `ai.work` | 证据 | 有 | agent.transcript | `{agent, session_ref, prompt_event_id, status: "done"\|"running"\|"interrupted", assistant_messages, tool_calls, files_touched, tokens?}`；不存回复正文 |

- 新增 kind 要同时登记 `event_kinds`、提供 payload 校验（`records::validate_payload` 与 `KINDS` 的必填字段）和测试、升级数据库版本，再在本表加一行。
- 邮箱地址和参会人邮箱只存加盐 hash；明文地址只在用户另行授权的人物记录里出现。
- AI agent 对话只有你发出的消息（`ai.prompt`）和 AI 工作摘要（`ai.work`）入库，回复正文不入库。`ai.turn` 仍登记但不再使用。`agent_event_revisions` 保存已接收消息与统计的逐次快照。

<a id="threads"></a>

## 7. 线程：会议、对话、邮件串

`threads` 把多条事件归到同一个会议、对话或邮件串：`thread_id`、`source_id`、`source_key`（`UNIQUE (source_id, source_key)`）、`kind`、`title`、`started_ms`、`ended_ms`、`participants_json`、`payload_json`（例如会议的来源应用、邮件串的标签）。索引 `threads_time(started_ms)`。

- `kind` 取值：`mail_thread`、`meeting`、`recording`、`calendar_series`、`chat`、`agent_session`，兼容保留 `ai_session`。
- 线程按 `source_key` 创建或更新，字段变化才记 `update` 变更。事件经 `thread_key` 指向线程，指向不存在的线程使整批失败。
- 线程下已无事件时随删除一并清理。

<a id="blobs"></a>

## 8. 原件

| 表 | 职责与关键字段 |
|---|---|
| `blobs` | 以 `sha256`（实际存储字节的 hash）为键：`byte_count`、`mime_type`、`rel_path`（相对数据根目录，由服务生成，唯一）、`pixel_w`、`pixel_h`、`duration_ms`、`created_ms`、`verified_ms`（最近一次重新校验 hash 的时间） |
| `event_blobs` | 主键 `(event_id, sha256, role)`；`role` 为 `original`/`preview`/`derived`；事件删除时级联删除 |

规则：

- 两次不同的发生可以引用同一份字节，各自保留自己的时间和权限。没有任何事件引用的原件，在删除流程中清理。
- 入库时调用方把文件放进 `staging/` 并声明 hash；服务重算 hash，不一致拒绝（`BlobMismatch`）；通过后移动到 `blobs/sha256/<前2位>/<hash>.<扩展名>`（权限 0600，fsync 后再提交事务）。同一份字节只存一份。
- 截图原件以 WebP 存放；录音以 AAC 存放；`derived` 角色的 JSON（屏幕 AX 证据、识别结果）以原始 DEFLATE 压缩存放，mime 为 `application/json+deflate`、后缀 `.json.deflate`，`sha256` 是压缩后字节的 hash。旧库的 JPEG 与 WAV 由后台一次性转换。保真度阶梯、降级与存储预算见[数据模型](data-model.md)。

<a id="text"></a>

## 9. 文字：段落去重与注意力

| 表 | 职责与关键字段 |
|---|---|
| `text_chunks` | 段落正文，全库一份：`chunk_id`（供全文索引 rowid）、`chunk_hash`（规范化文本的 sha256，唯一）、`text`、`char_count`、`lang`（`zh`/`en`/`mixed`，可空）、`first_seen_ms` |
| `event_text` | 事件对段落的引用，主键 `(event_id, field, ord)`：`field` 为 `prompt`/`body`/`selection`/`title`/`transcript`/`ocr`/`vlm`/`message`/`subject`/`note`/`quoted`；`chunk_id`；注意力标记 `visible`（是否在可见区域，未知为空）、`focus_distance`（与焦点元素的树距离，0 为焦点本身，未知为空）、`is_new`；`char_start`（在原始拼接文本中的码点偏移，供引用定位）。事件删除时级联删除 |

**切分与规范化**

1. 文本先做 Unicode NFC 规范化，统一换行，去掉行尾空白，把连续空白行折叠成一个，去掉首尾空行。
2. 按段落（空行）切分。超过 512 个字符的段落再按句末标点（`。` `.` `！` `？` `!` `?` 或换行）切成句子并贪心合并，单段不超过 512 字符；没有标点的超长句按 512 字符硬切。偏移是规范化文本里的 Unicode 标量偏移。
3. `chunk_hash` 对规范化后的段落计算（`core/crates/candid-store/src/chunk.rs`，与 Swift 实现一致）。同一段文字在全库只存一份。

**新内容判定（`is_new`）**：`screen.text` 事件与同一来源、同一应用和同一标题的上一条 `screen.text` 事件的段落集合比较，上次没有的段落记为新。其他所有事件（语音、邮件、AI 对话等）的段落都记为新。

**用途**：界面和 AI 摘要先读「新出现、可见、离焦点近（`focus_distance` ≤ 3）」的段落，其余作为背景，只在展开或搜索时出现。

- 长上下文重复出现时只增加 `event_text` 的引用行，不再写一份正文；这些规则对所有应用一致，不按应用单独调整。

<a id="fts"></a>

## 10. 检索索引（可重建投影）

```sql
-- 段落正文索引
CREATE VIRTUAL TABLE text_fts USING fts5(text, content='text_chunks', content_rowid='chunk_id', tokenize='trigram');
-- 元数据索引：标题、应用名、URL
CREATE VIRTUAL TABLE meta_fts USING fts5(title, app_name, url, content='', tokenize='trigram');
```

- 索引可以随时从 `text_chunks` 和 `events` 重建，不作为事实来源。入库时同事务写入，删除时同事务清理。
- **中文检索**：trigram 只能直接匹配三个字符及以上的查询。两个字的短词（如「预算」「样稿」）先用结构化过滤（时间、应用、来源）缩小范围，再做受限的字面匹配，扫描上限 5000 段（`query/search.rs` 的 `SCAN_LIMIT`）；超过上限时结果标 `scan_limited`。标题、应用名、URL 的短词用字面包含匹配。
- **排序**：在 BM25 分数基础上，叠加时间新近度和注意力权重（可见、离焦点近、新出现的段落权重更高）。权重在 `store_meta.rank_weights`，默认 `{"visible":1.2,"near_focus":1.2,"is_new":1.3,"recency_half_life_days":14}`。
- 向量检索（`chunk_vectors(chunk_id, model_id, dim, vector BLOB)`，换模型要新建索引、不与旧向量混用）（未实现）。

<a id="jobs-changes-deletion"></a>

## 11. 作业、变更与删除

### 11.1 作业

`jobs`：`job_id`、`kind`（`asr`、`ocr`、`vlm`、`episode`、`fts_rebuild`、`connector_sync` 等）、`input_ref`（事件 ID，或派生内容的 id 与 rev）、`config_version`（处理器、模型与提示词版本组合）、`state`（`queued`/`running`/`done`/`failed`/`cancelled`）、`attempts`、`next_run_ms`、`lease_until_ms`、`error_code`、`output_ref`、`feature`（所属功能的编号）。`UNIQUE (kind, input_ref, config_version)` 保证同一输入在同一配置下只排一次。`job_checkpoints` 保存作业检查点。调度与重试见[模型](models.md)。

### 11.2 变更流

`changes(seq, at_ms, entity, entity_id, op)`：`seq` 自增，`op` 为 `insert`/`update`/`delete`；`entity` 取 `event`、`thread`、`episode`、`memory`、`digest`、`source`、`blob` 等。界面刷新、同步和订阅都读它。

### 11.3 墓碑

`tombstones(entity, entity_id, deleted_ms, reason)`，主键 `(entity, entity_id)`。`reason` 取：`user`、`retention`、`source_revoked`、`import_rollback`、`superseded`（识别或转写重跑后，旧结果被新结果取代）、`source_deleted`（来源里已删除）。除 `event` 外，还有 `event_key` 墓碑：键是 `(source_id, source_key)` 的令牌，使迟到的重发（含已被文字合并吸收的样本）不能把已删除的事件带回来。墓碑不保存标题和正文。

### 11.4 `store_meta`

`store_meta(key, value)` 只放小配置与小状态（`ingest_seq_next`、`access_log_params`、`rank_weights`、`schema.upgrade`、各流程的游标等），大段内容不放这里。

### 11.5 删除

**删除顺序**（`delete.rs`，一个事务）：

1. 写墓碑，立即对查询和 Agent 隐藏；删除时向下展开到子事件。
2. 取消引用了这些事件的排队或运行中的作业。
3. 删除相关派生内容（片段、依赖这些事件的记忆、覆盖期间含这些事件的汇总、对应的人物提及）和索引条目。
4. 清理已无人引用的段落（含 `text_fts` 条目）、原件（行与文件，文件在事务提交后才删）、已无事件的线程。

规则：

- 删除原因必须是第 11.3 节的枚举值。按来源删除同时清除该来源的游标、Agent 检查点和线程。
- 迟到的作业在提交前都要检查墓碑，已删除的内容不能被重新生成。删除不在墓碑里保存标题和正文。
- 删除不做「软删除后仍可查询」。删除与压缩的连带关系见[数据模型](data-model.md)。

<a id="migration"></a>

## 12. 数据迁移规则

现有旧存储（Mac 桌面账本、Hub 资料库）只读，迁移在新库进行，核对通过前不删除旧库；已完成迁移的记录见 [legacy-migration](../research/legacy-migration.md)。

<a id="live-mirror"></a>

### 12.1 增量镜像与切换到直写

Mac 桌面采集直接写入本库，原桌面账本只在切换时读一次。两边都不改写对方：

- **桌面账本（切换前）**：每次从上次位置往前 10 分钟重读，已入库的记录在读取文件前跳过；账本删除的记录同步删除；识别结果按完成时间区分，重跑后新结果入库、旧结果以 `superseded` 删除。
- **Hub 资料库**：新条目入库；资料库删除的条目连同转写段落一并删除；转写或图片文字变化时整组替换（`sync_hub_media`）。服务的周期循环现调用 `migrate_media`（迁移核对，旧库保留）；`sync_hub_media` 仅测试调用（未验证是否仍有生产路径）。
- **原件**：同卷用硬链接放入 `blobs/`，跨卷复制，不改动原文件。
- **核对**：账本里的每条记录要么以同一种类在本库，要么在本库已被删除（有墓碑）；在库的截图引用同一 hash 的原件，首次导入、切换和手动「重新核对」时逐个校验。切换后直写多出的记录不计入。

**切换**（`POST /local/desktop/cutover`）：

1. 把账本剩下的增量同步完（直到一轮不再新增，最多 5 轮）；
2. 逐条核对数量和每张截图的 hash；
3. 通过后在 `store_meta` 记下 `desktop.capture_mode = direct` 和切换记录（`desktop.cutover`：时间、耗时、记录数、截图数、核对结果），从此不再镜像账本；
4. 不通过时保持原方式（继续镜像），原因出现在 `/local/status` 的 `cutover_error`；「重新核对」再试一次切换。

切换前，核心拒绝 App 的桌面批次（409 `cutover_pending`），App 把采集留在本机待写队列；没有账本的新安装直接直写。原账本目录保留、只读。

**App 读取**：App 是内部访问方，经同一个工具分发、同一套审计读取桌面时间线（`app_activities`、`app_activity`、`app_media`、`app_day_counts` 等）。这些工具只在本地通道上供内部访问方使用，不在工具清单和 MCP 里出现；截图和录音只传路径，不传字节。`app_day_counts` 只返回每天的计数（最多 400 天），不含内容。活动归并：截图和缺口跟随同一次运行里的父观察；有活动标识的观察按（运行，活动）归并；旧记录没有活动标识时，同运行、同应用、同标题的观察相连；读取一天时向前多读 6 小时。

**核对项**：按种类计数一致；截图原件 hash 全部一致；抽样对照时间线和搜索；再次运行迁移不产生新记录；中断后能续跑。

<a id="code"></a>

## 13. 实现位置

| 内容 | 代码（`core/crates/`） | 测试 |
|---|---|---|
| 数据根目录、`store.json`、写锁、启动恢复、完整性 | `candid-store/src/store.rs`、`db.rs` | `candid-store/tests/store_root.rs` |
| 表结构与 kind 登记 | `candid-store/src/schema.rs`、`durable.rs`、`performance.rs`、`agents/checkpoints.rs` | `candid-store/tests/store_root.rs`、`schema_upgrade.rs` |
| 表结构升级 | `candid-store/src/schema_upgrade.rs` | `candid-store/tests/schema_upgrade.rs` |
| 入库事务、幂等、冲突、受保护内容 | `candid-store/src/ingest.rs`、`records/` | `candid-store/tests/ingest.rs`、`records.rs`、`record_boundaries_acceptance.rs` |
| 段落切分与规范化 | `candid-store/src/chunk.rs` | `candid-store/tests/ingest.rs`、`chunk.rs` 内单元测试 |
| 原件、压缩与转换 | `candid-store/src/ingest.rs`、`json_blobs.rs`、`webp.rs`、`library_conversion.rs` | `candid-store/tests/ingest.rs`、`library_conversion.rs` |
| 检索索引与排序 | `candid-store/src/query/search.rs`、`query/core.rs` | `candid-store/tests/query.rs` |
| 删除与墓碑 | `candid-store/src/delete.rs` | `candid-store/tests/ingest.rs`、`schema_upgrade.rs` |
| 服务进程、端点、同步循环 | `candid-service/src/serve.rs`、`endpoint.rs`、`local.rs` | `candid-service/tests/server.rs`、`desktop.rs`、`local.rs` |
| 旧存储迁移与镜像、切换 | `candid-store/src/migration.rs`、`livesync.rs`、`desktop.rs` | `candid-store/tests/migration.rs`、`sync_status.rs`、`candid-service/tests/desktop.rs` |

<a id="open"></a>

## 14. 未实现与待定

| 项 | 状态 | 说明 |
|---|---|---|
| 来源 `retention_days` 的执行 | 未验证 | `policy_json` 保存该字段，没找到执行它的代码 |
| 向量检索 `chunk_vectors` | 未实现 | 见第 10 节 |
| `ai.prompt` 的 `origin: "screen"`（屏幕补位） | 未验证 | payload 约定里有该取值，产生逻辑未核对 |
| `browser.visit` 导入 | 未实现 | kind 已登记，导入未开启（见[连接器](connectors.md)） |
| 分级、保真度、精简记录、用量汇总、`chunk_families`、`evidence`、`retention_policies`、`settings` 等表 | 未实现 | 数据模型规格设计了这些表，代码里没有；见[数据模型](data-model.md) |
| 墓碑原因 `compacted` | 未实现 | 随压缩功能一起加 |
| 可迁移包导出与导入 | 未实现 | 见下 |
| 中文检索方案定稿 | 部分实现 | 现为 trigram 加 5000 段受限字面匹配；是否改为二字切分列，待用中英混合问题集比较 |
| `focus_distance` 阈值与排序权重 | 设计中 | 默认见第 10 节；待用真实窗口对比 |
| SQLite `synchronous` 级别与写入开销 | 设计中 | 现为 `FULL`；待长期测量 |
| Hub 资料库周期镜像 | 未验证 | 见第 12.1 节 |
| 旧桌面账本迁移与对账代码删除 | 未实现 | 代码仍在 |

**导出映射**（未实现）：计划把库内容导出为可迁移包（范围 `agent_text`、`evidence_selected`、`full_owned`），包不需要本服务和任何模型即可阅读；导入先暂存再校验，按 `(source_id, source_key)` 与 `(artifact_id, revision)` 幂等写入，冲突进入冲突区，不覆盖已有数据。

| 本库内容 | 可迁移包中的形式 |
|---|---|
| `events` 和 `event_text` | 每条事件一个 `core.observation` 系列 artifact（`desktop.*`、`calendar.*` 等类型），正文按段落顺序还原，附 selector |
| `blobs` | `blobs/<sha256>`，manifest 中列出 hash 和大小 |
| `threads` | 独立 artifact，事件通过引用指向它 |
| `episodes`、`memories`、`corrections` | 对应层的 artifact，保留 revision 和来源 |
| `sources`、`clients`、`ai_*` 配置 | 写入 manifest 的配置部分，**不含**任何密钥和令牌 |
| `ai_calls`、`access_log` | 仅 `full_owned` 范围导出，JSONL 格式 |

<a id="sources"></a>

## 15. 来源

内容迁自旧文档（均在 `docs/specs/`）：

- [timeline-v1.md](timeline-v1.md)：第 1 节「工程设计」、2.1、2.2、第 3 节、第 4 节开头、4.1–4.7、4.11、第 8 节（导出映射）、第 9 节中仍成立的规则（9.1）。
- [data-model-v1.md](data-model-v1.md)：第 12 节中已实现的 v9、v10 升级；其余内容归[数据模型](data-model.md)。
- 决定：[ADR-024](../decisions/024-rust-core.md)、[决定流水](../decisions/log.md)。
