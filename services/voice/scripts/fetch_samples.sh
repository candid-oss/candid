#!/bin/sh
# Download the public sample recordings used by tests into data/ (ignored by Git).
# Sources: Qwen3-ASR examples (github.com/QwenLM/Qwen3-ASR) and the pyannote-audio tutorial sample.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)"
mkdir -p data/asr data/speaker-demo/public-audio
fetch() { [ -s "$2" ] || curl -fsSL --proto '=https' -o "$2" "$1"; }
fetch https://qianwen-res.oss-cn-beijing.aliyuncs.com/Qwen3-ASR-Repo/asr_zh.wav data/speaker-demo/public-audio/chinese.wav
fetch https://qianwen-res.oss-cn-beijing.aliyuncs.com/Qwen3-ASR-Repo/asr_en.wav data/speaker-demo/public-audio/english.wav
fetch https://cdn.jsdelivr.net/gh/pyannote/pyannote-audio@develop/tutorials/assets/sample.wav data/speaker-demo/public-audio/conversation.wav
[ -s data/asr/sample-zh.wav ] || cp data/speaker-demo/public-audio/chinese.wav data/asr/sample-zh.wav
echo "Public samples are in data/asr and data/speaker-demo/public-audio."
