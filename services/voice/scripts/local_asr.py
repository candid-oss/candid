#!/usr/bin/env python3
"""Download, serve, transcribe, and control the local ASR process on macOS."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
VOICE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(VOICE))


def download():
    from huggingface_hub import HfApi, snapshot_download
    from asr.runtime import MODEL_DIR, MODEL_ID, MODEL_REVISION

    snapshot_download(MODEL_ID, revision=MODEL_REVISION, local_dir=str(MODEL_DIR), token=False)
    info = HfApi(token=False).model_info(MODEL_ID, revision=MODEL_REVISION, files_metadata=True)
    if info.sha != MODEL_REVISION:
        raise RuntimeError("Unexpected Hugging Face snapshot revision")
    verified = {}
    for entry in info.siblings:
        path = MODEL_DIR / entry.rfilename
        with path.open("rb") as f:
            digest = hashlib.file_digest(f, "sha256").hexdigest()
        if entry.lfs:
            valid = digest == entry.lfs.sha256
        else:
            # Small files use Git blob SHA1; record SHA256 locally after verifying.
            content = path.read_bytes()
            valid = hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest() == entry.blob_id
        if not valid:
            raise RuntimeError(f"Hub hash mismatch: {entry.rfilename}")
        verified[entry.rfilename] = digest
    manifest = {"model_id": MODEL_ID, "model_revision": MODEL_REVISION, "sha256": verified}
    (MODEL_DIR / "deployment.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Verified {len(verified)} files: {MODEL_DIR}")


def launchd_config(label, state):
    return {
        "Label": label, "ProgramArguments": [str(ROOT / ".venv-asr-mlx/bin/python"),
            str(Path(__file__).resolve()), "serve"], "WorkingDirectory": str(ROOT),
        "RunAtLoad": True, "StandardOutPath": str(state / "server.log"),
        "StandardErrorPath": str(state / "server.log"),
        # Transcription is background work: the scheduler may run it slowly and on efficiency cores.
        "ProcessType": "Background", "LowPriorityBackgroundIO": True,
        "EnvironmentVariables": {"HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1",
            "PYTHONUNBUFFERED": "1",
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin"},
    }


def service(action):
    label = "io.github.candid-oss.voice"
    domain = f"gui/{os.getuid()}"
    target = f"{domain}/{label}"
    if action in {"status", "stop"}:
        command = ["launchctl", "print", target] if action == "status" else ["launchctl", "bootout", target]
        return subprocess.run(command).returncode
    state = ROOT / "data/asr"
    state.mkdir(parents=True, exist_ok=True)
    plist = state / f"{label}.plist"
    config = launchd_config(label, state)
    with plist.open("wb") as f:
        plistlib.dump(config, f)
    # launchd may still be releasing a just-stopped service after bootout returns.
    for attempt in range(10):
        result = subprocess.run(["launchctl", "bootstrap", domain, str(plist)],
                                capture_output=True, text=True)
        if result.returncode == 0:
            break
        if result.returncode != 5 or attempt == 9:
            raise RuntimeError(result.stderr.strip())
        time.sleep(0.5)
    print("Starting http://127.0.0.1:8767; readiness: /health; log: data/asr/server.log")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    sub.add_parser("download")
    sub.add_parser("serve")
    for name in ("start", "stop", "status"):
        sub.add_parser(name)
    tr = sub.add_parser("transcribe")
    tr.add_argument("audio", type=Path)
    tr.add_argument("--language", default=None)
    args = p.parse_args()
    if args.command == "download":
        download()
    elif args.command == "serve":
        import uvicorn
        uvicorn.run("asr.app:create_app", factory=True, app_dir=str(VOICE),
                    host="127.0.0.1", port=8767, workers=1)
    elif args.command == "transcribe":
        import httpx
        with args.audio.open("rb") as f:
            r = httpx.post("http://127.0.0.1:8767/v1/audio/transcriptions",
                           files={"file": (args.audio.name, f)},
                           data={"language": args.language} if args.language else {}, timeout=600)
        r.raise_for_status()
        print(json.dumps(r.json(), ensure_ascii=False, indent=2))
    else:
        sys.exit(service(args.command))


if __name__ == "__main__":
    main()
