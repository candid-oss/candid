# Candid voice service

**English** · [中文](#中文)

An optional local speech service for Apple Silicon: transcription, speaker separation and a local voiceprint library. It runs as its own process on `127.0.0.1:8767`; the Candid core calls it over HTTP and never starts Python itself. Without it, Candid still records, stores and plays audio.

## Models

Downloaded on demand from Hugging Face at pinned revisions and verified by hash. They are not part of this repository; each model has its own license — check the model card before use.

| Role | Model |
|---|---|
| Transcription | `mlx-community/Qwen3-ASR-0.6B-8bit` |
| Conversation transcription with speakers | `vanch007/mlx-MOSS-Transcribe-Diarize-8bit` |
| Voiceprints | `Wespeaker/wespeaker-voxceleb-campplus-LM` (CAM++) |

## Setup

Requires an Apple Silicon Mac, [uv](https://docs.astral.sh/uv/) and about 1 GB of disk for the base model. Run from the repository root. The virtual environment is `.venv-asr-mlx/`; model files, logs and the voiceprint library go under `data/`. Both are ignored by Git.

```bash
sh services/voice/scripts/setup_qwen_asr.sh
```

```bash
sh services/voice/scripts/setup_speaker_demo.sh
```

```bash
.venv-asr-mlx/bin/python services/voice/scripts/local_asr.py start
```

`start`, `stop` and `status` manage the service through your user launchd; it does not start at login. Check readiness with `curl http://127.0.0.1:8767/health`. Logs are in `data/asr/server.log`. Foreground debugging: `local_asr.py serve`.

Public sample recordings used by some core tests: `sh services/voice/scripts/fetch_samples.sh`.

## API

| Endpoint | Purpose |
|---|---|
| `GET /health` | Readiness, backend and model |
| `POST /v1/audio/transcriptions` | OpenAI-style transcription (multipart `file`) |
| `POST /v1/voice` | `candid-voice-result/1`: `mode=quick` or `conversation`, segments with positions and anonymous speakers |
| `GET /v1/voice/capabilities`, `POST /v1/voice/jobs`, `GET`/`DELETE /v1/voice/jobs/{id}` | Cancellable file jobs (`candid-voice-jobs/1`) |
| `GET /v1/voiceprints`, `POST /v1/voiceprints/analyze`, `POST /v1/voiceprints/reconcile`, `POST /v1/voiceprints` | Local voiceprint library: analyze, match, enroll, rename, merge, delete |

Local use only: no authentication, TLS or CORS. Requests from browser origins are rejected. One inference runs at a time; a busy service returns 429 with a retry hint. Enrolling, merging and deleting people require explicit confirmation; names are never inferred from transcripts.

## Tests

```bash
.venv-asr-mlx/bin/python -m pytest -q services/voice/tests
```

Tests that need audio dependencies skip when they are not installed; a skip is not a pass.

---

## 中文

可选的本地语音服务（Apple Silicon）：转写、说话人分离和本机声纹库。它作为独立进程监听 `127.0.0.1:8767`，Candid 核心通过 HTTP 调用，自己不启动 Python。不装这个服务，Candid 照样能录音、保存和回放。

**模型**：按固定版本从 Hugging Face 按需下载并校验 hash，不包含在本仓库中；各模型有各自的许可证，使用前请查看模型卡。转写用 `mlx-community/Qwen3-ASR-0.6B-8bit`，带说话人的对话转写用 `vanch007/mlx-MOSS-Transcribe-Diarize-8bit`，声纹用 `Wespeaker/wespeaker-voxceleb-campplus-LM`（CAM++）。

**安装**：需要 Apple Silicon Mac、[uv](https://docs.astral.sh/uv/) 和约 1 GB 磁盘空间。在仓库根目录依次执行 `sh services/voice/scripts/setup_qwen_asr.sh`、`sh services/voice/scripts/setup_speaker_demo.sh`、`.venv-asr-mlx/bin/python services/voice/scripts/local_asr.py start`。虚拟环境在 `.venv-asr-mlx/`，模型、日志和声纹库在 `data/`，两者都不进 Git。`start`、`stop`、`status` 通过当前用户的 launchd 管理服务，不随登录自启；用 `curl http://127.0.0.1:8767/health` 检查是否就绪。部分核心测试用到的公开样例录音可用 `sh services/voice/scripts/fetch_samples.sh` 下载。

**接口**：见上方英文表格。仅供本机使用，没有认证、TLS 或跨域设置，浏览器来源的请求会被拒绝；同一时间只做一次推理，忙时返回 429 和重试间隔；登记、合并、删除人物都需要明确确认，不会从转写内容推断人名。

**测试**：`.venv-asr-mlx/bin/python -m pytest -q services/voice/tests`。缺少音频依赖时相关测试会跳过，跳过不算通过。
