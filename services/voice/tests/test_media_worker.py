import hashlib
import json

import pytest

np = pytest.importorskip("numpy")
sf = pytest.importorskip("soundfile")

from asr import media_worker


def test_quick_keeps_recording_offsets_across_silence(tmp_path, monkeypatch):
    monkeypatch.setattr(media_worker, "version", lambda _: "fixture")
    rate = media_worker.SAMPLE_RATE
    first = np.full(rate * 30, 0.1, dtype=np.float32)
    quiet = np.zeros(rate * 30, dtype=np.float32)
    last = np.full(rate * 2, 0.1, dtype=np.float32)
    source = tmp_path / "original.wav"
    sf.write(source, np.concatenate((first, quiet, last)), rate)

    class Engine:
        loaded = 0

        def load(self):
            self.loaded += 1

        def transcribe(self, audio):
            assert self.loaded == 1
            assert audio.startswith(b"RIFF")
            return {"text": "转写样例"}

    engine = Engine()
    result = media_worker.quick_result(source, tmp_path, engine)
    assert engine.loaded == 1
    assert result["producer"]["timestamp_kind"] == "audio_chunk"
    assert [(s["start_seconds"], s["end_seconds"]) for s in result["segments"]] == [(0, 30), (60, 62)]


def test_conversation_offsets_and_read_only_identity_lookup(tmp_path, monkeypatch):
    pytest.importorskip("scipy")
    from asr import speaker_pipeline
    from asr.speaker_models import VOICE_ID, VOICE_REV
    from asr.speaker_store import SpeakerStore

    monkeypatch.setattr(media_worker, "version", lambda _: "fixture")
    monkeypatch.setattr(media_worker, "MAX_SECONDS", 1)
    store = SpeakerStore(tmp_path / "library")
    with store.db() as db:
        db.execute("INSERT INTO people VALUES (?, ?, ?)", ("p1", "样例人物", "fixture"))
        db.execute("""INSERT INTO samples
            (person_id, source_hash, start, end, model_id, model_revision, vector, enrolled_at, confirmed)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            ("p1", "fixture", 0, 2, VOICE_ID, VOICE_REV, json.dumps([1, 0]), "fixture", 1))
    original_digest = hashlib.sha256(store.path.read_bytes()).digest()
    monkeypatch.setenv("CANDID_VOICE_LIBRARY", str(store.path))

    class Pipeline:
        def __init__(self, store):
            self.store = store

        def analyze(self, _path):
            identity = self.store.match([1, 0])
            return {"speakers": [{"speaker_id": "s1", "identity": identity, "vector": [1, 0]}],
                    "segments": [{"speaker_id": "s1", "start_s": 0.1, "end_s": 0.3, "text": "样例"}]}

    monkeypatch.setattr(speaker_pipeline, "SpeakerPipeline", Pipeline)
    source = tmp_path / "original.wav"
    sf.write(source, np.full(media_worker.SAMPLE_RATE * 2, 0.1), media_worker.SAMPLE_RATE)
    result = media_worker.conversation_result(source, tmp_path)
    assert result["producer"]["identity_kind"] == "candidate"
    assert [s["start_seconds"] for s in result["segments"]] == [0.1, 1.1]
    assert [s["speaker"] for s in result["segments"]] == ["说话人 1", "说话人 2"]
    assert {s["identity"] for s in result["segments"]} == {"样例人物"}
    match = result["segments"][0]["match"]
    assert match["person_id"] == "p1" and match["name"] == "样例人物" and match["status"] == "matched"
    assert 0.99 <= match["score"] <= 1.0
    first, second = (s["voice_ref"] for s in result["segments"])
    assert first != second, "each chunk's speaker keeps its own reference"
    from asr.speaker_store import VoiceRefs
    assert set(VoiceRefs(store.path.parent).vectors([first, second])) == {first, second}
    assert hashlib.sha256(store.path.read_bytes()).digest() == original_digest
