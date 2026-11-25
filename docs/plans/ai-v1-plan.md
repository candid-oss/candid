# AI 功能 v1 实施计划（搬迁对照页）

**状态**：已废弃。内容已于 2026-11-26 按主题迁出，本页只保留对照表，不再接受新内容。

这份文档原来把规则、决定、进度和实验数据写在一起。现在它们分别在按主题的[规格](../specs/README.md)、[决定流水](../decisions/log.md)、[版本记录](../changelog.md)、[待办](../roadmap.md)和[研究记录](../research/README.md)里。下表列出原来每一节去了哪里；代码注释里按章节号的引用（例如「§4.6」）在这里查新位置。原文可以在 git 历史里看到：`git log --follow -- docs/plans/ai-v1-plan.md`。

| 原章节 | 原标题 | 现在在 |
|---|---|---|
| 进度 | 进度 | [版本记录](../changelog.md)；[研究记录：AI 功能 v1 调研](../research/ai-v1-survey.md) |
| §0 | 调研结论 | [研究记录：AI 功能 v1 调研](../research/ai-v1-survey.md) |
| AI 可读层 | AI 可读层：分三层、存储、接口 | [整理层 · overview](../specs/curation.md#overview)；[整理层 · curated-sources](../specs/curation.md#curated-sources)；[查询接口 · tools](../specs/query.md#tools) |
| §1 | 里程碑 | [版本记录](../changelog.md)；[待办](../roadmap.md) |
| §2 | M1 地基 | [整理层 · curated-sources](../specs/curation.md#curated-sources)；[模型调用 · lanes](../specs/models.md#lanes) |
| §3 | M2 内容抽取 | [整理层 · screen-excerpt](../specs/curation.md#screen-excerpt) |
| §4 | M3 主动功能：基于历史回消息 | [固定流程与 agent · compose](../specs/workflows.md#compose) |
| §5 | M4 自动功能：日程、提醒、备忘 | [人物与条目 · items](../specs/people-items.md#items)；[人物与条目 · memos](../specs/people-items.md#memos) |
| §6 | M5 闭环与语音 | [待办](../roadmap.md) |
| §7 | 需要你决定 | [待办](../roadmap.md)；[决定流水](../decisions/log.md) |
| §8 | 不在这一版 | [待办](../roadmap.md) |
