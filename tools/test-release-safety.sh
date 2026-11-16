#!/bin/zsh
# Synthetic subprocess regression tests. Never sign, install or contact Apple.
set -euo pipefail
here=${0:A:h}
python3 - "$here" <<'PY'
import hashlib, json, os, pathlib, subprocess, sys, tempfile
source = pathlib.Path(sys.argv[1])
base = pathlib.Path.home() / 'Library/Developer/Lindo/release-safety-tests'
base.mkdir(parents=True, exist_ok=True)
run = pathlib.Path(tempfile.mkdtemp(prefix='run-', dir=base))
bin_dir = run / 'bin'
bin_dir.mkdir()
stub = bin_dir / 'stub'
stub.write_text('#!' + sys.executable + '\n' + r'''
import json, os, pathlib, shutil, signal, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
case = os.environ['LINDO_TEST_CASE']
with open(os.environ['LINDO_TEST_LOG'], 'a') as log:
    log.write(name + ' ' + repr(args) + '\n')
target = str(pathlib.Path(os.environ['LINDO_TEST_HOME']) / 'Applications/Lindo.app')
if name == 'pgrep': sys.exit(0 if case == 'build-running' else 1)
if name == 'PlistBuddy':
    older = case == 'downgrade' and args[1] == 'Print :CFBundleVersion' and '/source/' in args[2]
    print('100' if older else {'Print :CFBundleIdentifier':'com.piernicolademaria.lindo',
           'Print :CFBundleVersion':'123', 'Print :CFBundleShortVersionString':'0.4.0'}[args[1]])
elif name == 'codesign':
    if '-d' in args:
        if case == 'build-signature-unreadable' and args[-1] == target: sys.exit(7)
        if case in ('build-install', 'build-adhoc-invalid') and args[-1] == target:
            print('Signature=adhoc\nCDHash=synthetic', file=sys.stderr)
        else:
            print('TeamIdentifier=SYNTHETIC1\nCodeDirectory flags=0x10000(runtime)\nTimestamp=Today\nCDHash=synthetic', file=sys.stderr)
    elif args[-1] == target and (case == 'codesign' or
          (case == 'build-adhoc-invalid' and (pathlib.Path(target) / 'marker').exists())): sys.exit(7)
elif name == 'spctl':
    if case == 'spctl' and args[-1] == target: sys.exit(7)
elif name == 'swift':
    executable = pathlib.Path(os.environ['LINDO_TEST_HOME']) / 'fake-bin/Lindo'
    executable.parent.mkdir(exist_ok=True)
    executable.write_text('new executable')
    executable.chmod(0o755)
    if '--show-bin-path' in args: print(executable.parent)
elif name == 'open': pass
elif name == 'xcrun':
    if args[:2] == ['stapler', 'validate'] and case == 'stapler' and args[-1] == target: sys.exit(7)
    ticket_failures = {'build-install': 66, 'build-running': 66, 'build-ticket-nohost': 68,
                      'build-ticket-timeout': 124, 'build-ticket-unknown': 1,
                      'build-ticket-noinput': 66, 'build-adhoc-invalid': 66,
                      'build-signature-unreadable': 68}
    if args[:2] == ['stapler', 'validate'] and args[-1] == target and case in ticket_failures:
        sys.exit(ticket_failures[case])
    if args[:2] == ['notarytool', 'history']: print('{}')
    elif args[:2] == ['notarytool', 'submit']:
        marker = pathlib.Path(args[2]).parent / 'SUBMISSION_UNCERTAIN'
        assert marker.exists(), 'Uncertainty must predate submit'
        if case == 'interrupted':
            os.kill(os.getppid(), signal.SIGKILL)
        else: print(json.dumps({'id': 'invalid' if case == 'invalid-id' else '11111111-1111-1111-1111-111111111111'}))
    elif args[:2] in (['notarytool', 'wait'], ['notarytool', 'info']): print(json.dumps({'status':'Accepted'}))
    elif args[0] != 'stapler': raise RuntimeError('Unexpected Apple operation: ' + repr(args))
elif name == 'ditto':
    if args[0] == '-c':
        pathlib.Path(args[-1]).write_text('partial' if case == 'archive' else 'complete archive')
        if case == 'archive': sys.exit(7)
    else: shutil.copytree(args[0], args[1])
else: raise RuntimeError('Unexpected command: ' + name)
''')
stub.chmod(0o755)
for name in ['codesign', 'spctl', 'xcrun', 'ditto', 'pgrep', 'PlistBuddy', 'swift', 'open']:
    (bin_dir / name).symlink_to(stub)
