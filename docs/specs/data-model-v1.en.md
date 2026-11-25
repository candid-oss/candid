# Data model v1 (relocation map)

[中文](data-model-v1.md) · **English**

**Status**: retired. Its content moved out by topic on 2026-11-26; this page only keeps the map and takes no new content.

This document used to mix rules, decisions, progress and experiment data. They now live in the topic [specs](README.md), the [decision log](../decisions/log.md), the [changelog](../changelog.md), the [roadmap](../roadmap.md) and the [research notes](../research/README.md) (the last four are in Chinese). The table shows where each old section went; section-number references in code comments (such as "§4.6") resolve here. The original text is in git history: `git log --follow -- docs/specs/data-model-v1.en.md`.

| Old section | Old title | Now in |
|---|---|---|
| <a id="current"></a>§1 | Where the space goes | [Research: storage measurements](../research/storage-measurements.md) |
| <a id="features"></a>§2 | Initial features and the data they need | [Data model · features](data-model.en.md#features) |
| <a id="principles"></a>§3 | Principles | [Data model · principles](data-model.en.md#principles) |
| <a id="layers"></a>§4 | Layers | [Data model · layers](data-model.en.md#layers) |
| <a id="tiers"></a>§5 | Evidence tiers | [Data model · tiers](data-model.en.md#tiers) |
| <a id="fidelity"></a>§6 | Fidelity ladder | [Data model · fidelity](data-model.en.md#fidelity) |
| <a id="dedup"></a>§7 | Deduplication | [Data model · dedup](data-model.en.md#dedup) |
| <a id="budget"></a>§8 | Storage budget | [Data model · budget](data-model.en.md#budget) |
| <a id="storage"></a>§9、§9.1–§9.7 | Space-saving storage (§9.N is point N of that section) | [Data model · storage](data-model.en.md#storage) |
| <a id="tables"></a>§10 | Table list | [Data model · tables](data-model.en.md#tables); [Storage · schema](storage.en.md#schema) |
| <a id="deletion"></a>§11 | Deletion and compaction | [Data model · deletion](data-model.en.md#deletion) |
| <a id="migration"></a>§12 | Upgrading from v8 | [Data model · migration](data-model.en.md#migration); [Storage · schema](storage.en.md#schema) |
| <a id="validation"></a>§13 | Validation and open questions | [Research: storage measurements](../research/storage-measurements.md); [Data model · open](data-model.en.md#open) |
