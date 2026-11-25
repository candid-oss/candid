# 数据位置与迁移

Candid 的数据只在本机。本文列出它写到磁盘上的每个位置，以及 `candid backup` / `restore` / `move-root` 带走什么、不带走什么。

`<支持目录>` = `~/Library/Application Support/Candid`。演示、自测和 QA 启动（`--mock`、`--data-dir`、`CANDID_RUN_DIR`）用临时目录，不在此列。

## 盘点

| 位置 | 内容 | 备份 | 说明 |
|---|---|---|---|
| `timeline/timeline.sqlite` | 时间线库（事件、派生、人物、声纹、工作流……） | ✅ 一致性快照 | 用 `VACUUM INTO` 从只读连接做快照，App 运行中也能做 |
| `timeline/timeline.sqlite-wal`、`-shm` | WAL 与共享内存 | ❌ | 已并入快照 |
| `timeline/store.json` | 库标识、schema 版本、布局 | ✅ | |
| `timeline/blobs/sha256/**` | 截图（WebP/PNG）、录音（AAC/m4a）原件 | ✅（`--no-media` 时不带） | 只带快照里 `blobs` 表引用的文件 |
| `timeline/media/*` | 媒体库导入的原件 | ✅（`--no-media` 时不带） | 只带 `media_objects` 引用的文件 |
| `timeline/prompts/*.md` | 核心提示词覆盖（`activity-block.md`、`item-extract.md`、`screen-excerpt.md`） | ✅ | |
| `timeline/staging`、`exports`、`logs`、`.lock` | 未提交批次、导出产物、日志、写锁 | ❌ | 启动时清空或重建 |
| `capture/preferences.json`、`capture/outbox/` | 采集设置、未投递的采集 | ✅ | |
| `conversations/*.json` | AI 对话 | ✅ | |
| `ai-providers/config.json` | 模型、提供方、角色（含 App 的角色提示词覆盖） | ✅ | 不含密钥，密钥在 Keychain |
| `ai-providers/ai_calls.jsonl` | AI 调用记账 | ✅ | |
| `MediaWorkspace/Recordings/`、`Pending/`、`audio-preferences.json`、`pending.json` | 录音会话与音频设置 | ✅ | |
| `MediaWorkspace/Hub/memory.sqlite`、`Hub/vault/**` | 旧媒体 Hub 库（已镜像进时间线） | ✅ | `*.sqlite` 一律做快照；`tmp_uploads`、`staging` 不带 |
| `MediaWorkspace/connection.json`、`hub.pid`、`hub.log`、`launch.lock` | Hub 地址与令牌、进程文件 | ❌ | 运行时重建；令牌不该带走 |
| `pi/sessions/*.jsonl`、`pi/agent/models-store.json` | Pi 代理会话与模型列表 | ✅ | |
| `pi/agent/auth.json` | Pi 的明文登录令牌 | ❌ | 在新机器重新登录 |
| `pi/home`、`pi/work`、`pi/pi.log` | 临时工作区、日志 | ❌ | |
| `run/`（`cli.token`、`endpoint.json`、`core.sock`、`core.log`） | 核心运行目录 | ❌ | 每次启动重建；令牌按机器发放 |
| `components/` | 可选组件（Node、Pi 等） | ❌ | 可在 设置 → 组件 重新安装（约 130 MB） |
| `widget/status.json` | 小组件快照 | ❌ | 随时重建 |
| `desktop/` | 旧桌面账本（已导入后只读保留） | ❌ | 已并入时间线库 |
| `Signing/` | 本地开发签名身份 | ❌ | 只与这台机器的开发构建有关 |
| `timeline-backup-*` 之类 | 手工或旧版留下的备份 | ❌ | |
| `~/Library/Preferences/io.github.candid-oss.candid.plist` | App 的 UserDefaults（外观、快捷键等） | ✅ | 用 `defaults export` 导出，恢复时 `defaults import` |
| Keychain：`io.github.candid-oss.candid.*` | AI 提供方密钥、连接器（邮件等）的令牌和授权 | ❌ | 绝不导出；新机器上重新填写或重新授权 |
| 仓库 `data/`、`.venv-asr-mlx/` | 本地语音服务的模型、日志、声纹库（开发用） | ❌ | 模型可重新下载；App 用到的声纹在时间线库里，已随库带走 |
| 系统授权（屏幕录制、辅助功能、麦克风、日历） | TCC | ❌ | 新机器上重新授权 |

## 命令

```bash
candid backup <目标>       [--no-media] [--json] [--support <支持目录>]
candid restore <备份目录>  [--move-existing] [--json] [--support <支持目录>]
candid move-root <新位置>  [--remove-old] [--json] [--support <支持目录>]
```

- **backup**：App 运行中也能做。目标不存在或是空目录时直接写进去，否则在里面新建 `candid-backup-<时间>`。先写到 `*.partial`，完成后才改名，所以没有 `.partial` 的目录就是完整备份。`manifest.json` 记录格式、Candid 版本、schema 版本、库 ID 和每个文件的字节数与 SHA-256。快照之后被删掉的原件记在 `missing` 里，不算失败。
- **restore**：先核对清单格式和 schema 版本（比当前核心新就拒绝），再逐个校验哈希，全部通过才动磁盘。时间线库要求核心没在运行（拿不到写锁就拒绝）。目标处已有同名数据时默认拒绝并列出冲突；加 `--move-existing` 会先把它们整体移到 `<支持目录>/before-restore-<时间>/`，绝不覆盖。最后以正常方式打开一次库：旧版本的库走现有的 schema 升级，启动恢复会报告缺失的原件。
- **move-root**：把时间线库（`<支持目录>/timeline`，占绝大部分空间）复制到新位置，逐个文件重新读回校验哈希，再把原位置改成指向新位置的符号链接，App 和核心不用改任何配置。原数据改名为 `timeline.moved-<时间>` 保留，`--remove-old` 才删除。移回原处：`candid move-root ~/Library/Application\ Support/Candid/timeline`。

`--json` 时每行输出一个 JSON：`{"phase":…,"done":…,"total":…}` 表示进度（字节），最后一行 `{"ok":true,…}` 或 `{"ok":false,"error":…}`。App 的 设置 → 数据 用的就是这个。

## 已知限制

- 备份是目录而不是单个压缩包；截图本身已压缩，再打包收益很小。要传输可以自己 `tar`/`ditto -c -k`。
- `--no-media` 的备份恢复后，事件仍在，但截图和录音会显示为缺失。
- `move-root` 只移动时间线库；其余目录（MediaWorkspace、对话等）合计通常只有几 MB，留在原处。外置盘没挂载时核心会打不开库，App 显示失败，插上盘重启即可。首次访问外置盘时 macOS 可能弹「可移除的宗卷」授权。
- 恢复 UserDefaults 时 App 自身仍在运行，App 会在恢复后重启以读到新值。
- Keychain 里的密钥和连接器授权、系统隐私授权需要在新机器上重新设置。
