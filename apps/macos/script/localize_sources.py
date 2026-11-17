#!/usr/bin/env python3
"""Wrap Chinese UI string literals in L(...) for localization, and extract the keys.

    python3 script/localize_sources.py --apply      # rewrite sources in place
    python3 script/localize_sources.py --extract    # print every key (JSON list) found in L("...") calls

Rules: only literals containing CJK text are wrapped. Comments, multi-line and raw strings, log calls, `case`
patterns and raw values, dictionary keys, comparisons and string-matching arguments are left alone. A literal with
interpolation becomes L("…%@…", value, …); interpolations with format options (`\\(x, specifier:)`), Text or Image
are left alone and reported. Re-running is safe: literals already inside L(…) are skipped.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "Sources"
TARGETS = ["Candid", "ConsoleCore", "AIProviders", "DesktopPlatform", "CandidWidget", "DesktopCore", "CandidClient"]
CJK = re.compile(r"[㐀-鿿＀-￯　-〿]")
SKIP_BEFORE = re.compile(
    r"(\blogger\.\w+\(|\bLogger\(|\bos_log\(|\bprint\(|\bsignposter\.|\bfatalError\(|\bassert(ionFailure)?\(|\bprecondition(Failure)?\("
    r"|\bhasPrefix\(|\bhasSuffix\(|\bcontains\(|\brange\(of:|\bcomponents\(separatedBy:|\breplacingOccurrences\(of:|\bwith:\s*$"
    r"|\bfirstIndex\(of:|\bsplit\(separator:|\bstarts\(with:|\bverbatim:\s*$|@available\(|==\s*$|!=\s*$|\bL\(\s*$|\bLocalizedStringKey\(\s*$)"
)
report = {"wrapped": 0, "skipped": {}}


def skip(reason):
    report["skipped"][reason] = report["skipped"].get(reason, 0) + 1


def scan_string(src, i):
    """src[i] == '"' (not triple). Returns (end_index_after_quote, parts) where parts is a list of
    ('text', str) and ('interp', expr_source)."""
    j = i + 1
    parts, buf = [], []
    while j < len(src):
        c = src[j]
        if c == "\\":
            if src[j + 1] == "(":
                depth, k = 1, j + 2
                while depth:
                    ch = src[k]
                    if ch == '"':
                        k, _ = scan_string(src, k)
                        continue
                    if ch == "(":
                        depth += 1
                    elif ch == ")":
                        depth -= 1
                    k += 1
                if buf:
                    parts.append(("text", "".join(buf)))
                    buf = []
                parts.append(("interp", src[j + 2:k - 1]))
                j = k
                continue
            buf.append(src[j:j + 2])
            j += 2
            continue
        if c == '"':
            if buf:
                parts.append(("text", "".join(buf)))
            return j + 1, parts
        if c == "\n":
            raise ValueError("unterminated string")
        buf.append(c)
        j += 1
    raise ValueError("unterminated string")


def top_level_comma(expr):
    depth = 0
    i = 0
    while i < len(expr):
        c = expr[i]
        if c == '"':
            i, _ = scan_string(expr, i)
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            return True
        i += 1
    return False


def transform(src):
    out = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            out.append(src[i:j]); i = j; continue
        if src.startswith("/*", i):
            depth, j = 1, i + 2
            while depth and j < n:
                if src.startswith("/*", j): depth += 1; j += 2
                elif src.startswith("*/", j): depth -= 1; j += 2
                else: j += 1
            out.append(src[i:j]); i = j; continue
        if src.startswith('"""', i):
            j = src.find('"""', i + 3)
            j = n if j < 0 else j + 3
            out.append(src[i:j]); i = j; continue
        if c == "#" and i + 1 < n and src[i + 1] in '#"':
            m = re.match(r'(#+)"', src[i:])
            if m:
                hashes = m.group(1)
                if src.startswith('"""', i + len(hashes)):
                    end = src.find('"""' + hashes, i + len(hashes) + 3)
                    j = n if end < 0 else end + 3 + len(hashes)
                else:
                    end = src.find('"' + hashes, i + len(hashes) + 1)
                    j = n if end < 0 else end + 1 + len(hashes)
                out.append(src[i:j]); i = j; continue
        if c == '"':
            end, parts = scan_string(src, i)
            literal = src[i:end]
            out.append(rewrite(src, i, end, literal, parts))
            i = end
            continue
        out.append(c)
        i += 1
    return "".join(out)


