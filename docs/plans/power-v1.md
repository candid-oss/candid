# 功耗 v1：把「何时花电」做成核心的一条规则

> 设计稿，2026-11-26。起因：录音转写完成后功耗仍然很高，排查见本文 §1。目标是把功耗管理做成核心功能，而不是散落各处的开关：一处决定、三档可选、不重复算、不空转、每一笔有账。接在 [数据层规划 v1](data-layer-v1.md) 与 [数据层对照实验 v1](data-layer-lab.md) 之后，不改变它们的结论。

## 0. 一句话原则

**用户没在等的计算都是后台工作；后台工作只在允许花电的时候跑、同一份输入只跑一遍、没有事件不醒，而且每一笔都记账。**

用户看到的只有两件事：**做什么**（功能开关，已有）和**何时做**（一个三档开关，新增）。其余全部是实现。

## 1. 现状：电花在哪里

2026-11-25 一次短录音的后续作业（`run/core.log`）：

| 顺序 | 作业 | 实测 |
|---|---|---|
| 1 | `speech.recognize`，第 1 个 30 秒分段 | 29.7 s |
| 2 | `speech.recognize`，第 2 个分段 | 5.2 s |
| 3 | `workflow.speakers`，整条录音说话人整理 | 101 s |

界面在第 2 步就显示「转写完成」，电却在第 3 步之后才停。根因不是某个 bug，而是结构：

1. **同一段音频过了两遍重模型。** 默认 conversation 模式下每个 30 秒分段已经用 MOSS 分离过说话人；录音一停，`workflow.speakers` 又把整条录音按 10 分钟窗口重跑 MOSS 加声纹。
2. **每个分段一次冷启动。** 语音服务对每个作业新开 Python 子进程重新加载模型（`voice_job_worker.py`）。30 秒音频用 29.7 秒，大半是加载。
3. **到处轮询。** 核心每 250 ms 问一次语音作业状态；语音服务每 100 ms 看一次子进程；核心后台线程每 2 秒醒来扫一遍；同步线程每 30 秒扫 agent 目录并排序；App 的几个视图每 2 到 5 秒刷新一次。
4. **电源规则只有截图一路有。** `DesktopRecognitionPowerGate` 只管屏幕 OCR 与视觉模型；录音后处理、标题、摘要、压缩、语音服务完全不受电源约束。语音服务的 launchd 配置没有任何优先级声明，推理跑在默认 QoS。
5. **没有账。** 作业只记墙钟时间，不记 CPU 时间；没有任何地方能回答「今天后台花了多少」。

另外一个常态项：空闲时核心进程仍有百分之十几的 CPU，来自 App 每次数据变动就重算 `app_stats`、后台线程每次数据变动就跑全套扫描。这不属于本文范围，记在 §7。

## 2. 用户看到的

### 2.1 一个开关：后台处理

放在 AI 功能设置页顶部，三档：

| 档 | 含义 | 规则 |
|---|---|---|
| 随时 | 数据一到就处理 | 仍受过热保护；系统「低电量模式」打开时按「省电」执行 |
| **省电（默认）** | 用电池时不跑本机大模型 | 本机重模型作业排队，接电源后自动继续；远端模型调用和纯整理照常 |
| 手动 | 不自动处理 | 自动作业照常登记但一律等待（`waiting_manual`），在条目上点「现在处理」才跑；这样等待、展示、手动触发只有一套路径 |

没有电量百分比、没有「持续插电几分钟」、没有批次间隔与数量、没有每个功能各自的电源选项。这些现在在截图识别里都有，全部删除并入这一个开关（§5）。需要更细的控制时，先证明三档不够。

### 2.2 等待原因与「现在处理」

任何在等的条目沿用已有的原因展示（`waiting_*` 一族），新增一个原因 `waiting_power`，界面文字「等待接入电源」。每个等待中的条目旁有「现在处理」：点一次只跑这一个，走交互车道，跳过电源闸门但不跳过授权与每日上限。

### 2.3 一行账

