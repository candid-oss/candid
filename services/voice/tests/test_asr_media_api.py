"""Synthetic service-contract checks; no model, device or personal library."""
import hashlib
import io
import shutil
import threading

import pytest
pytest.importorskip("numpy")
pytest.importorskip("soundfile")
from fastapi.testclient import TestClient
from asr.app import create_app


class Engine:
    device = "synthetic"
    def load(self):
        pass


def result(_audio, mode):
    segment = {"start_seconds": 0, "end_seconds": 1, "text": "SYNTHETIC repeated words"}
    if mode == "conversation":
        segment.update(speaker="anonymous-1", identity=None)
    return {"producer": {"model_id": "synthetic", "model_revision": "fixture-1",
                         "timestamp_kind": "audio_chunk"},
            "duration_seconds": 2, "segments": [segment, dict(segment, start_seconds=1, end_seconds=2)]}


def test_voice_contract_preserves_repetition_and_marks_coarse_positions():
    with TestClient(create_app(Engine(), result)) as client:
        for mode in ("quick", "conversation"):
            response = client.post("/v1/voice", files={"file": ("synthetic.wav", b"SYNTHETIC")}, data={"mode": mode})
            assert response.status_code == 200
            body = response.json()
            assert body["format"] == "candid-voice-result/1"
            assert body["source_hash"] == hashlib.sha256(b"SYNTHETIC").hexdigest()
            assert len(body["segments"]) == 2
            assert body["producer"]["timestamp_kind"] == "audio_chunk"
            assert body["cancellation_supported"] is False


def test_voice_invalid_interval_retry_and_invalid_mode():
    calls = []
    def runner(audio, mode):
        calls.append(1)
        value = result(audio, mode)
        if len(calls) == 1:
            value["segments"][0]["end_seconds"] = 3
        return value
    with TestClient(create_app(Engine(), runner)) as client:
        assert client.post("/v1/voice", files={"file": ("x", b"synthetic")}).status_code == 400
        assert client.post("/v1/voice", files={"file": ("x", b"synthetic")}).status_code == 200
        assert client.post("/v1/voice", files={"file": ("x", b"synthetic")}, data={"mode": "unknown"}).status_code == 422
        assert client.post("/v1/voice", files={"file": ("x", b"")}).status_code == 413
        assert client.post("/v1/voice", files={"file": ("x", b"synthetic")},
                           headers={"Origin": "https://example.test"}).status_code == 403


def test_busy_refuses_a_second_request_then_allows_retry():
    entered, release = threading.Event(), threading.Event()
    def runner(audio, mode):
        entered.set()
        assert release.wait(5)
        return result(audio, mode)
    with TestClient(create_app(Engine(), runner)) as client:
        responses = []
        worker = threading.Thread(target=lambda: responses.append(client.post("/v1/voice", files={"file": ("x", b"synthetic")})))
        worker.start()
        try:
            assert entered.wait(5)
            response = client.post("/v1/voice", files={"file": ("x", b"synthetic")})
            assert response.status_code == 429
            assert response.headers["retry-after"] == "5"
        finally:
            release.set()
            worker.join(5)
        assert responses[0].status_code == 200
        assert client.post("/v1/voice", files={"file": ("x", b"synthetic")}).status_code == 200


def test_overlong_audio_is_rejected_before_model_inference():
    if shutil.which("ffmpeg") is None:
        pytest.skip("FFmpeg is required for the external voice service")
    import numpy as np
    import soundfile as sf
    audio = io.BytesIO()
    sf.write(audio, np.zeros(16000 * 301), 16000, format="WAV", subtype="PCM_16")
    # Engine has no inference method: an overlong input must never reach a model.
    with TestClient(create_app(Engine())) as client:
        response = client.post("/v1/voice", files={"file": ("synthetic-long.wav", audio.getvalue())})
        assert response.status_code == 400
