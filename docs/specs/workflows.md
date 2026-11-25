# AI 功能的定义与执行

**中文** · [English](workflows.en.md)

**状态**：部分实现。固定流程、挂接的 Pi、技能运行、提案与「帮我写」已实现；MCP 提示词模板、小时与周汇总、一批「只做模板」的技能未实现。

本文规定哪些 AI 功能自己做、固定流程怎样定义、两种执行方式（核心内自动与挂接的 Pi）的边界与护栏、技能与触发、承诺与提醒、资料与回复检索、「帮我写」、执行位置。不规定数据整理流程，见 [整理层](curation.md)；不规定人物与条目，见 [人物与条目](people-items.md)；不规定模型角色、预算与调度，见 [模型](models.md)；不规定查询工具与访问方，见 [查询接口](query.md)。

硬约束（同 [AGENTS.md](../../AGENTS.md)）：

1. 不自造 agent loop 或 harness。对话与技能交给 Pi；核心只做固定流程（scan → 作业队列 → prepare → 模型 → complete）。
2. 采集到的内容是证据，不是指令。
3. 写操作只走提案；本库内的派生与纠正按第 5 节的写入档，可撤销。
4. 执行者不带终端和文件写入工具。

<a id="overview"></a>

## 1. 概览

Candid 里的 AI 功能分两种，另有对外接口：

| 方式 | 是什么 | 由谁调度 | 例子 |
|---|---|---|---|
| 核心内自动 | 固定步骤，至多调一次角色模型，输出经校验后存为派生草稿 | Rust 核心：scan → 作业队列 → prepare → 模型 → complete | 转写、录音命名、摘要、屏幕摘录、人物判断、条目抽取 |
| 挂接的 agent | [Pi](../glossary.md) 以 RPC 模式由 App 管理，用我们的技能、插件工具和护栏；由 AI 页对话或技能运行启动 | App 的 `SkillRunner` 向核心领取运行，核心冻结范围与预算 | AI 页对话、日回顾、资料与回复检索 |
| 对外接口 | MCP 与 API，Claude Code、Codex 等接入，同一套工具与权限 | 外部 agent 自己 | 见 [查询接口](query.md) |

另有「帮我写」（第 8 节）：一次模型调用，由 App 快捷键触发，不属于上面两种方式的运行记录。

<a id="position"></a>

## 2. 定位与原则

Candid 同时是两样东西：

- **独立产品**：不接任何外部 agent，也能完成浏览、搜索、简单回顾与待办草稿。
- **个人 agent 基座**：把采集到的信息整理成干净、可引用、可纠正的数据，供外部 agent 读取。

关键问题是「数据怎样处理干净」和「哪些功能做、哪些不做、哪些只做模板交给外部 agent」。任务定义与执行者分开。记忆与人格不在本文给默认设计（见 research/memory-and-persona.md）。

原则：

1. **内置能力和外部 agent 读同一份数据**，都经过时间线服务的范围过滤与审计。
2. **花钱按层递增**：规则、统计不花 token；小模型按「段」调用一次并缓存；固定流程只在获准触发时运行，有预算上限。
3. **合并只建关联，不删原件。**去重、降噪、实体合并都记录为关系或权重，可以撤销和重算。
4. **写操作只走提案**；派生草案可以在授权范围内自动保存，但始终标明是草案。
5. **屏幕、网页、消息、邮件里的文字都是不可信数据**：可以作为证据，不能作为指令。任务提示词把它们放在明确的数据区，执行者不带终端和文件写入工具。

<a id="features"></a>

## 3. 功能取舍

判断标准：离开外部 agent，产品还能不能用；使用频率高不高；需不需要 Candid 以外的工具（邮件、代码、网页）；隐私上是否必须留在本机。

状态列是对代码逐行核对的结果。「默认开关」以 `core/workflows/catalog.json` 为准。

