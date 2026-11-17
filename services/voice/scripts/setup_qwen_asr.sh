#!/bin/sh
# Reproduce the tested Apple Silicon Python environment and verify model hashes.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)"
uv venv --python 3.12 --allow-existing .venv-asr-mlx
uv pip sync --python .venv-asr-mlx/bin/python services/voice/asr/requirements.lock.txt
.venv-asr-mlx/bin/python services/voice/scripts/local_asr.py download
