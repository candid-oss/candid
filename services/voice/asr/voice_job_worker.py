"""One inference process; its parent service may terminate its entire process group."""
import json
import os
from pathlib import Path
import sys


def main():
    root, mode = Path(sys.argv[1]), sys.argv[2]
    (root / 'work').mkdir()
    os.environ['CANDID_VOICE_WORKSPACE'] = str(root / 'work')
    os.environ['CANDID_VOICE_PROGRESS'] = str(root / 'progress.json')
    from .job_progress import report
    from .runtime import Runtime
    from .media_api import recognize_upload
    report('loading_model')
    runtime = Runtime()
    if mode == 'quick': runtime.load()
    result = recognize_upload(runtime, (root / 'input').read_bytes(), mode)
    report('finalizing')
    (root / 'result.json').write_text(json.dumps(result, ensure_ascii=False, allow_nan=False))


if __name__ == '__main__':
    main()
