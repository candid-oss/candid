#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLAGS=()
# CLT 27 ships TestingMacros but its swiftbuild discovery can omit the plugin.
TOOLCHAIN_DIR="$(dirname "$(/usr/bin/xcrun --find swiftc)")"
TESTING_PLUGIN="$TOOLCHAIN_DIR/../lib/swift/host/plugins/testing/libTestingMacros.dylib"
if [[ "$(/usr/bin/xcode-select -p)" == "/Library/Developer/CommandLineTools" && -f "$TESTING_PLUGIN" ]]; then
  FLAGS=(-Xswiftc -load-plugin-library -Xswiftc "$TESTING_PLUGIN")
fi
"$ROOT_DIR/script/check_style.sh"
python3 "$ROOT_DIR/script/test_recording_recovery.py"
# The Rust core (core): CandidClient tests start it as the App does.
CORE_DIR="$(cd "$ROOT_DIR/../../core" && pwd)"
cargo build -q --manifest-path "$CORE_DIR/Cargo.toml" -p candid-cli
export CANDID_CORE_BIN="$CORE_DIR/target/debug/candid"
# Tests must never reach the user's running core or library: give them an empty run directory of their own.
export CANDID_RUN_DIR="$(mktemp -d -t candid-test-run)"
TEST_LOG="$(mktemp -t candid-swift-tests)"
trap 'rm -f "$TEST_LOG"' EXIT
# A stuck test (or a child process holding the output pipe) must fail the run, never hang it.
TIMEOUT="${CANDID_TEST_TIMEOUT:-1500}"
( swift test --package-path "$ROOT_DIR" "${FLAGS[@]}" "$@" 2>&1 | tee "$TEST_LOG" ) &
RUN_PID=$!
( sleep "$TIMEOUT"; echo "Swift tests exceeded ${TIMEOUT}s; stopping them." >&2; pkill -TERM -P "$RUN_PID" ) &
WATCH_PID=$!
STATUS=0
wait "$RUN_PID" || STATUS=$?
# Stop the watchdog and its sleep: a leftover sleep would keep our output pipe open for the whole timeout.
pkill -P "$WATCH_PID" 2>/dev/null || true
kill "$WATCH_PID" 2>/dev/null || true
if [[ $STATUS -ne 0 ]]; then exit "$STATUS"; fi
# CLT's Swift Build engine can return success when an earlier test product
# failed and the last product passed. Never let that hide a failed suite.
if grep -Eq '(^✘|^error:|Test Suite .*failed)' "$TEST_LOG"; then
  echo "A test product reported failure; rejecting the combined run." >&2
  exit 1
fi
