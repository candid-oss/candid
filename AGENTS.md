# 给 coding agent 的仓库规则

Candid 是本地优先的 macOS 采集与上下文底座。仓库的主人只做上层设计，不读代码，所以判断有没有改坏只靠测试和这份规则。

## 动手前先读

| 读什么 | 为了知道 |
|---|---|
| [产品定位](docs/product.md) | 这个改动该不该做 |
| [架构总图](docs/architecture.md) | 代码该放哪一块，往哪个接缝扩展 |
| [工程规范](docs/engineering.md) | 设计原则、代码规范、测试秩序、完成的定义 |
| [契约清单](docs/contracts/README.md) | 哪些东西不能悄悄改 |
| [规格索引](docs/specs/README.md) | 你要改的那一块现在的行为和规则 |
| [术语表](docs/glossary.md) | 一个词在这个项目里指什么 |
| [待办](docs/roadmap.md)、[docs/plans/](docs/plans/) 里的当前计划 | 正在进行的事、已知问题、等主人决定的事 |
| [文档规范](docs/standard.md) | 文档放哪、怎么写 |
| 所改目录的 README | 这一块怎么构建、运行、测试 |

本页是摘要。与上面的文档冲突时以它们为准，并顺手修正本页。

## 模块边界

| 部分 | 位置 | 负责 |
|---|---|---|
| Rust 核心 | `core/` | 时间线库、入库、整理层（对话/消息/摘录/活动块）、人物、条目抽取、模型调用两条通道、权限与审计、HTTP/MCP/命令行 |
| Mac App | `apps/macos/` | 只管界面、系统采集（屏幕、应用、录音）、系统权限与平台能力（日历、通知、菜单栏） |
| Agent | `agents/` | 挂接的 Pi 扩展与技能正文；外部 agent 走 MCP |
| 语音服务 | `services/voice/` | 外部服务，核心调接口并校验结果 |

- **逻辑放 Rust 核心。** Swift 不写业务逻辑、不直接读写 SQLite，只经本地通道调用核心（见 [ADR-024](docs/decisions/024-rust-core.md)）。
  判断标准：这段代码换到 Windows 要不要重写一遍？要，就不该在 Swift 里。
