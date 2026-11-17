"""Optional per-process progress sink, enabled only by the service's isolated worker."""
import json
import os
from pathlib import Path
import time


def report(stage, completed_seconds=None, total_seconds=None):
    path = os.environ.get('CANDID_VOICE_PROGRESS')
    if not path:
        return
    value = {'stage': stage, 'updated_ms': int(time.time() * 1000)}
    if completed_seconds is not None: value['completed_seconds'] = completed_seconds
    if total_seconds is not None: value['total_seconds'] = total_seconds
    target = Path(path); temp = target.with_suffix('.tmp')
    temp.write_text(json.dumps(value)); temp.replace(target)
