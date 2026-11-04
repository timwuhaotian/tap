#!/bin/sh
# Assembles Tap.app around the cargo binary, with the icon from packaging/app-icon.png
# (app-icon-dev.png, on a graphite plate, for debug builds). Kil's scripts/bundle.sh, ported
# (docs/PLAN.md §4, §12).
# Usage: scripts/bundle.sh [debug|release]   → prints the .app path
set -eu

profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

case "$profile" in
  debug) cargo build -p tap-app; icon=packaging/app-icon-dev.png ;;
  release) cargo build --release -p tap-app; icon=packaging/app-icon.png ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

out="target/$profile"
app="$out/Tap.app"
bin="$app/Contents/MacOS/tap"
version="$(cargo pkgid -p tap-app | sed 's/.*[#@]//')"

rm -rf "$app" "$app.dSYM"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$out/tap" "$bin"
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"

# AppKit picks its look from the SDK the binary says it was built with: an old one draws
# old controls. Search's build.sh fixes this with vtool after SwiftPM; check it here too.
sdk="$(xcrun --sdk macosx --show-sdk-version)"
build="$(vtool -show-build "$bin")"
minos="$(echo "$build" | awk '$1 == "minos" {print $2}')"
linked="$(echo "$build" | awk '$1 == "sdk" {print $2}')"
if [ "$minos" != "14.0" ] || [ "$linked" != "$sdk" ]; then
  vtool -set-build-version macos 14.0 "$sdk" -replace -output "$bin" "$bin"
fi

# Release builds keep their symbols in a dSYM beside the app, then drop them from the binary.
if [ "$profile" = release ]; then
  dsymutil "$bin" -o "$app.dSYM"
  strip -x "$bin"
fi

iconset="$out/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$icon" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$icon" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

# Ad-hoc seal for local runs; scripts/release.sh re-signs with Developer ID (docs/PLAN.md §12).
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"
