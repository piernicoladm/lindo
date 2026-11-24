#!/bin/zsh
# Build and stage outside Dropbox; installation requires --install.
set -euo pipefail
here=${0:A:h}
root="$HOME/Library/Developer/Lindo"
config=release
launch=0
signed=0
install=0
replace_notarized=0
identity=${LINDO_SIGNING_IDENTITY:-}
team=${LINDO_SIGNING_TEAM:-}
for arg in "$@"; do
  case "$arg" in
    --debug) config=debug ;;
    --open) launch=1 ;;
    --signed) signed=1 ;;
    --install) install=1 ;;
    --no-install) install=0 ;;
    --replace-notarized) replace_notarized=1 ;;
    *) print -u2 "Unknown argument: $arg"; exit 2 ;;
  esac
done
if (( signed )); then
  [[ -n "$identity" && "$team" =~ '^[A-Z0-9]{10}$' ]] || {
    print -u2 'Signed builds require LINDO_SIGNING_IDENTITY and a ten-character LINDO_SIGNING_TEAM.'
    exit 2
  }
fi
mkdir -p "$root/staging" "$root/backups"
# rustup installs in ~/.cargo/bin even when Finder does not inherit the shell PATH.
export PATH="$PATH:$HOME/.cargo/bin"
export CARGO_TARGET_DIR="$root/rust-target"
cargo_flags=(--locked --manifest-path "$here/Cargo.toml" --bin Lindo)
if [[ "$config" == release ]]; then cargo_flags+=(--release); fi
cargo build "${cargo_flags[@]}"
bindir="$CARGO_TARGET_DIR/$config"
[[ -x "$bindir/Lindo" ]] || { print -u2 'Missing executable'; exit 1; }
stage=$(mktemp -d "$root/staging/install.XXXXXXXX")
app="$stage/Lindo.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bindir/Lindo" "$app/Contents/MacOS/Lindo"
cp "$here/launch/brand/assets/AppIcon.icns" "$app/Contents/Resources/"
sed "s/__BUILD__/$(date +%Y%m%d%H%M%S)/" "$here/Info.plist.in" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
if (( signed )); then
  codesign --force --sign "$identity" --options runtime --timestamp --entitlements "$here/Lindo.entitlements" "$app"
  requirement='anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[subject.OU] = "'"$team"'"'
  codesign --verify --deep --strict -R="$requirement" "$app"
else
  codesign --force --sign - --entitlements "$here/Lindo.entitlements" "$app"
fi
codesign --verify --deep --strict --verbose=2 "$app"
if (( ! install )); then
  print "Staged app: $app"
  print 'Installed app unchanged. Notarization is a separate step.'
  if (( launch )); then open "$app"; fi
  exit 0
fi
mkdir -p "$HOME/Applications"
target="$HOME/Applications/Lindo.app"
# Never swap the bundle under a running instance.
if pgrep -x Lindo > /dev/null; then
  print -u2 "Quit Lindo before replacing the installed app. Staged build kept at: $app"
  exit 1
fi
# A build from this script is never notarized. stapler errors cannot prove the
# installed ticket is absent: validation also needs Apple's ticketing service.
# Only a verified ad hoc signature establishes a local development install.
if [[ -d "$target" && ! -L "$target" ]] && (( ! replace_notarized )); then
  ticket_status=0
  xcrun stapler validate "$target" > /dev/null 2>&1 || ticket_status=$?
  development_install=0
  if (( ticket_status != 0 )) &&
     installed_signature=$(codesign -d --verbose=4 "$target" 2>&1) &&
     grep -Fxq 'Signature=adhoc' <<< "$installed_signature" &&
     codesign --verify --deep --strict "$target" > /dev/null 2>&1; then
    development_install=1
  fi
  if (( ! development_install )); then
    if (( ticket_status == 0 )); then
      print -u2 "The installed app carries an Apple notarization ticket and this build does not."
    else
      print -u2 "Cannot establish that the installed app is an unnotarized development build (ticket validation exit $ticket_status)."
    fi
    print -u2 "Use tools/install-notarized.sh for a notarized release, or pass --replace-notarized to replace it anyway."
    print -u2 "Staged build kept at: $app"
    exit 1
  fi
fi
backup=''
if [[ -e "$target" || -L "$target" ]]; then
  [[ -d "$target" && ! -L "$target" ]] || { print -u2 'Installation target is not a regular app directory'; exit 1; }
  backup="$root/backups/Lindo-$(date +%Y%m%d-%H%M%S)-$$.app"
  mv "$target" "$backup"
fi
if ! mv "$app" "$target"; then
  [[ -z "$backup" ]] || mv "$backup" "$target"
  print -u2 'Install failed; previous app restored when present.'
  exit 1
fi
if ! codesign --verify --deep --strict "$target"; then
  mv "$target" "$stage/Lindo-failed.app"
  [[ -z "$backup" ]] || mv "$backup" "$target"
  print -u2 'Installed signature failed verification; rolled back.'
  exit 1
fi
rmdir "$stage"
if (( signed )); then
  print "Installed $target (Developer ID signature; notarization is a separate step)."
else
  print "Installed $target (local ad hoc signature)."
fi
[[ -z "$backup" ]] || print "Previous app retained at $backup"
if (( launch )); then
  open "$target"
fi
