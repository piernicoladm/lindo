#!/bin/zsh
set -euo pipefail
here=${0:A:h:h}
root="$HOME/Library/Developer/Lindo"
swift build --package-path "$here" --scratch-path "$root/build"
bindir=$(swift build --package-path "$here" --scratch-path "$root/build" --show-bin-path)
APP_SAMPLE=1 APP_STORE=/tmp/lindo-qa APP_SNAPSHOT=/tmp/Lindo-shots "$bindir/Lindo"
