#!/bin/sh
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)"
uv venv --python 3.12 --allow-existing .venv-asr-mlx
# Install without syncing away the running ASR/debugger's other dependencies.
uv pip install --python .venv-asr-mlx/bin/python -r services/voice/asr/requirements-speaker.lock.txt
.venv-asr-mlx/bin/python services/voice/scripts/speaker_demo.py setup
