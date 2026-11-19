"""Cached multi-speaker transcription with separate, revisable identity matching."""
import hashlib
import json
import re
import time
from importlib.metadata import version
from math import gcd
from pathlib import Path

import numpy as np
from scipy.signal import resample_poly

from .runtime import MAX_BYTES, decode_audio
from .job_progress import report
from .speaker_models import MOSS_ID, MOSS_REV, VOICE_ID, VOICE_REV, Models, unit
from .speaker_store import now

PIPELINE_VERSION = "speaker-demo-2.0"
# A speaker's voiceprint is computed once from all of their clean speech joined together, up to this much.
# Joined speech is far more reliable than averaging short clips (same person 0.75–0.96 with 8 s or more,
# against 0.47–0.57 for 2–3 s clips).
POOL_SECONDS = 30
MIN_PIECE_SECONDS = 0.3


def clean_intervals(start, end, blockers):
    """Subtract predicted overlapping speech before extracting voiceprints."""
    pieces = [(start, end)]
    for left, right in blockers:
        next_pieces = []
        for a, b in pieces:
            if right <= a or left >= b:
                next_pieces.append((a, b))
            else:
                if a < left:
                    next_pieces.append((a, left))
                if right < b:
                    next_pieces.append((right, b))
        pieces = next_pieces
    return pieces


def pooled_clip(audio, pieces, rate=16000, limit=POOL_SECONDS):
    """Join clean pieces (longest first, then played in time order) up to `limit` seconds.

    Returns (clip, used pieces, total clean seconds available). Clipped pieces are skipped."""
    usable = [(a, b) for a, b in pieces if b - a >= MIN_PIECE_SECONDS]
    total = sum(b - a for a, b in usable)
    chosen, length = [], 0.0
    for a, b in sorted(usable, key=lambda p: p[1] - p[0], reverse=True):
        piece = audio[round(a * rate):round(b * rate)]
        if not len(piece) or np.mean(np.abs(piece) >= 0.999) > 0.05:
            continue
        take = min(b - a, limit - length)
        chosen.append((a, a + take))
        length += take
        if length >= limit - 1e-6:
            break
    chosen.sort()
    parts = [audio[round(a * rate):round(b * rate)] for a, b in chosen]
    clip = np.concatenate(parts).astype(np.float32) if parts else np.zeros(0, np.float32)
    return clip, chosen, total


