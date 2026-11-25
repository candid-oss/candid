#!/usr/bin/env python3
"""Checks the documentation rules that a machine can check (docs/standard.md).

1. Every relative link in a Markdown file points at a file that exists, and its #anchor exists there.
2. Specs and decision records exist in both languages (X.md and X.en.md).
3. Every spec, decision and research note is listed in the README.md of its directory.
4. Every spec has a status line and stays under the length limit.

Standard library only. Run from anywhere: python3 scripts/check_docs.py
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKIP_PARTS = {"node_modules", ".build", "target", "worktrees"}
# Two files per document: X.md (Chinese, authoritative) and X.en.md.
BILINGUAL_DIRS = ["docs/specs", "docs/decisions"]
# Chinese only, never paired.
SINGLE_LANGUAGE = {"README.md", "log.md"}
# Every document must be listed in the README.md of its own directory.
INDEXED_DIRS = ["docs/specs", "docs/decisions", "docs/research"]
# docs/standard.md 5.3: a spec covers one topic and stays readable in one sitting.
SPEC_LIMIT = {"zh": 20000, "en": 48000}
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
REFERENCE = re.compile(r"^\[[^\]]+\]:\s+(\S+)")
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


ANCHOR_ID = re.compile(r'<a id="([^"]+)"')
HEADING = re.compile(r"^#{1,6}\s+(.*)$", re.M)
_anchors: dict[Path, set[str]] = {}


def anchors(path: Path) -> set[str]:
    """Hand-written `<a id>` anchors plus GitHub-style heading slugs."""
    if path not in _anchors:
        text = path.read_text(encoding="utf-8")
        found = set(ANCHOR_ID.findall(text))
        for heading in HEADING.findall(text):
            plain = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading).replace("`", "").strip().lower()
            found.add(re.sub(r"\s", "-", re.sub(r"[^\w\s-]", "", plain)))
        _anchors[path] = found
    return _anchors[path]


def broken_links(path: Path) -> list[str]:
    problems = []
    in_fence = False
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        targets = LINK.findall(re.sub(r"`[^`]*`", "", line))
        reference = REFERENCE.match(line)  # reference-style definition: [label]: target
        if reference:
            targets.append(reference.group(1))
        for target in targets:
            if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target):
                continue
            file_part, _, fragment = target.partition("#")
            file_part = file_part.split("?", 1)[0]
            resolved = (path.parent / file_part).resolve() if file_part else path
            where = f"{path.relative_to(ROOT)}:{number}"
            if not resolved.exists():
                problems.append(f"{where}: broken link {target}")
            elif fragment and resolved.suffix == ".md" and resolved.is_file() and fragment not in anchors(resolved):
                problems.append(f"{where}: no anchor #{fragment} in {resolved.relative_to(ROOT)}")
    return problems


def missing_pairs() -> list[str]:
    problems = []
    for directory in BILINGUAL_DIRS:
        for path in sorted((ROOT / directory).glob("*.md")):
            if path.name in SINGLE_LANGUAGE:
                continue
            if path.name.endswith(".en.md"):
                other = path.with_name(path.name[: -len(".en.md")] + ".md")
            else:
                other = path.with_name(path.stem + ".en.md")
            if not other.exists():
                problems.append(f"{path.relative_to(ROOT)}: missing the other language ({other.name})")
    return problems


def unlisted() -> list[str]:
    problems = []
    for directory in INDEXED_DIRS:
        folder = ROOT / directory
        if not folder.is_dir():
            continue
        index_path = folder / "README.md"
        if not index_path.exists():
            problems.append(f"{directory}/README.md: missing (the index of this directory)")
            continue
        index = index_path.read_text(encoding="utf-8")
        for path in sorted(folder.glob("*.md")):
            if path.name != "README.md" and f"({path.name}" not in index:
                problems.append(f"{directory}/{path.name}: not listed in {directory}/README.md")
    return problems


def spec_shape() -> list[str]:
    """Specs carry a one-line status near the top and stay under the length limit."""
    problems = []
    for path in sorted((ROOT / "docs/specs").glob("*.md")):
        if path.name == "README.md":
            continue
        text = path.read_text(encoding="utf-8")
        english = path.name.endswith(".en.md")
        marker = "**Status**" if english else "**状态**"
        if not any(line.startswith(marker) for line in text.splitlines()[:12]):
            problems.append(f"{path.relative_to(ROOT)}: no {marker} line in the first 12 lines")
        limit = SPEC_LIMIT["en" if english else "zh"]
        if len(text) > limit:
            problems.append(f"{path.relative_to(ROOT)}: {len(text)} characters, over the limit of {limit}; split it by sub-topic")
    return problems


def main() -> int:
    problems = []
    for path in markdown_files():
        problems += broken_links(path)
    problems += missing_pairs()
    problems += unlisted()
    problems += spec_shape()
    if problems:
        print("\n".join(problems))
        print(f"\n{len(problems)} documentation problem(s).", file=sys.stderr)
        return 1
    print("Docs check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
