#!/bin/sh
# Renders design/dmg/background.html into src-tauri/dmg/background.tiff: one TIFF holding the
# 660 × 400 picture at 1x and 2x, so the disk image window is sharp on Retina screens.
# Needs Google Chrome (headless) and macOS's tiffutil.
set -eu
cd "$(dirname "$0")/../.."
chrome="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for scale in 1 2; do
  "$chrome" --headless --disable-gpu --hide-scrollbars --user-data-dir="$tmp/profile" \
    --window-size=660,400 --force-device-scale-factor="$scale" \
    --screenshot="$tmp/bg@${scale}x.png" "file://$PWD/design/dmg/background.html" >/dev/null 2>&1
done
tiffutil -cathidpicheck "$tmp/bg@1x.png" "$tmp/bg@2x.png" -out src-tauri/dmg/background.tiff
cp "$tmp/bg@2x.png" design/dmg/background@2x.png
echo "wrote src-tauri/dmg/background.tiff"
