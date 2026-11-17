#!/usr/bin/env bash
# Opt-in native synthetic acceptance. It opens only a local fixture, posts
# Return/mouse/scroll to that verified foreground process, and closes it.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR=/tmp/CandidCaptureChat.app
mkdir -p "$APP_DIR/Contents/MacOS"
swiftc "$ROOT_DIR/script/CaptureChatFixture.swift" -o "$APP_DIR/Contents/MacOS/CaptureChatFixture"
/usr/libexec/PlistBuddy -c 'Clear dict' -c 'Add :CFBundleIdentifier string ai.candid.CaptureChatFixture' -c 'Add :CFBundleExecutable string CaptureChatFixture' -c 'Add :CFBundleName string SyntheticChat' -c 'Add :CFBundlePackageType string APPL' "$APP_DIR/Contents/Info.plist" >/dev/null
codesign --force --sign - "$APP_DIR"
xcrun clang -O2 "$ROOT_DIR/script/resource_probe.c" -o /tmp/candid-capture-resource-probe
/tmp/candid-capture-resource-probe --self-test
# The platform acceptance package imports the repository sources directly and
# avoids unrelated app/UI tests competing for the foreground window.
CHECK_DIR=$(mktemp -d /tmp/candid-source-check.XXXXXX)
python3 - "$ROOT_DIR" "$CHECK_DIR" <<'PY_PACKAGE'
import pathlib, sys
root, check = map(pathlib.Path, sys.argv[1:])
(check/'Sources').mkdir()
(check/'Tests').mkdir()
for name in ['DesktopCore', 'DesktopPlatform', 'CandidClient', 'CandidShared']:
    (check/'Sources'/name).symlink_to(root/'Sources'/name)
(check/'Tests'/'DesktopPlatformTests').symlink_to(root/'Tests'/'DesktopPlatformTests')
(check/'Package.swift').write_text('''// swift-tools-version: 6.0
import PackageDescription
let package = Package(name: "CaptureSources", platforms: [.macOS(.v14)], targets: [
.target(name: "CandidClient"), .target(name: "CandidShared"),
.target(name: "DesktopCore", dependencies: ["CandidClient"]),
.target(name: "DesktopPlatform", dependencies: ["DesktopCore", "CandidShared"]),
.testTarget(name: "DesktopPlatformTests", dependencies: ["DesktopPlatform", "DesktopCore"])
])
''')
PY_PACKAGE
PLUGIN=/Library/Developer/CommandLineTools/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib
CANDID_CHAT_LIVE=1 CANDID_RESOURCE_PROBE=/tmp/candid-capture-resource-probe \
  swift test --package-path "$CHECK_DIR" --scratch-path "${CANDID_CAPTURE_SCRATCH:-/tmp/candid-source-check-build}" \
  -Xswiftc -load-plugin-library -Xswiftc "$PLUGIN" --filter DesktopChatLiveTests 2>&1 | tee /tmp/candid-capture-source-check.log
if rg -q '(^✘|^error:|Test Suite .*failed)' /tmp/candid-capture-source-check.log; then
  exit 1
fi