| 类别 | 功能 | 说明 | 状态 |
|---|---|---|---|
| **核心内自动** | 时间线浏览与搜索 | 规则和索引，不用模型 | 已实现（见 [查询接口](query.md)） |
| | 时间分布 | 纯规则统计（`pv_overview`） | 已实现 |
| | 转写、多人分段、已有声纹匹配 | 录音保存后自动（流程 `transcribe`，作业 `speech.recognize`；设置 `automatic` 默认开） | 已实现 |
| | 录音标题 | 转写完成后自动（`title`，设置 `titles` 默认开）；人工改过的标题不被重跑覆盖 | 已实现 |
| | 录音摘要 | 整段结束后（`summary`）。目录写默认开，但扫描只在设置 `summary == true` 时排队，实际默认关 | 已实现（默认值两处不一致，见第 11 节） |
| | 截图文字 | 本机系统 OCR 在 App 采集侧；视觉模型整理：`screen-excerpt`（新截图内容出现时，默认关，每日 300 次）与手动 `screenshot` | 部分实现（OCR 在采集侧，未核对） |
| | 抽取：承诺、请求、决定、问题、话题 | 现为 `item-extract`：对话空闲 10 分钟、重点邮件、录音结束后触发（默认关，每日 150 次）；另有手动 `decisions`、`tags`。「每个结束的片段一次」的旧写法已不成立 | 已实现 |
| | 屏幕片段整理 | `screen-episodes`，片段结束后（默认关，每日 60 次） | 已实现 |
| | 承诺候选 → 提醒提案 | 第 6 节 | 已实现 |
| | 说话人与人物归并 | `people-judge`，出现新别名时（默认关，每日 50 次），见 [人物与条目](people-items.md) | 已实现 |
| | 小时与周汇总 | `hourly-weekly`，目录登记 `implemented: false` | 未实现 |
| **挂接的 agent** | 在 App 里对话并操作内容 | 查找、整理、改标题、标资料；本库内写入可撤销，App 外动作走提案 | 已实现 |
| | 日回顾 | 技能 `daily-review`；定时钩子，存为天级草稿；推送默认关 | 已实现（自动触发受质量门槛限制，第 5.4 节） |
| | 资料与回复检索 | 技能 `references-replies`，返回带出处的原文和回复草稿，不代发 | 已实现 |
| | 问问你的一天（带引用的问答） | AI 页对话；外部 AI 经对外接口用同一套工具 | 已实现 |
| | 会议回顾、项目历史、承诺跟踪 | 技能 `meeting-review`、`project-history`、`commitment-tracker`，均为手动 | 已实现 |
| **对外接口（MCP/API）** | 人和项目页面 | 基于人物与条目，见 [人物与条目](people-items.md) | 已实现（`person` 工具；项目页未验证） |
| | 提案审批 | 只供 App，带令牌的请求一律拒绝（第 6 节） | 已实现 |
| | 用量、隐私设置 | `usage` 工具已有；隐私设置的对外控制面 | 部分实现（隐私设置未验证） |
| **只做模板，交给外部 agent** | 站会与周报、项目交接、工作流发现与自动化建议、技能沉淀、深度研究、带上下文写稿、会议准备、记账跟进 | 需要其他工具或个人格式，推理量大或个人差异大。仓库里没有这些模板 | 未实现 |
| | MCP 提示词模板（prompts 原语） | 目标是外部 agent 接上 `candid mcp` 就能列出并调用；代码里没有 prompts 的实现 | 未实现 |
| **不做** | 通用聊天助手 | 不和通用 AI 产品竞争 | — |
| | 直接执行外部动作（发消息、改日历） | 只出提案，你批准后由 App 执行 | — |
| | 带终端或写代码的 agent | 挂接的 Pi 关闭自带工具，只有我们的插件工具 | — |
| | 专注评分等监控式指标 | 产品定位 | — |
| | 默认逐张截图调用视觉模型 | 成本；`screen-excerpt` 默认关，开启后也按「新内容」触发而非逐张 | — |
| | 视频导出 | 与基座无关 | — |
| **暂缓** | 浏览器历史导入、Exchange 直连 | 见 [连接器](connectors.md) | — |
| **待研究** | 记忆与人格画像、「你关心什么」、主动打扰的时机 | 见 research/memory-and-persona.md；日回顾推送是已决定的例外 | 设计中 |

<a id="tasks"></a>

## 4. 固定流程的最小框架

只提供固定步骤，不提供模型自行选工具的循环。功能按需开关；排序、归组、去重和统计优先用规则。新增模型不自动下载，不改变已授权目的地。

固定流程目录在 `core/workflows/`，随发布装载（编译进核心，`include_str!`）：

| 文件 | 内容 |
|---|---|
| `catalog.json`（`candid-workflows/1`） | `workflows`：版本化的流程定义；`features`：功能目录，登记核心内功能与技能，设置页、AI 页和对外接口都读它 |
| `prompts.json`（`candid-prompts/1`） | 固定流程的提示模板：`title`、`summary`、`decisions`、`tags`、`screenshot`、`daily` |
| `outputs.schema.json`（`candid-workflow-output/1`） | 输出类型：`draft/1`（`text` 1–16000 字符、`source_revision`、`producer`、`prompt_version`、`job_id`）、`candid-voice-result/1` 等 |