# Patch only isolated copies: no HOME override, no real app installation path.
for name in ['install-notarized.sh', 'notarize.sh']:
    text = (source / name).read_text().replace('$HOME', '$LINDO_TEST_HOME')
    text = text.replace('/usr/libexec/PlistBuddy', str(bin_dir / 'PlistBuddy'))
    (run / name).write_text(text)
# Build the isolated script with a synthetic icon tool and plist template.
build_text = (source.parent / 'build.sh').read_text().replace('$HOME', '$LINDO_TEST_HOME')
(run / 'build.sh').write_text(build_text)
(run / 'tools').mkdir()
icon = run / 'tools/make_icon.sh'
icon.write_text('#!/bin/zsh\nmkdir -p "$LINDO_TEST_HOME/Library/Developer/Lindo/icon"\nprint icon > "$LINDO_TEST_HOME/Library/Developer/Lindo/icon/AppIcon.icns"\n')
icon.chmod(0o755)
(run / 'Info.plist.in').write_text('<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleVersion</key><string>__BUILD__</string></dict></plist>')
failures = []
def check(condition, label):
    line = ('PASS ' if condition else 'FAIL ') + label
    print(line)
    with (run / 'results.log').open('a') as output: output.write(line + '\n')
    if not condition: failures.append(label)
def invoke(script, home, case, args, env_overrides=None):
    env = dict(os.environ, PATH=str(bin_dir) + ':' + os.environ['PATH'],
               LINDO_TEST_HOME=str(home), LINDO_TEST_CASE=case,
               LINDO_SIGNING_TEAM='SYNTHETIC1', LINDO_SIGNING_IDENTITY='Synthetic Developer ID',
               LINDO_TEST_LOG=str(home / 'commands.log'))
    for key, value in (env_overrides or {}).items():
        if value is None: env.pop(key, None)
        else: env[key] = value
    result = subprocess.run(['/bin/zsh', str(run / script), *map(str,args)], env=env,
                            capture_output=True, text=True, timeout=15)
    (home / 'output.log').write_text(result.stdout + result.stderr)
    return result
for case, overrides in [('missing-identity', {'LINDO_SIGNING_IDENTITY': None}),
                        ('missing-team', {'LINDO_SIGNING_TEAM': None}),
                        ('malformed-team', {'LINDO_SIGNING_TEAM': 'INVALID"TEAM'})]:
    home = run / case
    home.mkdir()
    result = invoke('build.sh', home, case, ['--signed'], overrides)
    check(result.returncode == 2 and not (home / 'commands.log').exists(),
          case + ' refuses signed build before compilation or signing')
home = run / 'signed-build'
home.mkdir()
result = invoke('build.sh', home, 'signed-build', ['--signed'])
log = (home / 'commands.log').read_text()
check(result.returncode == 0 and "'--sign', 'Synthetic Developer ID'" in log and
      'subject.OU] = "SYNTHETIC1"' in log and "'--options', 'runtime'" in log and "'--timestamp'" in log,
      'signed build uses the explicitly configured identity and team with hardened runtime')
for case in ['codesign', 'stapler', 'spctl', 'install-success']:
    home = run / case
    old = home / 'Applications/Lindo.app'
    old.mkdir(parents=True)
    (old / 'marker').write_text('previous')
    app = home / 'source/Lindo.app'
    app.mkdir(parents=True)
    (app / 'marker').write_text('new')
    result = invoke('install-notarized.sh', home, case, [app])
    if case == 'install-success':
        check(result.returncode == 0 and (old / 'marker').read_text() == 'new', 'successful verified install')
        # The stub accepts anything, so assert what the script actually asked for.
        log = (home / 'commands.log').read_text().splitlines()
        verifies = [line for line in log if line.startswith("codesign ['--verify'")]
        check(len(verifies) == 3 and all("'--deep'" in line and "'--strict'" in line and '-R=anchor apple generic' in line
              and 'subject.OU] = "SYNTHETIC1"' in line for line in verifies),
              'install checks the Developer ID requirement on source, staged and installed copies')
        check(sum(line.startswith("xcrun ['stapler', 'validate'") for line in log) == 3
              and sum(line.startswith("spctl ['--assess', '--type', 'execute'") for line in log) == 3,
              'install validates ticket and Gatekeeper on source, staged and installed copies')
    else:
        check(result.returncode != 0 and (old / 'marker').read_text() == 'previous', case + ' failure restores previous app')
