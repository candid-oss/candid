#!/usr/bin/env bash
# Launch the packaged native app. --build explicitly invokes development tooling.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MEDIA_HOME="${CANDID_MEDIA_HOME:-$HOME/Library/Application Support/Candid/MediaWorkspace}"
if [[ "${1:-}" == "--build" ]]; then
  shift
  exec "$SCRIPT_DIR/build_and_run.sh" --verify --media-profile "$MEDIA_HOME" "$@"
fi
APP_BUNDLE="${CANDID_APP_BUNDLE:-$SCRIPT_DIR/../dist/Candid.app}"
if [[ ! -x "$APP_BUNDLE/Contents/MacOS/Candid" || ! -x "$APP_BUNDLE/Contents/Helpers/candid" ]]; then
  echo "请先构建原生 App 包，或用 CANDID_APP_BUNDLE 指定已有包。开发构建用 --build。" >&2
  exit 1
fi
exec "$APP_BUNDLE/Contents/MacOS/Candid" --media-profile "$MEDIA_HOME" "$@"
