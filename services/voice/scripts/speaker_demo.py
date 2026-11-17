#!/usr/bin/env python3
"""Independent local multi-speaker/voiceprint demo; does not control devices."""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
VOICE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(VOICE))
from asr.speaker_models import setup_models
from asr.speaker_pipeline import SpeakerPipeline, public_result
from asr.speaker_store import SpeakerStore, now

DEFAULT_ROOT = ROOT / "data/speaker-demo"
PUBLIC_URLS = {
    "conversation.wav": ("https://cdn.jsdelivr.net/gh/pyannote/pyannote-audio@develop/tutorials/assets/sample.wav",
                         "https://github.com/pyannote/pyannote-audio/blob/develop/tutorials/assets/sample.wav"),
    "chinese.wav": ("https://qianwen-res.oss-cn-beijing.aliyuncs.com/Qwen3-ASR-Repo/asr_zh.wav",
                    "https://github.com/QwenLM/Qwen3-ASR"),
    "english.wav": ("https://qianwen-res.oss-cn-beijing.aliyuncs.com/Qwen3-ASR-Repo/asr_en.wav",
                    "https://github.com/QwenLM/Qwen3-ASR"),
}


def write_report(root, result, filename=None):
    directory = root / "reports"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / (filename or (result["analysis_id"] + ".json"))
    path.write_text(json.dumps(public_result(result), ensure_ascii=False, indent=2) + "\n")
    return path


def show(result):
    print(f"analysis_id: {result['analysis_id']}")
    print(f"{result['duration_seconds']:.2f}s audio | {result['elapsed_seconds']:.3f}s elapsed | "
          f"RTF {result['real_time_factor']:.3f} | cache={result['cache_hit']}")
    people = {s["speaker_id"]: s["identity"]["name"] or "unknown" for s in result["speakers"]}
    for speaker in result["speakers"]:
        match = speaker["identity"]
        print(f"  {speaker['speaker_id']}: {people[speaker['speaker_id']]} | "
              f"score={match['score']} | samples={len(speaker['samples'])} | {match['reason']}")
    for s in result["segments"]:
        print(f"  {s['start_s']:7.2f}–{s['end_s']:7.2f} {s['speaker_id']} "
              f"({people[s['speaker_id']]}) {s['text']}")


