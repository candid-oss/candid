"""ASR API failure/retry and source provenance without downloading a model."""
import hashlib
import io
from types import SimpleNamespace

import pytest

sf = pytest.importorskip("soundfile", reason="Install services/voice/asr/requirements.txt for ASR tests")
np = pytest.importorskip("numpy")
from fastapi.testclient import TestClient
from asr.app import create_app
from asr.runtime import AudioError, MODEL_ID, Runtime, decode_audio


def wav(seconds=0.1):
    output = io.BytesIO()
    sf.write(output, np.zeros(int(16000 * seconds)), 16000, format="WAV")
    return output.getvalue()


class Engine:
    device = "test"
    error = None
    calls = 0

    def load(self):
        pass

    def transcribe(self, data, language):
        self.calls += 1
        decode_audio(data)
        if self.error:
            raise self.error
        return {"text": "测试", "source_hash": hashlib.sha256(data).hexdigest(),
                "model_id": MODEL_ID, "timestamps_available": False}


def test_upload_provenance_and_no_fake_timestamps():
    audio = wav()
    with TestClient(create_app(Engine())) as client:
        assert client.get("/health").json()["status"] == "ready"
        r = client.post("/v1/audio/transcriptions", files={"file": ("test.wav", audio)})
        assert r.status_code == 200
        assert r.json()["source_hash"] == hashlib.sha256(audio).hexdigest()
        assert r.json()["timestamps_available"] is False


def test_invalid_audio_model_format_and_missing_upload():
    engine = Engine()
    with TestClient(create_app(engine)) as client:
        assert client.post("/v1/audio/transcriptions").status_code == 422
        assert client.post("/v1/audio/transcriptions", files={"file": ("x.wav", b"broken")}).status_code == 400
        for data in ({"model": "other"}, {"response_format": "srt"}):
            assert client.post("/v1/audio/transcriptions", data=data,
                               files={"file": ("x.wav", wav())}).status_code == 400
        assert engine.calls == 1


def test_busy_and_inference_failure_allow_retry():
    engine = Engine()
    with TestClient(create_app(engine)) as client:
        for error, expected in ((BlockingIOError("busy"), 429), (RuntimeError("failed"), 500)):
            engine.error = error
            r = client.post("/v1/audio/transcriptions", files={"file": ("x.wav", wav())})
            assert r.status_code == expected
            if expected == 429:
                assert r.headers["retry-after"] == "5"
            engine.error = None
            assert client.post("/v1/audio/transcriptions", files={"file": ("x.wav", wav())}).status_code == 200


def test_duration_and_nonfinite_rejected():
    with pytest.raises(AudioError):
        decode_audio(wav(301))
    output = io.BytesIO()
    sf.write(output, np.array([np.nan]), 16000, format="WAV", subtype="FLOAT")
    with pytest.raises(AudioError):
        decode_audio(output.getvalue())


def test_runtime_lock_released_after_model_failure():
    runtime = Runtime()
    class Model:
        fail = True
        sample_rate = 16000
        config = SimpleNamespace(support_languages=["English", "Chinese", "Cantonese"])
        def generate(self, samples, **kwargs):
            if self.fail:
                raise RuntimeError("model failure")
            return SimpleNamespace(text="hello", language=["English"])
    runtime.model = Model()
    with pytest.raises(RuntimeError):
        runtime.transcribe(wav(), "en")
    assert not runtime.lock.locked()
    runtime.model.fail = False
    result = runtime.transcribe(wav(), "en")
    assert result["text"] == "hello"
    assert result["segments"] == []
    assert result["confidence"] is None
    assert result["source_hash"] == hashlib.sha256(wav()).hexdigest()
    runtime.lock.acquire()
    try:
        with pytest.raises(BlockingIOError):
            runtime.transcribe(wav())
    finally:
        runtime.lock.release()


def test_mlx_resamples_and_preserves_language_and_source():
    runtime = Runtime()
    class Model:
        sample_rate = 16000
        config = SimpleNamespace(support_languages=["Chinese", "English"])
        def generate(self, samples, **kwargs):
            assert samples.shape == (1600,)
            assert samples.dtype == np.float32
            assert kwargs["language"] == "Chinese"
            return SimpleNamespace(text="你好", language=["Chinese", "Chinese"])
    runtime.model = Model()
    output = io.BytesIO()
    sf.write(output, np.zeros(4800), 48000, format="WAV")
    result = runtime.transcribe(output.getvalue(), "zh")
    assert result["language"] == "Chinese"
    assert result["duration"] == pytest.approx(0.1)
    assert result["model_id"] == MODEL_ID
    assert result["quantization"] == "8bit"
    assert result["source_hash"] == hashlib.sha256(output.getvalue()).hexdigest()
    with pytest.raises(ValueError, match="Unsupported language"):
        runtime.transcribe(output.getvalue(), "bad-language")