设置页同一张卡片底部一行：「今天后台计算 X 分钟，等待 N 项」。数据来自 §3.6 的记账查询，不另算。

## 3. 架构

### 3.1 电源状态：平台层报，核心存

- App 读 IOKit 与 `ProcessInfo`（已有 `DesktopRecognitionPowerSource`），在变化时和每 5 分钟向核心报一次 `power.report`：`{external_power, battery_percent, low_power_mode, thermal, idle_seconds}`。核心存一条当前状态（`store_meta`），带时间戳。
- 超过 10 分钟没有更新视为「未知」；未知按用电池处理（保守）。命令行单独 `candid serve` 时可用 `--power ac|battery` 声明，默认未知。
- 时间线里已有的 `system.state` 电源事件继续作为证据存在，闸门不读它们：证据层和控制状态分开。

### 3.2 一处决定：`feature_check` 加 `waiting_power`

`candid-store/src/feature_settings.rs::feature_check` 已经是每个作业开跑前唯一的许可点（功能开关、来源、是否允许远端、每日上限）。电源闸门加在这里，是第五个条件，不另起一套：

- 作业的**成本类别**由两件事决定：是否调模型、模型是否本机（`feature_check` 已经在判断 provider 是否 loopback）。
  - **重**：调本机模型（截图摘录、活动块、本机转写与说话人、本机的人物判断与条目抽取）。
  - **轻**：调远端模型，或不调模型（整理层投影、音频压缩）。
  - **交互**：用户在等（对话、手动「现在处理」、Pi）。走已有的 `Lane::Interactive`，从不排队。
- 闸门规则就是 §2.1 那张表。省电且用电池：重作业返回 `waiting_power`；过热（thermal serious 或 critical）：重作业一律等待，任何档都一样。
- `pending_job_queue` 在省电且用电池时直接不取重作业，避免每轮逐个 prepare 再被拒：扫描本身也要省。
- 「现在处理」= `enqueue` 带 `manual: true`，`feature_check` 对它跳过电源条件。

`job_priority.rs` 那张表继续是唯一的作业登记处；新增一列标明「不调模型的纯 CPU 作业」，缺登记的 kind 测试直接失败。

### 3.3 同一份输入只跑一遍：录音链路合并

现在：分段 `speech.recognize`（conversation 模式下已跑 MOSS）+ 整条 `workflow.speakers`（再跑 MOSS）。

改为：录音停止后**一个作业**，复用已有的窗口机制（10 分钟窗口、重叠 1 分钟、窗口级断点，见 `voiceprints.rs::recording_windows`），一次 MOSS 得到转写、说话人和声纹，再 stitch。标题、摘要作为同一条链的后续步骤，按各自成本分类排队。删除分段 conversation 路径与独立的 speakers 作业。

quick 模式（只要文字）仍用小模型按分段，成本仍算「重」（本机）。录音进行中如果要实时字幕，只允许走 quick 小模型；停止后的一个重作业给最终版。

音频压缩不再由每分钟一次的定时器扫描，而是作业链结束时顺手触发；它不调模型，归「轻」。

### 3.4 模型进程的生命周期

- 语音服务改为常驻一个进程、模型第一次用到时加载、**空闲 10 分钟卸载**并释放显存；不再每个作业开子进程。取消改为窗口粒度（已有窗口 checkpoint），不再需要杀进程。
- launchd 配置加 `ProcessType: Background` 与 `LowPriorityBackgroundIO`。交互请求不临时提升优先级，接受慢一点，换取规则只有一条。
- 核心后台线程已是 `QOS_CLASS_BACKGROUND`，保持；App 侧录音后处理用 `.background`，采集用 `.utility`。
- LM Studio 等外部本机模型的驻留核心管不了；省电档下不调用它即可，调用次数记在账里。

### 3.5 没有事件不醒