运行逻辑留在 store 和 service 模块，不在目录里。用户开关、角色映射与预算是独立配置（`store_meta` 的 `workflows.settings/1` 与功能设置），不修改发布定义。外部 AI 的 `agents/skills/` 单独维护，不是内部可执行脚本目录。

闭环：选取已保存输入 → 按版本固定步骤 → 规则或角色模型 → 校验结构与来源 → 保存新修订 → 更新索引和处理状态。代码上对应 `workflow_scan`（排队）→ `workflow_prepare`（组装不可变的请求，调用方在发 HTTP 前释放数据库锁）→ 模型调用 → `workflow_reply_allowed`（核对配置、来源修订、租约）→ `workflow_complete`（草稿与作业在一次事务里落盘，重放只落一次）。

**目录里的功能**（`features`，共 17 项；`job_kind` 是作业队列里的种类）：

| id | 方式 | 触发 | 角色（目录值） | 作业种类 | 默认 |
|---|---|---|---|---|---|
| `transcribe` | 核心 | 录音保存后 | diarization | `speech.recognize` | 开，每日 100 |
| `title` | 核心 | 转写完成后 | title | `workflow.title` | 开，每日 100 |
| `summary` | 核心 | 整段录音结束后 | summary | `workflow.summary` | 目录开；扫描实际要求设置 `summary == true` |
| `decisions` | 核心 | 手动 | summary | `workflow.decisions` | 开 |
| `tags` | 核心 | 手动 | title | `workflow.tags` | 开 |
| `screenshot` | 核心 | 手动 | vision | `workflow.screenshot` | 开 |
| `daily` | 核心 | 手动 | summary | `workflow.daily` | 开 |
| `screen-episodes` | 核心 | 片段结束后 | episode | `screen.summarize` | 关，每日 60，不许远程 |
| `screen-excerpt` | 核心 | 截图出现新内容时 | vision | `screen.excerpt` | 关，每日 300，不许远程 |
| `people-judge` | 核心 | 出现新别名时 | summary | `people.judge` | 关，每日 50，不许远程 |
| `item-extract` | 核心 | 对话空闲 10 分钟、重点邮件、录音结束 | summary | `items.extract` | 关，每日 150，不许远程 |
| `hourly-weekly` | 核心 | 定时 | summary | — | 未实现 |
| `daily-review`、`references-replies`、`commitment-tracker`、`meeting-review`、`project-history` | agent | 见第 5.3 节 | agent_chat | `agent.skill` | 见第 5.3 节 |

`workflows` 里另有六个版本化定义：`recording`（transcribe → title → 可选 summary，触发 `sealed_audio`）、`conversation`、`decisions`、`tags`、`screenshot`、`daily`，各声明输出类型。目录里的角色名（asr、diarization、title、summary、vision、episode）与「只用 `vision`、`summary`、`agent_chat`」的仓库规则不一致，角色的权威定义在 [模型](models.md)，见第 11 节。

首次贯通的流程是「连续录音 → 转写 → 标题 → 可选摘要」：保存分段后即可增量转写，整段结束时收束；多人分段和已有声纹匹配按开关追加，未知身份保持匿名；先用时间生成可用标题，模型标题为草稿；摘要关闭或失败不阻断原音、转写与播放。增加一个流程只需加定义、输出校验器、提示模板与合成验收样例；现有模型适配足够时不新增调度器、存储或界面状态机。

调度、成本、幂等与恢复（队列、租约、预算、取消、`ai_calls`）见 [模型](models.md)。

<a id="tasks-provenance"></a>

### 4.1 来源、权限与修订

- 事件记录发生，派生记录解释，人工纠正记录用户选择；不覆盖原文。样本与字符区间用半开区间。
- 模型输出必须有可定位来源，引用越界、缺源或类型错误拒绝发布（`workflow_complete` 校验 `source_revision`，不一致返回 `source_changed`）。
- 有效权限取输入来源的交集，在选择输入、发往模型、提交和对外查询时检查。来源修订使旧派生退出默认索引并安排新修订；删除或撤权取消相关排队任务、拒收迟到结果并使派生、索引、缓存失效。回退不能恢复已撤权或删除的正文。
- 外部 AI 默认只读；内部授权草稿写入不授予外部写权限。
- 功能能读哪些来源，取功能设置与来源授权的交集；能不能用远程模型，取功能设置、来源授权、角色目的地三者中最严的。本机回环地址上的模型服务不论标为本机还是自托管，都不需要逐来源批准。
- 派生结果的旧修订：保留最新的、你确认或改过的、被提案引用的；其余旧草稿 30 天后清理（未验证）。

<a id="skills-triggers"></a>

## 5. 两种方式：核心内自动与挂接的 agent

