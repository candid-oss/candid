# Security

**English** · [中文](#中文)

Candid records sensitive material — screens, text, audio — so we treat security reports as a priority.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private vulnerability reporting: on this repository, open **Security → Report a vulnerability**. Include the version or commit, steps to reproduce and the impact you observed. We aim to acknowledge reports within 7 days.

## What Candid protects, and how

| Asset | Protection |
|---|---|
| Recorded data (timeline, screenshots, audio) | Stays on the Mac under `~/Library/Application Support/Candid/`; no telemetry, no cloud service of ours |
| The local API | The app's channel is a Unix socket (file 0600, directory 0700); loopback HTTP listens on `127.0.0.1` only and refuses every `/local/…` endpoint |
| AI clients | Each client has its own token and scope; every call is audited; revoking a client invalidates its token |
| The attached agent | Runs with Candid's tools only and a per-run token; writes are undoable; external actions are proposals until approved; instructions found inside recorded data are not followed |
| Secrets (API keys, IMAP passwords) | macOS Keychain |
| Capture | Starts paused at every launch; a visible menu bar indicator; password fields, private windows and excluded apps are skipped |

## Out of scope / known limits

- Someone with access to your unlocked user account can read your data. Use FileVault and a strong login password.
- Deleted data is not securely erased from SSD or SQLite free pages.
- The optional voice service has no authentication and must never be exposed beyond `127.0.0.1`.
- If you configure a remote model provider, the content sent for that feature leaves your machine under that provider's terms.

---

## 中文

Candid 记录的是屏幕、文字和声音这类敏感内容，所以我们优先处理安全问题。

**报告漏洞**：请**不要开公开 issue**。在本仓库点 **Security → Report a vulnerability** 私下提交，写明版本或提交、复现步骤和影响。我们会在 7 天内回复。

**保护措施**：数据只保存在本机 `~/Library/Application Support/Candid/`，没有遥测，也没有我们的云服务；App 与核心之间走 Unix 套接字（文件 0600、目录 0700），回环 HTTP 只监听 `127.0.0.1` 并拒绝所有 `/local/…` 接口；每个 AI 客户端有独立令牌和范围，每次调用都有审计，撤销后令牌立即失效；挂接的 agent 只有 Candid 的工具和本次运行的令牌，写入可撤销，对外动作需要你批准，记录里的指令不会被执行；API 密钥和 IMAP 密码存在钥匙串；每次启动默认暂停采集，菜单栏状态始终可见，密码框、隐私窗口和排除的应用不记录。

**已知边界**：能使用你已解锁账户的人可以读到数据，请开启 FileVault；删除不保证从 SSD 或 SQLite 空闲页中安全擦除；可选的语音服务没有认证，不能暴露到 `127.0.0.1` 之外；如果你为某个功能配置了远程模型，发送给它的内容会按该服务商的条款离开本机。
