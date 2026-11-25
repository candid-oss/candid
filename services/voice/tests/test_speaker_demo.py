"""Model-free speaker demo contracts; real model evidence is kept separately."""
import hashlib
import json
import sqlite3

import pytest
np = pytest.importorskip("numpy")
sf = pytest.importorskip("soundfile")
pytest.importorskip("scipy")
pytest.importorskip("onnxruntime", reason="Install requirements-speaker.txt for the speaker demo")
pytest.importorskip("mlx_audio", reason="Install requirements-speaker.txt for the speaker demo")
from asr.speaker_pipeline import SpeakerPipeline, clean_intervals, public_result
from asr.speaker_models import TokenLimit
from asr.speaker_store import SpeakerStore


class FakeModels:
    load_seconds = 0
    loads = 0
    calls = 0
    broken = False

    def load(self):
        self.loads += 1

    def transcribe(self, audio):
        self.calls += 1
        if self.broken:
            return [{"start": 0, "end": 99, "speaker_id": "S01", "text": "invalid"}]
        # Speaker 1 talks for 9 s, enough speech for an automatic name.
        return [{"start": 0, "end": 6, "speaker_id": "S01", "text": "[S01] hello"},
                {"start": 6, "end": 9, "speaker_id": "S02", "text": "[S02] hi"},
                {"start": 9, "end": 12, "speaker_id": "S01", "text": "[S01] yes"},
                {"start": 12, "end": 14, "speaker_id": "S02", "text": "[S02] bye"}]

    def embedding(self, audio):
        return np.array([1., 0.]) if audio[0] > 0 else np.array([0., 1.])


@pytest.fixture
def fixture(tmp_path):
    audio = np.full(16000 * 14, .1)
    audio[6 * 16000:9 * 16000] = -.1
    audio[12 * 16000:] = -.1
    path = tmp_path / "audio.wav"
    sf.write(path, audio, 16000)
    store = SpeakerStore(tmp_path / "library")
    models = FakeModels()
    pipeline = SpeakerPipeline(store, models)
    return path, store, models, pipeline


def test_full_registry_lifecycle_and_idempotence(fixture):
    path, store, models, pipeline = fixture
    result = pipeline.analyze(path)
    assert len(result["speakers"]) == 2
    assert result["segments"][0]["text"] == "hello"
    assert all(s["identity"]["status"] == "unknown" for s in result["speakers"])
    with pytest.raises(ValueError, match="explicit"):
        store.enroll(result["analysis_id"], "speaker_01", name="Alice")
    person = store.enroll(result["analysis_id"], "speaker_01", name="Alice", confirmed=True)
    repeated = store.enroll(result["analysis_id"], "speaker_01", person_id=person["id"], confirmed=True)
    assert person["sample_count"] == repeated["sample_count"]
    cached = pipeline.analyze(path)
    assert cached["cache_hit"] and models.calls == 1
    assert cached["speakers"][0]["identity"]["person_id"] == person["id"]
    assert cached["speakers"][1]["identity"]["status"] == "unknown"
    reopened = SpeakerStore(store.root)
    reopened.rename(person["id"], "Alicia")
    assert pipeline.analyze(path)["speakers"][0]["identity"]["name"] == "Alicia"
    reopened.delete(person["id"])
    assert not reopened.people()
    with reopened.db() as db:
        assert db.execute("SELECT count(*) FROM samples").fetchone()[0] == 0
    assert pipeline.analyze(path)["speakers"][0]["identity"]["status"] == "unknown"
    assert models.calls == 1  # library changes never rerun inference


def test_ambiguity_threshold_and_empty_voiceprint(fixture):
    path, store, _, pipeline = fixture
    result = pipeline.analyze(path)
    for name in ("Alice", "similar voice"):
        store.enroll(result["analysis_id"], "speaker_01", name=name, confirmed=True)
    match = store.match([1, 0])
    assert match["status"] == "unknown" and match["margin"] == 0
    assert len(match["candidates"]) == 2
    assert store.match(None)["status"] == "unknown"
    with pytest.raises(ValueError):
        store.match([0, 0])
    with pytest.raises(ValueError):
        store.match([1, 0], threshold=2)


