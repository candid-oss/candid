# Personal agent platform v1 (relocation map)

[中文](agent-platform-v1.md) · **English**

**Status**: retired. Its content moved out by topic on 2026-11-26; this page only keeps the map and takes no new content.

This document used to mix rules, decisions, progress and experiment data. They now live in the topic [specs](README.md), the [decision log](../decisions/log.md), the [changelog](../changelog.md), the [roadmap](../roadmap.md) and the [research notes](../research/README.md) (the last four are in Chinese). The table shows where each old section went; section-number references in code comments (such as "§4.6") resolve here. The original text is in git history: `git log --follow -- docs/specs/agent-platform-v1.en.md`.

| Old section | Old title | Now in |
|---|---|---|
| <a id="position"></a>§1 | Position and principles | [Workflows and agents · position](workflows.en.md#position); [Product](../product.md) |
| <a id="pipeline"></a>§2 | Curation pipeline | [Curation · pipeline](curation.en.md#pipeline) |
| <a id="sources"></a>§2.0 | Source coverage | [Connectors and ingest · sources](connectors.en.md#sources) |
| <a id="mail"></a>§2.0.1 | Mail connector | [Connectors and ingest · mail](connectors.en.md#mail) |
| <a id="source-filter"></a>§2.0.2 | Source filtering | [Connectors and ingest · source-filter](connectors.en.md#source-filter) |
| <a id="redaction"></a>§2.1 | Redaction at entry | [Curation · redaction](curation.en.md#redaction) |
| <a id="near-dup"></a>§2.2 | Near-duplicate removal | [Curation · near-dup](curation.en.md#near-dup) |
| <a id="boilerplate"></a>§2.3 | Boilerplate down-weighting | [Curation · boilerplate](curation.en.md#boilerplate) |
| <a id="segments"></a>§2.4 | Segmentation | [Curation · segments](curation.en.md#segments) |
| <a id="extraction"></a>§2.5 | Extraction | [Curation · extraction](curation.en.md#extraction); [People and items · item-extract](people-items.en.md#item-extract) |
| <a id="entities"></a>§2.6 | Entity layer | [People and items · entities](people-items.en.md#entities); [People and items · people](people-items.en.md#people) |
| <a id="speaker-labeling"></a>§2.6 | Speaker labelling | [People and items · speaker-labeling](people-items.en.md#speaker-labeling) |
| <a id="recall"></a>§2.7 | Recall | [Curation · recall](curation.en.md#recall) |
| <a id="digests"></a>§2.8 | Digests | [Curation · digests](curation.en.md#digests) |
| <a id="evaluation"></a>§2.9 | Evaluation set | [Curation · evaluation](curation.en.md#evaluation) |
| <a id="features"></a>§3 | Feature choices | [Workflows and agents · features](workflows.en.md#features) |
| <a id="tasks"></a>§4 | Minimal workflow framework | [Workflows and agents · tasks](workflows.en.md#tasks) |
| §4 | Scheduling, cost and recovery | [Model calls · scheduling](models.en.md#scheduling); [Model calls · budget](models.en.md#budget); [Model calls · retry](models.en.md#retry) |
| §4 | Provenance, permissions and revisions | [Workflows and agents · tasks-provenance](workflows.en.md#tasks-provenance) |
| <a id="skills-triggers"></a>§4.1 | Two modes: built-in and attached agent | [Workflows and agents · skills-triggers](workflows.en.md#skills-triggers) |
| <a id="ai-settings"></a>§4.2 | Settings and management | [Model calls · ai-settings](models.en.md#ai-settings); [Workflows and agents · ai-page](workflows.en.md#ai-page) |
| <a id="commitments"></a>§4.3 | Commitments and reminders | [Workflows and agents · commitments](workflows.en.md#commitments) |
| <a id="references-skill"></a>§4.4 | References and replies | [Workflows and agents · references-skill](workflows.en.md#references-skill) |
| <a id="executors"></a>§5 | Where things run | [Workflows and agents · executors](workflows.en.md#executors) |
| <a id="research"></a>§6 | Research topics | [Research: memory and persona](../research/memory-and-persona.md) |
| <a id="references"></a>§7 | References | [Research: references](../research/references.md) |
| <a id="plan"></a>§8 | Implementation slices | [Changelog](../changelog.md); [Roadmap](../roadmap.md) |
