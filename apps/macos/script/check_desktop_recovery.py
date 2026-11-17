#!/usr/bin/env python3
"""Seeded SIGKILL/reopen checks of the capture's outbox and its delivery into a throwaway timeline core.

`write` acknowledges a record once it is durable in the outbox; `verify` starts a core on the same synthetic folder
(`CANDID_CORE_BIN`), delivers what the outbox holds and reads every record back. This does not test power loss,
screen permissions, actual screen capture, or UI. The binaries must already be built by the caller; this script never
invokes a build.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import random
import selectors
import subprocess
import tempfile
import time


def run_json(binary: Path, mode: str, root: Path, timeout: float = 30) -> dict:
    result = subprocess.run(
        [str(binary), mode, "--data-dir", str(root)],
        check=True, capture_output=True, text=True, timeout=timeout,
    )
    lines = result.stdout.splitlines()
    if len(lines) != 1:
        raise RuntimeError(f"{mode}: expected one JSON result, got {len(lines)} lines")
    return json.loads(lines[0])


def killed_write(binary: Path, root: Path, receipt_target: int, log: Path) -> tuple[set[str], int]:
    child = subprocess.Popen(
        [str(binary), "write", "--data-dir", str(root), "--count", "500"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0,
    )
    assert child.stdout is not None
    received = bytearray()
    pending = bytearray()
    receipts: set[str] = set()
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)
    deadline = time.monotonic() + 30
    try:
        while len(receipts) < receipt_target:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("writer did not commit the expected receipts within 30 seconds")
            ready = selector.select(timeout=min(remaining, 2))
            if not ready:
                if child.poll() is not None:
                    raise RuntimeError("writer exited before the kill threshold")
                continue
            chunk = os.read(child.stdout.fileno(), 8192)
            if not chunk:
                raise RuntimeError("writer closed its receipt stream before the kill threshold")
            received.extend(chunk)
            pending.extend(chunk)
            while b"\n" in pending:
                line, _, tail = pending.partition(b"\n")
                pending = bytearray(tail)
                item = json.loads(line)
                receipts.add(item["id"])
        child.kill()  # SIGKILL: deliberately bypass application cleanup.
        stdout_tail, stderr = child.communicate(timeout=10)
        received.extend(stdout_tail)
        # Include complete receipts already emitted before SIGKILL but not read above.
        for line in received.splitlines():
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue  # A receipt interrupted before its complete line is not acknowledged.
            receipts.add(item["id"])
        log.write_bytes(received)
        log.with_suffix(".stderr.log").write_bytes(stderr)
        if child.returncode != -9:
            raise RuntimeError(f"expected SIGKILL (-9), got {child.returncode}; stderr={stderr.decode(errors='replace')}")
        return receipts, child.returncode
    finally:
        selector.close()
        if child.poll() is None:
            child.kill()
            child.communicate(timeout=10)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--seed", type=int, default=20261031)
    args = parser.parse_args()
    binary = args.binary.expanduser().resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must name an already-built executable")
    if not 1 <= args.rounds <= 100:
        parser.error("--rounds must be between 1 and 100")
    output = args.output_dir.expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="synthetic-desktop-recovery-", dir=output))
    root = run / "capture"
    rng = random.Random(args.seed)
    acknowledged: set[str] = set()
    previous_ids: set[str] = set()
    rounds: list[dict] = []
    started = time.monotonic()
    for index in range(args.rounds):
        threshold = rng.randint(2, 14)
        receipts, returncode = killed_write(binary, root, threshold, run / f"round-{index + 1:02d}.receipts.jsonl")
        acknowledged.update(receipts)
        verification = run_json(binary, "verify", root)
        ids = verification.get("ids")
        if not isinstance(ids, list) or len(ids) != len(set(ids)):
            raise AssertionError("verify returned invalid or duplicate IDs")
        current = set(ids)
        missing = acknowledged - current
        lost_previous = previous_ids - current
        if missing or lost_previous:
            raise AssertionError(f"round {index + 1}: {len(missing)} acknowledged and {len(lost_previous)} previous records lost")
        if verification["record_count"] != len(current):
            raise AssertionError("record count does not match the independently paginated IDs")
        result = {
            "round": index + 1, "kill_after_receipts_target": threshold,
            "receipts_read": len(receipts), "writer_returncode": returncode,
            "acknowledged_total": len(acknowledged), "recovered_records": len(current),
            "recovered_screenshots": verification["screenshot_count"],
            "verified_unique_images": verification["verified_unique_images"],
            "missing_acknowledged": len(missing), "lost_previous": len(lost_previous),
        }
        (run / f"round-{index + 1:02d}.verify.json").write_text(json.dumps(verification, indent=2) + "\n")
        rounds.append(result)
        previous_ids = current
    summary = {
        "result": "passed", "fixture": "synthetic_offscreen_jpeg", "rounds": rounds,
        "round_count": args.rounds, "seed": args.seed,
        "duration_seconds": time.monotonic() - started,
        "acknowledged_records": len(acknowledged), "recovered_records": len(previous_ids),
        "missing_acknowledged": 0, "lost_previous": 0,
        "scope": "process_SIGKILL_and_reopen; not power-cut, real screen capture, permissions, UI, or long-term stability",
        "data_dir": str(root), "binary": str(binary),
    }
    report = run / "summary.json"
    report.write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({"result": "passed", "round_count": args.rounds, "acknowledged_records": len(acknowledged), "recovered_records": len(previous_ids), "report_path": str(report)}))


if __name__ == "__main__":
    main()
