"""Whole-recording windows and stitching with a synthetic diarizer; no model or user audio is used.

Each synthetic voice is a constant sample level. The fake model labels runs of one level as one speaker,
numbering speakers afresh in every window like the real model does, and the fake voiceprint tells
levels apart."""
import io
import json

import pytest

np = pytest.importorskip("numpy")
sf = pytest.importorskip("soundfile")
pytest.importorskip("scipy")
from fastapi.testclient import TestClient

from asr.app import create_app
from asr.recording_windows import RESULT_FORMAT, recognize_window, stitch
from asr.speaker_models import VOICE_ID, VOICE_REV, TokenLimit
from asr.speaker_store import SpeakerStore, VoiceRefs

RATE = 16000
LEVELS = {"A": .1, "B": -.1, "C": .05}


def recording(turns):
    """turns: [(voice, seconds, text)] with 0.2 s of silence between turns."""
    audio, segments, t = [], [], 0.0
    for voice, seconds, text in turns:
        audio.append(np.full(int(seconds * RATE), LEVELS[voice], np.float32))
        segments.append((t, t + seconds, voice, text))
        audio.append(np.zeros(int(.2 * RATE), np.float32))
        t += seconds + .2
    return np.concatenate(audio), segments


class Diarizer:
    calls = 0

    def __init__(self, segments, permute=True, dense=False):
        self.segments, self.permute, self.dense = segments, permute, dense

    def transcribe(self, audio, max_tokens=4096):
        self.calls += 1
        if self.dense:
            raise TokenLimit("too dense")
        # Recover the window's position from its first sample run; tests pass the offset explicitly.
        offset = self.offset
        seen, out = {}, []
        for a, b, voice, text in self.segments:
            a, b = max(a, offset) - offset, min(b, offset + len(audio) / RATE) - offset
            if b - a <= 0.05:
                continue
            label = seen.setdefault(voice, len(seen) + 1)
            if self.permute and self.calls % 2 == 0:
                label = len(LEVELS) + 1 - label
            out.append({"start": a, "end": b, "speaker_id": f"S{label:02d}", "text": f"[S{label:02d}] {text}"})
        return out

    def embedding(self, audio):
        if len(audio) < 24000:
            return None
        level = float(np.median(audio))
        return np.array([1. if abs(level - v) < .02 else 0. for v in LEVELS.values()]) + 1e-3


def windows_for(audio, segments, models, refs, size, overlap):
    out, start, index, total = [], 0.0, 0, len(audio) / RATE
    while True:
        end = min(total, start + size)
        models.offset = start
        result = recognize_window(audio[int(start * RATE):int(end * RATE)], models, refs, "SYNTHETIC-recording", index)
        out.append({"start": start, "end": end, "result": json.loads(json.dumps(result))})
        if end >= total:
            return out
        start, index = end - overlap, index + 1


def library_with(tmp_path, name, vector):
    store = SpeakerStore(tmp_path / "library")
    with store.db() as db:
        db.execute("INSERT INTO people VALUES (?, ?, ?)", ("p1", name, "fixture"))
        db.execute("""INSERT INTO samples (person_id, source_hash, start, end, model_id, model_revision, vector,
            enrolled_at, confirmed) VALUES (?,?,?,?,?,?,?,?,1)""",
            ("p1", "synthetic", 0, 20, VOICE_ID, VOICE_REV, json.dumps(vector), "fixture"))
    return store


def run(tmp_path, turns, size=60, overlap=15, library=None):
    audio, segments = recording(turns)
    refs = VoiceRefs(tmp_path / "refs")
    models = Diarizer(segments)
    windows = windows_for(audio, segments, models, refs, size, overlap)
    request = {"scope": "SYNTHETIC-recording", "revision": "0" * 64, "windows": windows, "entries": []}
    return stitch(request, library, refs), windows, refs


def conversation(minutes=3):
    turns = []
    for n in range(int(minutes * 60 / 13)):
        turns += [("A", 8, f"甲的第{n}段话"), ("B", 4, f"乙的第{n}段话"), ("A", .4, "嗯")]
    return turns


def test_labels_restarting_in_every_window_become_one_set_of_speakers(tmp_path):
    result, windows, _ = run(tmp_path, conversation())
    assert len(windows) >= 3
    assert result["format"] == RESULT_FORMAT
    assert result["speaker_count"] == 2
    by_voice = {}
    for s in result["segments"]:
        by_voice.setdefault(s["text"][0], set()).add(s["speaker"])
    assert all(len(keys) == 1 for keys in by_voice.values())
    assert by_voice["甲"] != by_voice["乙"]


def test_overlapping_windows_do_not_repeat_or_lose_speech(tmp_path):
    turns = conversation()
    result, _, _ = run(tmp_path, turns)
    texts = [s["text"] for s in result["segments"]]
    expected = [t for _, seconds, t in turns if seconds >= 1]
    assert sorted(texts) == sorted(expected)
    starts = [s["start_seconds"] for s in result["segments"]]
    assert starts == sorted(starts)