def fetch_public(root):
    directory = root / "public-audio"
    directory.mkdir(parents=True, exist_ok=True)
    manifest = []
    for filename, (url, source) in PUBLIC_URLS.items():
        path = directory / filename
        if not path.exists():
            temp = path.with_suffix(".part")
            with urllib.request.urlopen(url, timeout=60) as response, temp.open("wb") as f:
                while block := response.read(1024 * 1024):
                    f.write(block)
                    if f.tell() > 64 * 1024 * 1024:
                        raise ValueError("Public sample exceeds download limit")
            temp.replace(path)
        manifest.append({"file": filename, "source_url": source, "download_url": url,
                         "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "checked_at": now(),
                         "purpose": "Public tutorial audio for local functional testing; no real identity enrollment"})
    (directory / "sources.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return directory


def demo(root):
    """An isolated library exercise; cached inference is reused across demo runs."""
    import soundfile as sf
    from asr.speaker_models import unit
    # Never use or erase the user's default library for automated sample enrollment.
    demo_root = root / "public-test"
    store = SpeakerStore(demo_root)
    for person in store.people():
        store.delete(person["id"])
    audio_dir = fetch_public(root)
    pipeline = SpeakerPipeline(store)
    paths = [audio_dir / name for name in PUBLIC_URLS]
    print("1/4 Analyze public audio with an empty, isolated test library", flush=True)
    results = [pipeline.analyze(path) for path in paths]
    assert len(results[0]["speakers"]) >= 2, "Public conversation did not produce multiple speakers"
    assert all(s["identity"]["status"] == "unknown" for r in results for s in r["speakers"])
    for index, result in enumerate(results):
        write_report(demo_root, result, f"before-{index}.json")
    enrollment = []
    heldout = []
    print("2/4 Enroll demo speaker labels; reserve different utterances for matching", flush=True)
    for speaker in results[0]["speakers"]:
        if not speaker["samples"]:
            continue
        person = store.enroll(results[0]["analysis_id"], speaker["speaker_id"],
                              name=f"Demo_{speaker['speaker_id']}", confirmed=True, sample_limit=1)
        enrollment.append(person)
        # A different utterance from the same recording, exported as a new file.
        # This is a held-out utterance test, not cross-session accuracy evidence.
        if len(speaker["samples"]) > 1:
            sample = speaker["samples"][1]
            audio, rate = sf.read(paths[0], dtype="float32")
            clip = audio[round(sample["start"] * rate):round(sample["end"] * rate)]
            if pipeline.models.voice is None:
                # Cache-only runs initially load just the small ONNX model.
                pipeline.models.load_voice()
            vector = pipeline.models.embedding(clip)
            clip_path = audio_dir / f"heldout-{speaker['speaker_id']}.wav"
            sf.write(clip_path, clip, rate)
            heldout.append({"expected_demo_person_id": person["id"], "file": str(clip_path),
                            "source_hash": results[0]["source_hash"],
                            "interval": {"start": sample["start"], "end": sample["end"]},
                            "vector": unit(vector).tolist()})
    after = [pipeline.analyze(path) for path in paths]
    assert all(r["cache_hit"] for r in after)
    for index, result in enumerate(after):
        write_report(demo_root, result, f"after-{index}.json")
        show(result)
    for item in heldout:
        item["match"] = store.match(item.pop("vector"))
        item["correct_demo_match"] = item["match"]["person_id"] == item["expected_demo_person_id"]
        check = pipeline.analyze(item["file"])
        write_report(demo_root, check)
        item["pipeline_matches"] = [s["identity"] for s in check["speakers"]]
        item["correct_pipeline_match"] = any(m["person_id"] == item["expected_demo_person_id"] for m in item["pipeline_matches"])
        print(f"Held-out {Path(item['file']).name}: {item['match']['status']}, score={item['match']['score']:.3f}")
    # A clearly labelled construction exercises actual multi-chunk execution;
    # it is not an independent meeting or an accuracy benchmark.
    import numpy as np
    base_audio, base_rate = sf.read(paths[0], dtype="float32")
    long_path = audio_dir / "constructed-repeat-120s.wav"
    sf.write(long_path, np.tile(base_audio, 4), base_rate)
    print("3/4 Analyze 120s constructed repeat to exercise chunk offsets and speaker stitching", flush=True)
    long_result = pipeline.analyze(long_path)
    write_report(demo_root, long_result)
    before_count = store.people()[0]["sample_count"] if enrollment else 0
    if enrollment:
        first = enrollment[0]
        store.rename(first["id"], "Demo_renamed")
        # Repeating the same explicit enrollment is idempotent.
        first_speaker = next(s for s in results[0]["speakers"] if s["samples"])
        repeated = store.enroll(results[0]["analysis_id"], first_speaker["speaker_id"],
                                person_id=first["id"], confirmed=True, sample_limit=1)
        assert repeated["sample_count"] == before_count
        for person in enrollment:
            store.delete(person["id"])
    assert not store.people()
    assert all(s["identity"]["status"] == "unknown" for s in pipeline.analyze(paths[0])["speakers"])
    print("4/4 Rename, idempotent enrollment, and delete passed; isolated test library is empty", flush=True)
    summary = {"schema_version": "1.0", "created_at": now(), "scope": "functional demo; not accuracy benchmark",
        "internet_audio_count": len(paths), "speakers_in_conversation": len(results[0]["speakers"]),
        "cache_reuse_passed": True, "empty_library_unknown_passed": True,
        "rename_idempotent_enrollment_delete_passed": True, "heldout_same_recording": heldout,
        "unregistered_audio_results": [{"file": r["source_name"],
            "identities": [s["identity"] for s in r["speakers"]]} for r in after[1:]],
        "cold_or_initial_results": [{"file": r["source_name"], "seconds": r["elapsed_seconds"],
            "rtf": r["real_time_factor"], "cache_hit": r["cache_hit"], "timings": r["timings"]} for r in results],
        "cached_seconds": [r["elapsed_seconds"] for r in after],
        "constructed_multichunk": {"source_hash": results[0]["source_hash"], "repeat_count": 4,
            "duration_seconds": long_result["duration_seconds"], "speaker_count": len(long_result["speakers"]),
            "segment_count": len(long_result["segments"]), "elapsed_seconds": long_result["elapsed_seconds"],
            "rtf": long_result["real_time_factor"], "cache_hit": long_result["cache_hit"]},
        "note": "Public names are demo labels. Demo library emptied after CRUD checks; reports retain the test snapshots."}
    output = demo_root / "summary.json"
    output.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
    print(f"\nFunctional demo complete: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="Local cache/library directory")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("setup", help="Explicitly download and verify the two models")
    sub.add_parser("demo", help="Download public audio and exercise an isolated test library")
    run = sub.add_parser("analyze")
    run.add_argument("audio", type=Path)
    run.add_argument("--threshold", type=float, default=0.65)
    run.add_argument("--margin", type=float, default=0.05)
    run.add_argument("--chunk-seconds", type=int, default=60)
    enroll = sub.add_parser("enroll")
    enroll.add_argument("analysis_id")
    enroll.add_argument("speaker_id")
    target = enroll.add_mutually_exclusive_group(required=True)
    target.add_argument("--name")
    target.add_argument("--person-id")
    enroll.add_argument("--confirmed", action="store_true", help="Confirm permission and the selected speaker's identity")
    sub.add_parser("list")
    rename = sub.add_parser("rename")
    rename.add_argument("person_id")
    rename.add_argument("name")
    delete = sub.add_parser("delete")
    delete.add_argument("person_id")
    forget = sub.add_parser("forget")
    forget.add_argument("analysis_id", help="Remove cached analysis; enrolled templates are deleted with delete")
    args = parser.parse_args()
    if args.command == "setup":
        setup_models()
        return
    if args.command == "demo":
        demo(args.root.resolve())
        return
    store = SpeakerStore(args.root)
    if args.command == "analyze":
        result = SpeakerPipeline(store).analyze(args.audio, args.threshold, args.margin, args.chunk_seconds)
        show(result)
        print(f"Report: {write_report(store.root, result)}")
    elif args.command == "enroll":
        print(json.dumps(store.enroll(args.analysis_id, args.speaker_id, args.name,
                                    args.person_id, args.confirmed), ensure_ascii=False, indent=2))
    elif args.command == "list":
        print(json.dumps(store.people(), ensure_ascii=False, indent=2))
    elif args.command == "rename":
        store.rename(args.person_id, args.name)
        print("Renamed")
    elif args.command == "delete":
        store.delete(args.person_id)
        print("Person and enrolled templates deleted; historical report files are independent snapshots")
    elif args.command == "forget":
        store.forget(args.analysis_id)
        print("Cached analysis deleted")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, OSError, sqlite3.Error) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(2)
