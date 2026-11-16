#!/usr/bin/env python3
"""Test export boundaries with disposable synthetic inputs, never real secrets."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location('source_export', Path(__file__).with_name('export-source.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SourceExportTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='lindo-export-tests-')
        self.base = Path(self.temporary.name)
        self.repo = self.base / 'repo'
        self.repo.mkdir()
        for name in module.ROOT_FILES:
            (self.repo / name).write_text('synthetic source\n')
        for folder in ('Sources/Lindo', 'Tests/LindoKitTests'):
            (self.repo / folder).mkdir(parents=True)
            (self.repo / folder / 'Example.swift').write_text('// synthetic source\n')
        for name in module.TOOLS:
            path = self.repo / 'tools' / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('# synthetic tool\n')
        for name in module.ICONS:
            path = self.repo / 'Icon' / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('synthetic icon\n')
        (self.repo / 'docs/public').mkdir(parents=True)
        for name in module.PUBLIC_DOCS:
            (self.repo / 'docs/public' / name).write_text('public documentation\n')
        (self.repo / 'tools/ci').mkdir()
        (self.repo / 'tools/ci/public-ci.yml').write_text('name: Synthetic\n')

    def tearDown(self):
        self.temporary.cleanup()

    def test_allowlist_excludes_private_artifacts_and_history(self):
        for name in ('.git/config', '.env', '_HANDOFF.md', 'handover/release.zip',
                     'docs/qa/receipt.json', 'tools/ci/notarize-lindo.yml',
                     'Sources/Lindo/private.pem', 'Sources/.git/private.swift', 'Icon/private-note.txt'):
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('PRIVATE SENTINEL')
        output = self.base / 'export'
        module.export(self.repo, output)
        self.assertEqual((output / 'README.md').read_text(), 'public documentation\n')
        self.assertTrue((output / '.github/workflows/ci.yml').is_file())
        for path in output.rglob('*'):
            if path.is_file():
                self.assertNotIn(b'PRIVATE SENTINEL', path.read_bytes())

    def test_deterministic_zip_and_manifest(self):
        tool = self.repo / 'build.sh'
        tool.chmod(0o755)
        first, second = self.base / 'one.zip', self.base / 'two.zip'
        module.export(self.repo, self.base / 'one', first)
        module.export(self.repo, self.base / 'two', second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with zipfile.ZipFile(first) as archive:
            manifest = json.loads(archive.read('Lindo/SOURCE-MANIFEST.json'))
            for entry in manifest['files']:
                self.assertEqual(entry['sha256'], hashlib.sha256(archive.read('Lindo/' + entry['path'])).hexdigest())
            self.assertEqual(archive.getinfo('Lindo/build.sh').external_attr >> 16 & 0o777, 0o755)

    def test_symlink_is_rejected_before_export(self):
        source = self.repo / 'Sources/Lindo/Example.swift'
        source.unlink()
        source.symlink_to(self.repo / 'LICENSE')
        with self.assertRaises(module.ExportError):
            module.export(self.repo, self.base / 'export')
        self.assertFalse((self.base / 'export').exists())

    def test_suspected_secret_reports_only_filename_and_rule(self):
        secret = 'ghp_' + 'A' * 36
        (self.repo / 'Sources/Lindo/Example.swift').write_text(secret)
        with self.assertRaises(module.ExportError) as failure:
            module.export(self.repo, self.base / 'export')
        self.assertIn('github-token', str(failure.exception))
        self.assertNotIn(secret, str(failure.exception))
        self.assertFalse((self.base / 'export').exists())

    def test_personal_home_path_is_rejected(self):
        path = '/Users/' + 'private-person' + '/Documents/file'
        (self.repo / 'Sources/Lindo/Example.swift').write_text(path)
        with self.assertRaises(module.ExportError) as failure:
            module.export(self.repo, self.base / 'export')
        self.assertNotIn(path, str(failure.exception))

    def test_existing_destination_is_preserved(self):
        output = self.base / 'existing'
        output.mkdir()
        (output / 'marker').write_text('keep')
        with self.assertRaises(module.ExportError):
            module.export(self.repo, output)
        self.assertEqual((output / 'marker').read_text(), 'keep')

    def test_missing_public_document_never_falls_back_to_internal_readme(self):
        (self.repo / 'docs/public/README.md').unlink()
        (self.repo / 'README.md').write_text('PRIVATE SENTINEL')
        with self.assertRaises(module.ExportError):
            module.export(self.repo, self.base / 'export')
        self.assertFalse((self.base / 'export').exists())

    def test_public_checkout_can_be_exported_again(self):
        first, second = self.base / 'one', self.base / 'two'
        module.export(self.repo, first)
        module.export(first, second)
        self.assertEqual((first / 'SOURCE-MANIFEST.json').read_bytes(),
                         (second / 'SOURCE-MANIFEST.json').read_bytes())


if __name__ == '__main__':
    unittest.main()
