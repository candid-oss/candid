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

PIPELINE_VERSION = "speaker-demo-1.0"


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


class SpeakerPipeline:
    def __init__(self, store, models=None):
        self.store = store
        self.models = models or Models()
        self.loaded = False

    def analyze(self, audio_path, threshold=0.65, margin=0.05, chunk_seconds=60):
        if not 10 <= chunk_seconds <= 120:
            raise ValueError("Chunk size must be between 10 and 120 seconds")
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
                samples = []
                for start, end in sorted(eligible, key=lambda x: x[1] - x[0], reverse=True):
                    end = min(end, start + 12)
                    clip = audio[round(start * 16000):round(end * 16000)]
                    if len(clip) < 24000 or np.mean(np.abs(clip) >= 0.999) > 0.05:
                        continue
                    vector = self.models.embedding(clip)
                    if vector is not None:
                        samples.append({"start": start, "end": end, "vector": unit(vector).tolist()})
                    if len(samples) == 3:
                        break
                vector = unit(np.mean([s["vector"] for s in samples], axis=0)).tolist() if samples else None
                local_speakers.append({"local_id": local_id, "chunk": utterances[0]["chunk"],
                                       "samples": samples, "vector": vector})
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
                if candidates and candidates[0][0] >= 0.72 and (len(candidates) == 1 or candidates[0][0] - candidates[1][0] >= 0.05):
                    target = candidates[0][1]
                    target["samples"] = sorted(target["samples"] + local["samples"],
                                               key=lambda s: s["end"] - s["start"], reverse=True)[:3]
                    target["vector"] = unit(np.mean([s["vector"] for s in target["samples"]], axis=0)).tolist()
                    target["chunks"].append(local["chunk"])
                else:
                    target = {"speaker_id": f"speaker_{len(speakers) + 1:02d}",
                              "samples": local["samples"], "vector": local["vector"], "chunks": [local["chunk"]]}
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
            speaker["identity"] = self.store.match(speaker["vector"], threshold, margin)
        result["cache_hit"] = cached
        result["matching_policy"] = {"cosine_threshold": threshold, "minimum_margin": margin,
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