def rewrite(src, start, end, literal, parts):
    text = "".join(p[1] for p in parts if p[0] == "text")
    exprs = [p[1] for p in parts if p[0] == "interp"]
    # Nested literals inside interpolations are rewritten on their own.
    if not CJK.search(text):
        if exprs:
            rebuilt = ['"']
            for kind, value in parts:
                rebuilt.append(value if kind == "text" else "\\(" + transform(value) + ")")
            rebuilt.append('"')
            return "".join(rebuilt)
        return literal
    line_start = src.rfind("\n", 0, start) + 1
    before = src[line_start:start]
    after = src[end:src.find("\n", end) if src.find("\n", end) >= 0 else len(src)]
    stripped_before = before.rstrip()
    if SKIP_BEFORE.search(before):
        skip("context"); return literal
    if re.search(r"\bcase\b[^:]*$", before):
        skip("case"); return literal
    if re.match(r"\s*:", after) and not stripped_before.endswith("?"):
        skip("dictionary key or label"); return literal
    if re.match(r"\s*(==|!=)", after):
        skip("comparison"); return literal
    for e in exprs:
        if top_level_comma(e) or re.search(r"\b(Text|Image)\(", e):
            skip("formatted interpolation"); return literal
    report["wrapped"] += 1
    if not exprs:
        return "L(" + literal + ")"
    key = ['"']
    args = []
    for kind, value in parts:
        if kind == "text":
            key.append(value.replace("%", "%%"))
        else:
            key.append("%@")
            args.append(transform(value).strip())
    key.append('"')
    return "L(" + "".join(key) + ", " + ", ".join(args) + ")"


def unescape(swift_literal_body):
    return swift_literal_body.encode("utf-8").decode("unicode_escape").encode("latin-1").decode("utf-8") \
        if "\\" in swift_literal_body else swift_literal_body


def extract():
    keys = set()
    pattern = re.compile(r'\bL\("((?:[^"\\\n]|\\.)*)"')
    for target in TARGETS:
        for path in (ROOT / target).rglob("*.swift"):
            for m in pattern.finditer(path.read_text()):
                body = m.group(1)
                body = body.replace('\\"', '"').replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\")
                keys.add(body)
    print(json.dumps(sorted(keys), ensure_ascii=False, indent=0))


def keys_in_sources():
    keys = set()
    pattern = re.compile(r'\bL\("((?:[^"\\\n]|\\.)*)"')
    for target in TARGETS:
        for path in (ROOT / target).rglob("*.swift"):
            for m in pattern.finditer(path.read_text()):
                keys.add(m.group(1).replace('\\"', '"').replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\"))
    return keys


def check():
    """Fails when a UI string is not wrapped, or a wrapped string has no English entry."""
    table = json.loads((ROOT.parent / "Resources" / "Localizable.en.json").read_text())
    unwrapped = [str(p.relative_to(ROOT)) for t in TARGETS for p in sorted((ROOT / t).rglob("*.swift"))
                 if transform(p.read_text()) != p.read_text()]
    missing = sorted(k for k in keys_in_sources() if k not in table and "(?:" not in k and "\\[" not in k)
    for path in unwrapped: print("unwrapped Chinese UI text in", path)
    for key in missing: print("no English entry:", json.dumps(key, ensure_ascii=False))
    if unwrapped or missing:
        print("Run script/localize_sources.py --apply and add the entries to Resources/Localizable.en.json.")
        sys.exit(1)
    print("Localization check passed.")


def main():
    if "--check" in sys.argv:
        check(); return
    if "--extract" in sys.argv:
        extract(); return
    apply = "--apply" in sys.argv
    changed = []
    for target in TARGETS:
        for path in sorted((ROOT / target).rglob("*.swift")):
            source = path.read_text()
            result = transform(source)
            if result != source:
                changed.append(str(path.relative_to(ROOT)))
                if apply:
                    if target != "CandidLocalization" and "import CandidLocalization" not in result:
                        result = re.sub(r"^(import [^\n]+\n)", r"\1import CandidLocalization\n", result, count=1, flags=re.M)
                    path.write_text(result)
    print(json.dumps({"files": len(changed), **report}, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
