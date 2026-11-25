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
- Background processing follows one power gate ([power v1](../docs/plans/power-v1.md)): `candid power status`, `candid power pace <anytime|saver|manual>` (through the app's local socket), and `candid energy summary` for what background work cost. A core run without the app has unknown power, which counts as battery; declare it with `candid serve --power ac|battery`.

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

**运行**：`candid serve --root <数据根目录>` 只监听 `127.0.0.1`，把 `endpoint.json` 和命令行令牌（0600）写入运行目录；`--root` 必填，不会默认指向 App 的资料库。作为 App 辅助进程时由 App 传入 `--socket` 和 `--parent-pid`，App 退出后核心随之退出；同一资料库已有核心运行时退出码为 75。运行目录默认 `~/Library/Application Support/Candid/run`，可用 `CANDID_RUN_DIR` 覆盖。后台处理只听一个电源闸门（[功耗 v1](../docs/plans/power-v1.md)）：`candid power status`、`candid power pace <anytime|saver|manual>`（经 App 核心的本地通道），`candid energy summary` 查后台计算的账；没有 App 时电源状态未知、按电池处理，可用 `candid serve --power ac|battery` 声明。

**接入 AI 客户端**：在 App 里登记访问方（或调用 `POST /local/clients`），得到只显示一次的令牌和 Claude Code、Codex、通用 MCP 的配置片段。每个访问方有独立范围，每次调用记入访问日志，撤销后令牌立即失效。`/local/…` 接口只在 Unix 套接字上提供，回环 HTTP 一律返回 403。
