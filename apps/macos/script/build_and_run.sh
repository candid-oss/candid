#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-run}"
if [[ $# -gt 0 ]]; then shift; fi
APP_NAME="Candid"
APP_BUNDLE="$ROOT_DIR/dist/$APP_NAME.app"
case "$MODE" in
  run|--mock|--verify|--build-only|--logs|--debug) ;;
  *) echo "usage: $0 [run|--mock|--verify|--build-only|--logs|--debug] [app arguments]" >&2; exit 2 ;;
esac
if [[ "$MODE" == "--build-only" ]] && /usr/bin/pgrep -f "^$APP_BUNDLE/Contents/MacOS/$APP_NAME([[:space:]]|$)" >/dev/null; then
  echo "Quit Candid before replacing its signed bundle." >&2
  exit 1
fi
CONFIGURATION="release"
if [[ "$MODE" == "--debug" ]]; then CONFIGURATION="debug"; fi
swift build --package-path "$ROOT_DIR" --configuration "$CONFIGURATION"
BUILD_DIR="$(swift build --package-path "$ROOT_DIR" --configuration "$CONFIGURATION" --show-bin-path)"
# Only this app is stopped, never device/Hub/ASR services.
if [[ "$MODE" != "--build-only" ]]; then
  /usr/bin/pkill -f "^$APP_BUNDLE/Contents/MacOS/$APP_NAME([[:space:]]|$)" >/dev/null 2>&1 || true