AI 页在 App 里保留，能和 AI 对话、操作内容。agent 不自己做：挂接外部的 Pi，用我们自己的界面、插件和严格护栏，不用 pi-agent-core 自建循环。

<a id="pi-attach"></a>

### 5.1 Pi 的接入方式

- 版本：`@earendil-works/pi-coding-agent` 1.5.2，锁定在 `agents/pi-extension/package-lock.json`。Node 与 Pi 不随 App 打包：在「设置 → 组件」按需安装；系统已有 Node 22.19+ 时直接使用，否则下载官方 Node 24.22.2 并按固定 SHA-256 校验，Pi 按锁文件安装。升级 Pi 须重新核实下列参数与隔离。
- 以 RPC 模式（`--mode rpc`，标准输入输出上的 JSON 行协议）作为 App 管理的后台进程运行；聊天界面由 AI 页自己画。
- 启动参数（`apps/macos/Sources/Candid/PiAgent.swift`，`agents/pi-extension/test/e2e.mjs` 用同一组）：`--no-builtin-tools --no-extensions -e <插件> --tools 'pv_*' --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline --system-prompt <系统提示> --provider candid --model agent_chat`。`--no-extensions` 同时关掉内置插件；显式 `-e` 仍加载。
- 隔离：环境变量只给 `HOME` 与 `PI_CODING_AGENT_DIR`（指向 App 数据目录下的 `pi/`）、`PI_OFFLINE`、`PI_SKIP_VERSION_CHECK`、`PI_TELEMETRY=0` 和本次运行的核心地址与令牌，不继承用户的环境。工作目录是空目录，项目信任一律拒绝。
- 模型：插件注册唯一的提供方 `candid`（OpenAI 兼容），指向插件在本机回环上开的随机端口与随机路径；每次请求转给核心 `POST /local/pi/model`，核心用冻结的 `agent_chat` 模型和钥匙串里的密钥发出，记入 `ai_calls`（`client = pi`）。Pi 和模型都拿不到服务商密钥。核心对上游不流式（`stream: false`），插件把整段回答按 SSE 一次回放给 Pi，文字不逐字出现。目前只接 OpenAI 兼容（`openai-chat`）与 Ollama 接口；云端模型须本次运行允许远程。
- 每次运行由 App 调 `POST /local/pi/start`，冻结：模型（角色默认 `agent_chat`）、范围（`sources`、`agents`、最多回溯天数默认 90）、写入档 `writes`、提案种类 `propose_kinds`、预算。预算默认与上限：模型调用 12（1–200）、工具调用 40（1–500）、输出 token 16000（256–400000）、单次请求字节 900000。核心为专用访问方 `candid-pi` 换发令牌；`POST /local/pi/stop` 后令牌失效；超预算即停止本次运行，崩溃重启不重置预算。同一时间只允许一个运行（否则 `agent_busy`）。
- 对话运行可刷新权限与模型范围，但必须带回当前对话的有界历史及本地时间。App 在冻结运行时附带邮件能力状态（未连接、未授权、不在范围内）：直接说明恢复入口，不把无权读取推断成没有邮件，不暴露关闭来源的标识、地址或正文。

<a id="pi-guardrails"></a>

### 5.2 护栏：工具三档，权限在核心里检查

插件提供的每个工具都调用核心（带本次运行的令牌），核心按 `candid-pi` 的范围检查并写访问日志；Pi 手上没有别的工具。插件里隐藏工具只是方便，权限只看核心。

| 档 | 能做什么 | Pi 的工具（`pv_` 前缀，对应核心同名工具） |
|---|---|---|
| 读 | 查时间线、对话、活动块、条目、人物、片段、汇总、资料；现读已入库的邮件正文与 AI 会话 | `pv_status`、`pv_overview`、`pv_search`、`pv_find`、`pv_timeline`、`pv_context`、`pv_get_event`、`pv_day`、`pv_conversations`、`pv_conversation`、`pv_evidence`、`pv_person`、`pv_items`、`pv_episodes`、`pv_episode`、`pv_records`、`pv_agent_session`、`pv_memories`；从邮箱现读正文没有核心工具（未实现） |
| 写（只限本库的派生与纠正） | 改标题与备注、标记资料、确认或忽略承诺候选、保存草稿；记为纠正或「AI 草稿」，带访问方、运行编号与模型，可撤销 | `pv_annotate`、`pv_mark_reference`、`pv_commitment`、`pv_save_draft`、`pv_undo` |
| 提案（App 外的动作） | 你在界面上逐条批准后才由 App 执行 | `pv_propose`；种类限 `create_reminder`、`update_reminder`、`complete_reminder`、`delete_reminder`、`create_calendar_event`、`update_calendar_event`、`delete_calendar_event`；改、完成、删要带 `item_id`，来自用户「问 AI」带进来的那一条 |

