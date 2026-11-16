#!/bin/zsh
# Install only an already notarized Lindo release, retaining the previous app.
set -euo pipefail
(( $# == 1 )) || { print -u2 'Usage: tools/install-notarized.sh /path/to/Lindo.app'; exit 2; }
source_app=${1:A}
[[ -d "$source_app" && ! -L "$source_app" ]] || exit 2
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source_app/Contents/Info.plist")" == com.piernicolademaria.lindo ]] || exit 2
if pgrep -x Lindo > /dev/null; then
  print -u2 'Quit Lindo before installing its verified replacement.'
  exit 1
fi
team=${LINDO_SIGNING_TEAM:-}
[[ "$team" =~ '^[A-Z0-9]{10}$' ]] || {
  print -u2 'Set LINDO_SIGNING_TEAM to the ten-character Developer ID team to verify.'; exit 2
}
requirement='anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[subject.OU] = "'"$team"'"'
verify() {
  # The final call is inside `if !`, where zsh disables errexit in functions.
  # Propagate each failure explicitly so later checks cannot mask it.
  codesign --verify --deep --strict -R="$requirement" "$1" || return $?
  xcrun stapler validate "$1" || return $?
  spctl --assess --type execute --verbose=2 "$1" || return $?
}
verify "$source_app"
source_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$source_app/Contents/Info.plist")
source_build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$source_app/Contents/Info.plist")
source_hash=$(codesign -d --verbose=4 "$source_app" 2>&1 | sed -n 's/^CDHash=//p')
print "Installing Lindo $source_version build $source_build, CDHash ${source_hash:-unknown}"
# Any notarized Lindo passes the checks above, including an older one. Builds are
# timestamps: refuse to go backwards unless that is stated explicitly.
installed_plist="$HOME/Applications/Lindo.app/Contents/Info.plist"
if [[ -f "$installed_plist" ]]; then
  installed_build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$installed_plist" 2>/dev/null || true)
  if [[ "${LINDO_ALLOW_DOWNGRADE:-0}" != 1 ]]; then
    # A build that cannot be compared is refused, never waved through.
    if [[ "$source_build" != <-> || "$installed_build" != <-> ]]; then
      print -u2 "Cannot compare build '$source_build' with installed build '$installed_build'."
      print -u2 'Set LINDO_ALLOW_DOWNGRADE=1 to install anyway on purpose.'
      exit 1
    fi
    if (( source_build < installed_build )); then
      print -u2 "Refusing to install build $source_build over the newer installed build $installed_build."
      print -u2 'Set LINDO_ALLOW_DOWNGRADE=1 to install an older release on purpose.'
      exit 1
    fi
  fi
fi
# When the release receipt sits beside the app, the binary must be the one it names.
receipt="${source_app:h}/ready.json"
if [[ -f "$receipt" ]]; then
  expected_hash=$(plutil -extract cdHash raw -o - "$receipt" 2>/dev/null || true)
  if [[ -n "$expected_hash" && "$expected_hash" != "$source_hash" ]]; then
    print -u2 "CDHash $source_hash does not match the release receipt ($expected_hash): $receipt"
    exit 1
  fi
  [[ -n "$expected_hash" ]] && print "Release receipt matches: $receipt" || print "Release receipt has no CDHash to compare: $receipt"
else
  print "No ready.json beside the app: its CDHash was not cross-checked against a release receipt."
fi
root="$HOME/Library/Developer/Lindo"
mkdir -p "$root/staging" "$root/backups" "$HOME/Applications"
stage=$(mktemp -d "$root/staging/notarized.XXXXXXXX")
ditto "$source_app" "$stage/Lindo.app"
verify "$stage/Lindo.app"
target="$HOME/Applications/Lindo.app"
backup=''
if [[ -e "$target" || -L "$target" ]]; then
  [[ -d "$target" && ! -L "$target" ]] || exit 1
  backup="$root/backups/Lindo-before-notarized-$(date +%Y%m%d-%H%M%S)-$$.app"
  mv "$target" "$backup"
fi
if ! mv "$stage/Lindo.app" "$target"; then
  [[ -z "$backup" ]] || mv "$backup" "$target"
  print -u2 'Installation failed; previous app restored.'
  exit 1
fi
if ! verify "$target"; then
  mv "$target" "$stage/Lindo-failed.app"
  [[ -z "$backup" ]] || mv "$backup" "$target"
  print -u2 'Installed verification failed; previous app restored.'
  exit 1
fi
rmdir "$stage"
print "Installed notarized app: $target"
[[ -z "$backup" ]] || print "Previous app retained: $backup"
