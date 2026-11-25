"""Process a Hub audio original locally and emit a single JSON result."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import wave
from contextlib import redirect_stdout
from importlib.metadata import version
from pathlib import Path

import numpy as np
import soundfile as sf

from .job_progress import report
from .runtime import MAX_SECONDS, MODEL_ID, MODEL_REVISION, Runtime


SAMPLE_RATE = 16_000
QUICK_SECONDS = 30
MAX_MEDIA_SECONDS = 4 * 60 * 60


def decode_original(source: Path, target: Path, max_seconds: int = MAX_MEDIA_SECONDS) -> None:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        ffmpeg = next((str(path) for path in map(Path, ("/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"))
                       if path.is_file()), None)
    if ffmpeg is None:
        raise ValueError("找不到 FFmpeg，无法解码录音。")
    try:
        subprocess.run([
            ffmpeg, "-nostdin", "-v", "error", "-xerror", "-protocol_whitelist", "file",
            "-format_whitelist", "wav,mov,mp3,aac,flac,ogg", "-i", str(source.resolve()),
            "-map", "0:a:0", "-t", str(max_seconds + 1), "-ac", "1",
            "-ar", str(SAMPLE_RATE), "-c:a", "pcm_s16le", str(target),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=120)
    except subprocess.CalledProcessError as exc:
        raise ValueError("无法解码录音，请检查文件格式和完整性。") from exc
    except subprocess.TimeoutExpired as exc:
        raise ValueError("录音解码超时，请缩短录音后重试。") from exc
    with wave.open(str(target), "rb") as decoded:
        duration = decoded.getnframes() / SAMPLE_RATE
    if not 0 < duration <= max_seconds:
        raise ValueError("录音为空或超过允许时长，请检查或分段后重试。")


def asr_chunks(decoded: Path, seconds: int):
    with sf.SoundFile(decoded) as stream:
        offset = 0
        for samples in stream.blocks(blocksize=seconds * SAMPLE_RATE, dtype="float32"):
            start = offset / SAMPLE_RATE
            offset += len(samples)
            yield start, offset / SAMPLE_RATE, samples


def quick_result(decoded: Path, temp: Path, engine: Runtime) -> dict:
    segments = []
    chunk = temp / "chunk.wav"
    loaded = False
    with wave.open(str(decoded)) as audio:
        total = audio.getnframes() / audio.getframerate()
    for start, end, samples in asr_chunks(decoded, QUICK_SECONDS):
        report("transcribing", start, total)
        if np.sqrt(np.mean(samples ** 2)) < 1e-4:
            continue
        if not loaded:
            engine.load()
            loaded = True
        sf.write(chunk, samples, SAMPLE_RATE, subtype="PCM_16")
        text = engine.transcribe(chunk.read_bytes())["text"].strip()
        report("transcribing", end, total)
        if text:
            segments.append({"start_seconds": start, "end_seconds": end, "text": text})
    return {"segments": segments, "producer": {
        "processor": "local-voice-worker", "processor_version": "1", "model_id": MODEL_ID,
        "model_revision": MODEL_REVISION, "runtime_version": version("mlx-audio"),
        "timestamp_kind": "audio_chunk", "chunk_seconds": QUICK_SECONDS,
    }}


def match_of(identity: dict) -> dict | None:
    """The closest library person even when not accepted, so the app can show 「可能是 X？」 with a score.

    Scores are cosine similarities from the library's own matcher, not probabilities."""
    candidates = identity.get("candidates") or []
    if not candidates or identity.get("score") is None:
        return None
    top = candidates[0]
    matched = identity.get("status") == "matched"
    out = {"status": "matched" if matched else "unmatched",
           "person_id": top["person_id"], "name": top["name"], "score": round(float(identity["score"]), 4),
           "margin": None if identity.get("margin") is None else round(float(identity["margin"]), 4)}
    # The bar this vector had to clear, so the app offers 「可能是」 relative to the same policy.
    if identity.get("threshold") is not None:
        out["threshold"] = round(float(identity["threshold"]), 4)
    if "allow_auto" in identity:
        out["allow_auto"] = bool(identity["allow_auto"]) and matched
    return out


