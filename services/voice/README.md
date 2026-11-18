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
| `POST /v1/voiceprints/window`, `POST /v1/voiceprints/stitch` | Whole-recording speakers (`recording-speakers/2`): one window of up to 12 minutes per call, then one set of speakers for the recording |

### Whole recordings

The capture writes recordings as 30-second files so that a crash loses little; the files stay on disk but are not what people see. Once a recording stops, the core sends it to `/window` in windows of 10 minutes that overlap by 1 minute (16 kHz mono WAV plus `meta` = `{"scope","index"}`), saving each finished window in the job's checkpoint. Each window is one model call (about 20–30 s on Apple Silicon), so stopping loses at most the window in progress; a window whose transcript does not fit one call returns 422 and the core retries it at half the length. `/stitch` joins the windows: labels are matched by where they coincide in the overlap, with voiceprints only as a fallback, and the transcript of the overlap is taken from the window where it sits further from an edge. Acknowledgements shorter than 1 s (three characters or fewer) and labels with no usable voice anywhere are left out instead of being attributed. Voice vectors stay in the service; callers get opaque references.

Names come from the library. A speaker's voiceprint is computed once from up to 30 s of their clean speech joined together. With at least 8 s of speech a name is applied automatically at cosine 0.62 or more and a lead of 0.05 over the next person; with 3–8 s the closest person is only offered as a suggestion; under 3 s there is no verdict. References stored before speech length was recorded keep the earlier 0.70 bar.

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

**整段录音**：采集仍按 30 秒一个文件写盘（防丢），但用户看到的是整段录音。录音停止后，核心把整段录音按 10 分钟一个窗口、相邻重叠 1 分钟送到 `/window`，每个窗口一次模型调用（约 20–30 秒），完成一个就存进任务进度，中断最多丢正在算的那个窗口；窗口内容太密放不下时返回 422，核心把窗口减半重试。`/stitch` 用重叠那段时间对齐各窗口的说话人，声纹只做后备；不足 1 秒的短应答（三个字以内）和整段都没有可用声音的标签直接去掉，不硬归到某人名下。名字从声纹库自动匹配：每位说话人的声纹用最多 30 秒拼接语音算一次；语音满 8 秒、分数不低于 0.62 且领先第二名 0.05 才自动标名字，3–8 秒只提示「可能是」，不足 3 秒不下结论。

**接口**：见上方英文表格。仅供本机使用，没有认证、TLS 或跨域设置，浏览器来源的请求会被拒绝；同一时间只做一次推理，忙时返回 429 和重试间隔；登记、合并、删除人物都需要明确确认，不会从转写内容推断人名。

**测试**：`.venv-asr-mlx/bin/python -m pytest -q services/voice/tests`。缺少音频依赖时相关测试会跳过，跳过不算通过。