fi
# Validate the new signed bundle before replacing the working build.
STAGING_ROOT="$(mktemp -d "$ROOT_DIR/.build/app-stage.XXXXXX")"
STAGED_APP="$STAGING_ROOT/$APP_NAME.app"
cleanup_stage() {
  if [[ -d "$STAGING_ROOT/Previous.app" && ! -e "$APP_BUNDLE" ]]; then
    mv "$STAGING_ROOT/Previous.app" "$APP_BUNDLE"
  fi
  rm -rf "$STAGING_ROOT"
}
trap cleanup_stage EXIT
mkdir -p "$STAGED_APP/Contents/MacOS" "$STAGED_APP/Contents/Resources"
cp "$BUILD_DIR/$APP_NAME" "$STAGED_APP/Contents/MacOS/$APP_NAME"
# SwiftPM Bundle.module resolves its resource bundle in Contents/Resources.
RESOURCE_BUNDLE=""
for CANDIDATE in "$BUILD_DIR"/*ConsoleCore.bundle; do
  if [[ -d "$CANDIDATE" ]]; then RESOURCE_BUNDLE="$CANDIDATE"; break; fi
done
if [[ -z "$RESOURCE_BUNDLE" ]]; then echo "Missing ConsoleCore resource bundle" >&2; exit 1; fi
cp -R "$RESOURCE_BUNDLE" "$STAGED_APP/Contents/Resources/"
# The Rust core (core): the App's helper `candid serve`, also the `candid` command and MCP server.
# Universal binary from release builds for both architectures; signed with the app's certificate below.
CORE_DIR="$(cd "$ROOT_DIR/../../core" && pwd)"
CORE_SLICES=()
for CORE_TARGET in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -q --manifest-path "$CORE_DIR/Cargo.toml" -p candid-cli --target "$CORE_TARGET"
  CORE_SLICES+=("$CORE_DIR/target/$CORE_TARGET/release/candid")
done
mkdir -p "$STAGED_APP/Contents/Helpers"
/usr/bin/lipo -create -output "$STAGED_APP/Contents/Helpers/candid" "${CORE_SLICES[@]}"
# The attached Pi agent (agent-platform-v1 §4.1): only the extension skeleton (lockfile, extension, prompt, skills).
# Node and Pi are not bundled; the user installs them from Settings → 组件 (see Sources/Candid/Components.swift).
# License and third-party notices, shown in Settings → 关于.
cp "$ROOT_DIR/../../LICENSE" "$ROOT_DIR/../../THIRD_PARTY_LICENSES.txt" "$STAGED_APP/Contents/Resources/"
PI_SKELETON="$STAGED_APP/Contents/Resources/Pi"
EXTENSION_DIR="$ROOT_DIR/../../agents/pi-extension"
rm -rf "$PI_SKELETON" && mkdir -p "$PI_SKELETON"
cp "$EXTENSION_DIR/package.json" "$EXTENSION_DIR/package-lock.json" "$EXTENSION_DIR/index.ts" "$EXTENSION_DIR/system-prompt.md" "$PI_SKELETON/"
cp -R "$EXTENSION_DIR/../skills" "$PI_SKELETON/skills"
# WidgetKit extension: sandboxed, reads only the aggregates the app writes for it.
WIDGET="$STAGED_APP/Contents/PlugIns/CandidWidget.appex"
mkdir -p "$WIDGET/Contents/MacOS"
cp "$BUILD_DIR/CandidWidget" "$WIDGET/Contents/MacOS/CandidWidget"
cat > "$WIDGET/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundlePackageType</key><string>XPC!</string>
<key>CFBundleExecutable</key><string>CandidWidget</string>
<key>CFBundleIdentifier</key><string>io.github.candid-oss.candid.widget</string>
<key>CFBundleName</key><string>Candid</string>
<key>CFBundleDisplayName</key><string>Candid</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSExtension</key><dict><key>NSExtensionPointIdentifier</key><string>com.apple.widgetkit-extension</string></dict>
</dict></plist>
PLIST
ICONSET="$ROOT_DIR/.build/ConsoleIcon.iconset"
mkdir -p "$ICONSET"
/usr/bin/xcrun swift "$ROOT_DIR/script/create_icon.swift" "$ICONSET/icon_512x512@2x.png"
for ICON_SIZE in 16 32 128 256 512; do
  /usr/bin/sips -z "$ICON_SIZE" "$ICON_SIZE" "$ICONSET/icon_512x512@2x.png" --out "$ICONSET/icon_${ICON_SIZE}x${ICON_SIZE}.png" >/dev/null
  RETINA_SIZE=$((ICON_SIZE * 2))
  if [[ "$ICON_SIZE" != 512 ]]; then
    /usr/bin/sips -z "$RETINA_SIZE" "$RETINA_SIZE" "$ICONSET/icon_512x512@2x.png" --out "$ICONSET/icon_${ICON_SIZE}x${ICON_SIZE}@2x.png" >/dev/null
  fi
done
/usr/bin/iconutil -c icns "$ICONSET" -o "$STAGED_APP/Contents/Resources/ConsoleIcon.icns"
cat > "$STAGED_APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleExecutable</key><string>Candid</string>
<key>CFBundleIdentifier</key><string>io.github.candid-oss.candid</string>
<key>CFBundleIconFile</key><string>ConsoleIcon</string>
<key>CFBundleName</key><string>Candid</string>
<key>CFBundleDisplayName</key><string>Candid</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>NSScreenCaptureUsageDescription</key><string>在你开启截图时保存画面；在你开始系统内录时获取播放声音，内录不保存屏幕画面。</string>
<key>NSMicrophoneUsageDescription</key><string>在你点击录音或通话提醒的开始内录后使用麦克风，与系统声音混合按段保存，可随时停止。</string>
<key>NSCalendarsFullAccessUsageDescription</key><string>AI 页显示今天的日程；只在你批准某条提案后才写入日历。日程不保存进资料库。</string>
<key>NSRemindersFullAccessUsageDescription</key><string>AI 页显示今天到期和逾期的提醒；只在你批准某条提案后才新建提醒。提醒不保存进资料库。</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict></plist>
PLIST
python3 "$ROOT_DIR/script/local_signing.py" "$STAGED_APP"
mkdir -p "$ROOT_DIR/dist"
if [[ -d "$APP_BUNDLE" ]]; then mv "$APP_BUNDLE" "$STAGING_ROOT/Previous.app"; fi
mv "$STAGED_APP" "$APP_BUNDLE"
if [[ "$MODE" == "--build-only" ]]; then echo "$APP_BUNDLE"; exit 0; fi
if [[ "$MODE" == "--mock" ]]; then set -- --mock "$@"; fi
if [[ "$MODE" == "--debug" ]]; then
  exec /usr/bin/lldb "$APP_BUNDLE/Contents/MacOS/$APP_NAME" -- "$@"
fi
/usr/bin/open -n "$APP_BUNDLE" --args "$@"
if [[ "$MODE" == "--verify" ]]; then
  sleep 2
  /usr/bin/pgrep -f "^$APP_BUNDLE/Contents/MacOS/$APP_NAME([[:space:]]|$)" >/dev/null
  echo "GUI process started: $APP_BUNDLE"
elif [[ "$MODE" == "--logs" ]]; then
  /usr/bin/log stream --info --style compact --predicate 'process == "Candid"'
fi
