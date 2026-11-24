#!/usr/bin/env python3
"""Export a reviewed allowlist, without copying Git history or local artifacts."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import tempfile
import zipfile

ROOT_FILES = ('.gitignore', 'Cargo.toml', 'Cargo.lock', 'Info.plist.in', 'Lindo.entitlements', 'build.sh', 'LICENSE')
TOOLS = ('selftest.sh', 'test-release-safety.sh', 'scale-ui.py', 'install-notarized.sh',
         'notarize.sh', 'export-source.py', 'test-export.py')
BUNDLE_ICON = 'launch/brand/assets/AppIcon.icns'
ICON_SHA256 = '7223e2013d2ff00e6f67fd8e80d7acd02ca2cba622daf30215cd3a551d4baf87'
REVIEWED_BINARIES = {
    BUNDLE_ICON: ICON_SHA256,
    'src/artwork/home.png': '83ff37d29d82bcf00db2ac8937e2e9b29b1371d6a23d6568cf849e956371bdd8',
    'src/artwork/logo.png': '71e73f57727eb31c8b47d083c4a5f81ceb2ba6671e18751877af5fbabf0eb332',
    'src/artwork/scan.png': '5a34240dc6a6ca989d6887b9df22b78495352a1ee9afe8b04f37cbb34c0b487e',
}
ICONS = ('Mark.svg', 'AppIcon.icon/icon.json', 'AppIcon.icon/Assets/back.svg',
         'AppIcon.icon/Assets/front.svg')
PUBLIC_DOCS = ('README.md', 'BUILD.md', 'SECURITY.md', 'CONTRIBUTING.md', 'FEATURES.md')
PATTERNS = {
    'private-key': re.compile(r'-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----'),
    'github-token': re.compile(r'(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})'),
    'aws-access-key': re.compile(r'(?:AKIA|ASIA)[A-Z0-9]{16}'),
    'slack-token': re.compile(r'xox[baprs]-[A-Za-z0-9-]{20,}'),
    'api-key': re.compile(r'sk-(?:proj-)?[A-Za-z0-9_-]{32,}'),
    'credential-url': re.compile(r'https?://[^\s/:@]+:[^\s/@]+@'),
    'credential-assignment': re.compile(r'''(?im)\b(?:api[_-]?key|auth[_-]?token|access[_-]?token|secret[_-]?key|password)\s*[:=]\s*["'][^"'\r\n]{12,}["']'''),
}
USER_PATH = re.compile('/' + r'Users/([^/\s"\'\\<>]+)')
SYNTHETIC_USERS = {'demo', 'demolition', 'SyntheticLindoUser'}


class ExportError(Exception):
    pass


def inspect_text(name: str, data: bytes) -> None:
    try:
        text = data.decode('utf-8')
    except UnicodeDecodeError:
        raise ExportError(f'Unexpected binary content: {name}') from None
    labels = [label for label, pattern in PATTERNS.items() if pattern.search(text)]
    if any(match.group(1) not in SYNTHETIC_USERS for match in USER_PATH.finditer(text)):
        labels.append('personal-home-path')
    if labels:
        # Report only a file name and rule, never the suspected value.
        raise ExportError(f'Content review required: {name} ({", ".join(labels)})')


def source_files(root: Path) -> dict[str, Path]:
    files = {name: root / name for name in ROOT_FILES}
    files.update({name: root / name for name in REVIEWED_BINARIES})
    for folder in ('src',):
        if not (root / folder).is_dir():
            raise ExportError(f'Missing source directory: {folder}')
        for path in (root / folder).rglob('*.rs'):
            if any(part.startswith('.') for part in path.relative_to(root).parts):
                continue
            files[path.relative_to(root).as_posix()] = path
    files.update({f'tools/{name}': root / 'tools' / name for name in TOOLS})
    files.update({f'Icon/{name}': root / 'Icon' / name for name in ICONS})
    docs = root / 'docs/public'
    if not docs.is_dir():
        if not (root / 'SOURCE-MANIFEST.json').is_file():
            raise ExportError('Missing public documentation; internal documents are never substituted.')
        docs = root  # A previously exported public checkout.
    for name in PUBLIC_DOCS:
        files[name] = docs / name
    workflow = root / 'tools/ci/public-ci.yml'
    files['tools/ci/public-ci.yml'] = workflow
    files['.github/workflows/ci.yml'] = workflow
    return files


def collect(root: Path) -> dict[str, tuple[bytes, int]]:
    result = {}
    for name, source in sorted(source_files(root).items()):
        relative = source.relative_to(root)
        if source.is_symlink() or any((root / Path(*relative.parts[:i])).is_symlink()
                                      for i in range(1, len(relative.parts))):
            raise ExportError(f'Symlink is not an exportable source: {name}')
        if not source.is_file():
            raise ExportError(f'Missing regular source file: {name}')
        data = source.read_bytes()
        if name in REVIEWED_BINARIES:
            if hashlib.sha256(data).hexdigest() != REVIEWED_BINARIES[name]:
                raise ExportError(f'Unreviewed bundled artwork: {name}')
        else:
            inspect_text(name, data)
        mode = 0o755 if source.stat().st_mode & 0o111 else 0o644
        result[name] = (data, mode)
    manifest = {'schema': 1, 'files': [
        {'path': name, 'sha256': hashlib.sha256(data).hexdigest(),
         'bytes': len(data), 'mode': f'{mode:04o}'}
        for name, (data, mode) in result.items()
    ]}
    result['SOURCE-MANIFEST.json'] = ((json.dumps(manifest, indent=2, sort_keys=True) + '\n').encode(), 0o644)
    return result


def export(root: Path, output: Path, archive: Path | None = None) -> dict[str, object]:
    root, output = root.resolve(), output.absolute()
    if output.exists() or output.is_symlink():
        raise ExportError('Output must be a new directory; existing data is never replaced.')
    if not output.parent.is_dir():
        raise ExportError('Output parent directory must already exist.')
    if archive is not None:
        archive = archive.absolute()
        if archive.exists() or archive.is_symlink() or not archive.parent.is_dir():
            raise ExportError('Archive must be a new file in an existing directory.')
        if archive.is_relative_to(output):
            raise ExportError('Archive must be outside the exported source directory.')
    files = collect(root)  # Validate everything before writing any export.
    temporary = Path(tempfile.mkdtemp(prefix='.lindo-export-', dir=output.parent))
    try:
        for name, (data, mode) in files.items():
            destination = temporary / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            destination.chmod(mode)
        # mkdir without exist_ok reserves a new destination and never replaces one.
        output.mkdir()
        for child in temporary.iterdir():
            child.rename(output / child.name)
    finally:
        shutil.rmtree(temporary)
    summary: dict[str, object] = {'files': len(files), 'contentScan': 'passed'}
    if archive is not None:
        with zipfile.ZipFile(archive, 'x', compression=zipfile.ZIP_STORED) as zipped:
            for name, (data, mode) in sorted(files.items()):
                info = zipfile.ZipInfo('Lindo/' + name, date_time=(2026, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = (stat.S_IFREG | mode) << 16
                zipped.writestr(info, data)
        summary['archiveSHA256'] = hashlib.sha256(archive.read_bytes()).hexdigest()
    return summary


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--zip', dest='archive', type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(export(args.source, args.output, args.archive), sort_keys=True))
    except (ExportError, OSError) as error:
        parser.exit(1, f'Export refused: {error}\n')


if __name__ == '__main__':
    main()
