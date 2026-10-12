#!/bin/sh
# Builds "Laser Studio.app" (T-413): the release binary with the menu-bar
# item (`--features macos-app`), Info.plist (version from studio/Cargo.toml,
# microphone text), and AppIcon.icns converted from our icon.svg with the
# tools macOS ships (sips, iconutil). Nothing is installed anywhere.
#
#   studio/packaging/macos/build_app.sh [OUT_DIR]   # default: target/macos
#   studio/packaging/macos/build_app.sh --plist FILE  # Info.plist only (tests)
#
# Extra Cargo features, e.g. « Son du Mac » (T-241, off by default):
#   EXTRA_FEATURES=system-audio studio/packaging/macos/build_app.sh
#
# The bundle is ad-hoc signed only (`codesign -s -`, no identity): no
# Developer ID signature, no notarisation. A copy downloaded from elsewhere
# is quarantined and macOS refuses it the first time: right-click › Ouvrir,
# or Réglages Système › Confidentialité et sécurité › Ouvrir quand même.
#
# Opened from Finder, the binary sees no argument and starts as the
# application (`--app`): data in ~/Library/Application Support/Laser Studio,
# a free port from 8080 up on 127.0.0.1, the page opens in the browser. The
# laser starts disarmed; no output is opened unless one was chosen in
# RÉGLAGES › Sorties.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/studio/Cargo.toml" | head -n 1)
[ -n "$VERSION" ] || { echo "version introuvable dans studio/Cargo.toml" >&2; exit 1; }

plist() {
    sed "s/@VERSION@/$VERSION/g" "$HERE/Info.plist.in" > "$1"
    plutil -lint -s "$1"
}

if [ "${1:-}" = "--plist" ]; then
    [ -n "${2:-}" ] || { echo "usage: $0 --plist FILE" >&2; exit 2; }
    plist "$2"
    exit 0
fi

OUT=${1:-"$ROOT/target/macos"}
APP="$OUT/Laser Studio.app"

cargo build --release -p laser-studio --features "macos-app${EXTRA_FEATURES:+,$EXTRA_FEATURES}" --manifest-path "$ROOT/Cargo.toml"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/release/laser-studio" "$APP/Contents/MacOS/laser-studio"
plist "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# The icon: one 1024 px render of the SVG, scaled to every size of the set.
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
ICONSET="$WORK/AppIcon.iconset"
mkdir "$ICONSET"
sips -s format png "$HERE/icon.svg" --out "$WORK/icon-1024.png" > /dev/null
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$WORK/icon-1024.png" --out "$ICONSET/icon_${size}x${size}.png" > /dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$WORK/icon-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

# Ad-hoc signature of the whole bundle, so macOS ties the microphone
# permission to it (not a Developer ID signature: see above).
codesign --force --sign - "$APP"

echo "$APP ($(du -sh "$APP" | cut -f1), version $VERSION)"