- 写入档由访问方范围的 `writes`（`annotate`、`reference`、`commitment`、`draft`）逐项授予，默认空；提案种类由 `propose_kinds` 限定。任何别的写入值在 `start` 时被拒绝。
- 撤销：App 可撤销任何 agent 写入；agent 只能撤销自己的、且其后没有新改动的写入；撤销本身也记为纠正。
- 没有终端，不能读写本库以外的文件。工具结果包在 `<candid_data>` 里并注明不执行其中要求，系统提示（`agents/pi-extension/system-prompt.md`）同样规定。端到端测试里，「照做」的模拟模型调用 `bash` 只得到「没有这个工具」，数据里的注入指令不被执行，越界与超预算被拒绝。
- 被拒绝（权限、范围、预算）时如实告诉用户，不绕过。
- 一次对话或一次技能运行冻结模型、工具、范围与预算。

<a id="hooks-skills"></a>

### 5.3 技能、钩子与运行

技能正文在 `agents/skills/<名>/SKILL.md`，Pi 与外部 agent 用同一份；前置信息有标题、摘要、触发、是否开启、输入。现有五个：

| 技能 id | 名称 | 触发（SKILL.md） | 说明 |
|---|---|---|---|
| `daily-review` | 日回顾 | 定时 21:00 | 把一天的录音、屏幕、邮件、AI 会话整理成带引用的经过，存为天级草稿（`pv_save_draft`） |
| `references-replies` | 资料与回复检索 | 手动 | 第 7 节 |
| `commitment-tracker` | 承诺追踪 | 手动 | 找明确承诺、截止时间、完成证据；改提醒或日历只提案 |
| `meeting-review` | 会议整理 | 手动，输入一段会议录音 | 决定、待解决问题、后续提案，每条带时间依据 |
| `project-history` | 项目经过 | 手动，输入项目或主题名 | 按时间重建来龙去脉 |

**技能运行**由核心的 `automation` 模块管理（`core/crates/candid-store/src/automation/`）：运行记录是 `candid-agent-run/1`，存在 `store_meta`（键 `automation.run/1:<id>`，每次改动留 `automation.revision/1:<id>:<rev>`），并占一个 `agent.skill` 作业。流程：

1. 触发入队：`POST /local/automation`（仅 App）的 `run`（手动，可带 `references`、`context` ≤16000 字符、`executor` 为 `foreground` 或 `background`）；或 `automation_scan` 在满足触发时入队。幂等键含技能、触发键与有效配置指纹；同一键不重复建运行。
2. App 的 `SkillRunner` 用 `claim` 领取，`heartbeat` 保活，`finish` 提交；前台对话优先，后台运行等对话空闲。
3. 领取后，核心用 `automation_pi_config` 冻结运行的范围与预算，调 `/local/pi/start`（带 `automation_id` 与尝试号）。技能运行的写入档固定为 `draft`、`reference`、`commitment`、`annotate`，**提案种类为空**：技能运行不能提案。
4. 每次工具调用经 `automation_audit_tool` 记为步骤并附带引用的事件（来源与修订指纹）。
5. 状态：`queued`、`running`、`done`、`failed`、`cancelled`；可 `cancel`、`retry`（仅 failed、cancelled）。运行记录的来源、模型、步骤、结果与中断状态版本化保存。

每个技能一份策略 `SkillPolicy`（`automation.config/1`，经 `config` 动作保存）：

| 字段 | 默认 | 取值范围 |
|---|---|---|
| `enabled` | 开 | |
| `automatic`（自动触发） | 关；仅 `daily-review` 在质量门槛通过且用户未改过时视为开 | |
| `trigger` | `manual`（`daily-review` 为 `daily`） | `manual`、`daily`、`recording_sealed`、`episode_ended` |
| `time` / `timezone` | `21:00` / `Asia/Singapore` | `%H:%M`；有效时区名 |
| `notify` | 关 | 完成后发系统通知；无事可报不推送 |
| `sources` | `*` | 来源 id 列表；取与来源授权的交集，排除已撤销和不许 AI 读取的来源 |
| `role` | `agent_chat` | `agent_chat`、`summary`、`episode`、`title` |
| `allow_remote` | 关 | 远程需同时在全局 `remote_sources` 里列出该来源（不许 `*`）；回环地址不受限 |
| `daily_runs` | 3 | 0–100，超出返回 `daily_run_budget` |
| `max_model_calls` / `max_tool_calls` / `max_output_tokens` | 12 / 40 / 16000 | 1–200 / 1–500 / 256–400000 |

