#!/bin/zsh
# Draw an original AppKit icon and compile without Xcode / actool.
set -euo pipefail
here=${0:A:h}
out="$HOME/Library/Developer/Lindo/icon"
mkdir -p "$out/AppIcon.iconset" "$out/module-cache"
CLANG_MODULE_CACHE_PATH="$out/module-cache" swift "$here/draw-icon.swift" "$out/AppIcon.iconset"
iconutil --convert icns "$out/AppIcon.iconset" --output "$out/AppIcon.icns"
[[ -s "$out/AppIcon.icns" ]] || { print -u2 'Icon compilation produced no output'; exit 1; }
print "Icon ready: $out/AppIcon.icns"
