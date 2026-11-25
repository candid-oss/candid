"""Whole-recording transcription and speakers, in long windows that can each be resumed.

The diarization model keeps its speaker labels consistent within one call and handles many minutes at a
time (measured: 28 minutes in one pass), so a recording is processed in windows of several minutes instead
of the capture's 30-second files. Neighbouring windows overlap; the same speech is labelled in both, so
labels are joined by where they coincide in time. Voiceprints are only a fallback for joining and the means
of naming people from the library. Vectors stay in this service; callers get opaque references.
"""
from __future__ import annotations

import hashlib
import io
import json
import math
import re
from collections import defaultdict

import numpy as np

from .speaker_models import MOSS_ID, MOSS_REV, VOICE_ID, VOICE_REV, unit
from .speaker_pipeline import clean_intervals, pooled_clip

WINDOW_FORMAT = "speaker-window/1"
RESULT_FORMAT = "recording-speakers/2"
VERSION = "recording-windows/1"
RATE = 16000
MAX_WINDOW_SECONDS = 12 * 60
WINDOW_TOKENS = 16384
# Short acknowledgements are where the model's speaker labels are unreliable; with no voice evidence of
# their own they are left out instead of being attributed to someone.
SHORT_SECONDS = 1.0
SHORT_CHARACTERS = 3
UNVOICED_SECONDS = 1.5
# Joining labels of neighbouring windows.
OVERLAP_MIN_SECONDS = 1.0
OVERLAP_SHARE = 0.6
VOICE_JOIN_LONG = 0.62     # both sides have 8 s or more of joined speech
VOICE_JOIN_SHORT = 0.72    # at least 3 s on both sides
VOICE_SAME_WINDOW = 0.80   # two labels in one window are one person only when nearly identical
VOICE_MARGIN = 0.05


