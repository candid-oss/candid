# 规格索引

规格按主题分，一份讲一块能力**现在**的行为和规则。写法见[文档规范](../standard.md#spec)。
英文版与中文版同名，后缀 `.en.md`；两版不一致时以中文为准。

## 从哪份读起

| 你要做的事 | 读 |
|---|---|
| 加一个数据来源，或做另一个平台的采集 | [连接器与入库协议](connectors.md)，然后[存储](storage.md) |
| 让 AI 读到更多、更准的内容 | [整理层](curation.md)、[人物与条目](people-items.md) |
| 加或改一个给 agent 用的工具 | [查询接口](query.md) |
| 加一次模型调用，调预算或调度 | [模型调用](models.md) |
| 加一个 AI 功能或技能 | [固定流程与 agent](workflows.md) |
| 改表结构、存储占用、保留策略 | [存储](storage.md)、[数据模型](data-model.md) |
| 做页面 | [界面规范](design-system.md) |

## 规格

按数据流的顺序排：先进来，再存下，再整理，再被读取。

| 规格 | English | 主题 | 状态 |
|---|---|---|---|
| [连接器与入库协议](connectors.md) | [en](connectors.en.md) | 两类连接器、入库批次、连续打字合并、AI 会话连接器、连接器清单、邮件、来源筛选 | 部分实现 |
| [存储](storage.md) | [en](storage.en.md) | 时间线服务、数据根目录、通用约定、来源与游标、事件、线程、原件、文字段落、检索索引、作业与删除 | 已实现；开放格式导出未实现 |
| [数据模型](data-model.md) | [en](data-model.en.md) | 分层、证据分级、保真度阶梯、去重原则、存储预算、省空间的存储方案、删除与压缩的连带关系 | 部分实现；证据分级与保留策略未实现 |
| [整理层](curation.md) | [en](curation.en.md) | 原始记录怎样变成对话、消息、摘录、活动块、汇总；截图摘录；屏幕清洗；派生层的修订与纠正 | 部分实现 |
| [人物与条目](people-items.md) | [en](people-items.en.md) | 别名与归并、候选、说话人标注；条目的抽取、合并、状态流转；备忘 | 部分实现 |
| [模型调用](models.md) | [en](models.en.md) | 模型来源、角色、两条通道、用量记账、调用清单、调度、预算、重试、设置 | 部分实现 |
| [查询接口](query.md) | [en](query.en.md) | 共同规则、23 个工具、调用顺序、访问方式与鉴权、范围、审计、提案、本地通道清单 | 已实现；个别项未实现 |
| [固定流程与 agent](workflows.md) | [en](workflows.en.md) | 功能取舍、固定流程框架、核心内自动与挂接的 Pi、技能与触发、承诺与提醒、「帮我写」 | 部分实现 |
| [界面规范](design-system.md) | [en](design-system.en.md) | Mac 界面的文字、颜色、表面、组件、反馈、动效、时间线交互 | 部分实现 |

各份规格的「未实现与待定」一节汇总在[待办](../roadmap.md)里。

<a id="legacy"></a>

## 搬迁对照页

2026-11-26 之前的四份规格按主题拆分后，原文件保留为对照页：旧章节号和旧锚点各去了哪里。代码注释里按章节号的引用（例如 `agent-platform-v1 §2.6`、`data model v1 §9.1`）经这些页面查到新位置。对照页不再接受新内容。

| 旧文件 | English | 内容去向 |
|---|---|---|
| [timeline-v1.md](timeline-v1.md) | [en](timeline-v1.en.md) | 存储、整理层、模型调用、查询接口、连接器 |
| [agent-platform-v1.md](agent-platform-v1.md) | [en](agent-platform-v1.en.md) | 整理层、人物与条目、连接器、固定流程与 agent、模型调用 |
| [data-model-v1.md](data-model-v1.md) | [en](data-model-v1.en.md) | 数据模型、存储 |
| [design-system-v1.md](design-system-v1.md) | [en](design-system-v1.en.md) | 界面规范 |
