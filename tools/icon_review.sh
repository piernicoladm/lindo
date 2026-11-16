#!/bin/zsh
# Renders Icon/AppIcon.icon the way macOS shows it, with Icon Composer's
# ictool, into one contact sheet:
#   Default, Dark, TintedDark, ClearLight at 256 pt, then Default at 32 and 16 pt.
# Usage: tools/icon_review.sh [out.png]     (default /tmp/Lindo-icon.png)
set -euo pipefail
here=${0:A:h}
repo=${here:h}
out=${1:-/tmp/Lindo-icon.png}
ictool="$(xcode-select -p)/../Applications/Icon Composer.app/Contents/Executables/ictool"
if [[ ! -x "$ictool" ]]; then
  "$here/make_icon.sh"
  icons="$HOME/Library/Developer/Lindo/icon/AppIcon.iconset"
  fallback=${out:r}-appkit-fallback.png
  swift "$here/icon_sheet.swift" "$fallback" "$icons/icon_256x256.png" "$icons/icon_32x32.png" "$icons/icon_16x16.png"
  echo "AppKit fallback only: 256, 32, 16 pixels. System appearance variants unverified."
  echo "wrote $fallback"
  exit 0
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

render() { # rendition size file
  "$ictool" "$repo/Icon/AppIcon.icon" --export-image --output-file "$work/$3" \
    --platform macOS --rendition "$1" --width "$2" --height "$2" --scale 2 >/dev/null
}
render Default 256 1.png
render Dark 256 2.png
render TintedDark 256 3.png
render ClearLight 256 4.png
render Default 32 5.png
render Default 16 6.png
swift "$here/icon_sheet.swift" "$out" "$work"/{1,2,3,4,5,6}.png
echo "wrote $out"