for case in ['downgrade', 'downgrade-allowed']:
    home = run / case
    old = home / 'Applications/Lindo.app'
    (old / 'Contents').mkdir(parents=True)
    (old / 'Contents/Info.plist').write_text('installed')
    (old / 'marker').write_text('previous')
    app = home / 'source/Lindo.app'
    app.mkdir(parents=True)
    (app / 'marker').write_text('older')
    if case == 'downgrade':
        result = invoke('install-notarized.sh', home, 'downgrade', [app])
        check(result.returncode != 0 and (old / 'marker').read_text() == 'previous' and 'Refusing' in result.stderr,
              'older notarized build is refused over a newer install')
    else:
        os.environ['LINDO_ALLOW_DOWNGRADE'] = '1'
        result = invoke('install-notarized.sh', home, 'downgrade', [app])
        del os.environ['LINDO_ALLOW_DOWNGRADE']
        check(result.returncode == 0 and (old / 'marker').read_text() == 'older', 'explicit override installs the older build')
for case in ['receipt-match', 'receipt-mismatch']:
    home = run / case
    old = home / 'Applications/Lindo.app'
    old.mkdir(parents=True)
    (old / 'marker').write_text('previous')
    app = home / 'source/Lindo.app'
    app.mkdir(parents=True)
    (app / 'marker').write_text('new')
    (home / 'source/ready.json').write_text(json.dumps({'status': 'ready', 'cdHash': 'synthetic' if case == 'receipt-match' else 'another-binary'}))
    result = invoke('install-notarized.sh', home, 'install-success', [app])
    if case == 'receipt-match':
        check(result.returncode == 0 and (old / 'marker').read_text() == 'new' and 'Release receipt matches' in result.stdout,
              'install accepts the binary named by the release receipt')
    else:
        check(result.returncode != 0 and (old / 'marker').read_text() == 'previous' and 'does not match' in result.stderr,
              'install refuses a binary that differs from the release receipt')
def write_manifest(release):
    (release / 'upload-manifest.json').write_text(json.dumps(dict(
        archiveSHA256=hashlib.sha256((release / 'Lindo-upload.zip').read_bytes()).hexdigest(),
        bundleId='com.piernicolademaria.lindo', version='0.4.0', build='123', cdHash='synthetic')))
for case in ['archive', 'archive-success']:
    home = run / case
    release = home / 'Library/Developer/Lindo/releases/0.4.0-123'
    app = release / 'Lindo.app'
    app.mkdir(parents=True)
    archive = release / 'Lindo-0.4.0-123-stapled.zip'
    archive.write_text('previous archive')
    (release / 'Lindo-upload.zip').write_text('upload')
    write_manifest(release)
    (release / 'submission.json').write_text(json.dumps({'id':'11111111-1111-1111-1111-111111111111'}))
    (release / 'ready.json').write_text(json.dumps({'status':'ready', 'archive':str(archive)}))
    result = invoke('notarize.sh', home, case, [app, '--profile', 'synthetic'])
    if case == 'archive':
        check(result.returncode != 0 and not (release / 'ready.json').exists(), 'failed repack invalidates ready receipt')
        check(any(p.read_text() == 'previous archive' for p in release.glob('*.zip')), 'failed repack retains previous archive')
        check(any(p.read_text() == 'partial' for p in release.glob('*.zip')), 'failed repack retains partial archive')
        check(bool(list(release.glob('ready-before-*.json'))), 'failed repack retains old receipt for recovery')
    else:
        ready = json.loads((release / 'ready.json').read_text())
        check(result.returncode == 0 and pathlib.Path(ready['archive']).read_text() == 'complete archive', 'successful repack publishes ready archive')
        check(ready.get('cdHash') == 'synthetic' and ready.get('version') == '0.4.0' and ready.get('build') == '123'
              and ready.get('archiveSHA256') == hashlib.sha256(pathlib.Path(ready['archive']).read_bytes()).hexdigest(),
              'ready receipt binds the exact binary and archive')
        check(any(p.read_text() == 'previous archive' for p in release.glob('*.zip')), 'successful repack retains previous archive')
for case in ['build-default', 'build-open', 'build-install', 'build-no-install', 'build-over-notarized', 'build-replace-notarized', 'build-running']:
    home = run / case
    old = home / 'Applications/Lindo.app'
    old.mkdir(parents=True)
    (old / 'marker').write_text('previous')
    args = {'build-default': [], 'build-open': ['--open'], 'build-install': ['--install'],
            'build-no-install': ['--install', '--no-install'], 'build-over-notarized': ['--install'],
            'build-replace-notarized': ['--install', '--replace-notarized'], 'build-running': ['--install']}[case]
    result = invoke('build.sh', home, case, args)
    if case in ('build-over-notarized', 'build-running'):
        check(result.returncode != 0 and (old / 'marker').read_text() == 'previous' and
              bool(list(home.glob('Library/Developer/Lindo/staging/*/Lindo.app/Contents/MacOS/Lindo'))),
              case + ' refuses to replace the installed app and keeps the staged build')
    elif case in ('build-install', 'build-replace-notarized'):
        check(result.returncode == 0 and (old / 'Contents/MacOS/Lindo').exists() and
              any(p.read_text() == 'previous' for p in home.glob('Library/Developer/Lindo/backups/*/marker')),
              'explicit build install retains previous app')
    else:
        check(result.returncode == 0 and (old / 'marker').read_text() == 'previous' and
              bool(list(home.glob('Library/Developer/Lindo/staging/*/Lindo.app/Contents/MacOS/Lindo'))),
              case + ' stages without replacing installed app')
        if case == 'build-open':
            check('open [' in (home / 'commands.log').read_text(), 'open launches the staged copy')
