# 时间线库、查询接口与连接器 v1（搬迁对照页）

**中文** · [English](timeline-v1.en.md)

**状态**：已废弃。内容已于 2026-11-26 按主题迁出，本页只保留对照表，不再接受新内容。

这份文档原来把规则、决定、进度和实验数据写在一起。现在它们分别在按主题的[规格](README.md)、[决定流水](../decisions/log.md)、[版本记录](../changelog.md)、[待办](../roadmap.md)和[研究记录](../research/README.md)里。下表列出原来每一节去了哪里；代码注释里按章节号的引用（例如「§4.6」）在这里查新位置。原文可以在 git 历史里看到：`git log --follow -- docs/specs/timeline-v1.md`。

| 原章节 | 原标题 | 现在在 |
|---|---|---|
| <a id="decisions"></a>§1 | 已确定的选择 | [决定流水](../decisions/log.md)；[存储 · overview](storage.md#overview) |
| <a id="deployment"></a>§2.1 | 组件 | [存储 · deployment](storage.md#deployment)；[架构总图：进程与通道](../architecture.md#processes) |
| §2.2 | 数据根目录布局 | [存储 · deployment](storage.md#deployment) |
| §2.3 | 迁移与导出 | [研究记录：旧数据迁移](../research/legacy-migration.md)；[存储 · open](storage.md#open)；[数据位置与迁移](../data-migration.md) |
| <a id="conventions"></a>§3 | 通用约定 | [存储 · conventions](storage.md#conventions) |
| <a id="schema"></a>§4 | 数据结构 | [存储 · schema](storage.md#schema) |
| §4.1 | 来源与同步进度 | [存储 · source-tables](storage.md#source-tables) |
| §4.2 | 事件 | [存储 · events](storage.md#events) |
| §4.3 | 事件种类与 payload | [存储 · event-kinds](storage.md#event-kinds) |
| §4.4 | 线程 | [存储 · threads](storage.md#threads) |
| §4.5 | 原件 | [存储 · blobs](storage.md#blobs) |
| <a id="text"></a>§4.6 | 文字：段落去重与注意力 | [存储 · text](storage.md#text) |
| <a id="fts"></a>§4.7 | 检索索引 | [存储 · fts](storage.md#fts) |
| <a id="derived-next"></a>§4.8 | 派生层：片段、记忆与纠正 | [整理层 · derived](curation.md#derived)；[整理层 · derived-next](curation.md#derived-next) |
| <a id="ai-usage"></a>§4.9 | 模型来源、角色与用量 | [模型调用 · providers](models.md#providers)；[模型调用 · roles](models.md#roles)；[模型调用 · ai-usage](models.md#ai-usage) |
| §4.10 | 访问方、审计与提案 | [查询接口 · clients](query.md#clients)；[查询接口 · scope](query.md#scope)；[查询接口 · audit](query.md#audit)；[查询接口 · proposals](query.md#proposals) |
| §4.11 | 作业、变更与删除 | [存储 · jobs-changes-deletion](storage.md#jobs-changes-deletion) |
| <a id="query-api"></a>§5、§5.1 | 查询接口：共同规则 | [查询接口 · query-api](query.md#query-api) |
| §5.2 | 工具清单 | [查询接口 · tools](query.md#tools) |
| §5.3 | 推荐的调用顺序 | [查询接口 · call-order](query.md#call-order) |
| §5.4 | 访问方式与鉴权 | [查询接口 · access](query.md#access) |
| <a id="connectors"></a>§6 | 连接器 | [连接器与入库协议 · overview](connectors.md#overview) |
| <a id="ingest-connectors"></a>§6.1 | 采集型连接器协议 | [连接器与入库协议 · ingest-connectors](connectors.md#ingest-connectors) |
| <a id="text-merge"></a>§6.1 | 连续打字合并 | [连接器与入库协议 · text-merge](connectors.md#text-merge) |
| <a id="agent-connectors"></a>§6.2 | 引用型连接器：AI agent 对话 | [连接器与入库协议 · agent-connectors](connectors.md#agent-connectors) |
| <a id="agent"></a>§6.2 | （同上） | [连接器与入库协议 · agent-connectors](connectors.md#agent-connectors) |
| <a id="connector-table"></a>§6.3 | 采集型连接器清单 | [连接器与入库协议 · connector-table](connectors.md#connector-table) |
| §7 | 内部固定处理与外部 AI | [固定流程与 agent · skills-triggers](workflows.md#skills-triggers)；[固定流程与 agent · executors](workflows.md#executors)；[决定流水](../decisions/log.md) |
| §8 | 导出映射 | [存储 · open](storage.md#open) |
| <a id="migration"></a>§9 | 现有数据迁移 | [研究记录：旧数据迁移](../research/legacy-migration.md)；[存储 · migration](storage.md#migration) |
| <a id="live-mirror"></a>§9.1 | 增量镜像与切换到直写 | [研究记录：旧数据迁移](../research/legacy-migration.md)；[存储 · live-mirror](storage.md#live-mirror) |
| <a id="slices"></a>§10 | 实施切片与验收 | [版本记录](../changelog.md) |
| <a id="open"></a>§11 | 待验证与待定 | [待办](../roadmap.md)；[存储 · open](storage.md#open)；[查询接口 · open](query.md#open)；[连接器与入库协议 · open](connectors.md#open) |
