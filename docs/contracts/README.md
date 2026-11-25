# 契约清单

契约是别人依赖、不能悄悄改的东西：外部 agent、平台端、旧数据、用户的备份。每条契约有三样：规格在哪、哪个测试守着它、改动时要做什么。
没有测试守着的契约标为「缺口」，文字再多也不算数。

本页是索引，不复述规格正文。新增一条契约时在这里加一行，并补上守护它的测试。

## 清单

| 契约 | 谁依赖 | 规格 | 守护它的测试或快照 | 状态 |
|---|---|---|---|---|
| MCP 工具（22 个：名字、说明、参数 schema） | 外部 agent、Pi | [时间线库 §5](../specs/timeline-v1.md) | `core/crates/candid-service/tests/mcp_contract.rs`；快照 `core/fixtures/contract/mcp-tools.json`、`mcp-initialize.json` | 已守护 |
| MCP 查询的输出字段 | 外部 agent、技能 | 同上 | 全流程测试的 `core/fixtures/lifecycle/snapshots/mcp/` | 已守护（抽样 7 个查询） |
| `candid` 命令行 | 脚本、外部 agent | [core/README](../../core/README.md) | `core/crates/candid-cli/tests/contract.rs`；快照 `core/fixtures/contract/candid-tools.txt` | 已守护 |
| 入库协议（`IngestBatch`） | Mac 采集、连接器，以后的 Windows 与手机端 | [时间线库 §6.1](../specs/timeline-v1.md) | `core/crates/candid-store/tests/ingest.rs`、`merge.rs` | 行为有测试；**协议本身没有 schema 快照** |
| App 与核心的本地通道（`/local/…`） | Mac App，以后的其他平台端 | 无成文规格；Swift 侧类型在 `apps/macos/Sources/CandidClient/` | `core/crates/candid-service/tests/local.rs`；Swift 的 `CandidClientTests` | **缺口：没有接口清单快照** |
| 表结构与升级 | 用户已有的库 | [数据模型](../specs/data-model-v1.md)、[时间线库 §4](../specs/timeline-v1.md) | `schema_upgrade.rs`、`migration.rs`；`core/fixtures/schema-v*.sql` | 已守护 |
| 模型调用的输入 | 提示词质量、成本 | `core/prompts/` | 全流程测试的 `snapshots/model-calls/` | 已守护 |
| 备份与恢复 | 用户换机器 | [数据位置与迁移](../data-migration.md) | `core/crates/candid-cli/tests/migrate.rs`（往返一致、不带走密钥） | 已守护 |
| 开放格式导出（可迁移包） | 用户离开 Candid、其他工具读取 | [时间线库 §8](../specs/timeline-v1.md) 只有映射表 | 无 | **缺口：未实现**。见下 |
| 技能正文的前置字段 | Pi、外部 agent、App 的技能页 | [agents/README](../../agents/README.md) | `agents/pi-extension/test/e2e.mjs` 间接覆盖 | 部分 |
| 固定流程目录 | 核心调度、App 的流程页 | `core/workflows/catalog.json`、`outputs.schema.json` | `core/crates/candid-store/tests/workflows.rs` | 已守护 |

## 改动一条契约时

1. 先确认真的要改：外部 agent 和用户的旧数据不会跟着你升级。能加字段就不改字段，能加工具就不改工具的含义。
2. 改规格，再改实现，再更新快照：

```bash
UPDATE_SNAPSHOTS=1 cargo test -p candid-service --test lifecycle --test mcp_contract
```

```bash
UPDATE_SNAPSHOTS=1 cargo test -p candid-cli --test contract
```

3. 提交说明写清楚：变了什么、为什么、谁会受影响。
4. 表结构改动另需升版本、带升级测试和失败回滚测试。

## 缺口，按重要性排

1. **开放格式导出没有实现。** 现在能带走数据的只有 `candid backup`，它产出的是 Candid 自己的库。产品承诺是「开放、可迁移」，但别的工具目前读不了这份数据。规格里引用的「artifact v1」封套格式也不在本仓库的文档里。这是产品定位和实现之间最大的一处落差。
2. **本地通道没有接口清单。** 约三十个 `/local/…` 接口是 App 和核心之间的全部往来，也是 Windows 端要对接的东西，但没有一份清单和快照。做第二个平台之前必须补。
3. **入库协议没有 schema 快照。** 行为有测试，但协议字段的增减不会让任何测试失败。让第三方或其他平台提交数据之前必须补。
