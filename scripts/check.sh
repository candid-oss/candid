#!/usr/bin/env bash
# Runs the checks your change needs, chosen from the files that differ from the base branch
# (the table in AGENTS.md, as one command).
#
#   scripts/check.sh            run what the changed files need
#   scripts/check.sh --list     print the commands without running them
#   scripts/check.sh --all      run every suite
#   scripts/check.sh --base X   compare against X instead of main
#
# Exit status: 0 all passed, 1 a check failed, 2 a needed check could not run (missing tool or environment).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LIST=0
ALL=0
BASE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --list) LIST=1 ;;
    --all) ALL=1 ;;
    --base) BASE="${2:-}"; shift ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [[ -z "$BASE" ]]; then
  if git rev-parse --verify -q main >/dev/null; then BASE=main; else BASE=origin/main; fi
fi
MERGE_BASE="$(git merge-base HEAD "$BASE" 2>/dev/null || git rev-parse HEAD)"
# Committed on this branch, changed in the working tree, and new untracked files.
CHANGED="$( (git diff --name-only "$MERGE_BASE"; git ls-files --others --exclude-standard) | sort -u)"

# Documentation never needs the code suites; the docs check below covers it. Prompts and skill texts are
# Markdown too, but they change behaviour, so they still count as code.
CODE_CHANGED="$(grep -vE '\.md$' <<<"$CHANGED" || true)
$(grep -E '^(core/prompts|core/workflows|agents/skills|agents/pi-extension)/.*\.md$' <<<"$CHANGED" || true)"

touches() { [[ $ALL -eq 1 ]] || grep -qE "$1" <<<"$CODE_CHANGED"; }

FAILED=()
BLOCKED=()

run() { # run <label> <command…>
  local label="$1"; shift
  echo
  echo "== $label"
  echo "   $*"
  [[ $LIST -eq 1 ]] && return 0
  if ! "$@"; then FAILED+=("$label"); fi
}

blocked() { # blocked <label> <how to fix>
  echo
  echo "== $1"
  echo "   cannot run: $2"
  [[ $LIST -eq 1 ]] || BLOCKED+=("$1")
}

if [[ -z "$CHANGED" && $ALL -eq 0 ]]; then
  echo "No changes against $BASE. Use --all to run every suite."
  exit 0
fi

# Documentation links and bilingual pairs: cheap, always on.
run "Docs" python3 scripts/check_docs.py

if touches '^core/'; then
  # Includes the lifecycle run and the MCP/CLI contract snapshots.
  run "Rust core" cargo test --manifest-path core/Cargo.toml --workspace
fi

if touches '^apps/macos/'; then
  run "macOS app: build" swift build --package-path apps/macos
  run "macOS app: style, localization, tests" apps/macos/script/test.sh
fi

if touches '^agents/'; then
  if ! command -v node >/dev/null; then
    blocked "Agent extension" "node is not installed"
  else
    if [[ ! -d agents/pi-extension/node_modules ]]; then
      run "Agent extension: dependencies" npm ci --prefix agents/pi-extension --ignore-scripts
    fi
    run "Agent extension: core binary" cargo build --manifest-path core/Cargo.toml -p candid-cli
    run "Agent extension: end to end" node agents/pi-extension/test/e2e.mjs
  fi
fi

if touches '^services/voice/'; then
  PY="${CANDID_VOICE_PYTHON:-.venv-asr-mlx/bin/python}"
  if [[ -x "$PY" ]]; then
    run "Voice service" "$PY" -m pytest -q services/voice/tests
  else
    blocked "Voice service" "no Python at $PY (set CANDID_VOICE_PYTHON; see services/voice/README.md)"
  fi
fi

[[ $LIST -eq 1 ]] && exit 0

echo
if [[ ${#FAILED[@]} -gt 0 ]]; then
  echo "FAILED: ${FAILED[*]}"
  exit 1
fi
if [[ ${#BLOCKED[@]} -gt 0 ]]; then
  echo "NOT RUN: ${BLOCKED[*]}"
  exit 2
fi
echo "All checks passed."
