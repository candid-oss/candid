# Docs

**English** · [中文](#中文)

Start with the one-page documents, then go to the spec for the part you are changing. How documents are organized and written is defined in [standard.md](standard.md).

| Document | Language | Topic |
|---|---|---|
| [Product](product.md) | 中文 | What Candid is, for whom, what it will not do, how we know it works |
| [Architecture](architecture.md) | 中文 | Processes and modules, data paths, dependency rules, extension seams, known debts |
| [Engineering](engineering.md) | 中文 | Design principles, code rules, test order, recipes per kind of change, definition of done |
| [Documentation standard](standard.md) | 中文 | Kinds of documents, required sections, status vocabulary, linking, versions |
| [Glossary](glossary.md) | 中文 | What each term means here |
| [Contracts](contracts/README.md) | 中文 | Everything others depend on, and the test that guards each |
| [Specs](specs/README.md) | English and 中文 | One spec per topic: connectors, storage, data model, curation, people and items, models, query, workflows, design system |
| [Decisions](decisions/README.md) | ADRs in English and 中文; log in 中文 | Why things are the way they are |
| [Changelog](changelog.md) | 中文 | What each version delivered |
| [Roadmap](roadmap.md) | 中文 | What is next, what is later, what awaits a decision |
| [Research](research/README.md) | 中文 | Measurements, experiments and references |
| [Data locations and migration](data-migration.md) | 中文 | Every place Candid writes to disk; what backup carries |

## 中文

### 从哪读起

| 你想知道 | 读 |
|---|---|
| 这个产品要成为什么、不做什么、一个功能该不该做 | [产品定位](product.md) |
| 系统有哪些进程和模块、数据怎么流、往哪里扩展、欠着什么 | [架构总图](architecture.md) |
| 代码怎么写、测试怎么跑、各类改动怎么做、什么算完成 | [工程规范](engineering.md) |
| 一个词在这个项目里指什么 | [术语表](glossary.md) |
| 哪些东西不能悄悄改 | [契约清单](contracts/README.md) |
| 某一块的具体行为和规则 | [规格索引](specs/README.md) |
| 为什么这么定 | [决定索引](decisions/README.md) |
| 每个版本做了什么 | [版本记录](changelog.md) |
| 接下来做什么、什么在等决定 | [待办](roadmap.md) |
| 做过哪些调研和实验 | [研究与实验](research/README.md) |
| 数据写在磁盘的哪里、备份带走什么 | [数据位置与迁移](data-migration.md) |
| 文档本身怎么写、怎么放 | [文档规范](standard.md) |

### 目录

| 位置 | 内容 | 索引 |
|---|---|---|
| `specs/` | 规格，一个主题一份；旧规格保留为搬迁对照页 | [specs/README.md](specs/README.md) |
| `contracts/` | 对外承诺及守护它们的测试 | [contracts/README.md](contracts/README.md) |
| `decisions/` | 决策记录与决定流水 | [decisions/README.md](decisions/README.md) |
| `research/` | 调研、实测、对照实验、参考 | [research/README.md](research/README.md) |
| `plans/` | 正在进行的计划，做完即删 | 见下 |

### 进行中的计划

| 计划 | 内容 |
|---|---|
| [数据层对照实验](plans/data-layer-lab.md) | 在真实数据副本上对比截图读取与整条数据层的效果 |
| [功耗 v1](plans/power-v1.md) | 后台工作何时允许花电：一个开关、一处决定、不轮询、有账；代码已合并，待真实使用核对 |
| [AI 功能 v1](plans/ai-v1-plan.md) | 搬迁对照页：内容已迁入规格、版本记录和待办 |
| [数据层规划 v1](plans/data-layer-v1.md) | 搬迁对照页：内容已迁入规格、决定流水和待办 |

### 检查

```bash
python3 scripts/check_docs.py
```

检查的内容见[文档规范](standard.md#upkeep)。CI 上每次推送都跑。