def test_short_acknowledgements_are_left_out(tmp_path):
    result, _, _ = run(tmp_path, conversation())
    assert all(s["text"] != "嗯" for s in result["segments"])
    assert result["dropped_segments"] > 0


def test_speaker_missing_from_an_overlap_is_joined_by_voice(tmp_path):
    turns = [("A", 9, "甲开场"), ("C", 10, "丙先说一段"), ("A", 10, "甲接着说")]
    turns += [("A", 9, f"甲独白{n}") for n in range(8)]
    turns += [("C", 10, "丙回来了"), ("A", 9, "甲收尾")]
    result, windows, _ = run(tmp_path, turns, size=60, overlap=10)
    assert len(windows) >= 3
    speakers = {s["text"]: s["speaker"] for s in result["segments"]}
    assert speakers["丙先说一段"] == speakers["丙回来了"]
    assert result["speaker_count"] == 2


def test_a_voice_without_any_usable_speech_is_not_a_person(tmp_path):
    turns = [("A", 9, "甲说了很久的一段话"), ("C", 1.2, "是的"), ("A", 9, "甲又说了一段话")]
    result, _, _ = run(tmp_path, turns, size=120)
    assert result["speaker_count"] == 1
    assert all(s["text"] != "是的" for s in result["segments"])


def test_library_names_only_with_enough_speech(tmp_path):
    library = library_with(tmp_path, "合成人物", [1, 0, 0])
    result, _, refs = run(tmp_path, conversation(), library=library)
    matches = {s["text"][0]: s["match"] for s in result["segments"]}
    assert matches["甲"]["status"] == "matched" and matches["甲"]["allow_auto"]
    assert matches["甲"]["threshold"] == pytest.approx(.62)
    assert matches["乙"] is None or matches["乙"]["status"] == "unmatched"
    short = [("A", 5, "甲只说了五秒"), ("B", 9, "乙说了很久")]
    result, _, _ = run(tmp_path / "short", short, size=120, library=library)
    match = next(s["match"] for s in result["segments"] if s["text"].startswith("甲"))
    assert match["status"] == "unmatched" and not match["allow_auto"] and match["score"] > .9


def test_window_references_are_removed_and_speaker_references_keep_speech_length(tmp_path):
    result, windows, refs = run(tmp_path, conversation())
    window_refs = [sp["ref"] for w in windows for sp in w["result"]["speakers"].values() if sp["ref"]]
    assert not refs.entries(window_refs)
    kept = refs.entries(result["voice_refs"])
    assert set(kept) == set(result["voice_refs"])
    assert all(seconds and seconds >= 8 for _, seconds in kept.values())


def test_earlier_names_carry_over_through_correction_keys(tmp_path):
    audio, segments = recording(conversation(1))
    refs = VoiceRefs(tmp_path)
    models = Diarizer(segments)
    windows = windows_for(audio, segments, models, refs, 120, 0)
    entries = [{"interval": [a, b], "correction_key": f"录音段1 · {voice}"} for a, b, voice, _ in segments]
    result = stitch({"scope": "SYNTHETIC-recording", "revision": "1" * 64, "windows": windows, "entries": entries}, None, refs)
    keys = {s["text"][0]: s["correction_key"] for s in result["segments"]}
    assert keys == {"甲": "录音段1 · A", "乙": "录音段1 · B"}


def test_stitching_is_deterministic(tmp_path):
    first, _, _ = run(tmp_path / "a", conversation())
    second, _, _ = run(tmp_path / "b", conversation())
    assert first["segments"] == second["segments"]


def test_invalid_requests_are_refused(tmp_path):
    refs = VoiceRefs(tmp_path)
    for bad in [{}, {"scope": "x", "revision": "short", "windows": []},
                {"scope": "x", "revision": "0" * 64, "windows": [{"start": 0, "end": 10, "result": {"format": "other"}}]},
                {"scope": "x", "revision": "0" * 64, "windows": [
                    {"start": 0, "end": 10, "result": {"format": "speaker-window/1", "segments": []}},
                    {"start": 20, "end": 30, "result": {"format": "speaker-window/1", "segments": []}}]}]:
        with pytest.raises(ValueError):
            stitch(bad, None, refs)


def wav(audio):
    buffer = io.BytesIO()
    sf.write(buffer, audio, RATE, format="WAV", subtype="PCM_16")
    return buffer.getvalue()


class Engine:
    device = "synthetic"

    def load(self):
        pass


