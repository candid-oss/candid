# 个人 Agent 基座 v1：数据整理、功能取舍与任务机制（搬迁对照页）

**中文** · [English](agent-platform-v1.en.md)

**状态**：已废弃。内容已于 2026-11-26 按主题迁出，本页只保留对照表，不再接受新内容。

这份文档原来把规则、决定、进度和实验数据写在一起。现在它们分别在按主题的[规格](README.md)、[决定流水](../decisions/log.md)、[版本记录](../changelog.md)、[待办](../roadmap.md)和[研究记录](../research/README.md)里。下表列出原来每一节去了哪里；代码注释里按章节号的引用（例如「§4.6」）在这里查新位置。原文可以在 git 历史里看到：`git log --follow -- docs/specs/agent-platform-v1.md`。

| 原章节 | 原标题 | 现在在 |
|---|---|---|
| <a id="position"></a>§1 | 定位与原则 | [固定流程与 agent · position](workflows.md#position)；[产品定位](../product.md) |
| <a id="pipeline"></a>§2 | 数据整理流程 | [整理层 · pipeline](curation.md#pipeline) |
| <a id="sources"></a>§2.0 | 来源覆盖：更多连接器 | [连接器与入库协议 · sources](connectors.md#sources) |
| <a id="mail"></a>§2.0.1 | 邮件连接器 | [连接器与入库协议 · mail](connectors.md#mail) |
| <a id="source-filter"></a>§2.0.2 | 来源筛选 | [连接器与入库协议 · source-filter](connectors.md#source-filter) |
| <a id="redaction"></a>§2.1 | 入口打码 | [整理层 · redaction](curation.md#redaction) |
| <a id="near-dup"></a>§2.2 | 近似去重 | [整理层 · near-dup](curation.md#near-dup) |
| <a id="boilerplate"></a>§2.3 | 降噪：界面固定文字 | [整理层 · boilerplate](curation.md#boilerplate) |
| <a id="segments"></a>§2.4 | 分段 | [整理层 · segments](curation.md#segments) |
| <a id="extraction"></a>§2.5 | 抽取 | [整理层 · extraction](curation.md#extraction)；[人物与条目 · item-extract](people-items.md#item-extract) |
| <a id="entities"></a>§2.6 | 实体层 | [人物与条目 · entities](people-items.md#entities)；[人物与条目 · people](people-items.md#people) |
| <a id="speaker-labeling"></a>§2.6 | 说话人标注 | [人物与条目 · speaker-labeling](people-items.md#speaker-labeling) |
| <a id="recall"></a>§2.7 | 召回 | [整理层 · recall](curation.md#recall) |
| <a id="digests"></a>§2.8 | 汇总 | [整理层 · digests](curation.md#digests) |
| <a id="evaluation"></a>§2.9 | 评估集 | [整理层 · evaluation](curation.md#evaluation) |
| <a id="features"></a>§3 | 功能取舍 | [固定流程与 agent · features](workflows.md#features) |
| <a id="tasks"></a>§4 | 固定流程的最小框架 | [固定流程与 agent · tasks](workflows.md#tasks) |
| §4 | 调度、成本与恢复 | [模型调用 · scheduling](models.md#scheduling)；[模型调用 · budget](models.md#budget)；[模型调用 · retry](models.md#retry) |
| §4 | 来源、权限与修订 | [固定流程与 agent · tasks-provenance](workflows.md#tasks-provenance) |
| <a id="skills-triggers"></a>§4.1 | 两种方式：核心内自动与挂接的 agent | [固定流程与 agent · skills-triggers](workflows.md#skills-triggers) |
| <a id="ai-settings"></a>§4.2 | 设置与管理 | [模型调用 · ai-settings](models.md#ai-settings)；[固定流程与 agent · ai-page](workflows.md#ai-page) |
| <a id="commitments"></a>§4.3 | 承诺与提醒 | [固定流程与 agent · commitments](workflows.md#commitments) |
| <a id="references-skill"></a>§4.4 | 资料与回复检索 | [固定流程与 agent · references-skill](workflows.md#references-skill) |
| <a id="executors"></a>§5 | 执行位置 | [固定流程与 agent · executors](workflows.md#executors) |
| <a id="research"></a>§6 | 研究议题 | [研究记录：记忆与人格](../research/memory-and-persona.md) |
| <a id="references"></a>§7 | 参考与取舍 | [研究记录：参考与取舍](../research/references.md) |
| <a id="plan"></a>§8 | 实施切片与验收 | [版本记录](../changelog.md)；[待办](../roadmap.md) |