| 现在 | 改为 |
|---|---|
| 后台线程每 2 秒醒来 | 条件变量：入库、作业变化、电源变化、设置变化时唤醒；兜底 10 分钟 |
| 核心每 250 ms 轮询语音作业 | 语音服务提供长轮询 `GET /jobs/{id}?wait=30`，一次请求等到有进展或超时 |
| 语音服务每 100 ms 看子进程 | 子进程不存在了；同进程内直接等待 |
| 同步线程每 30 秒扫 agent 目录 | App 用 FSEvents 监听目录变化后触发同步；兜底 10 分钟 |
| App 视图每 2 到 5 秒刷新 | 只订阅已有的 change feed |
| 压缩每分钟扫一遍 | 作业链结束时触发 |

规则：仓库里不再新增固定周期循环。用测试守住（§3.7），不靠人盯。

### 3.6 记账

- `jobs` 表新增 `cpu_ms`、`wall_ms`、`power_state`（跑的时候是电池还是外接）。Rust 用线程 CPU 时钟；语音服务在结果里带自己的 `cpu_seconds`。
- 新查询工具 `energy.summary`（MCP 与 CLI 契约）：按天、按作业种类给出次数、CPU 秒、墙钟秒、等待中的数量。§2.3 那一行账和 `core.log` 每小时一行摘要都从它来。
- 日志只写计数，不写内容。

### 3.7 测试

- lifecycle 加四个断言：省电加电池时所有重作业处于 `waiting_power`，报告接电源后全部完成；手动档不入队；「现在处理」跳过闸门但不跳过上限；每类作业 CPU 毫秒不超过基线（基线在 fixture 里，超过即失败）。
- **空转测试**：虚拟时钟前进 1 小时、没有任何输入，后台线程唤醒次数不超过 7 次。这是对 §3.5 的硬约束。
- 契约快照：`energy.summary` 进 `core/fixtures/contract/`。
- 语音服务测试：模型空闲卸载、长轮询返回时机。

## 4. 代码落点

| 位置 | 改动 |
|---|---|
| `core/crates/candid-store/src/feature_settings.rs` | `feature_check` 加电源条件与 `manual` 旁路；读 `power.state` |
| `core/crates/candid-store/src/job_priority.rs` | 表加「纯 CPU」列；缺登记即测试失败 |
| `core/crates/candid-store/src/performance.rs` | `pending_job_queue` 在省电加电池时跳过重作业 |
| `core/crates/candid-store/src/workflows.rs` | 删分段 conversation 入队与独立 speakers 入队，改为一个录音作业；`workflows.settings/1` 加 `pace` 三档 |
| `core/crates/candid-service/src/voiceprints.rs`、`voice_jobs.rs` | 录音作业统一走窗口；长轮询 |
| `core/crates/candid-service/src/serve.rs`、`workflows.rs` | 后台线程改条件变量；记 `cpu_ms`；每小时摘要 |
| `core/crates/candid-service/src/audio_compaction.rs` | 定时器改为链尾触发 |
| `core/crates/candid-store/src/query/` | `energy.summary` |
| `services/voice/asr/` | 去子进程；常驻加空闲卸载；`wait` 参数；结果带 `cpu_seconds`；plist 加优先级 |
| `apps/macos/.../DesktopRecognition*.swift` | 删 `DesktopRecognitionPowerGate` 与策略里的 5 个电源字段、batch 模式；电源状态改为上报核心 |
| `apps/macos/.../AIFunctionSettingsView.swift` | 三档开关、一行账；删截图识别的电源选项 |
| `apps/macos/.../RecordingProcessing.swift` | `waiting_power` 文案、「现在处理」 |
| `apps/macos/Sources/Candid/Views/` | 去掉固定周期刷新，只用 feed |
| `docs/specs/agent-platform-v1*.md`、`AGENTS.md` | 写入原则与「新增后台作业三问」 |

## 5. 删除清单

- 分段 conversation 识别路径与独立 `workflow.speakers` 作业（合并为一个）。
- `voice_job_worker.py` 子进程模式。
- `DesktopRecognitionPolicy` 的 `requireExternalPower`、`minimumBatteryPercent`、`pluggedMinutes`、`pauseInLowPower`、`batchIntervalMinutes`、`batchLimit` 与 batch 模式；`DesktopRecognitionPowerGate`。
- 五处固定周期循环（§3.5）。
- 音频压缩的每分钟定时器。

