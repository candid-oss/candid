# 术语表

**状态**：未验证。按各规格的字面整理，没有逐条核对代码；表名、字段名以 `core/crates/candid-store/src/schema.rs` 为准。各规格里用词不一致的地方集中在[第 7 节](#inconsistent)。

本文回答「这个词在 Candid 里指什么，和哪个相近的词不一样」。它不规定行为，行为在[规格](specs/README.md)里；词义与规格冲突时以规格为准，并改本文。

读法：每组一张表。「容易混」一列写最常被当成同一个东西的词。

<a id="system"></a>

## 1. 系统构成

| 术语 | 英文 / 代码名 | 指什么 | 容易混 | 详见 |
|---|---|---|---|---|
| 核心 | core、`candid serve` | 唯一能打开数据库的 Rust 进程，管存储、整理、查询、权限、模型调用 | 和「平台端」：界面和系统采集不在核心里 | [架构](architecture.md#processes) |
| 平台端 | Mac App、`apps/macos/` | 每个平台一份的界面、系统采集、系统权限和日历通知等平台能力 | 和核心：平台端不写业务逻辑、不碰数据库 | [架构](architecture.md#processes) |
| 本地通道 | `/local/…`、Unix 域套接字 | App 和核心之间的专用通道，外部访问不了 | 和回环 HTTP：同一个核心的两个入口，本地通道只给 App | [查询](specs/query.md#local-channel) |
| 回环 HTTP | `127.0.0.1`、`endpoint.json` | 核心在本机监听的接口，命令行、MCP 外壳、Pi 用它，带令牌 | 和 MCP：MCP 只是外壳，背后还是回环 HTTP | [存储](specs/storage.md#deployment) |
| MCP | `candid mcp` | 外部 agent 接入的标准入口，23 个工具 | 和命令行：同一套工具的两种壳 | [查询](specs/query.md#tools) |
| 数据根目录 | root、`--root` | 存放时间线库和原件的整个目录，可整体搬走 | 和运行目录：运行目录只放地址和令牌，不放数据 | [存储](specs/storage.md#deployment) |
| 运行目录 | `CANDID_RUN_DIR` | 放 `endpoint.json` 和命令行令牌的目录，让客户端找到核心 | 和数据根目录 | [存储](specs/storage.md#deployment) |
| 时间线库 | `timeline.sqlite` | 唯一的事实仓库（SQLite）加原件目录 | 和「时间线」界面：界面只是读它 | [存储](specs/storage.md#overview) |
| 访问方 | `clients` 表 | 在核心登记过的一个调用者身份（App、命令行、某个外部 agent、Pi），有令牌和范围 | 和外部 agent：外部 agent 只是访问方的一类 | [查询](specs/query.md#clients) |
| 令牌 | `token_hash` | 访问方的凭证，原文只显示一次 | | [查询](specs/query.md#clients) |
| 范围 | `scope_json` | 一个访问方能看到什么：哪些来源、种类、应用、多少天、能不能读正文、截图、派生内容、能不能提案 | 和来源策略：策略是数据来源的设置，范围是访问方的设置，两个都要过 | [查询](specs/query.md#scope) |
| 提案 | `proposals`、`propose` | agent 想在 App 之外做的事（建提醒、改日历），只进待批准列表，你批准后 App 才执行 | 和「写入」：写入是改本库的派生内容，可撤销；提案是改外面的世界 | [查询](specs/query.md#proposals) |
| 审计 | `access_log` | 每次工具调用记一行：谁、调了什么、返回多少、是否被拒 | 和用量记账：用量记的是模型调用 | [查询](specs/query.md#audit) |
| 单写者 | `.lock` | 同一个数据根目录只能有一个核心，第二个以退出码 75 结束 | | [存储](specs/storage.md#deployment) |

<a id="raw"></a>

## 2. 原始记录

| 术语 | 英文 / 代码名 | 指什么 | 容易混 | 详见 |
|---|---|---|---|---|
| 来源（数据来源） | `sources`、`sources.kind` | 一个接进来的数据通道，如 `mac.desktop`、`mail.imap`、`agent.transcript`，有授权、状态和策略 | 和模型来源、和「出处」，见[第 7 节](#inconsistent) | [存储](specs/storage.md#source-tables) |
| 来源策略 | `policy_json` | 每个数据来源的隐私设置：存不存正文、存不存原件、agent 可不可见、排除哪些应用 | 和范围，见上 | [存储](specs/storage.md#source-tables) |
| 事件 | `events` | 时间线的唯一主线，一行一条发生过的事实（一次焦点切换、一封邮件、一段录音） | 和消息：事件是原始事实，消息是整理后的对话单元 | [存储](specs/storage.md#events) |
| 事件种类 | `event_kinds`、`kind` | 事件的类型登记表，如 `screen.text`、`mail.message`、`ai.prompt`；证据层的由采集写，感知层的由作业生成 | 和对话的 `kind` 不是一回事 | [存储](specs/storage.md#event-kinds) |
| 线程 | `threads` | 把属于同一场会议、对话或邮件串的事件归在一起 | 和对话：线程在原始层，对话在整理层 | [存储](specs/storage.md#threads) |
| 原件 | `blobs` | 截图、录音、附件的字节，按内容哈希存放，同一份只存一份 | 和派生 JSON：识别结果也作为原件目录里的压缩文件存放 | [存储](specs/storage.md#blobs) |
| 段落 | `text_chunks` | 文字切成的最小存储单位，全库同一段只存一份，事件通过引用指向它 | 和片段、和摘录，见[第 7 节](#inconsistent) | [存储](specs/storage.md#text) |
| 新内容 | `is_new` | 相比同一窗口上一次屏幕文字，新出现的段落 | | [存储](specs/storage.md#text) |
| 检索索引 | `text_fts`、`meta_fts` | 全文搜索用的索引，可随时重建 | 不是事实，丢了不丢数据 | [存储](specs/storage.md#fts) |
| 入库批次 | `IngestBatch` | 采集端交给核心的一包数据：线程、事件、文字、原件引用和新游标，一个事务写完 | | [连接器](specs/connectors.md#ingest-connectors) |
| 游标 | `source_cursors` | 记录某个来源同步到哪了，只在批次提交后才前进 | | [存储](specs/storage.md#source-tables) |
| 冲突区 | `ingest_conflicts` | 同一来源键来了内容不同的记录，新的放这里，不覆盖旧的 | | [存储](specs/storage.md#events) |
| 墓碑 | `tombstones` | 删除的记录：立刻对查询隐藏，迟到的重发不会复活它，不保存正文 | 和「软删除」：这里没有软删除 | [存储](specs/storage.md#jobs-changes-deletion) |
| 变更流 | `changes` | 每次新增、更新、删除记一行，界面刷新和订阅都读它 | | [存储](specs/storage.md#jobs-changes-deletion) |
| 受保护内容 | `sensitivity='protected'` | 只存元数据，永不存正文和原件，如密码框 | | [存储](specs/storage.md#events) |
| 保真度阶梯 | fidelity ladder | 原件只能沿「原图 → 只留文字 → 只留元数据」往下降 | 精简记录和预演未实现 | [数据模型](specs/data-model.md#fidelity) |
| 证据分级 | `pinned`/`key`/`normal`/`ephemeral` | 记录的保留级别，决定整理先后（未实现） | 和证据，见下 | [数据模型](specs/data-model.md#tiers) |
| 连接器 | connector | 把一类数据送进入库接口的模块。采集型读文件、网络或系统接口；引用型（AI 对话）只引用不存回复 | 和来源：连接器是程序，来源是登记的通道 | [连接器](specs/connectors.md#ingest-connectors) |
| 来源定级 | `ignore`/`metadata`/`normal`/`important` | 每条内容先定级，级别决定处理多深；现只有邮件实现 | 和证据分级 | [连接器](specs/connectors.md#source-filter) |

<a id="curated"></a>

## 3. 整理层

整理层的东西都是派生的：可以删掉重算，你的修改另存。

| 术语 | 英文 / 代码名 | 指什么 | 容易混 | 详见 |
|---|---|---|---|---|
| 原始层 / 整理层 / 理解层 | | 数据的三层：截图和文字帧；对话、消息、摘录、活动块；人物、条目、汇总。AI 默认只读后两层 | 和数据模型的五层，见[第 7 节](#inconsistent) | [整理层](specs/curation.md#overview) |
| 对话 | `conversations` | 一场聊天、邮件串、会议或 AI 会话，`kind` 为 `chat`/`mail`/`meeting`/`agent` | 和线程 | [整理层](specs/curation.md#overview) |
| 消息 | `messages` | 对话里的一条发言，带发送方向（我/对方/未知）。邮件、转写、聊天截图都变成消息 | 和事件 | [整理层](specs/curation.md#overview) |
| 摘录 | `excerpts` | 阅读类内容的正文：网页、文档的无障碍正文，或截图里抄出来的阅读内容 | 和消息：聊天是消息，阅读是摘录 | [整理层](specs/curation.md#overview) |
| 截图摘录 | `excerpt_scan`、`screen.excerpt` | 流程：挑出有新文字的关键截图，让 `vision` 模型抄出主内容区，聊天写成消息、阅读写成摘录 | 和无障碍摘录 | [整理层](specs/curation.md#screen-excerpt) |
| 无障碍摘录 | AX | 用系统无障碍接口直接读到的正文，不用模型；有它的应用，截图摘录跳过 | 和截图摘录 | [整理层](specs/curation.md#curated-sources) |
| 记录投影 | `curated_sync` | 不调模型的规则转换：邮件、转写、AI 提示词直接变成消息和摘录 | | [整理层](specs/curation.md#curated-sources) |
| 屏幕清洗 | `screen_scan` | 对屏幕文字做近似去重和降噪，再切段 | | [整理层](specs/curation.md#near-dup) |
| 近似去重 / 降噪 | | 把差不多的段落归一族；给界面固定文字降权 | 和精确去重（段落哈希） | [整理层](specs/curation.md#near-dup) |
| 屏幕片段 | `screen:<哈希>` | 同一窗口键内相隔不超过 5 分钟的屏幕帧切成的一段，规则标题 | 和活动块：同一个对象的前后阶段，见[第 7 节](#inconsistent) | [整理层](specs/curation.md#segments) |
| 活动块 | `episodes`，`kind=activity` | 一个时间块的标题和概括。结束十分钟后写入，含整理内容的才调模型概括 | 和片段、活动段 | [整理层](specs/curation.md#segments) |
| 活动块概括 | `screen.summarize` | 用整理层内容给已结束的活动块写标题和概括，引用必须是输入编号 | | [整理层](specs/curation.md#segments) |
| 汇总 | `digests` | 小时、天、周三级精简摘要。现在只有天级草稿 | 和日回顾：日回顾是写天级汇总的技能 | [整理层](specs/curation.md#digests) |
| 派生 | derived | 由原件或记录算出来的东西：OCR、转写、活动块、抽取结果 | 和记录：记录是来源报的事实，不可重算 | [数据模型](specs/data-model.md#layers) |
| 修订 | `rev` | 派生对象每次重生成都是新修订，旧的标 `superseded` | | [整理层](specs/curation.md#derived) |
| 草稿 | `draft` | 模型或 agent 产出、尚未被你确认的状态 | 显示优先级：你改的 > 模型草稿 > 规则文本 | [整理层](specs/curation.md#derived-next) |
| 纠正 | `corrections` | 你的改名、合并、拒绝等决定，单独保存，重算后仍有效 | 和修订 | [整理层](specs/curation.md#derived) |
| 证据 | `evidence`、`episode_evidence` | 一条派生内容所依据的原始记录。同时是「证据层」事件、`evidence` 下钻工具、片段的证据行 | 见[第 7 节](#inconsistent) | [整理层](specs/curation.md#segments) |
| 入口打码 | redaction | 密钥类文字入库前替换（通用打码未实现） | | [整理层](specs/curation.md#redaction) |

<a id="understanding"></a>

## 4. 理解层

| 术语 | 英文 / 代码名 | 指什么 | 容易混 | 详见 |
|---|---|---|---|---|
| 实体 | `entities` | 人、项目、组织、地点、网站、应用、话题的统一记录 | 和人物：人物是 `kind=person` 的实体，现在只有人物有归并实现 | [人物与条目](specs/people-items.md#entities) |
| 人物 | person | 一个现实中的人，合并了他在各应用里的名字、邮箱、账号 | 和声纹说话人：声纹匹配只是候选关联 | [人物与条目](specs/people-items.md#alias-identity) |
| 别名 | `entity_aliases` | 一个人的某个名字、邮箱、账号或声纹编号，如 `handle:wechat/张三`。稳定，不随重算变 | 和实体编号：编号会变，别名不变，所以你的纠正挂别名 | [人物与条目](specs/people-items.md#alias-identity) |
| 提及 | `entity_mentions` | 某个实体在某条事件或活动块里出现了；随重算会被级联删除 | | [人物与条目](specs/people-items.md#entities) |
| 候选对 | `people.judge` | 规则找出的「可能是同一个人」的两个人，交小模型判，结果是候选，你确认才合并 | 和自动合并：模型判且有规则佐证才自动合并，可撤销 | [人物与条目](specs/people-items.md#candidates) |
| 归并规则 | | 人物自动合并的顺序规则；撤销记为「不是同一人」 | | [人物与条目](specs/people-items.md#merge-rules) |
| 说话人 | `voice:` | 录音里分出的发言人，声纹匹配只是候选，你指定后才确认 | 和人物 | [人物与条目](specs/people-items.md#speaker-labeling) |
| 条目 | `items` | 有时间和状态的事：日程、提醒、承诺，分「我答应的」和「等别人做的」 | 和备忘：备忘没有时间和状态 | [人物与条目](specs/people-items.md#items) |
| 候选 / 已确认 / 已忽略 | `review` | 条目的审阅状态；候选 7 天没处理自动收起 | 和 `state`（开着、完成、取消、过期）是两个字段 | [人物与条目](specs/people-items.md#item-states) |
| 备忘 | memo、`memories` 的 `kind=fact` | 关于某人某事的事实，挂在人的别名上，你确认前是候选 | 和记忆 | [人物与条目](specs/people-items.md#memos) |
| 记忆 | `memories` | 整合后的事实、偏好、判断、承诺、决定等，带状态和证据。条目抽取只写 `fact`，录音的决定写 `decision` | 记忆是大类，备忘是其中一种 | [整理层](specs/curation.md#derived) |
| 承诺 | commitment | 我答应了别人或别人答应了我的事，要有明确表述才算 | 同一个词在两处存储，见[第 7 节](#inconsistent) | [固定流程](specs/workflows.md#commitments) |
| 日程 / 提醒 | `kind=event`/`reminder` | 条目的另外两种，确认后经提案写进系统日历或提醒事项 | 和日历事件（`calendar.event`，原始事件） | [人物与条目](specs/people-items.md#item-states) |

<a id="ai"></a>

## 5. AI

| 术语 | 英文 / 代码名 | 指什么 | 容易混 | 详见 |
|---|---|---|---|---|
| 模型来源 | `ai_providers` | 你配置的一个模型服务地址和密钥引用，目的地分本机、自建、云端 | 和数据来源；架构文档里叫「模型服务商」 | [模型](specs/models.md#providers) |
| 模型 | `ai_models` | 模型来源下的一个具体模型名及价格 | | [模型](specs/models.md#providers) |
| 角色 | `ai_roles` | 给一类调用指定用哪个模型。只有三个：`vision`、`summary`、`agent_chat`，不新增 | 和访问方、和说话人的 actor | [模型](specs/models.md#roles) |
| 模型通道 | lane | 核心里两条互不等待的调用线：交互通道（有人等，不重试）和后台通道（作业队列，串行） | 和通道（本地通道） | [模型](specs/models.md#lanes) |
| 用量记账 | `ai_calls` | 每次模型调用记一行，不存提示词和回答正文 | 和审计 | [模型](specs/models.md#ai-usage) |
| 作业 | `jobs`、`job_kind` | 作业队列里的一件待办，如 `screen.summarize`、`items.extract`；模型只在作业里被调用 | 和流程：流程是定义，作业是一次执行 | [存储](specs/storage.md#jobs-changes-deletion) |
| 固定流程 | workflow、`core/workflows/` | 固定步骤（scan → 作业队列 → prepare → 模型 → complete），没有模型自选工具的循环 | 和技能：流程在核心里自动跑，技能由 agent 执行 | [固定流程](specs/workflows.md#tasks) |
| 功能（目录） | `features` | 流程目录里的一项开关，登记了触发、角色、每日上限，共 17 项，含五个技能 | 和流程、技能 | [固定流程](specs/workflows.md#tasks) |
| 技能 | skill、`agents/skills/<名>/SKILL.md` | 一份给 agent 的任务说明，现有五个，Pi 和外部 agent 共用 | 技能运行也占一个 `agent.skill` 作业 | [固定流程](specs/workflows.md#hooks-skills) |
| 挂接的 agent | Pi | App 启动的内置对话 agent，只带 Candid 扩展给的工具，关掉自带工具 | 和外部 agent | [固定流程](specs/workflows.md#pi-attach) |
| 扩展 | `agents/pi-extension/` | Pi 的插件，工具全部转发给核心 | | [固定流程](specs/workflows.md#pi-guardrails) |
| 外部 agent | Claude Code、Codex | 你自己的 agent，通过 MCP 或命令行接入，权限和 Pi 同一套 | 和挂接的 agent | [查询](specs/query.md#access) |
| 工具三档 | | 读、写（只限本库派生与纠正，可撤销）、提案 | | [固定流程](specs/workflows.md#pi-guardrails) |
| 写入档 | `writes` | 范围里逐项授予的写权限：`annotate`、`reference`、`commitment`、`draft` | 和提案 | [查询](specs/query.md#scope) |
| 回放 | replay | 在真实库副本上重跑生产流程的实验工具（`candid-lab`），不随产品发布 | | [架构](architecture.md#modules) |
| 帮我写 | `/local/compose` | 走交互通道、用 `agent_chat` 的回复起草 | | [固定流程](specs/workflows.md#compose) |
| 资料 | references | 你标记的参考材料，给回复检索用 | | [固定流程](specs/workflows.md#references-skill) |

<a id="docs"></a>

## 6. 文档与工程

| 术语 | 英文 / 代码名 | 指什么 | 容易混 | 详见 |
|---|---|---|---|---|
| 规格 | `docs/specs/` | 一个主题现在的行为和规则，随代码改 | 和决定、计划 | [文档规范](standard.md#kinds) |
| 决定 / 决策记录 | `decisions/` | 为什么这么定。流水是一行一条，ADR 是长篇，只追加 | 和规格：规格写现状，决定写理由 | [决定索引](decisions/README.md) |
| 计划 | `docs/plans/` | 一件正在做的事，做完即删 | | [文档规范](standard.md#kinds) |
| 契约 | contract | 仓库外的东西依赖的接口，每项必须有测试或快照守着 | 和规格 | [契约清单](contracts/README.md#list) |
| 快照 | snapshot | 把接口或模型输入存成文件，不一致就让测试失败 | 有意改动才用 `UPDATE_SNAPSHOTS=1` 重新生成 | [契约清单](contracts/README.md#compat) |
| 全流程测试 | `lifecycle.rs` | 用三天假数据和回环桩模型，跑完入库到查询的整条链 | 和单元测试 | [工程规范](engineering.md#tests) |
| 状态用语 | | 已实现、部分实现、未实现、设计中、已废弃、未验证 | 不确定就写未验证 | [文档规范](standard.md#header) |

<a id="inconsistent"></a>

## 7. 用词不一致

下表列出不同规格用词不一致的地方。这些规格本文没有改，建议对照处理。

| # | 现象 | 出处 | 建议统一为 |
|---|---|---|---|
| 1 | 同一个对象叫了四个词：「片段」「活动块」「活动段」「episode」。整理层概览写「活动块（片段）」；查询的 `episodes`/`episode` 工具叫「片段」并归在原始记录层，`day` 叫「活动块」；整理层 2.1 的环节 6 写「活动段、片段」，汇总小时级写「活动段统计」；设计规范第 12 节的「片段」又是时间线上的色条 | [整理层](specs/curation.md#overview)、[查询](specs/query.md#tools)、[设计规范](specs/design-system.md#timeline-lanes) | 数据对象一律叫「活动块」（代码 `episode`）；「屏幕片段」只用于未概括的规则切分阶段；废弃「活动段」；界面色条叫「时间条」 |
| 2 | 「片段」还指搜索结果里的高亮摘句；「段落」是 `text_chunks`，整理层「抽取」里又说「新出现的段落」 | [查询](specs/query.md#tools)、[存储](specs/storage.md#text) | `text_chunks` 叫「段落」；搜索摘句叫「摘句」 |
| 3 | 「承诺」存在两处：人物与条目写成 `items.kind=commitment`，固定流程第 6 节与整理层第 11 节写成 `memories.kind=commitment`，`pv_commitment` 也写 memories | [人物与条目](specs/people-items.md#items-storage)、[固定流程](specs/workflows.md#commitments) | 先核对代码确认是不是「候选在 memories，确认后进 items」。是，就在两份规格里写明这个流转；否，保留 items 一处 |
| 4 | 「层」有三套：整理层规格的原始/整理/理解三层；数据模型的原件/记录/派生/记忆/纠正五层；事件种类的 `evidence`/`perception` 两层。「派生层」又常指整理层加理解层 | [整理层](specs/curation.md#overview)、[数据模型](specs/data-model.md#layers)、[存储](specs/storage.md#event-kinds) | 「原始/整理/理解」叫数据层；五层改称「存放类型」；事件种类的叫「事件层」 |
| 5 | 「来源」指三样：数据来源（`sources`）、模型来源（`ai_providers`）、出处/依据（各规格的「来源」小节、`source_revision`）。架构文档把模型来源叫「模型服务商」 | [存储](specs/storage.md#source-tables)、[模型](specs/models.md#providers)、[架构](architecture.md#overview) | 数据来源、模型来源、出处三个词分开；模型来源在界面和规格里统一，不再用「服务商」 |
| 6 | 「访问方」和「客户端」混用：查询规格的表叫 `clients`、正文叫访问方，存储规格写「客户端范围」；「Agent」首字母大写与「agent」并存 | [查询](specs/query.md#clients)、[存储](specs/storage.md#jobs-changes-deletion)、[存储](specs/storage.md#source-tables) | 一律「访问方」，表名保留 `clients`；「外部 agent」小写 |
| 7 | 「角色」：规则只允许三个，但固定流程目录和模型规格 3.1 仍列出 `title`、`episode`、`asr`、`diarization`、`ocr`、`relevance`、`embedding`，规格已标为欠账 | [固定流程](specs/workflows.md#tasks)、[模型](specs/models.md#roles) | 以模型规格为准，目录的功能表改写成三个角色，语音服务设置独立 |
| 8 | 「流程」同时指固定流程（目录）、整理流程（scan 到 complete 的一整套）、录音流程；「功能」又是目录里流程加技能的总称 | [整理层](specs/curation.md#overview)、[固定流程](specs/workflows.md#tasks) | 「固定流程」专指目录里的定义；一整套叫「整理管线」；「功能」只指设置页的开关项 |
| 9 | 「记忆」在数据模型里是一层（人物、事实、偏好、承诺），在整理层是 `memories` 表，在人物规格里人物是实体而不是记忆；备忘是 `memories` 的一种 | [数据模型](specs/data-model.md#layers)、[人物与条目](specs/people-items.md#memos) | 「记忆」只指 `memories` 表；数据模型那一层改叫「整合层」 |
