"""Pinned, offline model adapters for the independent speaker demo."""
import hashlib
import json
import os
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3]
MOSS_ID = "vanch007/mlx-MOSS-Transcribe-Diarize-8bit"
MOSS_REV = "7210aef739be6b9e068fba9b1d60369ec053655b"
MOSS_DIR = ROOT / "data/asr/MOSS-Transcribe-Diarize-8bit"
VOICE_ID = "Wespeaker/wespeaker-voxceleb-campplus-LM"
VOICE_REV = "c5e01c6fcffcce160861e7e79782828320192b5c"
VOICE_DIR = ROOT / "data/asr/voiceprint"
VOICE_FILE = "voxceleb_CAM++_LM.onnx"


def setup_models():
    from huggingface_hub import HfApi, snapshot_download
    for model_id, revision, directory, patterns in (
        (MOSS_ID, MOSS_REV, MOSS_DIR, None),
        (VOICE_ID, VOICE_REV, VOICE_DIR, [VOICE_FILE, "config.yaml", "README.md"]),
    ):
        snapshot_download(model_id, revision=revision, local_dir=directory,
                          allow_patterns=patterns, token=False)
        info = HfApi(token=False).model_info(model_id, revision=revision, files_metadata=True)
        if info.sha != revision:
            raise RuntimeError("Unexpected model revision")
        hashes = {}
        for entry in info.siblings:
            if patterns is not None and entry.rfilename not in patterns:
                continue
            path = directory / entry.rfilename
            with path.open("rb") as f:
                digest = hashlib.file_digest(f, "sha256").hexdigest()
            if entry.lfs:
                valid = digest == entry.lfs.sha256
            else:
                content = path.read_bytes()
                valid = hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest() == entry.blob_id
            if not valid:
                raise RuntimeError(f"Hub hash mismatch: {entry.rfilename}")
            hashes[entry.rfilename] = digest
        manifest = {"model_id": model_id, "revision": revision, "sha256": hashes}
        temp = directory / "deployment.json.tmp"
        temp.write_text(json.dumps(manifest, indent=2) + "\n")
        temp.replace(directory / "deployment.json")
        print(f"Verified {model_id}: {len(hashes)} files", flush=True)


def verify_model(directory, model_id, revision):
    manifest = json.loads((directory / "deployment.json").read_text())
    if manifest["model_id"] != model_id or manifest["revision"] != revision:
        raise RuntimeError("Model manifest mismatch; run speaker_demo.py setup")
    # Detect changed weights/config before putting derived vectors into the library.
    for name, expected in manifest["sha256"].items():
        with (directory / name).open("rb") as f:
            if hashlib.file_digest(f, "sha256").hexdigest() != expected:
                raise RuntimeError(f"Model hash mismatch: {name}")


class TokenLimit(RuntimeError):
    """The transcript of this audio does not fit one model call; split it and retry."""


def unit(vector):
    vector = np.asarray(vector, dtype=np.float32).reshape(-1)
    norm = np.linalg.norm(vector)
    if not np.isfinite(vector).all() or norm < 1e-8:
        raise ValueError("Invalid speaker embedding")
    return vector / norm


class Models:
    def __init__(self):
        self.moss = None
        self.voice = None
        self.load_seconds = 0.0

    def load(self):
        import time
        started = time.monotonic()
        os.environ["HF_HUB_OFFLINE"] = "1"
        os.environ["TRANSFORMERS_OFFLINE"] = "1"
        verify_model(MOSS_DIR, MOSS_ID, MOSS_REV)
        from mlx_audio.stt import load
        self.moss = load(MOSS_DIR, strict=True)
        self.load_voice()
        self.load_seconds = time.monotonic() - started

    def load_voice(self):
        import onnxruntime as ort
        verify_model(VOICE_DIR, VOICE_ID, VOICE_REV)
        options = ort.SessionOptions()
        options.intra_op_num_threads = 4
        options.inter_op_num_threads = 1
        self.voice = ort.InferenceSession(str(VOICE_DIR / VOICE_FILE), options,
                                         providers=["CPUExecutionProvider"])

    def transcribe(self, audio, max_tokens=4096):
        result = self.moss.generate(audio, max_tokens=max_tokens, verbose=False)
        if result.generation_tokens >= max_tokens:
            raise TokenLimit("MOSS output reached token limit; shorten the audio and retry")
        return result.segments

    def embedding(self, audio):
        if len(audio) < 24000 or np.sqrt(np.mean(audio ** 2)) < 1e-4:
            return None
        import kaldi_native_fbank as knf
        options = knf.FbankOptions()
        options.frame_opts.samp_freq = 16000
        options.frame_opts.frame_length_ms = 25
        options.frame_opts.frame_shift_ms = 10
        options.frame_opts.dither = 0
        options.frame_opts.window_type = "hamming"
        options.mel_opts.num_bins = 80
        fbank = knf.OnlineFbank(options)
        fbank.accept_waveform(16000, (audio * 32768).tolist())
        fbank.input_finished()
        feats = np.stack([fbank.get_frame(i) for i in range(fbank.num_frames_ready)])
        feats -= feats.mean(axis=0, keepdims=True)
        vector = self.voice.run(None, {self.voice.get_inputs()[0].name: feats[None].astype(np.float32)})[0][0]
        return unit(vector)
