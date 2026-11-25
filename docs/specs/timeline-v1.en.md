# Timeline library, query interface and connectors v1 (relocation map)

[中文](timeline-v1.md) · **English**

**Status**: retired. Its content moved out by topic on 2026-11-26; this page only keeps the map and takes no new content.

This document used to mix rules, decisions, progress and experiment data. They now live in the topic [specs](README.md), the [decision log](../decisions/log.md), the [changelog](../changelog.md), the [roadmap](../roadmap.md) and the [research notes](../research/README.md) (the last four are in Chinese). The table shows where each old section went; section-number references in code comments (such as "§4.6") resolve here. The original text is in git history: `git log --follow -- docs/specs/timeline-v1.en.md`.

| Old section | Old title | Now in |
|---|---|---|
| <a id="decisions"></a>§1 | Decisions made | [Decision log](../decisions/log.md); [Storage · overview](storage.en.md#overview) |
| <a id="deployment"></a>§2.1 | Components | [Storage · deployment](storage.en.md#deployment); [Architecture: processes](../architecture.md#processes) |
| §2.2 | Data root layout | [Storage · deployment](storage.en.md#deployment) |
| §2.3 | Migration and export | [Research: legacy migration](../research/legacy-migration.md); [Storage · open](storage.en.md#open); [Data locations and migration](../data-migration.md) |
| <a id="conventions"></a>§3 | Conventions | [Storage · conventions](storage.en.md#conventions) |
| <a id="schema"></a>§4 | Data structures | [Storage · schema](storage.en.md#schema) |
| §4.1 | Sources and cursors | [Storage · source-tables](storage.en.md#source-tables) |
| §4.2 | Events | [Storage · events](storage.en.md#events) |
| §4.3 | Event kinds and payloads | [Storage · event-kinds](storage.en.md#event-kinds) |
| §4.4 | Threads | [Storage · threads](storage.en.md#threads) |
| §4.5 | Blobs | [Storage · blobs](storage.en.md#blobs) |
| <a id="text"></a>§4.6 | Text: chunks and attention | [Storage · text](storage.en.md#text) |
| <a id="fts"></a>§4.7 | Search index | [Storage · fts](storage.en.md#fts) |
| <a id="derived-next"></a>§4.8 | Derived layer | [Curation · derived](curation.en.md#derived); [Curation · derived-next](curation.en.md#derived-next) |
| <a id="ai-usage"></a>§4.9 | Model providers, roles and usage | [Model calls · providers](models.en.md#providers); [Model calls · roles](models.en.md#roles); [Model calls · ai-usage](models.en.md#ai-usage) |
| §4.10 | Clients, audit and proposals | [Query interface · clients](query.en.md#clients); [Query interface · scope](query.en.md#scope); [Query interface · audit](query.en.md#audit); [Query interface · proposals](query.en.md#proposals) |
| §4.11 | Jobs, changes and deletion | [Storage · jobs-changes-deletion](storage.en.md#jobs-changes-deletion) |
| <a id="query-api"></a>§5、§5.1 | Query interface: common rules | [Query interface · query-api](query.en.md#query-api) |
| §5.2 | Tool list | [Query interface · tools](query.en.md#tools) |
| §5.3 | Recommended call order | [Query interface · call-order](query.en.md#call-order) |
| §5.4 | Access and authentication | [Query interface · access](query.en.md#access) |
| <a id="connectors"></a>§6 | Connectors | [Connectors and ingest · overview](connectors.en.md#overview) |
| <a id="ingest-connectors"></a>§6.1 | Ingest connector protocol | [Connectors and ingest · ingest-connectors](connectors.en.md#ingest-connectors) |
| <a id="text-merge"></a>§6.1 | Typing merge | [Connectors and ingest · text-merge](connectors.en.md#text-merge) |
| <a id="agent-connectors"></a>§6.2 | Reference connectors: AI agent sessions | [Connectors and ingest · agent-connectors](connectors.en.md#agent-connectors) |
| <a id="agent"></a>§6.2 | (same) | [Connectors and ingest · agent-connectors](connectors.en.md#agent-connectors) |
| <a id="connector-table"></a>§6.3 | Ingest connector list | [Connectors and ingest · connector-table](connectors.en.md#connector-table) |
| §7 | Built-in processing and external AI | [Workflows and agents · skills-triggers](workflows.en.md#skills-triggers); [Workflows and agents · executors](workflows.en.md#executors); [Decision log](../decisions/log.md) |
| §8 | Export mapping | [Storage · open](storage.en.md#open) |
| <a id="migration"></a>§9 | Migrating existing data | [Research: legacy migration](../research/legacy-migration.md); [Storage · migration](storage.en.md#migration) |
| <a id="live-mirror"></a>§9.1 | Live mirror and switch to direct writes | [Research: legacy migration](../research/legacy-migration.md); [Storage · live-mirror](storage.en.md#live-mirror) |
| <a id="slices"></a>§10 | Implementation slices | [Changelog](../changelog.md) |
| <a id="open"></a>§11 | Open questions | [Roadmap](../roadmap.md); [Storage · open](storage.en.md#open); [Query interface · open](query.en.md#open); [Connectors and ingest · open](connectors.en.md#open) |
