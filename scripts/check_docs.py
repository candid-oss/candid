#!/usr/bin/env python3
"""Checks the documentation rules that a machine can check (docs/README.md, "文档标准").

1. Every relative link in a Markdown file points at a file that exists.
2. Specs, decisions and contracts exist in both languages (X.md and X.en.md).
3. Every spec, decision and contract is listed in docs/README.md.

Standard library only. Run from anywhere: python3 scripts/check_docs.py
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKIP_PARTS = {"node_modules", ".build", "target", "worktrees"}
BILINGUAL_DIRS = ["docs/specs", "docs/decisions", "docs/contracts"]
INDEXED_DIRS = BILINGUAL_DIRS
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FENCE = re.compile(r"^\s*(```|~~~)")


def markdown_files() -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "-co", "--exclude-standard", "*.md"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout.splitlines()
    files = []
    for line in out:
        path = ROOT / line
        # Parts relative to the repository: the checkout itself may live under a directory named like these.
        if path.is_file() and not SKIP_PARTS & set(Path(line).parts):
            files.append(path)
    return files


def broken_links(path: Path) -> list[str]:
    problems = []
    in_fence = False
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for target in LINK.findall(re.sub(r"`[^`]*`", "", line)):
            if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith("#"):
                continue
            file_part = target.split("#", 1)[0].split("?", 1)[0]
            if not file_part:
                continue
            if not (path.parent / file_part).resolve().exists():
                problems.append(f"{path.relative_to(ROOT)}:{number}: broken link {target}")
    return problems


def missing_pairs() -> list[str]:
    problems = []
    for directory in BILINGUAL_DIRS:
        for path in sorted((ROOT / directory).glob("*.md")):
            if path.name == "README.md":
                continue
            if path.name.endswith(".en.md"):
                other = path.with_name(path.name[: -len(".en.md")] + ".md")
            else:
                other = path.with_name(path.stem + ".en.md")
            if not other.exists():
                problems.append(f"{path.relative_to(ROOT)}: missing the other language ({other.name})")
    return problems


def unlisted() -> list[str]:
    index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
    problems = []
    for directory in INDEXED_DIRS:
        for path in sorted((ROOT / directory).glob("*.md")):
            relative = path.relative_to(ROOT / "docs").as_posix()
            if relative not in index:
                problems.append(f"docs/{relative}: not listed in docs/README.md")
    return problems


def main() -> int:
    problems = []
    for path in markdown_files():
        problems += broken_links(path)
    problems += missing_pairs()
    problems += unlisted()
    if problems:
        print("\n".join(problems))
        print(f"\n{len(problems)} documentation problem(s).", file=sys.stderr)
        return 1
    print("Docs check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
