"""Load quantized MLX Qwen weights locally and retain inference provenance."""
import hashlib
import io
import json
import os
import threading
import time
from datetime import datetime, timezone
from importlib.metadata import version
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parents[3]
MODEL_ID = "mlx-community/Qwen3-ASR-0.6B-8bit"
MODEL_REVISION = "89e96d92ba34aca20b3e29fb10cc284097d1219f"
MODEL_DIR = ROOT / "data/asr/Qwen3-ASR-0.6B-8bit"
MAX_BYTES = 64 * 1024 * 1024
MAX_SECONDS = 300


class AudioError(ValueError):
    pass


def decode_audio(data):
    if not data or len(data) > MAX_BYTES:
        raise AudioError("Audio must be nonempty and at most 64 MiB")
    try:
        with sf.SoundFile(io.BytesIO(data)) as f:
            duration = f.frames / f.samplerate
            if not 0 < duration <= MAX_SECONDS or f.channels > 2:
                raise AudioError("Use mono/stereo audio between 0 and 300 seconds")
            samples = f.read(dtype="float32", always_2d=True).mean(axis=1)
            rate = f.samplerate
    except (sf.LibsndfileError, RuntimeError) as exc:
        raise AudioError("Unsupported or damaged audio; use PCM WAV or FLAC") from exc
    if not np.isfinite(samples).all():
        raise AudioError("Audio contains non-finite samples")
    return samples, rate, duration


class Runtime:
    def __init__(self):
        self.model = None
        self.device = None
        self.lock = threading.Lock()

    def load(self):
        if self.model is not None:
            return
        # Local paths plus offline flags prevent inference from fetching weights.
        os.environ["HF_HUB_OFFLINE"] = "1"
        os.environ["TRANSFORMERS_OFFLINE"] = "1"
        import mlx.core as mx
        from mlx_audio.stt import load

        manifest = json.loads((MODEL_DIR / "deployment.json").read_text())
        if manifest["model_id"] != MODEL_ID or manifest["model_revision"] != MODEL_REVISION:
            raise RuntimeError("Model revision differs from the verified deployment")
        if not mx.metal.is_available():
            raise RuntimeError("This MLX deployment requires an Apple Silicon GPU")
        self.device = "mlx:gpu"
        mx.set_default_device(mx.gpu)
        self.model = load(MODEL_DIR, strict=True)

    def unload(self):
        self.model = None

    def transcribe(self, data, language=None):
        if language:
            language = language.strip().lower()
            language = {"zh": "chinese", "en": "english", "yue": "cantonese"}.get(language, language)
            supported = {s.lower(): s for s in self.model.config.support_languages}
            if language not in supported:
                raise ValueError(f"Unsupported language: {language}")
            language = supported[language]
        samples, rate, duration = decode_audio(data)
        if not self.lock.acquire(blocking=False):
            raise BlockingIOError("ASR busy; retry after the current request completes")
        try:
            started = time.monotonic()
            if rate != self.model.sample_rate:
                from math import gcd
                from scipy.signal import resample_poly
                factor = gcd(rate, self.model.sample_rate)
                samples = resample_poly(samples, self.model.sample_rate // factor, rate // factor).astype(np.float32)
            result = self.model.generate(samples, language=language, max_tokens=4096,
                                         chunk_duration=30.0, batch_size=1, verbose=False)
            languages = result.language if isinstance(result.language, list) else [result.language]
            detected_language = ",".join(dict.fromkeys(s for s in languages if s))
            return {
                "text": result.text, "language": detected_language, "duration": duration,
                "provider_id": "qwen-asr-local", "model_id": MODEL_ID,
                "model_revision": MODEL_REVISION, "runtime_version": version("mlx-audio"),
                "backend": "mlx-audio", "quantization": "8bit",
                "device": self.device, "source_hash": hashlib.sha256(data).hexdigest(),
                "processed_at": datetime.now(timezone.utc).isoformat(),
                "elapsed_seconds": round(time.monotonic() - started, 3),
                "confidence": None, "segments": [], "timestamps_available": False,
            }
        finally:
            self.lock.release()
