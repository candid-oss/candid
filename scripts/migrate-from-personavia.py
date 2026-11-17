#!/usr/bin/env python3
"""Copy a Personavia / Personal AI library into Candid's data folder.

The original folder is only read: the timeline database is copied with SQLite's backup API (consistent even with WAL),
other files are copied as they are, and then the *copy* is rewritten:

  * format identifiers  personavia-…/N  →  candid-…/N, and  personavia://  →  candid://
  * paths               …/PersonalAI/…   →  …/Candid/…

Quit the old app before running. The target must not exist yet. Settings stored in the old app's preferences domain
and API keys in its Keychain entries are not moved: enter them again in Candid.

Usage:
  python3 scripts/migrate-from-personavia.py [--source DIR] [--target DIR] [--dry-run]
"""
import argparse
import shutil
import sqlite3
import sys
from pathlib import Path

SUPPORT = Path.home() / "Library/Application Support"
# Folders Candid reads; everything else in the old folder (backups, signing identities, logs) stays behind.
FOLDERS = ["timeline", "capture", "pi", "widget", "ai-providers", "MediaWorkspace"]
REPLACEMENTS = [("personavia://", "candid://"), ("personavia-", "candid-"), ("/PersonalAI/", "/Candid/")]


def rewrite_database(path: Path, dry_run: bool) -> int:
    """Rewrite text columns in place (on the copy). Returns the number of changed values."""
    db = sqlite3.connect(path)
    changed = 0
    tables = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
    for table in tables:
        try:
            columns = [r[1] for r in db.execute(f'PRAGMA table_info("{table}")') if (r[2] or "").upper() in ("TEXT", "")]
        except sqlite3.DatabaseError:
            continue
        for column in columns:
            condition = " OR ".join(f'"{column}" LIKE ?' for _ in REPLACEMENTS)
            params = [f"%{old}%" for old, _ in REPLACEMENTS]
            try:
                count = db.execute(f'SELECT count(*) FROM "{table}" WHERE typeof("{column}")=\'text\' AND ({condition})', params).fetchone()[0]
            except sqlite3.OperationalError:
                continue  # virtual-table shadow columns and the like
            if not count:
                continue
            changed += count
            if dry_run:
                print(f"  {table}.{column}: {count}")
                continue
            expression = f'"{column}"'
            for old, new in REPLACEMENTS:
                expression = f"replace({expression}, '{old}', '{new}')"
            db.execute(f'UPDATE "{table}" SET "{column}" = {expression} WHERE typeof("{column}")=\'text\' AND ({condition})', params)
    if not dry_run:
        db.commit()
        result = db.execute("PRAGMA integrity_check").fetchone()[0]
        if result != "ok":
            raise SystemExit(f"integrity_check failed on {path}: {result}")
    db.close()
    return changed


def copy_database(source: Path, target: Path) -> None:
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    dst = sqlite3.connect(target)
    with dst:
        src.backup(dst)
    src.close()
    dst.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=Path, default=SUPPORT / "PersonalAI")
    parser.add_argument("--target", type=Path, default=SUPPORT / "Candid")
    parser.add_argument("--dry-run", action="store_true", help="only report what would be rewritten")
    args = parser.parse_args()
    source, target = args.source.expanduser(), args.target.expanduser()
    if not source.is_dir():
        print(f"No old library at {source}", file=sys.stderr)
        return 1
    existing = [name for name in FOLDERS if (target / name).exists()]
    if existing and not args.dry_run:
        print(f"{target} already has {', '.join(existing)}; move it aside first. Nothing was changed.", file=sys.stderr)
        return 1

    for name in FOLDERS:
        folder = source / name
        if not folder.is_dir():
            continue
        databases = [p for p in folder.rglob("*") if p.suffix in (".sqlite", ".sqlite3", ".db") and p.is_file()]
        print(f"{name}: {len(databases)} database(s)")
        if args.dry_run:
            for db in databases:
                rewrite_database_copy = target.parent / f".candid-dry-run-{db.name}"
                copy_database(db, rewrite_database_copy)
                rewrite_database(rewrite_database_copy, True)
                rewrite_database_copy.unlink()
            continue
        skip = {p.name for p in databases} | {p.name + suffix for p in databases for suffix in ("-wal", "-shm", "-journal")}
        shutil.copytree(folder, target / name, ignore=lambda d, names: [n for n in names if n in skip], copy_function=shutil.copy2)
        for db in databases:
            destination = target / name / db.relative_to(folder)
            destination.parent.mkdir(parents=True, exist_ok=True)
            copy_database(db, destination)
            print(f"  {db.relative_to(source)}: {rewrite_database(destination, False)} value(s) rewritten, integrity ok")
    print("Dry run only; nothing was written." if args.dry_run else f"Copied to {target}. The original at {source} is unchanged.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
