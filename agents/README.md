# Agents

**English** · [中文](#中文)

Candid uses AI in two ways ([agent platform](../docs/specs/agent-platform-v1.md)):

1. **Built-in processing** inside the core: transcription, speakers, titles and summaries, screenshot text, extraction.
2. **An attached agent** for conversations on the AI page and skill runs: [Pi](https://www.npmjs.com/package/@earendil-works/pi-coding-agent), launched with Candid's extension as its *only* tools and model. External agents (Claude Code, Codex, …) connect through MCP instead, with the same tools and permissions.

## Layout

| Path | Contents |
|---|---|
| `pi-extension/` | The extension (`index.ts`), system prompt and the pinned Pi version (`package-lock.json`) |
| `skills/` | Skill texts shared by Pi and external agents (front matter: title, summary, trigger, enabled, input) |

Every tool call goes to the core with the run's own token, so scope checks, budgets and the access log apply. Writes are recorded as corrections or AI drafts and can be undone; external actions are only proposals until you approve them. Content returned from your records is wrapped as data, and instructions inside it are not followed.

## Development

```bash
npm ci --prefix agents/pi-extension --ignore-scripts
```

```bash
cargo build --manifest-path core/Cargo.toml -p candid-cli
```

```bash
node agents/pi-extension/test/e2e.mjs
```

The end-to-end test starts a real core, the pinned Pi over RPC and a scripted local mock model in a temporary directory, and checks reads, writes and undo, proposals, out-of-scope refusals, ignored instructions in data, cancellation and call accounting. No real model is used.

---

## 中文

Candid 用两种方式使用 AI（见 [agent 平台](../docs/specs/agent-platform-v1.md)）：

1. **核心内置处理**：转写、说话人、标题与摘要、截图文字、信息抽取。
2. **挂接的 agent**：AI 页的对话和技能运行由 [Pi](https://www.npmjs.com/package/@earendil-works/pi-coding-agent) 执行，启动时只给它 Candid 扩展提供的工具和模型。外部 agent（Claude Code、Codex 等）通过 MCP 接入，使用同一套工具和权限。

`pi-extension/` 是扩展本体（`index.ts`）、系统提示和锁定的 Pi 版本；`skills/` 是 Pi 与外部 agent 共用的技能正文。每次工具调用都带着本次运行的令牌发给核心，因此范围检查、预算和访问日志都生效；写入记为纠正或 AI 草稿，可以撤销；对外动作在你批准前只是提案；从记录中读出的内容按数据包装，其中的指令不会被执行。

**开发**：依次执行 `npm ci --prefix agents/pi-extension --ignore-scripts`、`cargo build --manifest-path core/Cargo.toml -p candid-cli`、`node agents/pi-extension/test/e2e.mjs`。端到端测试在临时目录中启动真实核心、固定版本的 Pi 和脚本化的本地模拟模型，逐项检查读写与撤销、提案、越界拒绝、数据中指令不执行、取消和调用记录，不调用任何真实模型。
