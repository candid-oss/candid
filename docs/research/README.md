# 研究与实验索引

**状态**：已生效（2026-11-26 起）。

本目录收一次调研或实验看到了什么：日期、问题、方法、数据、结论，以及结论被哪份规格或决定采用。它只追加，不改写，也不放现行规则；规则在[规格](../specs/README.md)里，当初为什么这么定在[决定](../decisions/README.md)里。每份记录的格式见[文档规范](../standard.md#templates)。

| 文件 | 日期 | 问题 | 结论被谁采用 |
|---|---|---|---|
| [storage-measurements.md](storage-measurements.md) | 2026-11-18 至 2026-11-19 | 资料库的空间花在哪里？换什么存储方案能省空间又不损失可追溯性？ | 已采用：[data-model](../specs/data-model.md) |
| [legacy-migration.md](legacy-migration.md) | ≤2026-11-18 | 怎样把旧的桌面账本和 Hub 资料库迁进统一时间线库，并保证核对通过前不丢数据？ | 部分采用：迁移与幂等规则并入 [storage](../specs/storage.md)；旧账本退役见 [data-model](../specs/data-model.md) |
| [ai-v1-survey.md](ai-v1-survey.md) | 2026-11-20 至 2026-11-21 | 回消息和抽日程的数据在哪里、质量如何？截图读取怎样分工（OCR 与视觉模型）？ | 已采用：[curation](../specs/curation.md)、[models](../specs/models.md)；部分判断被 data-layer-lab 修正 |
| [references.md](references.md) | ≤2026-11-18 | screenpipe、MineContext、OpenClaw 与各种邮件接入方案哪些值得借鉴、哪些不能采用？ | 部分采用：[curation](../specs/curation.md)、[connectors](../specs/connectors.md)、[workflows](../specs/workflows.md) |
| [memory-and-persona.md](memory-and-persona.md) | ≤2026-11-18 | 记忆与人格画像、「你关心什么」、主动打扰怎么做？ | 未采用：只列议题，尚无结论 |
| [data-layer-lab.md](../plans/data-layer-lab.md) | 2026-11-25 | 在真实数据副本上，数据层各来源现在走到哪一步？截图读取选什么读法和模型？ | 仍在进行，文件暂留在 `plans/`，完成后再迁入本目录 |

日期写「≤」表示原文没有注明日期，该日期是原文件首次进入仓库的日期。
