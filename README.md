# Candid

> Work in progress — not yet released. / 开发中，尚未发布。

**English** · [中文](#中文)

Candid is a local-first capture and context foundation for macOS. It records what you choose — desktop activity, screenshots, audio — into a timeline on your own machine, organizes it with sources and evidence kept, and lets the AI of your choice read it through scoped, audited access (MCP and a local API).

- **Local first.** Data stays on your Mac. No telemetry, no account.
- **Visible capture.** Every launch starts paused; the menu bar dot always shows whether Candid is recording.
- **Evidence over guesses.** Raw records, episodes and memories are kept apart; AI inferences carry their source, model and confidence.
- **Scoped access.** Each AI client gets its own token and scope; every query is audited and revocable.

## Repository layout

| Path | Contents |
|---|---|
| `core/` | Rust core: timeline store, query service, MCP server and `candid` CLI |
| `apps/macos/` | SwiftUI app; runs the core as its helper process |
| `agents/` | Agent extension and skills bundled into the app |
| `services/voice/` | Optional local speech service (ASR, speaker separation) for Apple Silicon |
| `docs/` | Design documents ([index](docs/README.md)) |

## Build

Requires macOS 14+, Apple Silicon recommended, Rust 1.88+ and a Swift 6 toolchain.

```bash
cd core && cargo test --workspace
```

```bash
apps/macos/script/build_and_run.sh
```

Module guides: [core](core/README.md) · [macOS app](apps/macos/README.md) · [voice service](services/voice/README.md) · [agents](agents/README.md)

## Coming from Personavia

If you ran the earlier private builds (Personavia / Personal AI), quit them and copy your library into Candid's folder. The original is only read:

```bash
python3 scripts/migrate-from-personavia.py --dry-run
```

```bash
python3 scripts/migrate-from-personavia.py
```

## License

[Apache-2.0](LICENSE)

---

## 中文

Candid 是一个本地优先的 macOS 采集与上下文底座。它把你选择记录的内容——桌面活动、截图、录音——存进你自己电脑上的时间线，保留来源与证据进行整理，再通过有范围、可审计的接口（MCP 与本地 API）交给你选择的 AI 使用。

- **本地优先**：数据只在你的 Mac 上，没有遥测，不需要账号。
- **采集可见**：每次启动默认暂停，菜单栏的圆点始终显示是否在记录。
- **证据优先**：原始记录、片段与记忆分层保存；AI 推断保留来源、模型与置信度。
- **按范围授权**：每个 AI 客户端有独立令牌和范围，每次查询都有审计，随时可撤销。

**目录**：`core/` Rust 核心；`apps/macos/` SwiftUI App；`agents/` agent 扩展与技能；`services/voice/` 可选的本地语音服务；`docs/` 设计文档。构建命令见上文，各模块说明：[核心](core/README.md) · [Mac App](apps/macos/README.md) · [语音服务](services/voice/README.md) · [Agents](agents/README.md)。**从 Personavia 迁移**：如果用过早期的私有版本（Personavia / Personal AI），先退出旧 App，再运行 `python3 scripts/migrate-from-personavia.py`（可先加 `--dry-run` 预览），它只读取原资料，把副本整理到 Candid 的目录。许可证：[Apache-2.0](LICENSE)。