def conversation_result(decoded: Path, temp: Path, models=None) -> dict:
    from contextlib import closing, contextmanager
    import sqlite3

    from .speaker_models import MOSS_ID, MOSS_REV, VOICE_ID, VOICE_REV
    from .speaker_pipeline import PIPELINE_VERSION, SpeakerPipeline
    from .speaker_store import SpeakerStore

    class ReadOnlyLibrary(SpeakerStore):
        def __init__(self, path):
            self.path = Path(path).resolve(strict=True)
            with self.db() as db:
                if db.execute("PRAGMA user_version").fetchone()[0] != 1:
                    raise ValueError("不支持此版本的声纹库。")

        @contextmanager
        def db(self):
            with closing(sqlite3.connect(self.path.as_uri() + "?mode=ro", uri=True)) as db:
                db.row_factory = sqlite3.Row
                yield db

    class JobSpeakerStore(SpeakerStore):
        def match(self, vector, threshold=0.65, margin=0.05, *, seconds=None):
            if library is None:
                return {"status": "unknown", "name": None}
            return library.match(vector, threshold, margin, seconds=seconds)

    from .voiceprint_api import library_path
    candidate_path = library_path()
    library = ReadOnlyLibrary(candidate_path) if candidate_path.exists() else None
    from .speaker_store import VoiceRefs
    refs = VoiceRefs(candidate_path.parent)
    source_hash = hashlib.sha256(decoded.read_bytes()).hexdigest()
    pipeline = SpeakerPipeline(JobSpeakerStore(temp / "analysis"), models)
    chunk = temp / "chunk.wav"
    segments = []
    speaker_count = 0
    for start, end, samples in asr_chunks(decoded, MAX_SECONDS):
        if np.sqrt(np.mean(samples ** 2)) < 1e-4:
            continue
        sf.write(chunk, samples, SAMPLE_RATE, subtype="PCM_16")
        result = pipeline.analyze(chunk)
        speakers = {}
        for i, speaker in enumerate(result["speakers"], 1):
            # Keep this chunk's speaker vector so the recording can be matched again when the library changes.
            ref = None
            if speaker.get("vector") is not None:
                ref = hashlib.sha256(f"{source_hash}:{start}:{speaker['speaker_id']}:{VOICE_REV}".encode()).hexdigest()[:32]
                refs.put(ref, speaker["vector"], seconds=speaker.get("seconds"))
            speakers[speaker["speaker_id"]] = (f"说话人 {speaker_count + i}", speaker["identity"]["name"], match_of(speaker["identity"]), ref)
        speaker_count += len(speakers)
        for segment in result["segments"]:
            text = segment["text"].strip()
            segment_start = start + segment["start_s"]
            segment_end = min(end, start + segment["end_s"])
            if not text or segment_start >= segment_end:
                continue
            speaker, identity, match, ref = speakers[segment["speaker_id"]]
            entry = {"start_seconds": segment_start, "end_seconds": segment_end,
                     "text": text, "speaker": speaker, "identity": identity}
            if match:
                entry["match"] = match
            if ref:
                entry["voice_ref"] = ref
            segments.append(entry)
    segments.sort(key=lambda segment: (segment["start_seconds"], segment["end_seconds"]))
    return {"segments": segments, "producer": {
        "processor": "local-voice-worker", "processor_version": "1", "model_id": MOSS_ID,
        "model_revision": MOSS_REV, "speaker_pipeline_version": PIPELINE_VERSION,
        "voice_model_id": VOICE_ID, "voice_model_revision": VOICE_REV,
        "runtime_version": version("mlx-audio"), "timestamp_kind": "model_segment_estimate",
        "speaker_scope_seconds": MAX_SECONDS, "identity_kind": "candidate" if library else "anonymous",
    }}


def recognize(source: Path, mode: str) -> dict:
    with tempfile.TemporaryDirectory(prefix="candid-voice-") as directory:
        temp = Path(directory)
        decoded = temp / "decoded.wav"
        decode_original(source, decoded)
        if mode == "quick":
            return quick_result(decoded, temp, Runtime())
        return conversation_result(decoded, temp)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("quick", "conversation"), required=True)
    parser.add_argument("--input", type=Path, required=True)
    args = parser.parse_args()
    try:
        with redirect_stdout(sys.stderr):
            result = recognize(args.input, args.mode)
        print(json.dumps(result, ensure_ascii=False, allow_nan=False))
    except (OSError, ValueError, RuntimeError) as exc:
        print(str(exc), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
