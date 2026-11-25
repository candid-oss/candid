# 参考与取舍：外部项目与方案调研

**状态**：已完成（≤2026-11-18）。结论被 [curation](../specs/curation.md)、[connectors](../specs/connectors.md) 和 [workflows](../specs/workflows.md) 部分采用；「不采用」项都已写进对应规格的约束。

**问题**：做数据整理、邮件接入、任务机制、记忆时，已有的外部项目和方案哪些值得借鉴，哪些不能采用？

**方法**：阅读公开文档与开源项目，对照 Candid 的约束（本地优先、权限是真正的边界、合并不删原件、写操作只走提案），逐项记下「借鉴」和「不采用」。没有做对照实验，邮件部分的取舍引用了公开资料而非本地测试。

## 观察与数据

### 参考与取舍表（原 agent-platform-v1 §7）

| 来源 | 借鉴 | 不采用 |
|---|---|---|
| screenpipe | 任务文件 = 配置加提示词；清单条目有稳定编号，结合你的处理逐次核对；「没出现不等于完成」这类证据规则；精简活动汇总 | 执行者带终端，权限不是真正的边界；记忆只是平铺的条目 |
| MineContext | 上下文分类（实体、活动、意图、知识、步骤、状态）可作为抽取输出的参考；图像相似度去重加批处理；问答循环里的「信息是否足够」检查 | 每张截图都调视觉模型；合并时删掉旧条目；没有用户纠正 |
| 邮件接入（Littlebird、Nylas、EmailEngine、EagleFiler、DEVONthink、社区 Apple Mail MCP、MailKit） | 读邮件 App 的本机数据加完全磁盘访问；只读打开索引并识别表结构；MailKit 作为新邮件的实时补充 | 各家分别 OAuth；云端或付费的统一邮件接口；邮件 App 插件；以解释器运行的第三方邮件 MCP |
| OpenClaw | 画像和记忆可读、可编辑；注入内容有长度上限；定时整理并附审阅说明；来源可疑的写入隔离；无事可报时静默；只在空闲和活跃时段运行 | 安全只靠提示词约束；遗忘删不到改写过的内容 |

### 邮件接入方案的调研（原 agent-platform-v1 §2.0.1「为什么这样选」）