## 6. 分步，每步一件事

1. **闸门**：电源上报、`power.state`、三档设置、`feature_check` 的 `waiting_power`、`manual` 旁路、App 的开关与文案、删截图识别的电源选项。不改任何作业本身。验证：lifecycle 新断言。
2. **录音链路合并**：一个录音作业；语音服务常驻加空闲卸载；plist。验证：lifecycle 录音部分快照更新，并写明变了什么。
3. **零轮询**：条件变量、长轮询、FSEvents、视图只订阅 feed、压缩链尾触发。验证：空转测试。
4. **记账**：`cpu_ms`、`energy.summary`、每小时摘要、一行账、契约快照。

每步都要跑 `cargo test --workspace`、`cargo test -p candid-service --test lifecycle`；第 1、4 步还要跑契约测试；涉及 `apps/macos/` 的步骤跑 `swift build` 与 `script/test.sh`；语音服务跑 pytest。

## 7. 本文不做的

- 不做按电量百分比的阈值、不做时间表（例如夜间处理）、不做每功能各自的电源策略。
- 不读 GPU 功耗（需要管理员权限）；CPU 时间加模型调用次数够用。
- 不管理 LM Studio 等外部进程的加载与卸载。

（原先列在这里的常态 CPU 两项——App 每次变动重算 `app_stats`、后台线程每次变动跑全套扫描——已并入 §9 的工作流 C。）

## 8. 给 coding agent 的规则（拟写入 AGENTS.md）

新增或修改任何自动后台作业，提交说明里必须回答三问：**谁触发**（哪个事件，不允许是定时器）、**何时允许跑**（成本类别：重、轻、交互）、**花多少**（lifecycle 里该作业的 CPU 基线）。同一份输入不过两遍重模型；不新增固定周期循环；后台线程与进程声明低优先级；作业记 CPU 时间而不只记墙钟。

## 9. 实施：三条并行工作流与共享接口

按文件边界切成三条，可以同时做；交汇处只靠下面的接口约定，合并时由协调会话解决冲突。

### 9.1 共享接口（三条都按此实现，不得各自改名）

**成本登记**（`candid-store/src/job_priority.rs`，工作流 A 落地）
- 每个后台作业种类必须在 `JOB_CLASSES` 登记成本：`Cost::Heavy`、`Cost::Light` 或 `Cost::ByModel`（调本机模型算重，调远端算轻，不调模型算轻）。
- 平台层（Swift）的后台工作也登记在同一张表，种类名以 `app.` 开头（如 `app.ocr`、`app.webp`），供平台层查询许可；核心不执行它们。
- 测试：作业种类出现在代码里却没登记，测试失败。以后新增任何作业，加一行即可，闸门、队列顺序、记账自动生效。

**电源状态**（工作流 A 落地，B、C 只读）
- 写：`TimelineStore::power_report(&Value)`，本地通道 `POST /local/power {"action":"report", ...}`；字段 `external_power: bool|null`、`battery_percent: int|null`、`low_power_mode: bool`、`thermal: "nominal"|"fair"|"serious"|"critical"|null`、`idle_seconds: int|null`。核心补 `reported_ms`。存在 `store_meta` 的 `power.state/1`。
- 档位：`store_meta` 的 `power.pace/1`，取值 `anytime`、`saver`、`manual`，缺省 `saver`。`POST /local/power {"action":"set_pace","pace":...}`。
- 读：`TimelineStore::power_status() -> Value`，`POST /local/power {"action":"status"}` 与 `candid power status`：上面的状态加 `pace`、`stale: bool`（超过 10 分钟未更新）、`heavy_allowed: bool`、`reason: null|"waiting_power"|"waiting_thermal"|"waiting_manual"`。
- 许可：`TimelineStore::power_allows(cost: Cost, manual: bool) -> Result<()>`，拒绝时返回上面的 reason。`feature_check` 和平台层查询都只调它，这是全仓库唯一的「能不能花电」判断。
- 状态或档位变化后，核心发布 change feed 事件 `power`，并唤醒后台线程（工作流 C 提供唤醒，A 调用：`Core::wake_background()`；C 合并前 A 可先用 `feed.publish("power")` 加已有的 `trigger.request()`）。

