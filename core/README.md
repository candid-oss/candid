# Candid core

**English** · [中文](#中文)

The Rust core owns the timeline: storage, ingest, queries, permissions and audit, background processing, the local API, the MCP server and the `candid` command. The macOS app runs it as a helper process (`Candid.app/Contents/Helpers/candid serve`) and talks to it over a private Unix socket. Design notes: [timeline spec](../docs/specs/timeline-v1.md), [ADR-024](../docs/decisions/024-rust-core.md).

## Crates

| Crate | Contents |
|---|---|
| `candid-store` | Schema and migrations, ingest (dedup, idempotent fingerprints, blob staging and hash checks), deletion and tombstones, query tools with scope filters and audit, derived records |
| `candid-service` | Loopback HTTP and the app's local channel, routing and auth, background jobs, change feed, MCP (stdio), model gateway, Pi agent runs |
| `candid-connectors` | Capture connectors (Mail.app data, IMAP); they write only through the ingest API |
| `candid-metrics` | Offline daily metrics over synthetic fixtures or a read-only timeline |
| `candid-cli` | The `candid` command, including `serve` |

Dependencies are MIT/Apache-2.0 compatible (plus ISC, BSD, Zlib, Unicode-3.0, CDLA-Permissive-2.0 and unmodified MPL-2.0 `option-ext`). SQLite is bundled.

## Build and test

Requires Rust 1.88+. Run from `core/`:

```bash
cargo build --release -p candid-cli
```

```bash
cargo test --workspace
```

Some service tests use public speech samples under `data/` at the repository root and skip when they are missing; see [services/voice](../services/voice/README.md) to fetch them.

## Run

```bash
candid serve --root <data-root> [--port 0] [--run-dir <dir>] [--socket <path>] [--parent-pid <pid>]
```

```bash
candid overview --start today
```

```bash
candid mcp
```

- `serve` opens (or creates) the data root, listens only on `127.0.0.1`, and writes `endpoint.json` and the CLI token (mode 0600) into the run directory. `--root` is required and never defaults to the app's library.
- As the app's helper it receives `--socket` and `--parent-pid`, and exits when the app goes away. A second core on the same root or socket exits with code 75.
- Run directory: `CANDID_RUN_DIR`, otherwise `~/Library/Application Support/Candid/run` on macOS. `CANDID_TOKEN` and `CANDID_ENDPOINT` override the token and address.
- The CLI and MCP shell never open the database; they call the running service.

## Background work

One background thread (`candid-workflows`) runs the fixed workflows: scans that keep the organized layer current and
queue jobs, then the jobs themselves. It never wakes on a beat. The SQLite connection counts writes per table in memory;
every background step declares what it reads (`candid_store::background::SCANS`, plus the job queue, voice cleanup and
audio compaction in `candid-service/src/background.rs`) and runs only when one of those tables was written or its own
due time came (a retry delay, a lease, a chat going quiet, the next local day). Any write wakes the thread;
`Core::wake_background()` covers changes outside the library (power, model authorization); a 10-minute fallback covers
anything missed. With nothing changed, a wake runs no SQL.

The sync thread mirrors the legacy ledger and Hub, reads mail and lists agent transcript folders on request and at
the latest every 10 minutes (`--sync-interval`). In between, the app watches the consented agent folders with FSEvents
and sends `POST /local/sync {"paths": [...]}`, which reads only those files. Job progress reaches the app as `jobs`
events on the change feed (at most one a second), so app views refresh on events instead of timers. See
[CONTRIBUTING](../CONTRIBUTING.md#background-work-and-timers) for the rule and its check.

## Connecting an AI client

Register a client in the app (or via `POST /local/clients`); it returns a one-time token and ready-made snippets for Claude Code, Codex and generic MCP JSON. Each client has its own scope, every call is written to the access log, and revoking the client invalidates its token. The local endpoints (`/local/…`) are only served on the Unix socket; on loopback HTTP they return 403.

---

## 中文

Rust 核心负责时间线的一切：存储、入库、查询、权限与审计、后台处理、本地接口、MCP 服务和 `candid` 命令。Mac App 把它作为辅助进程运行（`Candid.app/Contents/Helpers/candid serve`），通过私有 Unix 套接字通信。设计说明见[时间线规格](../docs/specs/timeline-v1.md)与 [ADR-024](../docs/decisions/024-rust-core.md)。

| Crate | 内容 |
|---|---|
| `candid-store` | 表结构与迁移、入库（去重、幂等指纹、原件暂存与校验）、删除与墓碑、带范围过滤和审计的查询工具、派生记录 |
| `candid-service` | 回环 HTTP 与 App 本地通道、路由与鉴权、后台作业、变更推送、MCP（stdio）、模型网关、Pi 运行 |
| `candid-connectors` | 采集型连接器（本机「邮件」数据、IMAP），只经入库接口写入 |
| `candid-metrics` | 基于合成样例或只读时间线的离线每日指标 |
| `candid-cli` | `candid` 命令，含 `serve` |

**构建与测试**（需 Rust 1.88+，在 `core/` 下执行）：`cargo build --release -p candid-cli`、`cargo test --workspace`。部分服务测试使用仓库根目录 `data/` 下的公开语音样例，缺失时跳过，获取方式见 [services/voice](../services/voice/README.md)。

**运行**：`candid serve --root <数据根目录>` 只监听 `127.0.0.1`，把 `endpoint.json` 和命令行令牌（0600）写入运行目录；`--root` 必填，不会默认指向 App 的资料库。作为 App 辅助进程时由 App 传入 `--socket` 和 `--parent-pid`，App 退出后核心随之退出；同一资料库已有核心运行时退出码为 75。运行目录默认 `~/Library/Application Support/Candid/run`，可用 `CANDID_RUN_DIR` 覆盖。

**后台工作**：一个后台线程（`candid-workflows`）跑固定流程：先是保持整理层最新、给作业排队的扫描，再是作业本身。它从不按节拍醒来。
SQLite 连接在内存里按表计数写入；每个后台步骤声明自己读哪些表（`candid_store::background::SCANS`，以及 `candid-service/src/background.rs`
里的作业队列、声纹清理和音频压缩），只有这些表被写过或到了它自己的到期时刻（重试延迟、租约、聊天安静下来、本地次日）才跑。任何写入都会唤醒线程；
库外的变化（电源、模型授权）调用 `Core::wake_background()`；10 分钟兜底防止遗漏。没有变化时一次唤醒不执行任何 SQL。
同步线程在被请求时、最迟每 10 分钟（`--sync-interval`）镜像旧账本与 Hub、读邮件、列出 agent 会话目录；其间 App 用 FSEvents 监听已授权的
agent 目录，发 `POST /local/sync {"paths": [...]}`，只读这些文件。作业进度以变更推送里的 `jobs` 事件到达 App（最多每秒一次），App 视图按事件刷新而不是定时。
规则与检查方式见 [CONTRIBUTING](../CONTRIBUTING.md#中文)。

**接入 AI 客户端**：在 App 里登记访问方（或调用 `POST /local/clients`），得到只显示一次的令牌和 Claude Code、Codex、通用 MCP 的配置片段。每个访问方有独立范围，每次调用记入访问日志，撤销后令牌立即失效。`/local/…` 接口只在 Unix 套接字上提供，回环 HTTP 一律返回 403。
