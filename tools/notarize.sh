#!/bin/zsh
# Notarize a retained release copy using credentials in a Keychain profile.
set -euo pipefail
usage() {
  print -u2 'Usage: tools/notarize.sh /path/to/Lindo.app --profile KEYCHAIN_PROFILE [--prepare-only]'
  exit 2
}
(( $# == 3 || $# == 4 )) || usage
prepare_only=0
if (( $# == 4 )); then
  [[ "$4" == --prepare-only ]] || usage
  prepare_only=1
fi
source_app=${1:A}
[[ "$2" == --profile && -n "$3" ]] || usage
profile=$3
[[ -d "$source_app" && ! -L "$source_app" ]] || { print -u2 'Expected an app directory.'; exit 2; }
team=${LINDO_SIGNING_TEAM:-}
[[ "$team" =~ '^[A-Z0-9]{10}$' ]] || {
  print -u2 'Set LINDO_SIGNING_TEAM to the ten-character Developer ID team to verify.'; exit 2
}
requirement='anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[subject.OU] = "'"$team"'"'
verify_signed() {
  codesign --verify --deep --strict --verbose=2 -R="$requirement" "$1"
  local details
  details=$(codesign -d --verbose=4 "$1" 2>&1)
  [[ "$details" == *"TeamIdentifier=$team"* && "$details" == *'flags='*'(runtime)'* && "$details" == *$'\nTimestamp='* ]] || {
    print -u2 'Missing expected team, hardened runtime or secure timestamp.'
    return 1
  }
}
valid_id() { [[ "$1" =~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' ]]; }
json_value() { plutil -extract "$2" raw -o - "$1" 2>/dev/null; }
verify_signed "$source_app"
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$source_app/Contents/Info.plist")
build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$source_app/Contents/Info.plist")
bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$source_app/Contents/Info.plist")
[[ "$bundle_id" == com.piernicolademaria.lindo && -n "$version" && -n "$build" && "$version" != *[^0-9.]* && "$build" != *[^0-9.]* ]] || {
  print -u2 'Unexpected bundle identifier or unsafe version/build.'; exit 2
}
release="$HOME/Library/Developer/Lindo/releases/$version-$build"
mkdir -p "$release"
lock="$release/.notarize-lock"
mkdir "$lock" 2>/dev/null || { print -u2 "Release locked: $lock. Check for a running notarization before removing a stale empty lock."; exit 1; }
trap 'rmdir "$lock"' EXIT
app="$release/Lindo.app"
if [[ ! -e "$app" ]]; then
  # Interrupted copies remain inspectable and never become the submitted app.
  partial="$release/Lindo-copy-$(date +%Y%m%d-%H%M%S)-$$.app"
  ditto "$source_app" "$partial"
  verify_signed "$partial"
  mv "$partial" "$app"
else
  verify_signed "$app"
  source_hash=$(codesign -d --verbose=4 "$source_app" 2>&1 | sed -n 's/^CDHash=//p')
  release_hash=$(codesign -d --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p')
  [[ -n "$source_hash" && "$source_hash" == "$release_hash" ]] || {
    print -u2 'This version/build already contains another signed binary. Rebuild with a new build number.'; exit 1
  }
fi
run="$(date +%Y%m%d-%H%M%S)-$$"
receipt="$release/submission.json"
upload="$release/Lindo-upload.zip"
manifest="$release/upload-manifest.json"
release_hash=$(codesign -d --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p')
[[ -n "$release_hash" ]] || { print -u2 'Missing release CDHash.'; exit 1; }
for field expected in CFBundleIdentifier "$bundle_id" CFBundleShortVersionString "$version" CFBundleVersion "$build"; do
  actual=$(/usr/libexec/PlistBuddy -c "Print :$field" "$app/Contents/Info.plist")
  [[ "$actual" == "$expected" ]] || { print -u2 'Retained bundle metadata mismatch.'; exit 1; }
done
if [[ ! -e "$upload" ]]; then
  [[ ! -e "$manifest" && ! -e "$receipt" && ! -e "$release/SUBMISSION_UNCERTAIN" ]] || {
    print -u2 'Missing recorded upload archive; recover it before resuming.'; exit 1
  }
  # Publish the archive first. A crash before manifest publication fails closed.
  partial_zip="$release/Lindo-upload-partial-$run.zip"
  ditto -c -k --sequesterRsrc --keepParent "$app" "$partial_zip"
  mv "$partial_zip" "$upload"
  python3 - "$upload" "$manifest" "$bundle_id" "$version" "$build" "$release_hash" <<'PYMANIFEST'
import hashlib, json, os, sys
archive, path, bundle, version, build, cdhash = sys.argv[1:]
with open(archive, 'rb') as source:
    hasher = hashlib.sha256()
    for block in iter(lambda: source.read(1024 * 1024), b''):
        hasher.update(block)
    digest = hasher.hexdigest()
with open(path + '.partial', 'x') as output:
    json.dump(dict(archiveSHA256=digest, bundleId=bundle, version=version,
                   build=build, cdHash=cdhash), output)
    output.flush()
    os.fsync(output.fileno())
os.replace(path + '.partial', path)
PYMANIFEST
fi
# Every reuse, including preparation-only, checks the same immutable binding.
python3 - "$upload" "$manifest" "$bundle_id" "$version" "$build" "$release_hash" <<'PYVERIFY'
import hashlib, json, sys
archive, path, bundle, version, build, cdhash = sys.argv[1:]
try:
    with open(path) as source:
        manifest = json.load(source)
    with open(archive, 'rb') as source:
        hasher = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b''):
            hasher.update(block)
        digest = hasher.hexdigest()
    expected = dict(archiveSHA256=digest, bundleId=bundle, version=version,
                    build=build, cdHash=cdhash)
    if manifest != expected:
        raise ValueError('upload digest or bundle identity differs')
except (OSError, ValueError) as error:
    sys.exit('Upload manifest verification failed; recover or explicitly verify the original archive: ' + str(error))
PYVERIFY
if (( prepare_only )); then
  prepared="$release/prepared-$run.json"
  release_hash=$(codesign -d --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p')
  archive_hash=$(shasum -a 256 "$upload")
  archive_hash=${archive_hash%% *}
  python3 - "$prepared" "$bundle_id" "$version" "$build" "$release_hash" "$archive_hash" "$upload" <<'PY'
import json, sys
path, bundle, version, build, cdhash, digest, archive = sys.argv[1:]
with open(path, 'w') as output:
    json.dump(dict(status='prepared', ready=False, bundleId=bundle, version=version,
                   build=build, cdHash=cdhash, archiveSHA256=digest, archive=archive), output, indent=2)
    output.write('\n')
PY
  cp "$prepared" "$release/prepared.json"
  print "Prepared only; not ready for distribution: $upload"
  print 'No credentials accessed and no upload attempted.'
  exit 0
fi
if [[ ! -e "$receipt" ]]; then
  [[ ! -e "$release/SUBMISSION_UNCERTAIN" ]] || {
    print -u2 "An earlier upload has an uncertain outcome. Check Apple submission history, then recover submission.json with its ID before resuming. Release: $release"; exit 1
  }
  # Check profile availability before attempting an upload. Authentication
  # failure is certain not to have submitted this package and is safe to retry.
  auth_log="$release/auth-$run.stderr.log"
  xcrun notarytool history --keychain-profile "$profile" --output-format json > /dev/null 2> "$auth_log" || {
    print -u2 "Notary profile preflight failed. No upload attempted. Inspect $auth_log, configure the Keychain profile, then rerun the same command."
    exit 1
  }
  submit_output="$release/submit-$run.json"
  # Persist uncertainty before Apple can receive any bytes. Receipt publication
  # is the only automatic transition that can clear this marker.
  python3 - "$release/SUBMISSION_UNCERTAIN" <<'PYMARKER'
import os, sys
with open(sys.argv[1], 'x') as output:
    output.write('Submission may have reached Apple; recover its ID before retrying.\n')
    output.flush()
    os.fsync(output.fileno())
PYMARKER
  submit_exit=0
  xcrun notarytool submit "$upload" --keychain-profile "$profile" --output-format json > "$submit_output" 2> "$release/submit-$run.stderr.log" || submit_exit=$?
  submission_id=$(json_value "$submit_output" id) || {
    print -u2 "No submission ID received (exit $submit_exit). Inspect $submit_output and its stderr log before retrying; Apple may have received the upload."
    # Do not silently upload again following an ambiguous network failure.
    touch "$release/SUBMISSION_UNCERTAIN"
    exit 1
  }
  valid_id "$submission_id" || { print -u2 'Unexpected submission ID; uncertainty retained.'; exit 1; }
  python3 - "$submit_output" "$receipt" <<'PYRECEIPT'
import json, os, sys
with open(sys.argv[1]) as source:
    receipt = json.load(source)
with open(sys.argv[2] + '.partial', 'w') as output:
    json.dump(receipt, output)
    output.flush()
    os.fsync(output.fileno())
os.replace(sys.argv[2] + '.partial', sys.argv[2])
PYRECEIPT
  rm "$release/SUBMISSION_UNCERTAIN"
else
  submission_id=$(json_value "$receipt" id) || { print -u2 'Receipt has no submission ID; inspect it before retrying.'; exit 1; }
fi
valid_id "$submission_id" || { print -u2 'Unexpected submission ID.'; exit 1; }
print "Submission: $submission_id"
wait_output="$release/wait-$run.json"
wait_exit=0
xcrun notarytool wait "$submission_id" --keychain-profile "$profile" --timeout 50s --output-format json > "$wait_output" 2> "$release/wait-$run.stderr.log" || wait_exit=$?
info_output="$release/status-$run.json"
xcrun notarytool info "$submission_id" --keychain-profile "$profile" --output-format json > "$info_output" 2> "$release/status-$run.stderr.log" || {
  print -u2 "Status unavailable; release is not ready. Resume with the same command. Receipt: $receipt"; exit 3
}
cp "$info_output" "$release/status.json"
notary_status=$(json_value "$info_output" status) || { print -u2 'Missing status; release is not ready.'; exit 3; }
case "$notary_status" in
  Accepted) ;;
  Invalid|Rejected)
    xcrun notarytool log "$submission_id" --keychain-profile "$profile" "$release/notary-log-$run.json" > "$release/log-$run.stdout.log" 2> "$release/log-$run.stderr.log" || true
    print -u2 "Notarization $notary_status. Retained logs: $release"; exit 1 ;;
  *) print -u2 "Notarization $notary_status (wait exit $wait_exit); release is not ready. Resume with the same command and profile. Receipt: $receipt"; exit 3 ;;
esac
# Retain the old receipt for recovery, but stop advertising readiness before
# mutating the app or replacing its archive. Failed retries remain unready.
if [[ -e "$release/ready.json" ]]; then
  mv "$release/ready.json" "$release/ready-before-$run.json"
fi
xcrun stapler staple "$app" > "$release/staple-$run.log" 2>&1
xcrun stapler validate "$app" > "$release/staple-validate-$run.log" 2>&1
verify_signed "$app"
spctl --assess --type execute --verbose=4 "$app" > "$release/gatekeeper-$run.log" 2>&1
final_zip="$release/Lindo-$version-$build-stapled.zip"
partial_final_zip="$release/Lindo-$version-$build-stapled-partial-$run.zip"
# Build off to the side: retain both the last complete archive and any failed
# partial output. Only publish the replacement after ditto completes.
ditto -c -k --sequesterRsrc --keepParent "$app" "$partial_final_zip"
[[ ! -e "$final_zip" ]] || mv "$final_zip" "$release/Lindo-$version-$build-stapled-before-$run.zip"
mv "$partial_final_zip" "$final_zip"
# A ready receipt is written only after every validation and final archive succeeds.
# The receipt binds the ready state to one exact binary and one exact archive.
final_hash=$(shasum -a 256 "$final_zip")
final_hash=${final_hash%% *}
stapled_hash=$(codesign -d --verbose=4 "$app" 2>&1 | sed -n 's/^CDHash=//p')
[[ -n "$stapled_hash" && -n "$final_hash" ]] || { print -u2 'Missing final CDHash or archive digest.'; exit 1; }
python3 - "$release/ready-$run.json" "$submission_id" "$final_zip" "$stapled_hash" "$final_hash" "$version" "$build" <<'PY'
import json, sys
with open(sys.argv[1], 'w') as output:
    json.dump(dict(status='ready', submissionId=sys.argv[2], archive=sys.argv[3], cdHash=sys.argv[4],
                   archiveSHA256=sys.argv[5], version=sys.argv[6], build=sys.argv[7]), output, indent=2)
    output.write('\n')
PY
cp "$release/ready-$run.json" "$release/ready-partial-$run.json"
mv "$release/ready-partial-$run.json" "$release/ready.json"
print "Ready: $final_zip"
