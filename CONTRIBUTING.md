# Contributing

**English** · [中文](#中文)

Thanks for helping. Candid handles people's most private data, so changes are held to a few firm rules.

## Ground rules

1. **No surprise capture.** Capture has a visible state and a pause; nothing records or uploads without the user turning it on.
2. **Local first.** No telemetry, no analytics, no network calls the user did not configure.
3. **Evidence over guesses.** Raw records, episodes and memories stay separate; AI output keeps its source, model and confidence.
4. **No real personal data in the repository.** Tests use synthetic data, clearly marked. Never commit recordings, screenshots, databases, models or secrets.
5. **Schema changes** bump the version and come with an upgrade test (including failure and rollback).
6. **New endpoints** get tests for the normal, rejected and retried paths.

## Build and test

| Part | Command |
|---|---|
| Rust core | `cd core && cargo test --workspace` |
| macOS app | `apps/macos/script/test.sh` (needs `npm ci --prefix agents/pi-extension --ignore-scripts` for the agent tests) |
| Voice service | `.venv-asr-mlx/bin/python -m pytest -q services/voice/tests` |

`scripts/check.sh` picks and runs the checks your change needs (`--list` previews them, `--all` runs everything).
Design principles, code rules and the definition of done are in [docs/engineering.md](docs/engineering.md) (Chinese);
what counts as a contract is listed in [docs/contracts](docs/contracts/README.md).

Optional public speech samples for the core's audio tests: `sh services/voice/scripts/fetch_samples.sh`.

CI on pushes and pull requests runs the basic checks only: Rust core, voice service (on Linux, without MLX), licenses
and a secret scan. The macOS app suite does not run there; run `apps/macos/script/test.sh` before a PR that touches
`apps/macos`, or start it on GitHub with Actions → CI → Run workflow.

## Pull requests

- Keep a PR to one topic, and describe what you changed and how you verified it.
- Sign off your commits to certify the [Developer Certificate of Origin](https://developercertificate.org/): `git commit -s`.
- Update the module README or design doc that your change affects.
- Security issues go through [SECURITY.md](SECURITY.md), not public issues.

By contributing you agree that your contributions are licensed under [Apache-2.0](LICENSE).

---

## 中文

感谢参与。Candid 处理的是最私密的个人数据，因此有几条硬规则：

1. **不做意外采集**：采集要有可见状态和暂停；用户没有开启就不记录、不上传。
2. **本地优先**：没有遥测、统计，也没有用户未配置的网络请求。
3. **证据优先**：原始记录、片段与记忆分开保存；AI 输出保留来源、模型与置信度。
4. **仓库里不放真实个人数据**：测试使用明确标记的合成数据；不提交录音、截图、数据库、模型或密钥。
5. **表结构变更**要升级版本，并带升级测试（含失败回滚）。
6. **新增接口**要测试正常、拒绝和重试三种路径。

**构建与测试**：见上方英文表格；`scripts/check.sh` 会按改动自动选择并执行要跑的检查。设计原则、代码规范和完成的定义见[工程规范](docs/engineering.md)，对外契约见[契约清单](docs/contracts/README.md)。**提交 PR**：一个 PR 只做一件事并写明验证方式；用 `git commit -s` 签署 [DCO](https://developercertificate.org/)；同步更新受影响的模块 README 或设计文档；安全问题按 [SECURITY.md](SECURITY.md) 私下报告。贡献内容按 [Apache-2.0](LICENSE) 授权。