**手动触发**
- 「现在处理」= 对一个等待中的作业设置手动标记后正常排队，`power_allows(_, manual=true)` 放行；不跳过授权、来源、每日上限。

**记账**（工作流 A 落地 schema 与查询；B、C 只往里报数）
- schema 升一版：`jobs` 加 `cpu_ms INTEGER`、`wall_ms INTEGER`、`power_state TEXT`（`ac`、`battery`、`unknown`）、`manual INTEGER NOT NULL DEFAULT 0`。只有工作流 A 改 schema。
- 模型调用之外的 CPU（Rust 自己的线程时间）由后台线程按作业累加；语音服务在每个结果里带 `cpu_seconds`，核心把它加进该作业的 `cpu_ms`（B 负责在语音服务里给出，A 负责读取并累加）。
- 查询：`energy.summary`（MCP 工具与 CLI），按天和作业种类给出次数、CPU 秒、墙钟秒、电池上的 CPU 秒、等待中的数量与原因。

**唤醒**（工作流 C 落地）
- `Core::wake_background()`：唤醒后台作业线程。入库、作业变化、电源与设置变化都调它；兜底间隔 10 分钟。
- 不允许新增固定周期循环；空转测试守住。

### 9.2 工作流

| 工作流 | 范围 | 主要文件 |
|---|---|---|
| **A 闸门、档位与记账** | §3.1、§3.2、§3.6、§2 全部界面；删除截图识别的独立电源策略、batch 模式与 `conserveBattery` 开关（采集节流改为跟随档位）；平台层后台工作改为向核心查询许可；schema 升版；`energy.summary`；AGENTS.md 规则与设计文档 | `candid-store` 的 `feature_settings.rs`、`job_priority.rs`、`performance.rs`、新 `power.rs`、`schema*.rs`、`query/`；`candid-service` 的 `local.rs`（`/local/power`）、`workflows.rs` 的调用与记账部分；CLI；Swift 的设置页、`DesktopRecognition*`、`DesktopSession` 节流、录音处理状态文案 |
| **B 录音链路与语音服务** | §3.3、§3.4：录音停止后一个窗口式作业（quick 与 conversation 两种模式共用）；录音进行中不跑重作业；删分段 conversation 路径与独立 speakers 作业；`workflow_scan` 不再每次列 1000 条媒体；语音服务常驻、按需加载、空闲卸载、去子进程、窗口粒度取消、长轮询、结果带 `cpu_seconds`、launchd 低优先级；音频压缩改为链尾触发 | `candid-store` 的 `workflows.rs`（scan 的录音部分）、`recording_speakers.rs`、`media*.rs`；`candid-service` 的 `voiceprints.rs`、`voice_jobs.rs`、`audio_compaction.rs`；`services/voice/` 全部 |
| **C 空闲零开销** | §3.5：后台线程改为事件唤醒；`Core::wake_background()`；agent 目录改由 App 的 FSEvents 触发同步；App 视图去掉固定周期刷新、只订阅 feed；`app_stats` 等统计按修订号缓存；后台各 scan 在无变化时零查询；空转测试 | `candid-service` 的 `serve.rs`、`local.rs`（唤醒与 feed 部分）、`workflows.rs` 的 `Worker`；`candid-store` 的 `query/app.rs`、各 `*_scan` 的入口短路、`agents/sync.rs`；Swift 的各视图刷新循环与 `CoreProcess`、新增 FSEvents 监听 |

三条都要往 `lifecycle.rs` 加断言、更新各自快照；冲突在合并时统一重生成，并在提交说明里写清楚变了什么。
