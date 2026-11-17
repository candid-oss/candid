#!/usr/bin/env python3
"""Measure a consistent, disposable copy. The source and its media are never changed."""
import argparse
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--start", required=True)
    parser.add_argument("--end", required=True, help="exclusive local date")
    parser.add_argument("--timezone", required=True)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/candid-metrics")
    args = parser.parse_args()
    # mode=ro must not be replaced by immutable: a live WAL must be included.
    source = args.db.resolve().as_uri() + "?mode=ro"
    with tempfile.TemporaryDirectory(prefix="candid-metrics-") as tmp:
        copy = Path(tmp) / "timeline.sqlite"
        with sqlite3.connect(source, uri=True) as src, sqlite3.connect(copy) as dst:
            src.execute("PRAGMA query_only=ON")
            src.backup(dst)
        os.chmod(copy, 0o600)
        return subprocess.run([str(args.binary.resolve()), "--timeline", str(copy),
                               "--start", args.start, "--end", args.end,
                               "--timezone", args.timezone], check=False).returncode


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, sqlite3.Error):
        raise SystemExit("无法备份或读取测量库；请检查路径、只读权限与指标程序构建结果。")