# Ticket failures include missing/invalid tickets, network errors and unknown
# failures. None identifies a non-notarized app without a valid ad hoc signature.
for case in ['build-ticket-nohost', 'build-ticket-timeout', 'build-ticket-unknown',
             'build-ticket-noinput', 'build-adhoc-invalid', 'build-signature-unreadable']:
    for override in [False, True]:
        home = run / (case + ('-override' if override else ''))
        old = home / 'Applications/Lindo.app'
        old.mkdir(parents=True)
        (old / 'marker').write_text('previous')
        args = ['--install'] + (['--replace-notarized'] if override else [])
        result = invoke('build.sh', home, case, args)
        log = (home / 'commands.log').read_text()
        if override:
            check(result.returncode == 0 and (old / 'Contents/MacOS/Lindo').exists() and
                  any(p.read_text() == 'previous' for p in home.glob('Library/Developer/Lindo/backups/*/marker')) and
                  "xcrun ['stapler', 'validate'" not in log,
                  case + ' explicit override replaces the app and retains its backup')
        else:
            check(result.returncode != 0 and (old / 'marker').read_text() == 'previous' and
                  'Cannot establish' in result.stderr and '--replace-notarized' in result.stderr and
                  bool(list(home.glob('Library/Developer/Lindo/staging/*/Lindo.app/Contents/MacOS/Lindo'))) and
                  "xcrun ['stapler', 'validate'" in log,
                  case + ' refuses uncertain replacement and retains the staged build')
for case in ['tampered', 'manifestless', 'identity-mismatch', 'interrupted', 'invalid-id', 'submit-success']:
    home = run / case
    app = home / 'source/Lindo.app'
    app.mkdir(parents=True)
    release = home / 'Library/Developer/Lindo/releases/0.4.0-123'
    args = [app, '--profile', 'synthetic']
    prepared = invoke('notarize.sh', home, case, args + ['--prepare-only'])
    check(prepared.returncode == 0 and (release / 'upload-manifest.json').exists(), case + ' creates bound archive')
    if case == 'tampered': (release / 'Lindo-upload.zip').write_text('tampered')
    if case == 'manifestless': (release / 'upload-manifest.json').unlink()
    if case == 'identity-mismatch':
        manifest = json.loads((release / 'upload-manifest.json').read_text())
        manifest['cdHash'] = 'other'
        (release / 'upload-manifest.json').write_text(json.dumps(manifest))
    if case in ['tampered', 'manifestless', 'identity-mismatch']:
        for flags in [[], ['--prepare-only']]:
            result = invoke('notarize.sh', home, case, args + flags)
            check(result.returncode != 0 and 'xcrun ' not in (home / 'commands.log').read_text(),
                  case + ' blocked before network ' + repr(flags))
    else:
        result = invoke('notarize.sh', home, case, args)
        if case == 'submit-success':
            check(result.returncode == 0 and (release / 'submission.json').exists() and
                  not (release / 'SUBMISSION_UNCERTAIN').exists(), 'valid submission atomically published and marker cleared')
            continue
        check(result.returncode != 0 and (release / 'SUBMISSION_UNCERTAIN').exists() and
              not (release / 'submission.json').exists(), case + ' retains uncertainty without receipt')
        lock = release / '.notarize-lock'
        if lock.exists(): lock.rmdir()  # Simulate explicit stale-lock recovery after process death.
        retry = invoke('notarize.sh', home, case, args)
        check(retry.returncode != 0 and (home / 'commands.log').read_text().count("['notarytool', 'submit'") == 1,
              case + ' retry never resubmits')
        # Legacy recovery receipts only contain the UUID; they remain supported.
        (release / 'submission.json').write_text(json.dumps({'id':'11111111-1111-1111-1111-111111111111'}))
        recovered = invoke('notarize.sh', home, case, args)
        check(recovered.returncode == 0 and (home / 'commands.log').read_text().count("['notarytool', 'submit'") == 1,
              case + ' legacy receipt resumes without upload')
print('Evidence: ' + str(run))
sys.exit(bool(failures))
PY