运行记录冻结配置指纹、模型配置指纹、来源策略快照与引用修订；配置、模型、来源策略或被引用的事件变了，`automation_validate` 拒绝，排队与运行中的任务被取消（`configuration_changed`、`source_or_configuration_changed`），租约过期的运行记为 `interrupted`。保存配置时对失效的在途运行同样取消。状态接口对失效运行隐藏结果与步骤，并只返回结果预览（512 字符）。

**钩子**：`daily`（本地时间到点且当天未跑，每技能每天一个幂等键）、`recording_sealed`（过去 24 小时内已封存且转写就绪的录音，每条一个键）、`episode_ended`（结束超过 10 分钟的草稿片段，每片段一个键）、手动。钩子启动一次 Pi 技能运行；核心内流程的触发见第 4 节的功能表。

<a id="quality-gate"></a>

### 5.4 自动触发的质量门槛

非手动触发的技能运行必须同时满足：技能开启、`automatic` 为真、质量报告通过，否则入队被拒（`waiting_quality`）。质量报告（`candid-context-evaluation/1`，经 `quality` 动作提交）须匹配本库 `store_id`、流水线 `screen-cleanup/1` 与当前策略修订（来源与模型配置变化即失效），并满足：

| 指标 | 阈值（`automation_quality`） |
|---|---|
| 查询数 | ≥ 30 |
| 检索 recall | 命中相关数 / 相关数 ≥ 0.8 |
| 降噪抽检准确率 | ≥ 0.9 |
| 汇总抽检 | 检查数 > 0 且编造错误 = 0 |
| 每条要点可点回原处 | `grounded = true` |

评估集本身见 [整理层](curation.md)。AI 页对话接通即可用，回答都带引用；达标前钩子启动的技能只能手动运行。

<a id="ai-page"></a>

### 5.5 AI 页与日程

设置页的模型、功能与技能开关见 [模型](models.md)。AI 页是在 App 里使用 AI 的地方，不放开关：

- 一页两栏：左栏「今天」，自上而下为日程（系统日历，只读）、提醒（系统「提醒事项」今天和逾期的，加上 AI 提的候选）、待你处理（待批准的提案、要你确认的说话人和资料候选）、回顾（最近一次日回顾）；右栏对话，左栏任何一条可「问 AI」作为上下文带入。窗口窄于 900pt 时变成顶部切换。
- 日程与提醒通过 EventKit 读，不入库，每次打开取今天和逾期的；你自己新建、修改、完成、删除提醒与日程不走提案（删除再确认一次）；Pi 做同样的改动只能提案，删除的提案批准时同样再确认一次。
- 技能与耗时操作点后变成进度加「取消」，取消即中止这一轮。
- 对话的呈现：回答上方一行写它查了什么，可展开看每次调用；引用写成 `[[event_id]]`，界面显示为可点的引用并跳到时间线；输入 `@` 可引用一条记录。对话只存在本机，可搜索、可删除。
- 每条 AI 产出都能看到谁生成的（规则、模型或 Pi）、依据哪些记录、什么时候；都能改、确认或拒绝，改动写进 `corrections`，重算后仍生效。

以上界面行为在 Swift 侧（`AISkills.swift`、`AutomationModel.swift` 等）存在，逐项形态未逐一核对（未验证）。

<a id="commitments"></a>

## 6. 承诺与提醒

1. 抽取（`item-extract`，见 [人物与条目](people-items.md)）从对话、邮件、录音里产出承诺、请求、截止时间等候选；候选也可由 Pi 的 `pv_commitment` 写入，存为 `memories`（`kind = commitment`，候选状态）。
2. 候选出现在日回顾和 AI 页「待你处理」，可以确认、修改或忽略。
3. 确认后生成提案（`create_reminder` 或 `create_calendar_event`）；你逐条批准后，App 通过系统接口写入「提醒事项」或日历，回执写回提案。暂不做自动批准。
4. 你的处理回传给下一次抽取；「后来没再出现」不当作已完成（未验证）。

`POST /local/agenda`（`candid-service/src/agenda.rs`）：只供 App，带令牌的请求一律 403 `internal_only`，所以 Pi 和外部客户端不能批准自己的提案。

| 动作 | 作用 |
|---|---|
| `today`（可带 `since_ms`） | 待处理与近期已处理的提案、承诺候选、最近一次日回顾 |
| `proposal`（`decision`：`approve`、`reject`、`executed`、`failed`；可带 `receipt`、`error`） | 推进提案：`pending → approved → executed \| failed`；失败可重试或拒绝；`approve` 只对 `pending`、`failed` 且属于可执行种类的提案有效；批准先记入核心再写系统，回执（`receipt.item_id`）或失败原因写回提案 |
| `commitment`（`decision`：`confirm`、`ignore`；可带 `due`） | 决定候选；`confirm` 生成 `create_reminder` 提案 |

