# 数据模型 v1：分层、证据分级与存储预算（搬迁对照页）

**中文** · [English](data-model-v1.en.md)

**状态**：已废弃。内容已于 2026-11-26 按主题迁出，本页只保留对照表，不再接受新内容。

这份文档原来把规则、决定、进度和实验数据写在一起。现在它们分别在按主题的[规格](README.md)、[决定流水](../decisions/log.md)、[版本记录](../changelog.md)、[待办](../roadmap.md)和[研究记录](../research/README.md)里。下表列出原来每一节去了哪里；代码注释里按章节号的引用（例如「§4.6」）在这里查新位置。原文可以在 git 历史里看到：`git log --follow -- docs/specs/data-model-v1.md`。

| 原章节 | 原标题 | 现在在 |
|---|---|---|
| <a id="current"></a>§1 | 现状：空间花在哪里 | [研究记录：存储实测](../research/storage-measurements.md) |
| <a id="features"></a>§2 | 初期功能与所需数据 | [数据模型 · features](data-model.md#features) |
| <a id="principles"></a>§3 | 原则 | [数据模型 · principles](data-model.md#principles) |
| <a id="layers"></a>§4 | 分层 | [数据模型 · layers](data-model.md#layers) |
| <a id="tiers"></a>§5 | 证据分级 | [数据模型 · tiers](data-model.md#tiers) |
| <a id="fidelity"></a>§6 | 保真度阶梯与精简记录 | [数据模型 · fidelity](data-model.md#fidelity) |
| <a id="dedup"></a>§7 | 去重 | [数据模型 · dedup](data-model.md#dedup) |
| <a id="budget"></a>§8 | 存储预算 | [数据模型 · budget](data-model.md#budget) |
| <a id="storage"></a>§9、§9.1–§9.7 | 节省空间的存储方案（§9.N 是该节的第 N 点） | [数据模型 · storage](data-model.md#storage) |
| <a id="tables"></a>§10 | 表清单（骨架） | [数据模型 · tables](data-model.md#tables)；[存储 · schema](storage.md#schema) |
| <a id="deletion"></a>§11 | 删除与压缩的连带关系 | [数据模型 · deletion](data-model.md#deletion) |
| <a id="migration"></a>§12 | 从 v8 升级 | [数据模型 · migration](data-model.md#migration)；[存储 · schema](storage.md#schema) |
| <a id="validation"></a>§13 | 验证与待定 | [研究记录：存储实测](../research/storage-measurements.md)；[数据模型 · open](data-model.md#open) |
