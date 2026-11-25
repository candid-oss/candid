# 数据层与 AI 处理规划 v1（搬迁对照页）

**状态**：已废弃。内容已于 2026-11-26 按主题迁出，本页只保留对照表，不再接受新内容。

这份文档原来把规则、决定、进度和实验数据写在一起。现在它们分别在按主题的[规格](../specs/README.md)、[决定流水](../decisions/log.md)、[版本记录](../changelog.md)、[待办](../roadmap.md)和[研究记录](../research/README.md)里。下表列出原来每一节去了哪里；代码注释里按章节号的引用（例如「§4.6」）在这里查新位置。原文可以在 git 历史里看到：`git log --follow -- docs/plans/data-layer-v1.md`。

| 原章节 | 原标题 | 现在在 |
|---|---|---|
| §0 | 已拍板的决定 | [决定流水](../decisions/log.md)；[人物与条目 · decided-rules](../specs/people-items.md#decided-rules) |
| §1 | 人物层 | [人物与条目 · people](../specs/people-items.md#people) |
| §1.1 | 别名是稳定身份 | [人物与条目 · alias-identity](../specs/people-items.md#alias-identity) |
| §1.2 | 归并规则 | [人物与条目 · merge-rules](../specs/people-items.md#merge-rules) |
| §1.3 | 候选对怎么来 | [人物与条目 · candidates](../specs/people-items.md#candidates) |
| §1.4 | 对外 | [人物与条目 · people-external](../specs/people-items.md#people-external) |
| §2 | 条目层 | [人物与条目 · items](../specs/people-items.md#items) |
| <a id="21-存储时间线库-v12"></a>§2.1 | 存储（时间线库 v12） | [人物与条目 · items-storage](../specs/people-items.md#items-storage) |
| §2.2 | 抽取 | [人物与条目 · item-extract](../specs/people-items.md#item-extract) |
| §2.3 | 去重与跨来源合并 | [人物与条目 · item-dedup](../specs/people-items.md#item-dedup) |
| §2.4 | 状态流转 | [人物与条目 · item-states](../specs/people-items.md#item-states) |
| §2.5 | 呈现；备忘 | [人物与条目 · item-presentation](../specs/people-items.md#item-presentation)；[人物与条目 · memos](../specs/people-items.md#memos) |
| §3 | 整理层补齐的来源 | [整理层 · curated-sources](../specs/curation.md#curated-sources) |
| §4 | 模型调用规划 | [模型调用 · dispatch](../specs/models.md#dispatch) |
| §4.1 | 调用清单 | [模型调用 · call-list](../specs/models.md#call-list) |
| §4.2 | 依赖顺序 | [模型调用 · order](../specs/models.md#order) |
| §4.3 | 调度 | [模型调用 · scheduling](../specs/models.md#scheduling)；[模型调用 · lanes](../specs/models.md#lanes) |
| §4.4 | 预算 | [模型调用 · budget](../specs/models.md#budget) |
| §4.5 | 失败与重试 | [模型调用 · retry](../specs/models.md#retry) |
| §4.6 | 验证 | [模型调用 · open](../specs/models.md#open) |
| §5 | 实施顺序 | [版本记录](../changelog.md)；[待办](../roadmap.md) |
| §6 | 不在这一版 | [待办](../roadmap.md) |