def _hash(*parts):
    return hashlib.sha256(json.dumps(parts, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


def _characters(text):
    return len(re.sub(r"[\s\W_]+", "", text or ""))


def read_window(data):
    """16 kHz mono PCM WAV from the caller; nothing else is accepted."""
    import soundfile as sf
    with sf.SoundFile(io.BytesIO(data)) as sound:
        if sound.channels != 1 or sound.samplerate != RATE:
            raise ValueError("Window audio must be 16 kHz mono")
        if not 0 < sound.frames <= (MAX_WINDOW_SECONDS + 1) * RATE:
            raise ValueError("Window audio is empty or too long")
        audio = sound.read(dtype="float32")
    if not np.isfinite(audio).all():
        raise ValueError("Window audio contains invalid samples")
    return audio


def recognize_window(audio, models, refs, scope, index):
    """Transcribe one window in a single model call and keep one joined voiceprint per local speaker."""
    if not isinstance(scope, str) or not 1 <= len(scope) <= 128 or type(index) is not int or not 0 <= index < 10000:
        raise ValueError("Invalid window identity")
    duration = len(audio) / RATE
    result = {"format": WINDOW_FORMAT, "duration": duration, "segments": [], "speakers": {}}
    if not len(audio) or np.sqrt(np.mean(audio ** 2)) < 1e-4:
        return result
    raw = models.transcribe(audio, max_tokens=WINDOW_TOKENS)
    segments = []
    for entry in raw:
        start, end = float(entry["start"]), float(entry["end"])
        local = entry.get("speaker_id")
        if not local or not math.isfinite(start) or not math.isfinite(end) or not 0 <= start < end <= duration + 0.5:
            raise ValueError("Model returned missing speaker labels or invalid timestamps")
        text = re.sub(r"^\[S\d+\]\s*", "", entry.get("text") or "").strip()
        if text:
            segments.append({"start": start, "end": min(end, duration), "text": text, "speaker": str(local)})
    for local in dict.fromkeys(s["speaker"] for s in segments):
        others = [(t["start"], t["end"]) for t in segments if t["speaker"] != local]
        pieces = [p for s in segments if s["speaker"] == local for p in clean_intervals(s["start"], s["end"], others)]
        clip, _, total = pooled_clip(audio, pieces)
        vector = models.embedding(clip) if len(clip) >= 1.5 * RATE else None
        ref = None
        if vector is not None:
            ref = _hash(VERSION, "window", scope, index, local)[:32]
            refs.put(ref, unit(vector), seconds=len(clip) / RATE)
        result["speakers"][local] = {"ref": ref, "seconds": round(total, 3),
                                     "voice_seconds": round(len(clip) / RATE, 3) if ref else 0.0}
    result["segments"] = segments
    return result


QUICK_SECONDS = 30


def _wav(samples):
    import soundfile as sf
    buffer = io.BytesIO()
    sf.write(buffer, samples, RATE, format="WAV", subtype="PCM_16")
    return buffer.getvalue()


def transcribe_window(audio, engine, scope, index):
    """Text only (`mode=quick`): one window in 30-second pieces on the small ASR model, no speakers or voiceprints.

    The model gives no timestamps, so each piece is one segment spanning the piece."""
    if not isinstance(scope, str) or not 1 <= len(scope) <= 128 or type(index) is not int or not 0 <= index < 10000:
        raise ValueError("Invalid window identity")
    from .job_progress import report
    duration = len(audio) / RATE
    segments = []
    step = QUICK_SECONDS * RATE
    for start in range(0, len(audio), step):
        piece = audio[start:start + step]
        report("transcribing", start / RATE, duration)
        if not len(piece) or np.sqrt(np.mean(piece ** 2)) < 1e-4:
            continue
        text = (engine.transcribe(_wav(piece)).get("text") or "").strip()
        if text:
            segments.append({"start": start / RATE, "end": (start + len(piece)) / RATE, "text": text, "speaker": None})
    return {"format": WINDOW_FORMAT, "mode": "quick", "duration": duration, "segments": segments, "speakers": {}}


def _checked(request):
    if not isinstance(request, dict) or not isinstance(request.get("scope"), str) or not 1 <= len(request["scope"]) <= 128 \
            or not isinstance(request.get("revision"), str) or len(request["revision"]) != 64:
        raise ValueError("Invalid recording request")
    windows = request.get("windows")
    if not isinstance(windows, list) or not 1 <= len(windows) <= 1000:
        raise ValueError("Invalid recording windows")
    quick = request.get("mode", "conversation") == "quick"
    if request.get("mode", "conversation") not in ("quick", "conversation"):
        raise ValueError("Invalid recording mode")
    previous_end = None
    for w in windows:
        if not isinstance(w, dict) or any(type(w.get(k)) not in (int, float) or not math.isfinite(w[k]) for k in ("start", "end")) \
                or not 0 <= w["start"] < w["end"] or not isinstance(w.get("result"), dict) \
                or w["result"].get("format") != WINDOW_FORMAT:
            raise ValueError("Invalid recording window")
        if previous_end is not None and not w["start"] <= previous_end:
            raise ValueError("Recording windows must overlap or touch")
        previous_end = w["end"]
        for s in w["result"].get("segments", []):
            speaker_ok = s.get("speaker") is None if quick else isinstance(s.get("speaker"), str)
            if not isinstance(s, dict) or not speaker_ok or not isinstance(s.get("text"), str) \
                    or any(type(s.get(k)) not in (int, float) or not math.isfinite(s[k]) for k in ("start", "end")) \
                    or not 0 <= s["start"] < s["end"]:
                raise ValueError("Invalid window segment")
    entries = request.get("entries", [])
    if not isinstance(entries, list) or len(entries) > 20000:
        raise ValueError("Invalid speaker entries")
    return windows, entries


def _cuts(windows):
    """Each stretch of speech is taken from the window where it sits furthest from an edge."""
    return [None] + [(windows[i]["start"] + windows[i - 1]["end"]) / 2 for i in range(1, len(windows))] + [None]


def _owned(cuts, wi, a, b):
    middle = (a + b) / 2
    return not ((cuts[wi] is not None and middle < cuts[wi]) or (cuts[wi + 1] is not None and middle >= cuts[wi + 1]))


def stitch_text(request, windows):
    """Text-only recordings (`mode=quick`): windows joined at the middle of each overlap, no speakers."""
    from .runtime import MODEL_ID, MODEL_REVISION
    cuts, segments = _cuts(windows), []
    for wi, w in enumerate(windows):
        for s in w["result"].get("segments", []):
            a, b = w["start"] + s["start"], min(w["end"], w["start"] + s["end"])
            if b > a and _owned(cuts, wi, a, b):
                segments.append({"start_seconds": round(a, 3), "end_seconds": round(b, 3), "text": s["text"],
                                 "speaker": None, "label": None, "voice_ref": None, "match": None,
                                 "correction_key": None})
    segments.sort(key=lambda s: (s["start_seconds"], s["end_seconds"]))
    return {"format": RESULT_FORMAT, "mode": "quick", "revision": request["revision"], "segments": segments,
            "speakers": [], "speaker_count": 0, "unresolved_segments": 0, "dropped_segments": 0, "voice_refs": [],
            "producer": {"model_id": MODEL_ID, "model_revision": MODEL_REVISION, "processor_version": VERSION}}


def stitch(request, library, refs):
    """Join window labels into one set of speakers for the whole recording, then name them from the library."""
    windows, entries = _checked(request)
    if request.get("mode") == "quick":
        return stitch_text(request, windows)
    scope, revision = request["scope"], request["revision"]
    vectors = refs.entries([sp["ref"] for w in windows for sp in w["result"].get("speakers", {}).values()
                            if isinstance(sp, dict) and sp.get("ref")])

    def voice(w, local):
        sp = w["result"].get("speakers", {}).get(local) or {}
        found = vectors.get(sp.get("ref"))
        return (unit(found[0]), float(found[1] or 0)) if found else (None, 0.0)

    speakers = []           # {"members": [(window, local)], "vectors": [(v, seconds)]}
    owner = {}              # (window, local) -> speaker index
    placed = []             # per window: [(abs start, abs end, speaker index)]

    def joined(index):
        found = speakers[index]["vectors"]
        if not found:
            return None, 0.0
        v = np.average([v for v, _ in found], axis=0, weights=[max(s, 1e-3) for _, s in found])
        return unit(v), sum(s for _, s in found)

    def accepts(score, a, b):
        shortest = min(a, b)
        return score >= VOICE_JOIN_LONG if shortest >= 8 else score >= VOICE_JOIN_SHORT if shortest >= 3 else False

    for wi, w in enumerate(windows):
        segs = [(w["start"] + s["start"], min(w["end"], w["start"] + s["end"]), s["speaker"]) for s in w["result"].get("segments", [])]
        locals_ = list(dict.fromkeys(local for _, _, local in segs))
        taken = {}
        if wi:
            low, high = w["start"], windows[wi - 1]["end"]
            together = defaultdict(lambda: defaultdict(float))
            for a, b, local in segs:
                for x, y, target in placed[wi - 1]:
                    seconds = min(b, y, high) - max(a, x, low)
                    if seconds > 0:
                        together[local][target] += seconds
            for local in sorted(together, key=lambda l: -sum(together[l].values())):
                target, seconds = max(together[local].items(), key=lambda kv: kv[1])
                if seconds >= OVERLAP_MIN_SECONDS and seconds / sum(together[local].values()) >= OVERLAP_SHARE \
                        and target not in taken.values():
                    taken[local] = target
        for local in locals_:
            if local in taken:
                continue
            v, seconds = voice(w, local)
            if v is not None:
                options = []
                for index in range(len(speakers)):
                    other, other_seconds = joined(index)
                    if other is None:
                        continue
                    score = float(v @ other)
                    in_window = index in taken.values()
                    if (score >= VOICE_SAME_WINDOW if in_window else accepts(score, seconds, other_seconds)):
                        options.append((score, index))
                options.sort(reverse=True)
                if options and (len(options) == 1 or options[0][0] - options[1][0] >= VOICE_MARGIN):
                    taken[local] = options[0][1]
                    continue
            speakers.append({"members": [], "vectors": []})
            taken[local] = len(speakers) - 1
        for local, index in taken.items():
            owner[(wi, local)] = index
            speakers[index]["members"].append((wi, local))
            v, seconds = voice(w, local)
            if v is not None:
                speakers[index]["vectors"].append((v, seconds))
        placed.append([(a, b, taken[local]) for a, b, local in segs])

    cuts = _cuts(windows)
    kept, dropped = [], 0
    for wi, w in enumerate(windows):
        for s in w["result"].get("segments", []):
            a, b = w["start"] + s["start"], min(w["end"], w["start"] + s["end"])
            if not _owned(cuts, wi, a, b):
                continue
            if b - a < SHORT_SECONDS and _characters(s["text"]) <= SHORT_CHARACTERS:
                dropped += 1
                continue
            kept.append({"start": a, "end": b, "text": s["text"], "speaker": owner[(wi, s["speaker"])]})
    kept.sort(key=lambda s: (s["start"], s["end"]))

    # A label with no voice evidence anywhere is not a person we can stand behind.
    voiced = {i for i, sp in enumerate(speakers) if sp["vectors"]}
    final = []
    for s in kept:
        if s["speaker"] not in voiced:
            if s["end"] - s["start"] < UNVOICED_SECONDS:
                dropped += 1
                continue
            s = {**s, "speaker": None}
        final.append(s)

    order = list(dict.fromkeys(s["speaker"] for s in final if s["speaker"] is not None))
    people, voice_refs = {}, []
    for number, index in enumerate(order, 1):
        members = sorted(speakers[index]["members"])
        key = "recording-speaker:" + _hash(VERSION, scope, revision, members)[:20]
        v, seconds = joined(index)
        ref = _hash(VERSION, "speaker", scope, revision, key)[:32]
        refs.put(ref, v, seconds=seconds)
        voice_refs.append(ref)
        spans = [(s["start"], s["end"]) for s in final if s["speaker"] == index]
        # The earlier label this speaker covers most, so names given before carry over.
        weight = defaultdict(float)
        for entry in entries:
            interval, correction = entry.get("interval"), entry.get("correction_key")
            if not isinstance(correction, str) or not isinstance(interval, list) or len(interval) != 2 \
                    or any(type(t) not in (int, float) or not math.isfinite(t) for t in interval):
                continue
            for a, b in spans:
                weight[correction] += max(0.0, min(b, interval[1]) - max(a, interval[0]))
        correction = max(weight.items(), key=lambda kv: kv[1])[0] if weight and max(weight.values()) > 0 else None
        identity = library.match(v, seconds=seconds) if library is not None else None
        people[index] = {"key": key, "label": f"说话人 {number}", "voice_ref": ref, "correction_key": correction,
                         "match": _match(identity), "seconds": round(sum(b - a for a, b in spans), 3)}

    window_refs = [sp.get("ref") for w in windows for sp in w["result"].get("speakers", {}).values()
                   if isinstance(sp, dict) and sp.get("ref")]
    refs.forget([r for r in window_refs if r not in voice_refs])
    segments = []
    for s in final:
        p = people.get(s["speaker"])
        segments.append({"start_seconds": round(s["start"], 3), "end_seconds": round(s["end"], 3), "text": s["text"],
                         "speaker": p["key"] if p else "unresolved", "label": p["label"] if p else "未确定说话人",
                         "voice_ref": p["voice_ref"] if p else None, "match": p["match"] if p else None,
                         "correction_key": p["correction_key"] if p else None})
    return {"format": RESULT_FORMAT, "mode": "conversation", "revision": revision, "segments": segments,
            "speakers": [{k: people[i][k] for k in ("key", "label", "voice_ref", "seconds")} for i in order],
            "speaker_count": len(order), "unresolved_segments": sum(1 for s in final if s["speaker"] is None),
            "dropped_segments": dropped, "voice_refs": voice_refs,
            "producer": {"model_id": MOSS_ID, "model_revision": MOSS_REV, "voice_model_id": VOICE_ID,
                         "voice_model_revision": VOICE_REV, "processor_version": VERSION}}


def _match(identity):
    """Same shape as transcription matches: closest person with score, margin and the bar applied."""
    if not identity or identity.get("score") is None or not identity.get("candidates"):
        return None
    top = identity["candidates"][0]
    matched = identity.get("status") == "matched"
    return {"status": "matched" if matched else "unmatched", "person_id": top["person_id"], "name": top["name"],
            "score": round(float(identity["score"]), 4),
            "margin": None if identity.get("margin") is None else round(float(identity["margin"]), 4),
            "threshold": round(float(identity["threshold"]), 4), "allow_auto": bool(identity.get("allow_auto")) and matched}