class SpeakerPipeline:
    def __init__(self, store, models=None):
        self.store = store
        self.models = models or Models()
        self.loaded = False

    def analyze(self, audio_path, threshold=None, margin=0.05, chunk_seconds=300):
        # The diarization model handles several minutes in one pass and keeps its speaker labels
        # consistent within it, so long chunks avoid stitching speakers back together by voice.
        if not 10 <= chunk_seconds <= 600:
            raise ValueError("Chunk size must be between 10 and 600 seconds")
        # Validate options even when the input is silent or has no usable speaker.
        self.store.match(None, threshold, margin)
        path = Path(audio_path).resolve()
        if path.stat().st_size > MAX_BYTES:
            raise ValueError("Audio exceeds 64 MiB")
        content = path.read_bytes()
        source_hash = hashlib.sha256(content).hexdigest()
        key = json.dumps([source_hash, PIPELINE_VERSION, MOSS_REV, VOICE_REV, chunk_seconds,
                          version("mlx-audio"), version("kaldi-native-fbank"),
                          version("onnxruntime"), version("scipy")])
        analysis_id = hashlib.sha256(key.encode()).hexdigest()
        started = time.monotonic()
        result = self.store.cached(analysis_id)
        cached = result is not None
        if not cached:
            audio, rate, duration = decode_audio(content)
            if rate != 16000:
                factor = gcd(rate, 16000)
                audio = resample_poly(audio, 16000 // factor, rate // factor).astype(np.float32)
            load_seconds = 0.0
            if not self.loaded:
                report("loading_model")
                self.models.load()
                self.loaded = True
                load_seconds = self.models.load_seconds
            inference_started = time.monotonic()
            segments, local_speakers = [], []
            if np.sqrt(np.mean(audio ** 2)) >= 1e-4:
                for index, offset in enumerate(range(0, len(audio), int(chunk_seconds * 16000))):
                    chunk = audio[offset:offset + int(chunk_seconds * 16000)]
                    report("transcribing", offset / 16000, duration)
                    raw = self.models.transcribe(chunk)
                    report("transcribing", min(duration, (offset + len(chunk)) / 16000), duration)
                    for entry in raw:
                        start, end = float(entry["start"]), float(entry["end"])
                        sid = entry.get("speaker_id")
                        if not sid or not np.isfinite([start, end]).all() or not 0 <= start < end <= len(chunk) / 16000 + 0.5:
                            raise ValueError("MOSS returned missing speaker IDs or invalid timestamps; shorten audio and retry")
                        segments.append({"start_s": start + offset / 16000,
                            "end_s": min(end, len(chunk) / 16000) + offset / 16000,
                            "text": re.sub(r"^\[S\d+\]\s*", "", entry["text"]),
                            "local_id": f"c{index}:{sid}", "chunk": index})
            asr_seconds = time.monotonic() - inference_started
            report("voiceprints", duration, duration)
            voice_started = time.monotonic()
            for local_id in dict.fromkeys(s["local_id"] for s in segments):
                utterances = [s for s in segments if s["local_id"] == local_id]
                eligible = []
                for s in utterances:
                    blockers = [(t["start_s"], t["end_s"]) for t in segments if t["local_id"] != local_id]
                    eligible.extend(clean_intervals(s["start_s"], s["end_s"], blockers))
                clip, used, total = pooled_clip(audio, eligible)
                vector = self.models.embedding(clip) if len(clip) >= 24000 else None
                samples = []
                if vector is not None:
                    vector = unit(vector).tolist()
                    samples.append({"start": used[0][0], "end": used[-1][1], "seconds": len(clip) / 16000,
                                    "vector": vector})
                local_speakers.append({"local_id": local_id, "chunk": utterances[0]["chunk"], "samples": samples,
                                       "vector": vector, "seconds": len(clip) / 16000 if vector else 0.0,
                                       "speech_seconds": total})
            speakers, mapping = [], {}
            for local in local_speakers:
                # IDs can restart in each MOSS request. Stitch only across chunks;
                # never collapse two different local IDs from the same chunk.
                candidates = []
                if local["vector"] is not None:
                    for speaker in speakers:
                        if local["chunk"] not in speaker["chunks"] and speaker["vector"] is not None:
                            candidates.append((float(np.dot(local["vector"], speaker["vector"])), speaker))
                candidates.sort(key=lambda x: x[0], reverse=True)
                # Joined speech on both sides is reliable from 8 s; shorter evidence needs a higher score.
                floor = 0.62 if candidates and min(local["seconds"], candidates[0][1]["seconds"]) >= 8 else 0.72
                if candidates and candidates[0][0] >= floor and (len(candidates) == 1 or candidates[0][0] - candidates[1][0] >= 0.05):
                    target = candidates[0][1]
                    target["samples"] = sorted(target["samples"] + local["samples"],
                                               key=lambda s: s.get("seconds", s["end"] - s["start"]), reverse=True)[:3]
                    weights = [s.get("seconds", s["end"] - s["start"]) for s in target["samples"]]
                    target["vector"] = unit(np.average([s["vector"] for s in target["samples"]], axis=0,
                                                       weights=weights)).tolist()
                    target["seconds"] = min(POOL_SECONDS * 3, target["seconds"] + local["seconds"])
                    target["speech_seconds"] += local["speech_seconds"]
                    target["chunks"].append(local["chunk"])
                else:
                    target = {"speaker_id": f"speaker_{len(speakers) + 1:02d}",
                              "samples": local["samples"], "vector": local["vector"], "chunks": [local["chunk"]],
                              "seconds": local["seconds"], "speech_seconds": local["speech_seconds"]}
                    speakers.append(target)
                mapping[local["local_id"]] = target["speaker_id"]
            for s in segments:
                s["speaker_id"] = mapping[s["local_id"]]
                del s["local_id"]
                del s["chunk"]
            result = {"schema_version": "1.0", "pipeline_version": PIPELINE_VERSION,
                "analysis_id": analysis_id, "source_hash": source_hash, "source_name": path.name,
                "duration_seconds": duration, "created_at": now(), "chunk_seconds": chunk_seconds,
                "transcription_model": {"id": MOSS_ID, "revision": MOSS_REV},
                "voice_model": {"id": VOICE_ID, "revision": VOICE_REV},
                "runtime": {name: version(name) for name in ("mlx-audio", "onnxruntime", "kaldi-native-fbank")},
                "segments": segments, "speakers": speakers, "timings": {
                    "load_seconds": load_seconds, "transcription_seconds": asr_seconds,
                    "embedding_seconds": time.monotonic() - voice_started},
                "timestamp_kind": "model_segment_estimate", "confidence": None}
            self.store.save(result)
        for speaker in result["speakers"]:
            speaker["identity"] = self.store.match(speaker["vector"], threshold, margin, seconds=speaker.get("seconds"))
        result["cache_hit"] = cached
        from .speaker_store import AUTO_SECONDS, AUTO_THRESHOLD, MAYBE_SECONDS
        result["matching_policy"] = {"cosine_threshold": AUTO_THRESHOLD if threshold is None else threshold,
                                      "minimum_margin": margin, "auto_seconds": AUTO_SECONDS,
                                      "minimum_seconds": MAYBE_SECONDS,
                                      "calibrated": False, "score_is_probability": False}
        result["elapsed_seconds"] = time.monotonic() - started
        result["real_time_factor"] = result["elapsed_seconds"] / result["duration_seconds"]
        return result


def public_result(result):
    # Embeddings stay in the local library/cache, not in the shareable report.
    report = json.loads(json.dumps(result))
    for speaker in report["speakers"]:
        speaker.pop("vector", None)
        for sample in speaker["samples"]:
            sample.pop("vector", None)
    return report