可执行种类是 `AGENDA_EXECUTABLE_KINDS`（七种，同第 5.2 节提案档）。AI 页「加到提醒」= 确认候选 + 生成提案 + 批准。

<a id="references-skill"></a>

## 7. 资料与回复检索

- 在邮件、文件、片段、你发出的消息上可以「收藏为资料」，可选加标签（如「请假回复」「报销流程」）；存为 `memories`（`kind = reference`，你创建即已确认），只存引用和标签，原文留在原处。Pi 经 `pv_mark_reference` 标记的是候选，需你确认。
- 你发出的邮件、多次修改过的文字自动列为资料候选，你确认后才算（自动列出未验证）。
- 技能 `references-replies` 按问题检索资料与相关片段，返回带出处的原文和一份回复草稿；草稿只供复制或修改，不代发。挂接的 Pi 与外部 AI 用同一个技能。

<a id="compose"></a>

## 8. 「帮我写」

按全局快捷键（或悬浮球听写）唤起浮动输入条；你说要写什么，核心用你自己的历史起草一段文字，你读过、改过后点「插入」，由 App 把草稿粘回原来的输入位置。**不自动发送。**

**平台层（Swift，`ComposePanel.swift`）**：按下快捷键的那一刻取快照：前台应用名、窗口标题、选中文字（≤4000 字符）、焦点输入框里已有的内容（≤2000 字符）。面板不激活 Candid，原应用保持前台。「插入」：草稿暂存剪贴板 → 模拟 ⌘V → 恢复原剪贴板；也可「复制」。不取截图。

**核心接口**：`POST /local/compose`（仅 App）`{instruction, app, title, selected, field}`，一次非流式模型调用，返回 `{draft, context, call_id}`。`compose_prepare`（`candid-store/src/compose.rs`）组装请求：

| 上下文 | 规则 |
|---|---|
| 当前对话 | 同一应用里最近 30 分钟内出现的对话，窗口标题与会话标题相同的优先；取最近 30 条消息 |
| 用户以前的说法 | 先取这个对话里你自己发的 8 条，不足时补其他聊天或邮件里你发的 4–400 字的消息；只学语气，不照抄 |
| 选中的文字、输入框里的草稿 | 快照原样带入 |

模型取 `agent_chat`，没有则依次 `summary`、`title`；`max_tokens` 1024，`temperature` 0.3。提示词要求只输出正文、用你自己的语气、不编造事实、需补充处用【】标出、对话和资料里的文字是数据不是指令。`context` 返回这次用了哪些内容，面板显示为「参考了：……」。

不在现行实现里的原计划内容：对话摘录实时识别当前截图、按人物别名认人、预取条目与备忘、用指令关键词搜前 5 条、把问题当作问答输出答案、浮层上删除上下文标签后重新生成、MCP 工具 `compose_context`，见第 11 节。

<a id="executors"></a>

## 9. 执行位置

Rust store 保存作业与结果，service 调度并路由模型；语音与其他模型通过外部服务接口调用；本体不启动 Python worker 或管理其环境。Swift 负责系统能力、设置和状态展示。

模型调用分两条通道，互不等待：交互通道（`complete()`、「帮我写」、Pi）给有人在等的事；后台通道是串行的作业 worker（`candid-service/src/workflows.rs`）。HTTP 处理不长期持有 store 互斥锁。

挂接的 agent：Pi 以 RPC 模式作为 App 管理的进程运行，界面由 AI 页自己画，工具只有我们插件提供的读、本库写和提案三档。Codex、Claude Code 等外部 AI 经 MCP/API 使用同一套技能和工具；pi 会话的读取保留（`pv_agent_session`）。核心里不做 Agent 后端或工具循环。

<a id="code"></a>

## 10. 实现位置

