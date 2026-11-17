# Candid for macOS

**English** · [中文](#中文)

The SwiftUI app: menu bar status, timeline, recording, screenshots with OCR, settings, the AI page and connectors. It bundles and runs the [Rust core](../../core/README.md) as a helper process; all data lives in the core's timeline library. UI rules: [design system](../../docs/specs/design-system-v1.md).

## Requirements

- macOS 14 or later (developed on recent macOS; 14 is not yet verified on hardware)
- Xcode 26 or later (Swift 6.2+; CI builds with Xcode 26.3), Rust 1.88+
- Python 3 and OpenSSL, used only by the build/signing scripts
- Node.js, only for the agent tests (users install Node and Pi from Settings → Components)

## Build and run

Run from the repository root.

```bash
apps/macos/script/build_and_run.sh
```

| Option | Effect |
|---|---|
| *(none)* | Build a Release app into `apps/macos/dist/Candid.app` and open it. Capture starts paused. |
| `--mock` | Isolated synthetic timeline with a temporary core; no real capture, no permission prompts |
| `--mock --dark` | Synthetic timeline in dark mode, without changing the system appearance |
| `--build-only` | Build and package without launching |
| `--debug` | Debug build under LLDB |
| `--logs` | Stream this app's system log |

## Components

Node and Pi for the AI page are **not bundled** (they would triple the app's size). Settings → Components installs them on demand into `~/Library/Application Support/Candid/components/`: a Node 22.19+ already on the system is reused, otherwise the pinned Node is downloaded from nodejs.org and checked against its SHA-256; Pi is installed from the extension's lockfile with `npm ci --ignore-scripts`. A Pi you installed yourself is detected and its compatibility shown, but the AI page always launches the pinned copy, so its isolation flags and the extension match. The same page shows whether the optional [voice service](../../services/voice/README.md) is running.

## Signing

There is no Developer ID signing or notarization yet. The build script signs with a **stable local certificate** (`script/local_signing.py`) created on first build in `~/Library/Application Support/Candid/Signing/` (directory 0700, files 0600, never committed). Because the certificate stays the same, macOS keeps Screen Recording and Accessibility permissions across rebuilds. If you download an unsigned build instead, open it via System Settings → Privacy & Security → Open Anyway, and expect to grant permissions again after each update.

## Data and privacy

- Timeline library: `~/Library/Application Support/Candid/timeline/` (screenshots under `blobs/sha256/`, verified by hash on ingest).
- Capture settings and the write-ahead queue: `~/Library/Application Support/Candid/capture/`.
- Every launch starts paused. Automatic screenshots are off until you enable them and choose apps. Password fields, private windows and excluded apps are skipped.
- No telemetry. Nothing leaves the machine unless you configure a remote model provider for a feature.

## Tests

```bash
apps/macos/script/test.sh
```

Runs the style check, a recording-recovery check, builds the core and runs all Swift tests against an isolated run directory. The agent tests need `npm ci` in [agents/pi-extension](../../agents/README.md) first.

---

## 中文

SwiftUI 客户端：菜单栏状态、时间线、录音、截图与文字识别、设置、AI 页和连接器。App 内置并以辅助进程运行 [Rust 核心](../../core/README.md)，所有数据保存在核心的时间线库中。界面规则见[设计系统](../../docs/specs/design-system-v1.md)。

**环境**：macOS 14+（14 尚未实机验证）、Xcode 26 或更新版本（Swift 6.2+）、Rust 1.88+；Python 3 与 OpenSSL 只在构建签名脚本中使用；Node.js 只用于 AI 页的 agent 测试。

**构建与启动**（在仓库根目录执行）：`apps/macos/script/build_and_run.sh`，产物在 `apps/macos/dist/Candid.app`，启动后默认暂停采集。`--mock` 使用隔离的合成时间线，不做真实采集也不请求权限；`--mock --dark` 为深色合成界面；`--build-only` 只构建打包；`--debug` 用 Debug 构建进入 LLDB；`--logs` 查看本 App 日志。

**组件**：AI 页需要的 Node 和 Pi **不随 App 打包**，在「设置 → 组件」里按需安装到 `~/Library/Application Support/Candid/components/`：系统已有 Node 22.19+ 时直接使用，否则从 nodejs.org 下载固定版本并校验 SHA-256；Pi 按扩展的锁文件用 `npm ci --ignore-scripts` 安装。你自己装的 Pi 会被识别并显示是否兼容，但 AI 页始终启动固定版本的副本，保证隔离参数与扩展一致。同一页面还显示可选的[语音服务](../../services/voice/README.md)是否在运行。

**签名**：目前没有 Developer ID 签名和公证。构建脚本用**固定的本地证书**签名（`script/local_signing.py`），首次构建时生成在 `~/Library/Application Support/Candid/Signing/`（目录 0700、文件 0600，不进 Git）。证书不变，重新构建后屏幕录制和辅助功能权限会保留。如果使用未签名的下载包，需要在「系统设置 → 隐私与安全性」里点「仍要打开」，并且每次更新后可能要重新授权。

**数据与隐私**：时间线库在 `~/Library/Application Support/Candid/timeline/`（截图在 `blobs/sha256/`，入库时校验 hash）；采集设置与待写队列在 `Candid/capture/`。每次启动默认暂停；自动截图需要你主动开启并选择应用；密码框、隐私窗口和排除的应用不会记录。没有遥测；除非你为某个功能配置了远程模型，否则数据不会离开本机。

**测试**：`apps/macos/script/test.sh` 运行样式检查、录音恢复检查，构建核心并在隔离的运行目录下运行全部 Swift 测试。agent 相关测试需要先在 [agents/pi-extension](../../agents/README.md) 执行 `npm ci`。