def test_window_and_stitch_endpoints(tmp_path):
    audio, segments = recording(conversation(1))
    models = Diarizer(segments)
    models.offset = 0
    store = SpeakerStore(tmp_path)
    with TestClient(create_app(Engine(), voiceprint_path=store.path, voiceprint_models=models)) as client:
        assert "recording_windows" in client.get("/v1/voiceprints").json()["capabilities"]
        meta = json.dumps({"scope": "SYNTHETIC-recording", "index": 0})
        response = client.post("/v1/voiceprints/window", files={"file": ("w.wav", wav(audio))}, data={"meta": meta})
        assert response.status_code == 200, response.text
        window = response.json()
        assert "vector" not in response.text and window["speakers"]
        request = {"scope": "SYNTHETIC-recording", "revision": "2" * 64,
                   "windows": [{"start": 0, "end": len(audio) / RATE, "result": window}]}
        stitched = client.post("/v1/voiceprints/stitch", json=request)
        assert stitched.status_code == 200 and stitched.json()["speaker_count"] == 2
        assert "vector" not in stitched.text
        stereo = np.stack([audio, audio], axis=1)
        assert client.post("/v1/voiceprints/window", files={"file": ("w.wav", wav(stereo))}, data={"meta": meta}).status_code == 400
        models.dense = True
        assert client.post("/v1/voiceprints/window", files={"file": ("w.wav", wav(audio))}, data={"meta": meta}).status_code == 422
        assert client.post("/v1/voiceprints/window", files={"file": ("w.wav", wav(audio))}, data={"meta": meta},
                           headers={"Origin": "https://example.test"}).status_code == 403


def test_matching_policy_depends_on_speech_length(tmp_path):
    library = library_with(tmp_path, "合成人物", [1, 0, 0])
    near = [1, .3, 0]   # cosine about 0.96
    assert library.match(near, seconds=2)["candidates"] == []
    assert library.match(near, seconds=2)["reason"] == "too_little_speech"
    five = library.match(near, seconds=5)
    assert five["status"] == "unknown" and five["reason"] == "needs_more_speech" and five["score"] > .9
    assert library.match(near, seconds=12)["status"] == "matched"
    middling = [1, 1.1, 0]  # cosine about 0.67: enough for long speech, not for legacy short-clip vectors
    assert library.match(middling, seconds=20)["status"] == "matched"
    assert library.match(middling)["status"] == "unknown" and library.match(middling)["threshold"] == .70
    with pytest.raises(ValueError):
        library.match(near, seconds=float("nan"))


def test_rematch_uses_stored_speech_length(tmp_path):
    library = library_with(tmp_path, "合成人物", [1, 0, 0])
    refs = VoiceRefs(library.path.parent)
    refs.put("long", [1, 1.1, 0], seconds=20)
    refs.put("short", [1, 1.1, 0], seconds=5)
    refs.put("legacy", [1, 1.1, 0])
    with TestClient(create_app(Engine(), voiceprint_path=library.path)) as client:
        matches = client.post("/v1/voiceprints", json={"action": "rematch", "refs": ["long", "short", "legacy"]}).json()["matches"]
    assert matches["long"]["status"] == "matched" and matches["long"]["allow_auto"]
    assert matches["short"]["status"] == "unmatched" and matches["short"]["threshold"] == .62
    assert matches["legacy"]["status"] == "unmatched" and matches["legacy"]["threshold"] == .70


class SmallAsr:
    """Synthetic text-only model: says which 30-second piece it heard, never loads anything real."""

    def __init__(self):
        self.calls = 0

    def transcribe(self, data, language=None):
        self.calls += 1
        return {"text": f"SYNTHETIC 第{self.calls}段"}


def test_quick_windows_use_the_small_model_and_stitch_without_speakers(tmp_path):
    asr = SmallAsr()
    store = SpeakerStore(tmp_path)
    audio = np.concatenate([np.full(70 * RATE, .1, np.float32), np.zeros(30 * RATE, np.float32)])
    with TestClient(create_app(asr, voiceprint_path=store.path, voiceprint_models=Diarizer([]))) as client:
        assert "recording_windows_quick" in client.get("/v1/voiceprints").json()["capabilities"]
        meta = json.dumps({"scope": "SYNTHETIC-recording", "index": 0, "mode": "quick"})
        response = client.post("/v1/voiceprints/window", files={"file": ("w.wav", wav(audio))}, data={"meta": meta})
        assert response.status_code == 200, response.text
        window = response.json()
        # Silence is skipped; the diarization model is never touched in quick mode.
        assert [(s["start"], s["end"], s["speaker"]) for s in window["segments"]] == [(0, 30, None), (30, 60, None), (60, 90, None)]
        assert window["cpu_seconds"] >= 0 and asr.calls == 3
        assert client.get("/health").json()["models_loaded"] == {"asr": True, "speaker": False}
        request = {"scope": "SYNTHETIC-recording", "revision": "3" * 64, "mode": "quick",
                   "windows": [{"start": 0, "end": 100, "result": window},
                               {"start": 40, "end": 140, "result": window}]}
        stitched = client.post("/v1/voiceprints/stitch", json=request).json()
        assert stitched["format"] == RESULT_FORMAT and stitched["mode"] == "quick" and stitched["speaker_count"] == 0
        # Overlap 40–100 s is split at 70 s: pieces whose middle is before it come from the first window.
        assert [(s["start_seconds"], s["end_seconds"]) for s in stitched["segments"]] == [(0, 30), (30, 60), (70, 100), (100, 130)]
        assert all(s["speaker"] is None and s["label"] is None for s in stitched["segments"])
        bad = dict(request, mode="conversation")
        assert client.post("/v1/voiceprints/stitch", json=bad).status_code == 400