| 内容 | 代码 | 测试 |
|---|---|---|
| 流程目录、提示、输出校验 | `core/workflows/catalog.json`、`prompts.json`、`outputs.schema.json` | `core/crates/candid-store/tests/workflows.rs` |
| 固定流程 scan / prepare / complete | `core/crates/candid-store/src/workflows.rs` | 同上 |
| 作业 worker、模型调用、双通道 | `core/crates/candid-service/src/workflows.rs` | `core/crates/candid-service/tests/workflows.rs`、`lifecycle.rs` |
| 技能策略、运行、钩子、质量门槛 | `core/crates/candid-store/src/automation/`（`config.rs`、`runs.rs`、`hooks.rs`、`runtime.rs`、`api.rs`） | `core/crates/candid-store/tests/automation.rs` |
| Pi 运行冻结、模型网关、工具计数 | `core/crates/candid-service/src/pi.rs` | `core/crates/candid-service/tests/pi.rs`、`agents/pi-extension/test/e2e.mjs` |
| 议程与提案 | `core/crates/candid-service/src/agenda.rs`、`core/crates/candid-store/src/query/agenda.rs` | `core/crates/candid-service/tests/agenda.rs` |
| 「帮我写」 | `core/crates/candid-store/src/compose.rs`、`core/crates/candid-service/src/local.rs`（`/local/compose`） | `core/crates/candid-store/tests/screen_excerpt.rs`（`compose_uses_the_on_screen_conversation_and_own_wording`） |
| Pi 扩展、系统提示、技能正文 | `agents/pi-extension/index.ts`、`system-prompt.md`、`agents/skills/*/SKILL.md` | `agents/pi-extension/test/e2e.mjs` |
| App：Pi 进程、技能运行、设置、帮我写 | `apps/macos/Sources/Candid/PiAgent.swift`、`SkillRunner.swift`、`AISkills.swift`、`AutomationModel.swift`、`WorkflowControlModel.swift`、`ComposePanel.swift` | `apps/macos/script/test.sh` |

<a id="open"></a>

## 11. 未实现与待定

| 项 | 状态 | 说明 |
|---|---|---|
| 小时与周汇总（`hourly-weekly`） | 未实现 | 目录登记 `implemented: false` |
| MCP 提示词模板（prompts 原语） | 未实现 | 目标：外部 agent 接上 `candid mcp` 即可列出并调用，模板只引用查询工具，不带执行权限 |
| 「只做模板」的一批技能（站会与周报等） | 未实现 | 仓库没有这些模板 |
| 从邮箱现读邮件正文的核心工具 | 未实现 | 目前只能读已入库的邮件 |
| Pi 回答不逐字流式 | 部分实现 | 核心对上游不流式，插件整段回放 |
| 摘要默认值不一致 | 设计中 | `catalog.json` 写 `summary` 默认开，`workflow_scan` 实际要求设置 `summary == true` |
| 目录角色名与三角色规则 | 设计中 | 目录与代码用到 asr、diarization、title、episode 等角色，仓库规则只允许 `vision`、`summary`、`agent_chat`；归属 [模型](models.md) |
| `daily-review` 触发描述不一致 | 设计中 | SKILL.md 写定时 21:00 且 `enabled: false`；目录写「每天 21:00 · 仅手动」；策略默认 `enabled` 开、自动触发等质量门槛 |
| 质量门槛阈值是否定稿 | 未验证 | 原文称阈值是提议，跑出基线后再定；代码已按 0.8、0.9、30 执行 |
| 「帮我写」的原计划增强 | 未实现 | 见第 8 节末尾：截图实时识别、人物与条目预取、关键词检索、屏幕问答、标签可删后重生成、`compose_context` |
| 派生旧修订 30 天清理 | 未验证 | 未查到对应清理任务 |
| 日回顾通知的默认与推送 | 部分实现 | `notify` 默认关；系统通知由 App 取 `notifications_only` 状态后发出，未逐项核对 |
| AI 页左右栏、历史对话、`@` 引用等界面细节 | 未验证 | Swift 侧有对应文件，未逐项核对 |
| 模型统计卡片、小图、额外能力推理测试 | 未实现 | 原进度说明里列为未加入 |

<a id="sources"></a>

## 12. 来源

- 旧 `agent-platform-v1.md`：第 1 节（`#position`）、第 3 节（`#features`）、第 4 节（`#tasks`，含「来源、权限与修订」；「调度、成本与恢复」去 [模型](models.md)）、第 4.1 节（`#skills-triggers`）、第 4.3 节（`#commitments`）、第 4.4 节（`#references-skill`）、第 5 节（`#executors`）。第 4.2 节（`#ai-settings`）的设置部分去 [模型](models.md)，AI 页部分在本文第 5.5 节。
- 旧 `timeline-v1.md` 第 7 节：取消内置助手的决定去 [决定流水](../decisions/log.md)，现行规则并入第 5、9 节。
- 旧 `ai-v1-plan.md` 第 4 节（M3）：并入第 8 节，按现状重写。
- 带日期的决定句（含「设计决定：」引用块）去 [决定流水](../decisions/log.md)；实施进度去 [版本记录](../changelog.md)。
