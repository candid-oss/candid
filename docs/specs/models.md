# 模型调用

**中文** · [English](models.en.md)

**状态**：部分实现。核心的模型来源、用量记账、两条通道、预算、重试已实现；角色收敛为三个、平台端不直连模型两项尚未做到。

本文规定核心如何调用模型：模型来源与角色、每次调用的记账、调用清单与依赖顺序、调度、预算、失败与重试、设置与管理。不规定各调用的提示词与输出内容（见[整理层](curation.md)、[人物与条目](people-items.md)），不规定 Pi 对话与技能（见[工作流](workflows.md)），不规定本地通道之外的查询接口（见[查询接口](query.md)）。

<a id="overview"></a>

## 1. 概览

<a id="rules"></a>

### 1.1 规则

1. **模型角色只有三个**：`vision`（截图摘录）、`summary`（活动块、抽取、人物判断）、`agent_chat`（回复与问答）。不新增角色；质量不够就换模型，不加角色。（仓库规则，见 [AGENTS.md](../../AGENTS.md)）
2. **模型只由核心调用。** 平台端（Mac App）要用模型时请核心代为调用；核心负责密钥、预算、授权、记账。
3. **每次调用都记一行 `ai_calls`**，不保存提示词和回答正文。
4. **不静默改用远端。** 缺模型、未授权、超预算时作业停在等待状态，写明原因。
5. **不重跑模型。** 后台调用先查缓存键，输入、提示词、模型都没变就不再调用。
6. **没有核心不知道的网络请求。** 模型地址由用户在设置里配置；请求是否经系统代理由网络代理设置决定（`proxy.rs`，见[实现位置](#code)；这是网络设置，不是模型代理）。

<a id="lanes"></a>

### 1.2 两条通道

核心里的模型调用分两条通道，互不等待，各自串行（`core/crates/candid-service/src/workflows.rs` 的 `Lane`）：

| 通道 | 谁走 | 特点 |
|---|---|---|
| 交互通道 | `POST /local/models/complete`、`POST /local/compose`（帮我写） | 有人在等；不进作业队列；不重试；最多等 5 秒（50 毫秒×100 次）拿通道，拿不到返回「模型忙」 |
| 后台通道 | 作业队列的唯一消费者（`Worker`） | 按优先级串行取作业；失败按第 6 节重试；线程设为系统后台 QoS（`scheduling.rs`） |

Pi 的模型网关 `POST /local/pi/model` 不占这两条通道：按「运行」冻结模型与预算（第 7.2 节），不进作业队列，用 `agent_call_start` 记账（`client = pi`），同一时刻只有一个运行。

<a id="providers"></a>

## 2. 模型来源与模型

<a id="ai-usage"></a>

### 2.1 配置与表

一份模型配置（版本 1：`providers`、`models`、`roles`）整体保存在 `store_meta` 的 `models.config/1`，保存时同步写入 `ai_providers`、`ai_models`、`ai_roles` 三张表（`candid-store/src/workflows.rs` 的 `model_command`）。配置才是读取的依据；三张表是它的镜像，供查询与外键。保存时的校验：

- 只接受版本 1 和已知字段；`secret_ref` 只能是钥匙串引用（不能以 `sk-` 开头、不超过 80 字符）；
- `base_url` 必须是 `http(s)://`，不含 `@`、`?`、`#`；
- `headers_json` 不得含 `authorization`、`x-api-key`、`cookie` 等凭据头；
- 配置有变化时，取消排队中和运行中的 `workflow.*`、`speech.recognize` 作业（`configuration_changed`）。

密钥只由 App 通过 `POST /local/models/authorize` 交给核心进程内存（`workflows::authorize`），不落盘；核心发请求时按 `secret_ref` 取。

| 表 | 内容 | 说明 |
|---|---|---|
| `ai_providers` | `provider_id`、名称、`api`、`base_url`、`secret_ref`、`destination`、`enabled`、`headers_json` | `api` 取 `openai-chat`、`openai-responses`、`anthropic-messages`、`gemini`、`ollama`、`other`；`destination` 取 `on_device`、`self_hosted`、`cloud` |
| `ai_models` | 服务商的模型名、`modalities_json`、`cost_json`（每百万 token 价格、币种、来源、日期） | 表里还有 `context_window`、`supports_tools` 等能力探测列，核心保存时不写（未实现） |
| `ai_roles` | 角色到模型的对应、`fallback_json`、`limits_json` | `limits_json` 读取 `max_input_tokens`、`max_output_tokens`、`daily_budget`、`timeout_ms`；`fallback_json` 核心不读取（未实现） |
| `ai_calls` | 每次调用一行，见 2.3 | |

实际调用支持的接口：`openai-chat`、`ollama`（补 `/v1`）走 `/chat/completions`；`anthropic-messages` 走 `/v1/messages`；其余 `api` 值返回 `unsupported_model_api`。Pi 目前只接 `openai-chat` 和 `ollama`。

<a id="destination"></a>

### 2.2 目的地与授权

- `destination` 为 `on_device` 的来源，地址必须是本机回环（`127.0.0.1`、`localhost`、`[::1]`），否则作业等待授权。
- 本机回环上的模型服务不论标为本机还是自托管，都不需要逐来源批准（用户的自愿选择）。
- 其他地址的来源，每个「数据来源 × 模型来源」需要一次明确批准（`workflows.approval/1:<来源>:<来源编号>`，值是该来源配置的哈希；配置改了批准失效）。
- 功能能不能用远程模型，取功能设置的 `allow_remote` 与来源授权两者中最严的；回环地址不受 `allow_remote` 限制（`feature_check`）。

<a id="calls"></a>

### 2.3 用量记账

`ai_calls` 记录核心发起的每一次模型调用（字段见 `schema.rs`）：

- `feature` 是功能编号（目录里的 `id`，如 `screen-excerpt`、`title`）、`compose`，或 Pi 运行的 `agent_chat`/自动化编号；`client` 取 `app`、`pi`、`worker`。
- 调用开始时先写一行，状态预置为 `error`、`error_code = interrupted_unknown_charge`、`cost_source = unknown`；调用结束再改写为 `ok`、`error`、`cancelled`、`timeout`。进程在中途死亡时，这一行就如实留下「可能已计费」。
- 开始时把当时的模型价格快照存进 `models.call-prices/1:<call_id>`；价格表后来变化不改写历史。
- 费用：服务商返回实际计费记 `provider`；用价格表和 token 数估算记 `estimated`；两者都没有记 `unknown`，**不记零**，币种为空。
- 回复回来时若来源已改变、权限被撤销、配置已变，调用记为 `cancelled`（`source_or_configuration_changed`），结果丢弃。
- 后台调用的结束只回写 `input_tokens`、`output_tokens`（`cache_read_tokens`、`cache_write_tokens` 列已存在，后台路径不回写，未实现）。
- Codex、Claude 等外部 agent 的用量不入库；`usage` 工具在 `include_external=true` 且该 agent 已授权读取时由连接器从源文件现算并标明来源。
- 外部来的旧用量记录经 `model_command` 的 `import_usage` 按 `call_id` 幂等导入。

<a id="roles"></a>

## 3. 角色

三个角色各管一类调用：

| 角色 | 用途 |
|---|---|
| `vision` | 截图摘录；手动的截图文字整理 |
| `summary` | 活动块概括、条目抽取、人物判断、录音摘要、决定与待办、日回顾 |
| `agent_chat` | 帮我写、Pi 对话与技能（日回顾成文、回复检索、承诺跟进等） |

**一个功能只取自己的角色；文字类功能在角色缺模型时借用别的角色已配的文字模型，用本功能自己的提示词**（核心的取用顺序，已核对）：

| 调用 | 取用顺序 |
|---|---|
| 截图摘录、手动截图整理 | `vision` |
| 条目抽取、人物判断 | `summary` → `title` |
| 活动块概括 | `episode` → `summary` → `title` |
| 录音摘要、决定与待办、日回顾 | `summary` → `title` → `vision` |
| 录音标题 | `title` → `summary` → `vision` |
| 帮我写 | `agent_chat` → `summary` → `title` |
| Pi | 请求里的角色，默认 `agent_chat`，缺则等待 |

借用不改变角色设置；设置里只需配 `vision`、`summary`、`agent_chat`。本机同时只能高效跑一个模型，建议把 `vision` 与 `summary` 配成同一个能处理文字的多模态模型，避免本机服务来回换模型（设置建议，不是规则）。

### 3.1 与三角色规则不符的现状

代码和 App 里还在用下面这些更早设计留下的角色名。它们不是新增角色的先例，是待收敛的欠账（见[未实现与待定](#open)）：

| 角色名 | 在哪里用 | 现状 |
|---|---|---|
| `title` | 录音标题、标签草稿；其他文字功能的借用回退 | 核心使用；应并入 `summary` |
| `episode` | 活动块概括（目录里 `screen-episodes`） | 核心使用；应并入 `summary` |
| `asr`、`diarization` | 录音转写（`quick` 用 `asr`，`conversation` 用 `diarization`），请求发给语音服务的 `/voice`，结果由核心校验 | 核心使用；语音服务是外部服务，不是「模型角色」，应改为独立的语音服务设置（设计中） |
| `ocr`、`relevance`、`embedding` | 只出现在 App 的角色目录里 | 核心无调用（`ocr` 的 App 侧调用见 7.3）；应删除 |

<a id="dispatch"></a>

## 4. 调用清单

下表以 `core/workflows/catalog.json`（功能目录，唯一依据）和各模块代码为准。「默认上限」是每日调用次数，设置页可改；表中三个后台功能默认关闭（`default_enabled: false`）且不允许远程。

<a id="call-list"></a>

| 功能（目录编号 / 作业类型） | 角色 | 触发 | 输入 | 输出 | 默认上限/天 | 优先级 |
|---|---|---|---|---|---|---|
| 帮我写（`/local/compose`） | `agent_chat` | 快捷键 | 当前应用、选中文字、近期对话文字；无图像（计划中的「当前帧摘录」未实现） | 一段草稿文字 | 不计功能上限 | 交互 |
| Pi 对话、手动技能 | `agent_chat` | 用户、日回顾钩子 | 对话与工具结果 | 对话回答 | 每次运行冻结，见 7.2 | 交互 |
| 截图摘录（`screen-excerpt`，`screen.excerpt`） | `vision` | 截图出现新内容 | 截图（缩放为 WebP）加窗口、网址、OCR 参考；聊天类截图最多 3 帧合并 | JSON：类型、标题、消息、摘录；上限 4096 token | 300 | P1 后台 |
| 条目抽取（`item-extract`，`items.extract`） | `summary` | 聊天对话空闲 10 分钟；录音结束；重点邮件（未验证） | 新消息（每次最多 40 条，上文 20 条，未结条目最多 15 条，每条截 500 字符） | JSON，上限 2048 token，坏 JSON 重试一次 | 150 | P1 后台 |
| 录音转写（`transcribe`，`speech.recognize`） | `asr` / `diarization`（语音服务） | 录音保存后 | 音频；超过单块上限时分块并存检查点 | 带时间戳的转写与说话人，由核心校验来源哈希与生产者 | 100 | P2 后台 |
| 录音标题、摘要、决定与待办、标签、日回顾（`title`、`summary`、`decisions`、`tags`、`daily`，`workflow.*`） | 见第 3 节 | 标题：转写完成后自动；摘要：录音结束后（设置里开）；其余手动 | 转写文字（超过 32000 字节拒绝） | 一段草稿，进派生层 | 各 100 | P2 后台 |
| 截图文字整理（`screenshot`，`workflow.screenshot`） | `vision` | 手动 | 一张图片（超过 16 MiB 拒绝） | Markdown 草稿 | 100 | P2 后台 |
| 人物判断（`people-judge`，`people.judge`） | `summary` | 出现有候选对的新别名 | 每批最多 10 对，同时最多排 5 批；每边最多 4 条消息、每条 80 字符 | JSON，上限 2048 token，坏 JSON 重试一次 | 50 | P3 后台 |
| 活动块概括（`screen-episodes`，`screen.summarize`） | `episode` → `summary` | 活动块结束 | 块内摘录与前台应用统计 | JSON：标题、摘要、引用 | 60 | P3 后台 |

后台功能的占位项：`hourly-weekly`（小时/周汇总）在目录里标 `implemented: false`（未实现）。

缓存键（相同则不再调用，作业唯一键是 `(kind, input_ref, config_version)`）：

| 调用 | 缓存键包含 |
|---|---|
| 条目抽取 | 输入消息编号集合、未结条目快照、提示词版本与文本、模型 |
| 人物判断 | 两边别名集合、提示词版本与文本、模型；回答「不确定」的对子 30 天内不重问，同一对子最多问 3 次 |
| 活动块概括 | 输入哈希、提示词版本与文本、模型 |
| 截图摘录 | 作业版本含提示词版本与文本、模型配置、功能设置；按截图编号只入队一次（图像哈希键未验证） |
| 录音类 | 模型配置、转写文字、内容修订的哈希 |

提示词可被资料库根目录 `prompts/` 下的同名文件覆盖，否则用 `core/prompts/` 内置版本；录音类默认提示词在 `core/workflows/prompts.json`。

<a id="order"></a>

## 5. 依赖顺序

```
OCR → 关键帧 → 截图摘录
邮件 / 录音转写 / AI 会话 / AX 摘录（投影，不调模型）─┴→ 对话、消息、摘录
  → 规则人物归一（不调模型）→ 条目抽取 → 条目 / 备忘
  → 人物判断（异步，不阻塞抽取；合并后条目对方经别名自动跟随）
  → 活动块概括（等该时段摘录处理完，未验证）
```

同一个对话，摘录任务未清空前不触发它的抽取（已核对：抽取扫描跳过仍有摘录作业的对话）。

<a id="scheduling"></a>

## 6. 调度、预算、失败与重试

<a id="scheduling-queue"></a>

### 6.1 调度

- 后台通道是作业队列的唯一消费者：一个工作循环按「优先级，再按创建时间」取到期作业，逐个预检，第一个能跑的领取执行。截图摘录按截图拍摄时间排序，保证聊天消息按序追加。
- 优先级表（`candid-store/src/job_priority.rs`，一行一个作业类型）：P1 `screen.excerpt`、`items.extract`；P2 `speech.recognize`、`workflow.*`；P3 `screen.summarize`、`people.judge`。不在表里的作业类型不会被后台通道取走。交互调用（P0）不进队列。
- 工作循环不空转：只在作业、流程配置或数据的版本计数变化、授权变化、下一个到期时间到达时才取队列，兜底每 30 秒查一次。
- 触发作业入队的扫描（录音、整理层同步、人物、截图摘录、条目、活动块、自动化）在同一轮循环里先于取队列执行。
- 领取用租约（180 秒）与尝试号；崩溃后过期任务由下一轮重领；结果提交时核对尝试号、取消、来源修订、权限与配置哈希，迟到结果不能覆盖新结果。已完成的语音分块有检查点，重启后不重复计费。
- 作业状态：`queued`、`running`、`done`、`failed`、`cancelled`。等待条件不是单独状态，而是 `queued` 加 `error_code`（见 6.3），同一原因只记一次日志，不重复写库。

后台通道在交互调用进行时让路、电池低于 20% 暂停 P2/P3：未实现；两条通道目前互不感知。后台作业何时允许运行的规则正在另行制定。

<a id="budget"></a>

### 6.2 预算

| 项 | 规则 | 存在哪里 |
|---|---|---|
| 每功能每日调用上限 | 每个目录功能一个，设置页可改（0 至 10000）；本机与云端调用都计数，失败的也计数；日界为 UTC 日；超限作业等待 `waiting_feature_budget`，同类剩余作业本轮跳过 | `catalog.json` 的 `default_daily_calls`；`store_meta` 的 `features.settings/1`；`feature_budget()` 数 `ai_calls` |
| 单次输入上限 | 角色 `max_input_tokens`，超出拒绝 `input_limit`（核心以请求字节数近似比较） | `limits_json` |
| 单次输出上限 | 每步自带 `max_tokens`（默认 1024，上限 4096），角色 `max_output_tokens` 只能调低 | `limits_json` |
| 超时 | `timeout_ms`，默认 120000，限制在 50 至 120000 毫秒 | `limits_json` |
| 每日金额上限 | 角色 `daily_budget`：当日同一模型已记的费用加本次预留（输入字节数×输入单价＋输出上限×输出单价）超限则等待 `waiting_budget`；价格缺失、已有调用费用未知或币种不一致时等待 `waiting_budget_cost_unknown`，不当作零；语音和图像输入的金额无法预留，设了金额上限就等待 | `limits_json`；价格在 `cost_json` |
| Pi 每次运行 | 模型调用数、工具调用数、输出 token、请求字节数四项，开始时冻结 | 见 7.2 |

计划里的「云端另设全局每日金额上限，默认 0」未实现；现在只有按角色的 `daily_budget`，不设则不限金额。功能的 `allow_remote` 默认 `false`（`agent_chat` 以用户配置为准）。

超限后的处理：截图摘录留在队列里等次日额度，排队超过 24 小时作废（`expired`）；条目抽取、录音类、人物判断、活动块不按时间作废，作业留在队列，额度恢复后再跑（条目抽取下一次会合成覆盖更多消息的批次）。

<a id="retry"></a>

### 6.3 失败与重试

| 情况 | 处理 |
|---|---|
| 超时（`timeout`）、模型离线（`offline`） | 后台作业退避 30 秒、2 分钟、10 分钟再试，共最多 4 次尝试（3 次重试），之后 `failed`；原因在作业上，界面可见。交互调用不重试 |
| 其他 HTTP 错误、传输错误 | 不重试，作业 `failed`；活动块遇到 400/401/403/404/422 这类确定性拒绝时，封锁该配置，避免对每个排队块重复计费（`waiting_model_request_rejected`） |
| 要求 JSON 的步骤（条目抽取、人物判断）回复不含 JSON 对象 | 带坏回复和「只输出符合结构的 JSON」再问一次（`json_repair`），第二次回复按步骤自己的解析器判定 |
| 缺模型或来源未启用 | 等待 `waiting_model` |
| 密钥未交给核心、来源未批准 | 等待 `waiting_authorization` |
| 超功能上限 / 超金额上限 | 等待 `waiting_feature_budget`、`waiting_budget`、`waiting_budget_cost_unknown` |
| 功能关闭、来源不在允许范围、不允许远程 | `feature_disabled`、`waiting_feature_source`、`waiting_feature_remote`；重新开启只恢复被这个开关取消的作业 |
| 结果回来时来源已改变、权限撤销、配置或功能设置已变 | 丢弃结果，调用记 `cancelled` |
| 预检时输入已不成立（来源没了、需要的转写没有等） | 作废（`cancelled`，带原因），不重试 |

不静默改用远端；超时或中断的尝试不做模型回退。重试期间已落库的 `ai_calls` 行如实保留，每次尝试各一行。

<a id="ai-settings"></a>

## 7. 设置与管理

<a id="settings-where"></a>

### 7.1 设置入口

| 位置 | 管什么 |
|---|---|
| 设置 → 模型 | 服务商、模型、角色分配、回退、预算、网络代理 |
| 设置 → AI 功能 | 目录里每个功能一行：开关、读哪些来源、能否用远程模型、每日调用上限、上次运行与结果 |
| 连接页的每个来源 | 这个来源能不能给 AI 读、能不能发往远程模型 |

「AI 功能」的保存格式（`features.settings/1`，核心校验）：每个功能恰好四个字段 `enabled`、`sources`、`allow_remote`、`daily_calls`；`sources` 取 `import`、`mac_microphone`、`mac_system_audio`、`mac_region`，截图摘录、条目抽取、人物判断、活动块概括另可取 `*` 或具体来源编号。关闭功能时取消该功能排队与运行中的作业（`feature_disabled`）。

规则：

- 一个功能能读哪些来源，取功能设置与来源授权的交集；能不能用远程模型，取功能设置、来源授权、角色目的地三者中最严的。
- 保存配置同时取消受影响的待处理作业；迟到回复由租约和修订检查拒收。
- 每条 AI 产出都能看到谁生成的（规则、模型或 Pi）、依据哪些记录、什么时候，都能改、确认或拒绝；见[整理层](curation.md)。AI 页的对话、日程与技能呈现见[工作流](workflows.md)。

<a id="pi-gateway"></a>

### 7.2 Pi 的模型网关与运行预算

App 通过 `POST /local/pi/start` 开始一次运行：核心解析角色对应的模型，检查来源已启用、接口受支持、远程是否被允许，冻结作用域和预算，登记专用客户端 `candid-pi` 并返回令牌。Pi 扩展只经 `POST /local/pi/model` 调模型，模型、token 上限、密钥都是核心的；回答不流式。

| 预算项 | 默认 | 范围 |
|---|---|---|
| `max_model_calls` | 12 | 1–200 |
| `max_tool_calls` | 40 | 1–500 |
| `max_output_tokens` | 16000 | 256–400000 |
| `max_request_bytes` | 900000 | 10000–1000000 |

用尽后本次运行被停（`budget_exhausted`）；同一时刻只有一个运行（`agent_busy`）。技能自动运行（自动化）的预算按自动化配置冻结，并校验来源与远程授权。

<a id="app-side"></a>

### 7.3 平台端的模型调用

规则是平台端不直连模型。现状有偏差（未实现与待定里的前两项）：App 里的 `AIClient` 对云端语音转写直连服务商；App 另存一份模型配置和用量日志再用 `import_usage` 同步进核心。其余经 `POST /local/models/complete` 由核心代为调用的路径是目标形态。

<a id="code"></a>

## 8. 实现位置

| 内容 | 代码 | 测试 |
|---|---|---|
| 模型配置、保存校验、预检、用量记账、预算 | `core/crates/candid-store/src/workflows.rs` | `core/crates/candid-store/tests/workflows.rs` |
| 功能设置、功能预算、来源与远程检查 | `core/crates/candid-store/src/feature_settings.rs` | 同上 |
| 作业优先级与过期 | `core/crates/candid-store/src/job_priority.rs`、`performance.rs` | `candid-store/tests/job_scheduling.rs`、`performance.rs` |
| 后台通道、交互通道、HTTP 调用、检查点 | `core/crates/candid-service/src/workflows.rs` | `candid-service/tests/workflows.rs`、`lifecycle.rs` |
| 重试与 JSON 修复 | `core/crates/candid-service/src/retry.rs` | 同文件单元测试 |
| 后台 QoS | `core/crates/candid-service/src/scheduling.rs` | 同文件单元测试 |
| 本地通道 `/local/models…`、`/local/compose` | `core/crates/candid-service/src/local.rs` | `candid-service/tests/workflows.rs` |
| Pi 运行与模型网关 | `core/crates/candid-service/src/pi.rs` | `agents/pi-extension/test/e2e.mjs` |
| 网络代理设置（系统或直连，不是模型代理） | `core/crates/candid-service/src/proxy.rs` | `candid-service/tests/proxy.rs` |
| 技能自动运行的预算与来源 | `core/crates/candid-store/src/automation/` | 未验证 |
| 各调用的准备与回复校验 | `candid-store` 的 `screen_excerpt.rs`、`item_extract.rs`、`people/judge.rs`、`screen_pipeline.rs`、`compose.rs` | `lifecycle.rs`（模型为回环桩，输入输出存快照） |
| 功能目录、提示词、输出结构 | `core/workflows/catalog.json`、`prompts.json`、`outputs.schema.json`；`core/prompts/` | 快照 `core/fixtures/` |
| 表结构 | `candid-store/src/schema.rs`、`schema_upgrade.rs` | `cargo test --workspace` |

<a id="open"></a>

## 9. 未实现与待定

| 项 | 状态 | 说明 |
|---|---|---|
| 角色收敛为三个 | 未实现 | 核心与 App 仍用 `title`、`episode`、`asr`、`diarization`，App 目录另有 `ocr`、`relevance`、`embedding`；见 3.1 |
| Mac App 云端语音转写由 Swift 直连服务商 | 未实现 | 不经核心，不进 `ai_calls`；与「模型只由核心调用」不符，见[架构](../architecture.md#debts) |
| Swift 端另存模型配置和用量日志再同步进核心 | 未实现 | 应只在核心保存一份；见[架构](../architecture.md#debts) |
| 后台让路于交互、电池低于 20% 暂停 | 未实现 | 后台作业何时允许运行的规则正在另行制定 |
| 云端全局每日金额上限（默认 0） | 未实现 | 现只有按角色的 `daily_budget` |
| 帮我写的「当前帧摘录」与屏幕问答的图像输入 | 未实现 | 帮我写目前只读文字上下文 |
| 小时 / 周汇总（`hourly-weekly`） | 未实现 | 目录占位 |
| `ai_models` 能力探测列、`fallback_json` | 未实现 | 表里有列，核心不写不读；设计约束：`fallback_json` 只能回退到同一 `destination` 或更本地的来源，`limits_json` 含每次输入输出 token 上限与每日预算（旧文，未验证） |
| 后台调用的缓存 token 回写 | 未实现 | 列已存在 |
| 模型统计卡片、小图与额外的能力推理测试 | 未实现 | 旧文档写明未加入 |
| 重点邮件触发条目抽取 | 未验证 | 触发条件写在目录里，未逐项核对 |
| 活动块概括等待该时段摘录处理完 | 未验证 | |
| 每类调用的样例对比 | 部分实现 | `candid-lab` 与 `lifecycle` 快照可对比提示词或模型变化；旧文档设想的 `excerpt_replay` 子命令是否保留未验证 |
| 估算与上限以一周实测修正 | 设计中 | 按 `ai_calls` 的实际数调整 |
| 取消与并发的旧设计：取消先持久化、再取消外部模型请求；提供方无法取消时记录仍可能计费、迟到输出不发布；网络失败不假定未计费、先核对提供方请求状态；资源并发有统一上限、采集保存优先；同内容的两次录音不合并幂等键 | 未验证 | 来自旧智能平台 v1 §4；核心已有 `interrupted_unknown_charge` 与迟到结果丢弃（第 5、6 节），其余未逐项核对 |
| 上下文评估门槛达标前自动技能默认关闭 | 未验证 | 来自旧功能目录说明 |

<a id="sources"></a>

## 10. 来源

- [时间线 v1 §4.9「模型来源、角色与用量」](timeline-v1.md#ai-usage)：第 2 节、3.1（表与记账部分）。
- [数据层 v1 计划 §4「模型调用规划」](../plans/data-layer-v1.md)：第 4 节（调用清单）、第 5 节（依赖顺序）、第 1.2 节与 6.1（调度）、6.2（预算）、6.3（失败与重试）；§4.6 验证并入第 9 节。
- [智能平台 v1 §4「调度、成本与恢复」](agent-platform-v1.md)：第 6 节与 2.3（幂等、租约、预留与结算、取消）。
- [智能平台 v1 §4.2「设置与管理」](agent-platform-v1.md#ai-settings)：第 7.1 节。
