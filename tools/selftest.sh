#!/bin/zsh
set -euo pipefail
here=${0:A:h:h}
root="$HOME/Library/Developer/Lindo"
# Swift Testing is available in CLT; XCTest is not required by this package.
clt_frameworks=/Library/Developer/CommandLineTools/Library/Developer/Frameworks
clt_libraries=/Library/Developer/CommandLineTools/Library/Developer/usr/lib
flags=()
if [[ "$(xcode-select -p)" == /Library/Developer/CommandLineTools && -d "$clt_frameworks" ]]; then
  flags=(-Xswiftc -F -Xswiftc "$clt_frameworks"
         -Xlinker -F -Xlinker "$clt_frameworks"
         -Xlinker -rpath -Xlinker "$clt_frameworks"
         -Xlinker -rpath -Xlinker "$clt_libraries")
fi
swift test --disable-xctest --package-path "$here" --scratch-path "$root/test-build" "${flags[@]}"
swift build --package-path "$here" --scratch-path "$root/test-build"
bindir=$(swift build --package-path "$here" --scratch-path "$root/test-build" --show-bin-path)
APP_SAMPLE=1 APP_STORE=/tmp/lindo-selftest "$bindir/Lindo" --selftest
