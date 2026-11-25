# Docs

**English** · [中文](#中文)

Start with the three one-page documents, then go to the spec for the part you are changing.

| Document | Language | Topic |
|---|---|---|
| [Product](product.md) | 中文 | What Candid is, for whom, what it will not do, how we know it works |
| [Architecture](architecture.md) | 中文 | The five parts, dependency directions, the three extension seams, known debts |
| [Engineering](engineering.md) | 中文 | Design principles, code rules, test order, definition of done |
| [Contracts](contracts/README.md) | 中文 | Everything others depend on, and the test that guards each |
| [Data model](specs/data-model-v1.en.md) | [中文](specs/data-model-v1.md) | Layers, evidence tiers, dedup, storage budget (validation pending) |
| [Timeline library](specs/timeline-v1.en.md) | [中文](specs/timeline-v1.md) | Data model, ingest, query tools, access and audit, connectors |
| [Agent platform](specs/agent-platform-v1.en.md) | [中文](specs/agent-platform-v1.md) | How data is organized, built-in processing, the attached agent, skills and permissions |
| [Design system](specs/design-system-v1.en.md) | [中文](specs/design-system-v1.md) | macOS UI rules and component usage |
| [ADR-024: Rust core](decisions/024-rust-core.en.md) | [中文](decisions/024-rust-core.md) | Why the core is Rust and runs as a helper process |
| [Data locations and migration](data-migration.md) | 中文 | Every place Candid writes to disk; what backup carries |

Specs, decision records and module READMEs are kept in English and Chinese. Product, architecture, engineering and plans are Chinese only.

## 中文

### 从哪读起

| 你想知道 | 读 |
|---|---|
| 这个产品要成为什么、不做什么 | [产品定位](product.md) |
| 系统有哪几块、往哪里扩展、欠着什么 | [架构总图](architecture.md) |
| 代码怎么写、测试怎么跑、什么算完成 | [工程规范](engineering.md) |
| 哪些东西不能悄悄改 | [契约清单](contracts/README.md) |
| 某一块的具体行为 | 下面的规格 |
| 现在正在做什么 | [plans/](plans/) |

### 规格

| 文档 | 主题 |
|---|---|
| [数据模型](specs/data-model-v1.md) | 分层、证据分级、去重与存储预算（待验证） |
| [时间线库](specs/timeline-v1.md) | 数据结构、入库、查询工具、访问与审计、连接器 |
| [Agent 基座](specs/agent-platform-v1.md) | 数据整理流程、功能取舍、固定流程、挂接的 agent |
| [界面规范](specs/design-system-v1.md) | Mac 界面规则与组件用法 |
| [数据位置与迁移](data-migration.md) | 写到磁盘上的每个位置，备份带走什么 |

决策记录：[ADR-024：Rust 核心](decisions/024-rust-core.md)。

<a id="standard"></a>

### 文档标准

**六种文档，各放各处。** 一份文档只做一件事；混在一起就没人知道哪句还是真的。

| 种类 | 位置 | 写什么 | 不写什么 | 寿命 |
|---|---|---|---|---|
| 定位 | `product.md` | 方向与取舍 | 实现 | 长期，只有主人改 |
| 地图 | `architecture.md` | 模块、依赖方向、接缝、欠账 | 细节 | 长期，结构变了才改 |
| 规范 | `engineering.md` | 所有改动都要守的规则 | 某个模块的行为 | 长期 |
| 契约 | `contracts/` | 对外承诺，各绑定一个测试 | 没有测试守护的愿望 | 长期，改动要更新快照 |
| 规格 | `specs/` | 一个模块**现在**的行为 | 进度、日期流水、被否决的方案 | 随代码改 |
| 决策记录 | `decisions/` | 一个决定：背景、选项、结论、代价 | 之后的修改：新决定写新的一条 | 写完不改 |
| 计划 | `plans/` | 正在做的事：目标、步骤、进度、待主人决定的问题 | 长期规则 | 做完就删，结论并入规格或决策记录 |

**规则：**

1. **规格只写现在为真的。** 还没实现的写在计划里；规格里确实需要提到时标「未实现」。没有测试支撑的断言标「未验证」。
2. **进度只写在计划里。** 规格开头的状态行只写一句话：已实现、部分实现还是设计中。
3. **一个事实只在一处写。** 别处用链接指过去。两处写得不一样时，以种类靠上的为准，并修掉另一处。
4. **改代码时同步改文档。** 行为变了改规格，承诺变了改契约，结构变了改地图。不同步的改动不算完成。
5. **链接必须指向存在的文件。** 引用一份文档就给链接；不在仓库里的文档不引用。
6. **双语范围：** 规格、决策记录、模块 README、仓库 README 中英文各一份（`X.md` 与 `X.en.md`，或同一文件两段），改一版必须同时改另一版。定位、地图、规范、契约索引和计划只写中文。
7. **面向主人的文字说人话。** 先给结论，再给依据；不贴代码，不用内部代号。

**自动检查：**

```bash
python3 scripts/check_docs.py
```

检查三件事：相对链接都指向存在的文件；规格、决策记录、契约的两种语言都在；每份规格、决策记录、契约都列在本页。CI 上每次推送都会跑。

**新文档的最小模板：**

```markdown
# 标题

**状态**：设计中 / 部分实现 / 已实现（一句话）

这份文档回答什么问题，不回答什么。
```

### 已知的文档问题

这些是整理时发现、还没有处理的，处理一项删一项：

- **引用了不在仓库里的文档。** 规格里多处写「以 artifact v1 为准」「见桌面采集 v1」「见原始记录层 v1」「按智能处理」，这四份文档不在本仓库。要么补进来，要么把相关规则并入现有规格并删掉引用。在此之前，把这些引用当作不存在。
- **规格里混着进度和日期流水。** `specs/timeline-v1.md` 和 `specs/agent-platform-v1.md` 的状态行很长，正文里有「某日已实现」「某日决定取消」这类记录。按上面的规则 1、2 应该移到计划或决策记录。
- **决策记录只有一条。** 编号是 024，之前的没有留下。今后的重要决定从 025 起记。
