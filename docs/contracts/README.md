# 契约清单

**状态**：十一项契约中七项有测试或快照守护，一项部分守护，三项是缺口（第 5 节）。

本文列出所有「别人依赖、不能悄悄改」的东西：每项的规格在哪、哪个测试守着它、怎样改才算合规。
它是索引，不复述规格正文；各项的具体内容在链接到的[规格](../specs/README.md)里。

<a id="what"></a>

## 1. 什么算契约

一样东西满足下面任何一条，就是契约：

- **仓库外的东西依赖它。** 外部 agent 依赖工具名和输出字段；用户的脚本依赖命令行；用户磁盘上的旧库依赖表结构能升级；用户的备份依赖能恢复。
- **两块独立演进的代码靠它对接。** 平台端和核心靠本地通道与入库协议对接；以后的 Windows 端会按同一份协议实现。
- **它变了，主人看不出来，但系统的行为或成本变了。** 模型收到的输入就是这样：提示词多一句，质量和费用都可能变。

契约的共同点是**改动的代价由别人承担**，所以每项契约必须有一个测试或快照，让改动无法悄悄发生。没有测试守着的契约是缺口，文字写得再多也不算数。

<a id="list"></a>

## 2. 清单

| # | 契约 | 谁依赖 | 规格 | 守护它的测试或快照 | 状态 |
|---|---|---|---|---|---|
| 1 | MCP 工具：23 个工具的名字、说明、参数 schema | 外部 agent、Pi | [查询接口](../specs/query.md#tools) | `core/crates/candid-service/tests/mcp_contract.rs`；快照 `core/fixtures/contract/mcp-tools.json`、`mcp-initialize.json` | 已守护 |
| 2 | MCP 查询的输出字段 | 外部 agent、技能 | [查询接口](../specs/query.md) | 全流程测试的 `core/fixtures/lifecycle/snapshots/mcp/` | 已守护，抽样 7 个查询 |
| 3 | `candid` 命令行 | 脚本、外部 agent | [查询接口](../specs/query.md#access)、[core/README](../../core/README.md) | `core/crates/candid-cli/tests/contract.rs`；快照 `core/fixtures/contract/candid-tools.txt` | 已守护 |
| 4 | 入库协议（`IngestBatch`） | Mac 采集、各连接器；以后的 Windows 与手机端 | [连接器与入库协议](../specs/connectors.md#ingest-connectors) | `core/crates/candid-store/tests/ingest.rs`、`merge.rs` | 部分：行为有测试，协议的字段没有快照 |
| 5 | 本地通道（`/local/…`，约 34 条路径） | Mac App；以后的其他平台端 | [查询接口的本地通道附录](../specs/query.md#local-channel) | `core/crates/candid-service/tests/local.rs`；Swift 的 `CandidClientTests` | 缺口：没有接口清单快照 |
| 6 | 表结构与升级（当前版本 12） | 用户已有的库 | [存储](../specs/storage.md)、[数据模型](../specs/data-model.md#migration) | `candid-store/tests/schema_upgrade.rs`、`migration.rs`；`core/fixtures/schema-v*.sql` | 已守护 |
| 7 | 模型调用的输入 | 质量、成本 | [模型调用](../specs/models.md#call-list)；提示词在 `core/prompts/` | 全流程测试的 `snapshots/model-calls/` | 已守护 |
| 8 | 备份与恢复 | 用户换机器 | [数据位置与迁移](../data-migration.md) | `core/crates/candid-cli/tests/migrate.rs`：往返一致、不带走密钥、恢复前先全部校验 | 已守护 |
| 9 | 开放格式导出 | 用户离开 Candid；别的工具读取 | [存储的未实现部分](../specs/storage.md#open)只有一张映射表 | 无 | 缺口：未实现 |
| 10 | 技能正文的前置字段 | Pi、外部 agent、App 的技能页 | [固定流程与 agent](../specs/workflows.md#skills-triggers) | `agents/pi-extension/test/e2e.mjs` 间接覆盖 | 部分 |
| 11 | 固定流程目录 | 核心调度、App 的流程页 | [固定流程与 agent](../specs/workflows.md#tasks)；`core/workflows/catalog.json`、`outputs.schema.json` | `core/crates/candid-store/tests/workflows.rs` | 已守护 |

<a id="compat"></a>

## 3. 兼容规则

改契约之前先判断这次改动属于哪一类：

| 改动 | 是否兼容 | 要求 |
|---|---|---|
| 给工具输出加一个字段；加一个新工具；加一个可选参数 | 兼容 | 更新快照，在版本记录的「内部」里记一行 |
| 给表加一列、加一张表 | 兼容，但要升表结构版本 | 升级测试、失败回滚测试 |
| 改字段的含义或类型；改工具名；删字段、删工具；把可选参数变成必填 | **不兼容** | 需要一条[决定](../decisions/README.md)。优先用「加新的、旧的保留一段时间」代替直接改 |
| 改提示词或模型输入的拼法 | 对外兼容，但改变质量与成本 | 更新调用快照；在真实数据的副本上抽样对比，见 [.claude/skills/real-data-trial](../../.claude/skills/real-data-trial/SKILL.md) |
| 改备份包的内容或布局 | 旧备份必须仍能恢复 | 往返测试覆盖旧格式 |

1.0 之前允许不兼容的改动，但每一次都必须出现在[版本记录](../changelog.md)里，写明谁受影响。

<a id="change"></a>

## 4. 改动一项契约的步骤

1. 判断属于第 3 节的哪一类。不兼容的先记决定。
2. 改规格，再改实现。
3. 重新生成快照，逐项看差异，确认只有预期的变化：

```bash
UPDATE_SNAPSHOTS=1 cargo test -p candid-service --test lifecycle --test mcp_contract
```

```bash
UPDATE_SNAPSHOTS=1 cargo test -p candid-cli --test contract
```

4. 更新本页的清单（如果增减了契约或状态变了）。
5. 提交说明写清楚：变了什么、为什么、谁受影响。版本记录的「内部」加一行。
6. 表结构改动另按[工程规范](../engineering.md#recipes)的 5.1 做。

**新增一项契约**时：在同一个改动里加上守护它的测试，并在第 2 节加一行。不允许先加契约、以后再补测试。

<a id="gaps"></a>

## 5. 缺口

按重要性排。每项在[待办](../roadmap.md)里有对应条目。

### 5.1 开放格式导出没有实现

现在能带走数据的只有 `candid backup`，产出的是 Candid 自己的库，别的工具读不了。产品定位的第四段是「带得走」，这是定位和实现之间最大的落差。

旧规格里有一张从库内对象到导出包的映射表，但它依赖一份不在本仓库里的封套格式定义。所以这件事要从「导出包长什么样」定起，是一个需要主人参与的设计，不是照着做的实现。

### 5.2 本地通道没有接口清单快照

App 和核心之间的全部往来走本地通道，这也是 Windows 端要照着对接的东西。接口清单已经整理进[查询接口的附录](../specs/query.md#local-channel)，但它是手写的：增减一个接口不会让任何测试失败。

做第二个平台之前必须补上：让核心导出接口清单，存成快照，由测试比对。

### 5.3 入库协议没有 schema 快照

行为有测试，但协议字段的增减不会让任何测试失败。另外，规格描述的是每个连接器实现五个能力的统一协议，代码里是一组按来源分发的函数，两者的形态不一致，见[连接器](../specs/connectors.md#open)。

让第三方或其他平台提交数据之前必须补上。

### 5.4 技能前置字段只有间接覆盖

技能正文的前置字段（标题、摘要、触发、是否启用、输入）被三处读取，但没有一个测试直接检查格式。风险较低，排在最后。

<a id="sources"></a>

## 6. 来源

- 第 2 节各项的规格位置：2026-11-26 按主题重构后的[规格](../specs/README.md)。
- 第 5 节：2026-11-26 整理时对照代码与测试得出。