def test_overlap_subtraction_and_failed_analysis_retry(fixture):
    path, store, models, pipeline = fixture
    assert clean_intervals(0, 5, [(1, 2), (3, 8)]) == [(0, 1), (2, 3)]
    models.broken = True
    with pytest.raises(ValueError, match="invalid timestamps"):
        pipeline.analyze(path)
    with store.db() as db:
        assert db.execute("SELECT count(*) FROM analyses").fetchone()[0] == 0
    models.broken = False
    assert len(pipeline.analyze(path)["segments"]) == 4
    assert models.loads == 1


def test_cache_provenance_and_private_vectors(fixture):
    path, store, models, pipeline = fixture
    result = pipeline.analyze(path)
    assert result["source_hash"] == hashlib.sha256(path.read_bytes()).hexdigest()
    assert result["timestamp_kind"] == "model_segment_estimate"
    assert result["matching_policy"]["calibrated"] is False
    report = public_result(result)
    assert "vector" not in report["speakers"][0]
    assert "vector" not in report["speakers"][0]["samples"][0]
    # A fresh process can reuse inference without loading either model.
    fresh = FakeModels()
    assert SpeakerPipeline(SpeakerStore(store.root), fresh).analyze(path)["cache_hit"]
    assert fresh.calls == 0 and fresh.loads == 0
    store.forget(result["analysis_id"])
    assert store.cached(result["analysis_id"]) is None


def test_schema_initialization_reopen_future_rejection(tmp_path):
    store = SpeakerStore(tmp_path)
    with store.db() as db:
        assert db.execute("PRAGMA user_version").fetchone()[0] == 1
    assert SpeakerStore(tmp_path).people() == []
    with sqlite3.connect(store.path) as db:
        db.execute("PRAGMA user_version=2")
    with pytest.raises(RuntimeError, match="version: 2"):
        SpeakerStore(tmp_path)


def test_multichunk_offsets_and_cross_chunk_voice_stitching(tmp_path):
    class TwoChunks(FakeModels):
        def transcribe(self, audio):
            self.calls += 1
            return [{"start": 0, "end": 4, "speaker_id": "S01", "text": "one"},
                    {"start": 5, "end": 9, "speaker_id": "S02", "text": "two"}]
    audio = np.full(20 * 16000, .1)
    audio[5*16000:9*16000] = -.1
    audio[15*16000:19*16000] = -.1
    path = tmp_path / "chunks.wav"
    sf.write(path, audio, 16000)
    result = SpeakerPipeline(SpeakerStore(tmp_path / "db"), TwoChunks()).analyze(path, chunk_seconds=10)
    assert len(result["speakers"]) == 2
    assert [s["start_s"] for s in result["segments"]] == [0, 5, 10, 15]
    assert [s["speaker_id"] for s in result["segments"]] == ["speaker_01", "speaker_02"] * 2


def test_token_limit_retries_both_halves_and_preserves_offsets(tmp_path):
    class DenseSpeech(FakeModels):
        durations = []

        def transcribe(self, audio):
            duration = len(audio) / 16000
            self.durations.append(duration)
            if duration > 10:
                raise TokenLimit("too dense")
            return [{"start": 1, "end": 10, "speaker_id": "S01", "text": "speech"}]

    path = tmp_path / "dense.wav"
    sf.write(path, np.full(40 * 16000, 0.1), 16000)
    models = DenseSpeech()
    result = SpeakerPipeline(SpeakerStore(tmp_path / "db"), models).analyze(path)
    assert models.durations == [40, 20, 10, 10, 20, 10, 10]
    assert [(s["start_s"], s["end_s"]) for s in result["segments"]] == [
        (1, 10), (11, 20), (21, 30), (31, 40)
    ]
    assert len({s["speaker_id"] for s in result["segments"]}) == 1
