"""Regenerate only the explicitly synthetic R1 evaluation input; no source library is opened."""
import datetime as dt
import json
from pathlib import Path

BASE = int(dt.datetime(2026, 11, 10, tzinfo=dt.timezone(dt.timedelta(hours=8))).timestamp() * 1000)
MINUTE = 60_000

def span(start, end):
    return {"start_ms": BASE + start * MINUTE, "end_ms": BASE + end * MINUTE}

def at(active_minutes):
    # 09–12 and 13–18: eight effective hours, plus one hour with explicit gaps.
    return BASE + (540 + active_minutes + (60 if active_minutes >= 180 else 0)) * MINUTE

visits = [{"id": f"visit-{i}", "object_id": f"synthetic:document-{i % 60}",
           "active": [{"start_ms": at(i * 4), "end_ms": at(i * 4) + 4 * MINUTE}]}
          for i in range(120)]
text = "预算 Plan\n通知 1\n通知 2\nOCR O\nOCR 0\n张伟（设计）\n张伟（工程）"
inputs = [{"id": f"input-{i}", "at_ms": at(i * 12) + 1000, "sent": i < 25,
           "expected_text": f"合成输入{i:02} 预算"} for i in range(40)]
items = [{"kind": "usage_segment", "span": {"start_ms": at(i * 8), "end_ms": at(i * 8) + 8 * MINUTE}} for i in range(60)]
for recording, start in enumerate([600, 900]):
    identity = f"synthetic-recording-{recording}"
    items.append({"kind": "audio_recording", "id": identity, "span": span(start, start + 30)})
    for minute in range(30):
        items.append({"kind": "audio_segment", "id": f"{identity}-segment-{minute}",
                      "parent_id": identity, "span": span(start + minute, start + minute + 1)})
        items.append({"kind": "speech_segment", "parent_id": identity,
                      "span": span(start + minute, start + minute + 1),
                      "text": f"（合成转写）预算讨论第 {minute + 1} 句。",
                      "speaker": f"anonymous-{minute % 2}"})
items += [{"kind": "calendar_item", "span": span(600 + i * 60, 630 + i * 60)} for i in range(3)]
items += [{"kind": "mail_item", "span": span(610 + i * 60, 611 + i * 60)} for i in range(4)]
items += [{"kind": kind, "span": span(start, end)} for kind, start, end in [
    ("gap.locked", 720, 740), ("gap.paused", 740, 760), ("gap.idle", 760, 780),
    ("gap.permission", 1076, 1080)]]
data = {"format": "candid-measurement/1", "synthetic": True,
        "observed": [span(540, 1080)], "context_items": items,
        "visits": visits, "effective": [span(540, 720), span(780, 1080)],
        "covered": [span(540, 720), span(780, 1076)],
        "inputs": inputs,
        "input_records": [{"at_ms": s["at_ms"], "session_id": s["id"], "visit_id": f"visit-{i * 3}",
                           "text": s["expected_text"], "truncated": False} for i, s in enumerate(inputs)],
        "text_records": [{"at_ms": at(i * 4), "visit_id": f"visit-{i}", "text": text} for i in range(119)],
        "shots": [{"at_ms": at(i * 3), "bytes": 100_000, "blob_id": f"synthetic-frame-{i}"} for i in range(150)],
        "resources": [{"span": span(540, 1080), "cpu_ms": 32400, "wakeups": 3240}],
        "searchable": [{"occurred_ms": at(i * 12), "searchable_ms": at(i * 12) + 2000,
                        "poll_interval_ms": 100} for i in range(40)], "counts": {}}
Path(__file__).with_name("ideal-day.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
