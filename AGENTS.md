# 给 coding agent 的仓库规则

Candid 是本地优先的 macOS 采集与上下文底座。仓库的主人只做上层设计，不读代码，所以判断有没有改坏只靠测试和这份规则。
动手前先读与改动相关的文档：[docs/README.md](docs/README.md)（设计文档索引）、[CONTRIBUTING.md](CONTRIBUTING.md)、
所改目录的 README，以及 [docs/plans/](docs/plans/) 里的当前计划。

## 模块边界

| 部分 | 位置 | 负责 |
|---|---|---|
| Rust 核心 | `core/` | 时间线库、入库、整理层（对话/消息/摘录/活动块）、人物、条目抽取、模型调用两条通道、权限与审计、HTTP/MCP/命令行 |
| Mac App | `apps/macos/` | 只管界面、系统采集（屏幕、应用、录音）、系统权限与平台能力（日历、通知、菜单栏） |
| Agent | `agents/` | 挂接的 Pi 扩展与技能正文；外部 agent 走 MCP |
| 语音服务 | `services/voice/` | 外部服务，核心调接口并校验结果 |

- **逻辑放 Rust 核心。** Swift 不写业务逻辑、不直接读写 SQLite，只经本地通道调用核心（见 [ADR-024](docs/decisions/024-rust-core.md)）。
- **连接器：读文件、走网络的放核心；调系统接口的放平台层。** 所有写入都经核心的入库接口。
- **CLI 和 MCP 是对外契约。** 工具名、参数、schema、输出字段被外部 agent 依赖；改动要有理由，并更新契约快照（见下）。
- 模型提示词在 `core/prompts/`，固定流程目录在 `core/workflows/`。

## 禁止事项

- **不新增模型角色（ai_roles）。** 只用 `vision`（截图摘录）、`summary`（活动块、抽取、人物判断）、`agent_chat`（回复与问答）。质量不够就换模型，不加角色。
- **不自造 agent loop 或 harness。** 对话与技能交给 Pi（`agents/`）；核心只做固定流程（scan → 作业队列 → prepare → 模型 → complete）。
- **不重复实现核心已有的能力。** 先搜 `core/crates/candid-store/src/`（入库、整理、人物、条目、查询工具都在这里），扩展现有模块，不另起一套。
- **界面文字用中文作 key，并补英文。** Swift 里写 `L("中文")`，在 `apps/macos/Resources/Localizable.en.json` 补英文；
  `python3 apps/macos/script/localize_sources.py --check` 会检查。界面规则见 [界面规范](docs/specs/design-system-v1.md)。
- **仓库里不放真实个人数据。** 测试只用明确标记的合成数据；不提交录音、截图、数据库、模型或密钥。
- **不新增遥测或用户没配置的网络请求。** 测试里的模型只能是本机回环上的桩（参考 `core/crates/candid-service/tests/lifecycle.rs`）。

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

## 每类改动要跑的命令

Rust 命令在 `core/` 下执行。

| 改动 | 必跑 |
|---|---|
| 任何 Rust 代码 | `cargo test --workspace` |
| 入库、整理层、截图摘录、活动块、人物、条目、模型调用、提示词 | 上面一条，并单独看全流程测试：`cargo test -p candid-service --test lifecycle` |
| MCP 工具、CLI、查询输出字段 | 上面一条，并看契约测试：`cargo test -p candid-service --test mcp_contract`、`cargo test -p candid-cli --test contract` |
| 表结构 | 升版本，并带升级测试（含失败回滚）；跑 `cargo test --workspace` |
| `apps/macos/` | `swift build --package-path apps/macos`，再跑 `apps/macos/script/test.sh`（样式、本地化、Swift 测试；CI 不跑这一套） |
| `agents/` | `node agents/pi-extension/test/e2e.mjs`（先 `npm ci --prefix agents/pi-extension --ignore-scripts`） |
| `services/voice/` | `.venv-asr-mlx/bin/python -m pytest -q services/voice/tests` |

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

## 工作方式

- 先打通再打磨：先让完整链路能跑，再做抽样检查和优化。
- 一个改动只做一件事；同步更新受影响的 README 或设计文档（中英文两版都要改）。
- 发现与当前任务无关的问题，记下来报告，不顺手改。