参考资料：[EagleFiler 导入邮件](https://c-command.com/eaglefiler/help/importing-mail-from-app)、[DEVONthink 与完全磁盘访问](https://www.devontechnologies.com/support/faq/full-disk-access)、[EmailEngine 文档](https://learn.emailengine.app/docs)、[Littlebird 连接工具](https://docs.littlebird.ai/learn/connect-your-tools/)、[claude-connector-apple-mail](https://github.com/falconbradley/claude-connector-apple-mail)、[MailKit MEMessageActionHandler](https://developer.apple.com/documentation/mailkit/memessageactionhandler)。

- 各家分别接官方接口，要分别注册 OAuth 应用；读邮件权限审核严，公司的 Microsoft 365 常常禁止第三方应用。云端助手类产品（如 Littlebird）也只接了两三家。
- 统一邮件接口服务（Nylas、EmailEngine）只是把各家的差异藏在服务后面，仍要自己注册 OAuth 应用，而且是云服务或付费的自部署服务，和本地、独立运行的目标冲突。
- Mac 本地归档工具（EagleFiler、DEVONthink）走的是「读邮件 App 的本机文件 + 完全磁盘访问」；依赖邮件 App 插件的做法已被苹果废弃。
- 社区的 Apple Mail MCP 项目（如 claude-connector-apple-mail、apple-mail-mcp）面向 agent 临时查询，只提供搜索和读取，没有增量同步、删除检测和线程。它们以 Python 或 Node 运行，完全磁盘访问只能授给解释器或终端，范围过大。只把它们当作表结构和文件格式的参考，借鉴前先看许可证，不复制代码。
- 邮件入库后，外部 agent 经 Candid 自己的 MCP 就能查到，带范围过滤和审计，不需要另接邮件 MCP。

### 其他邮件适配器的调研记录（原 agent-platform-v1 §2.0.1「存储适配器」）

| 方案 | 调研结论 |
|---|---|
| Thunderbird 本机数据 | Thunderbird 自己处理 Gmail、Outlook 的 OAuth 登录；145 版（2025 年 11 月）起原生支持 Exchange 邮件，走的是微软正在淘汰的 EWS 接口，有报道称 2026 年 10 月起 Exchange Online 停止第三方 EWS 访问，接入前核实。需要在 Thunderbird 里保留离线副本；本机存储格式（mbox 或 Maildir）实测后确定 |
| 自己登录（IMAP） | email-lib 0.27（himalaya 背后的 pimalaya 库）只用 EXAMINE、UID SEARCH、BODY.PEEK 读取，不改标记、不移动；Gmail 用应用专用密码，或用户自己注册的 OAuth 应用；不借用 Thunderbird 等其他软件的 OAuth 应用编号。以 Candid 名义的一键登录需要通过 Google 和微软的审核，按需求再评估 |
| MailKit 实时扩展 | 新邮件到达时由「邮件」App 调用，可拿到正文；只覆盖新邮件。新版 macOS 有扩展开关不出现的报告，实测可用后作为补充 |
| Exchange Online | EWS 自 2026 年 10 月起逐步关闭，以后要直连 Exchange 时按 Microsoft Graph 评估 |

### 其他连接器候选的取舍（原 timeline-v1 §6.3）

| 来源 | 取舍 |
|---|---|
| 微信 | 本机数据加密，暂不考虑 |
| iMessage、飞书、Slack | iMessage 走本机库（需完全磁盘访问），飞书或 Slack 走开放接口；作为候选，群聊和单聊分开授权 |
| 笔记与选定文件夹、工作服务（GitHub、Notion、飞书文档） | 候选；只读选定目录，只把「你参与了什么」作为事件入库，内容按引用 |

### 本地调研：本机最近 7 天的数据形态（2026-11-20）

此项属于本机数据调研，完整记录见 [AI v1 调研与实测](ai-v1-survey.md)。

## 结论

- screenpipe 与 MineContext：采用「任务 = 配置加提示词」「稳定编号」「只降权不删」「上下文分类」等做法，不采用终端执行者、逐张截图调视觉模型、合并时删旧条目；体现在 [curation](../specs/curation.md) 的降噪与抽取规则和 [workflows](../specs/workflows.md) 的固定流程。
- 邮件：自己实现的采集型连接器，主路线读本机邮件 App 数据，加 IMAP 适配器，不依赖第三方邮件 MCP，不分别注册 OAuth 应用；体现在 [connectors](../specs/connectors.md)。
- OpenClaw：采用画像可读可编辑、注入有上限、定时整理并附审阅说明等思路，目前只是研究议题（见 [记忆与人格研究](memory-and-persona.md)），没有被任何规格采用。
- 取舍均为对公开资料的判断，借鉴任何第三方代码前先看许可证。

### Pi 隔离与护栏的对照实验（原 agent-platform-v1 §4.1，≤2026-11-18）

- 对照实验：在伪造的 `~/.pi/agent` 里放插件与设置，不加隔离时会被 Pi 加载，加隔离（`HOME` 与 `PI_CODING_AGENT_DIR` 指向 App 数据目录下的 `pi/`、不继承用户环境）后不加载。
- 启动参数里是 `--no-extensions`（复数）：这一版 Pi 用它同时关掉内置插件（MCP、codemode），显式 `-e` 仍加载。升级 Pi 须重新核实。
- 提示注入抽查：一个「照做」的模拟模型去调用 `bash`，只得到「没有这个工具」；在只读运行里让它改名，被核心拒绝。
- 核心对上游不流式，插件把整段回答按 SSE 一次回放给 Pi，所以文字不逐字出现（待改进）。