- **模型只由核心调用。** 平台端要用模型，请核心代为调用，预算和记账才完整。
- **连接器：读文件、走网络的放核心；调系统接口的放平台层。** 所有写入都经核心的入库接口。
- **扩展走三个接缝：** 新数据来源走入库协议，新消费方走 MCP/CLI，新 AI 行为走技能。不做通用插件系统。
- **CLI 和 MCP 是对外契约。** 工具名、参数、schema、输出字段被外部 agent 依赖；改动要有理由，并更新契约快照（见下）。
- 模型提示词在 `core/prompts/`，固定流程目录在 `core/workflows/`。
- **Swift 里已有的越界逻辑**列在[架构总图 §5](docs/architecture.md#debts)。不要再往那些位置加逻辑；碰到相关改动时优先把它搬进核心。

## 禁止事项

- **不新增模型角色（ai_roles）。** 只用 `vision`（截图摘录）、`summary`（活动块、抽取、人物判断）、`agent_chat`（回复与问答）。质量不够就换模型，不加角色。
- **不自造 agent loop 或 harness。** 对话与技能交给 Pi（`agents/`）；核心只做固定流程（scan → 作业队列 → prepare → 模型 → complete）。
- **不重复实现核心已有的能力。** 先搜 `core/crates/candid-store/src/`（入库、整理、人物、条目、查询工具都在这里），扩展现有模块，不另起一套。
- **界面文字用中文作 key，并补英文。** Swift 里写 `L("中文")`，在 `apps/macos/Resources/Localizable.en.json` 补英文；
  `python3 apps/macos/script/localize_sources.py --check` 会检查。界面规则见 [界面规范](docs/specs/design-system.md)。
- **仓库里不放真实个人数据。** 测试只用明确标记的合成数据；不提交录音、截图、数据库、模型或密钥。
- **不新增遥测或用户没配置的网络请求。** 测试里的模型只能是本机回环上的桩（参考 `core/crates/candid-service/tests/lifecycle.rs`）。
- **不在功能改动里顺手格式化整个文件**（全仓库还没有统一跑过 `cargo fmt`）。
- **不为了让测试通过而放松断言、加跳过或删测试。**

## 后台作业与功耗

功耗是核心问题（[功耗 v1](docs/plans/power-v1.md)）。用户没在等的计算都是后台工作：只在允许花电时跑、同一份输入只跑一遍、没有事件不醒、每一笔记账。

- **新增后台作业只加一行登记。** 每个作业种类（核心作业，以及 App 侧以 `app.` 开头的后台工作）都在
  `core/crates/candid-store/src/job_priority.rs` 的 `JOB_CLASSES` 登记成本（`Heavy` / `Light` / `ByModel`）、执行方和优先级；
  闸门、队列顺序和记账随之生效。代码里出现未登记的种类，`every_job_kind_is_registered` 测试失败。
- **「能不能花电」只问一处。** 核心里经 `feature_check` → `power_allows`，App 里经 `POST /local/power {"action":"allows"}`；
  不另写电量、插电、批次之类的判断。用户只看到一个开关「后台处理」：随时 / 省电（默认）/ 手动。
- **新增或修改任何自动后台作业，提交说明里必须回答三问**：谁触发（哪个事件，不允许是定时器）、何时允许跑（成本类别：重、轻、交互）、
  花多少（lifecycle 里该作业的 CPU 基线）。
- 同一份输入不过两遍重模型；不新增固定周期循环；后台线程与进程声明低优先级；作业记 CPU 时间而不只记墙钟（`jobs.cpu_ms`，`energy_summary` 可查）。
- 测试里的模型桩在本机回环上，算「重」：测试默认声明接电源（`tests/common/mod.rs` 的 `open`、`ServeOptions.power`），测闸门的测试自己改。

## 真实数据

真实资料库（`~/Library/Application Support/Candid/timeline/`）**只能在副本上试**：先复制到临时目录，用
`candid serve --root <副本>` 起一个独立核心。永远不要对用户正在用的资料库或运行目录执行写入、迁移或删除；
测试一律用自己的临时目录和运行目录（`CANDID_RUN_DIR`）。报告真实数据上的结果时只给计数和差异，不贴内容。

## 改完要跑的检查

一条命令，按改动涉及的目录自动选择：

```bash
scripts/check.sh
```

`--list` 只看会跑什么，`--all` 全跑。退出码 0 全过，1 有失败，2 有检查因缺环境没跑成。**没跑成不等于通过**，要在汇报里说明。

它执行的就是下表。Rust 命令在 `core/` 下执行。

| 改动 | 必跑 |
|---|---|
| 任何 Rust 代码 | `cargo test --workspace` |
| 入库、整理层、截图摘录、活动块、人物、条目、模型调用、提示词 | 上面一条，并单独看全流程测试：`cargo test -p candid-service --test lifecycle` |
| MCP 工具、CLI、查询输出字段 | 上面一条，并看契约测试：`cargo test -p candid-service --test mcp_contract`、`cargo test -p candid-cli --test contract` |
| 表结构 | 升版本，并带升级测试（含失败回滚）；跑 `cargo test --workspace` |
| `apps/macos/` | `swift build --package-path apps/macos`，再跑 `apps/macos/script/test.sh`（样式、本地化、Swift 测试；CI 不跑这一套） |
| `agents/` | `node agents/pi-extension/test/e2e.mjs`（先 `npm ci --prefix agents/pi-extension --ignore-scripts`） |
| `services/voice/` | `.venv-asr-mlx/bin/python -m pytest -q services/voice/tests` |
| 任何 Markdown | `python3 scripts/check_docs.py`（链接、锚点、双语成对、索引、规格长度） |

## 全流程测试与快照

- `core/crates/candid-service/tests/lifecycle.rs`：在固定的三天假数据（`core/fixtures/lifecycle/world.json`）上，
  用真实后台 worker 跑完 入库 → 整理层 → 截图摘录 → 活动块 → 人物 → 条目 → MCP 查询。模型是回环桩，
  按 `core/fixtures/lifecycle/replies.json` 匹配输入返回固定 JSON。一秒以内跑完，超过 30 秒算失败。
- 每次模型调用的输入和回复存在 `core/fixtures/lifecycle/snapshots/model-calls/`，MCP 查询输出存在
  `.../snapshots/mcp/`；工具清单与 schema 存在 `core/fixtures/contract/`。
- 快照不一致时测试失败。**确认是有意的改动**（接口或提示词真的变了）才重新生成，并在提交说明里写清楚变了什么：

```bash
UPDATE_SNAPSHOTS=1 cargo test -p candid-service --test lifecycle --test mcp_contract
```

```bash
UPDATE_SNAPSHOTS=1 cargo test -p candid-cli --test contract
```

- 新增流程或模型调用时，往 fixture 里加能触发它的数据、往 `replies.json` 加回复，并在 `lifecycle.rs` 加断言。
  没有匹配回复的模型请求会让测试失败并列出输入。保持轻量：抽样检查关键结果，不追求覆盖率。

## 文档

完整规则见[文档规范](docs/standard.md)。要点：

- **每种文档只做一件事**：规格写现在的行为，决定写为什么，计划写正在做的事，研究记录写实验和调研，版本记录写已做，待办写要做。不混写。
- **规格只写现在为真的。** 没实现的标「（未实现）」，没核对过的标「（未验证）」，并列进该规格的「未实现与待定」。不确定就标未验证，不要猜成已实现。
- **改代码时同步改文档**：行为变了改规格；对外承诺变了改契约清单和快照；结构变了改架构总图并记一条决定；合并进主分支的改动在[版本记录](docs/changelog.md)的「未发布」加一条；做完的事从[待办](docs/roadmap.md)删掉。
- **和主人讨论出的决定**当天在[决定流水](docs/decisions/log.md)记一行；影响结构、难以反悔的写决策记录（`docs/decisions/`，从 025 起编号）。
- **引用**：文档之间用相对链接加锚点，不用章节号；代码注释引用文档写 `docs/specs/<文件>.md#<锚点>`。旧注释里的 `agent-platform-v1 §2.6`、`data model v1 §9.1` 这类章节号，到同名的搬迁对照页（`docs/specs/*-v1.md`）查新位置。
- **双语**：规格、决策记录、各 README 中英文都要改，中文为准。其余只写中文。
- 规格一份不超过两万字符；`python3 scripts/check_docs.py` 检查链接、锚点、双语成对、索引和长度。

## 工作方式

- 先打通再打磨：先让完整链路能跑，再做抽样检查和优化。
- 一个改动只做一件事；同步更新受影响的 README 或设计文档。
- 发现与当前任务无关的问题，记下来报告，不顺手改。
- 多个会话同时工作时，各自在自己的分支和工作目录里做，不在主目录留未提交的改动；升表结构版本的改动一次只能有一个在进行。
- 汇报给主人时说人话：用户能看到什么变化、怎么验证的、哪项检查没跑成、有什么需要主人决定。不贴代码。

## 技能

`.claude/skills/` 里有本仓库的操作技能（检查改动、写文档、在真实数据副本上试验）。不支持技能的 agent 直接读对应的 `SKILL.md`。
