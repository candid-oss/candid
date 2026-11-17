#!/usr/bin/env python3
"""Real SIGKILL of synthetic-only writer children at three persistence boundaries."""
import argparse
import hashlib
import json
import signal
import struct
import subprocess
import tempfile
import time
import wave
from pathlib import Path

MAC = Path(__file__).resolve().parents[1]
SOURCE = MAC / "Sources/Candid/Recording"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--disk-full", action="store_true", help="also create, fill and detach a private 12 MiB HFS+ image")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="app008-kill-synthetic-") as temporary:
        work = Path(temporary)
        probe = work / "probe"
        # The writer files are compiled into one probe module, so their `import CandidLocalization` is dropped and the
        # localization source is compiled alongside them.
        sources = []
        for name in ("RecordingInput.swift", "RecordingJournal.swift", "SegmentedRecordingWriter.swift"):
            copy = work / name
            copy.write_text("".join(line for line in (SOURCE / name).read_text().splitlines(keepends=True)
                                    if line.strip() != "import CandidLocalization"))
            sources.append(str(copy))
        subprocess.run([
            "xcrun", "swiftc", *sources, str(MAC / "Sources/CandidLocalization/Localization.swift"),
            str(MAC / "script/recording_recovery_probe.swift"), "-o", str(probe),
        ], check=True)
        experiments = 0
        for mode in ("mid-write", "media-renamed", "committed"):
            for repeat in range(5):
                profile = work / f"{mode}-{repeat}"
                process = subprocess.Popen([str(probe), mode, str(profile)], stdout=subprocess.DEVNULL)
                try:
                    deadline = time.monotonic() + 15
                    while not (profile / "ready").exists():
                        if process.poll() is not None:
                            raise AssertionError(f"writer exited before checkpoint: {mode}")
                        if time.monotonic() > deadline:
                            raise AssertionError(f"checkpoint timed out: {mode}")
                        time.sleep(0.01)
                    process.kill()  # Only this exact child PID; never an App or service.
                    assert process.wait(timeout=5) == -signal.SIGKILL
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=5)
                def recover():
                    return json.loads(subprocess.check_output([str(probe), "recover", str(profile)], timeout=10))
                segments = recover()
                assert len(segments) == (1 if mode == "mid-write" else 2)
                assert recover() == segments, "repeated recovery changed IDs or facts"
                joined = bytearray()
                for index, segment in enumerate(segments):
                    path = profile / "Recordings" / segment["recordingID"] / f"{index:06d}.wav"
                    assert segment["sampleStart"] == index * 240_000
                    assert segment["sampleCount"] == 240_000 and segment["recovered"]
                    assert hashlib.sha256(path.read_bytes()).hexdigest() == segment["sha256"]
                    with wave.open(str(path), "rb") as audio:
                        assert (audio.getframerate(), audio.getnchannels(), audio.getsampwidth(), audio.getnframes()) == (8000, 1, 2, 240_000)
                        joined.extend(audio.readframes(audio.getnframes()))
                expected = struct.pack(f"<{len(joined) // 2}h", *(
                    (i + 32768) % 65536 - 32768 for i in range(len(joined) // 2)
                ))
                assert joined == expected, "sample discontinuity"
                experiments += 1
        if args.disk_full:
            image = work / "synthetic.dmg"
            mount = work / "mount"
            mount.mkdir()
            subprocess.run(["hdiutil", "create", "-size", "12m", "-fs", "HFS+", "-volname", "APP008Synthetic", str(image)],
                           check=True, capture_output=True)
            subprocess.run(["hdiutil", "attach", "-nobrowse", "-noautoopen", "-mountpoint", str(mount), str(image)],
                           check=True, capture_output=True)
            try:
                subprocess.run([str(probe), "disk-full", str(mount / "synthetic")], check=True, timeout=30)
            finally:
                subprocess.run(["hdiutil", "detach", str(mount)], check=True, capture_output=True)
        print(f"APP-008: {experiments} SIGKILL experiments passed (synthetic audio; no power-loss experiment).")


if __name__ == "__main__":
    main()
