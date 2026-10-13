"""Verify Linux archives reject missing private libraries and lost launch modes."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import zipfile

import delivery

spec = importlib.util.spec_from_file_location('elf_fixtures', Path(__file__).with_name('test-linux-elf.py'))
elf_fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(elf_fixtures)
COMMIT = 'a' * 40


class LinuxPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / 'bundle'
        home = self.bundle / 'NOH.AppDir/usr/lib/noh'
        home.joinpath('lib').mkdir(parents=True)
        self.app = home / 'noh-app'
        self.app.write_bytes(elf_fixtures.fixture(('libnative.so.1',), '$ORIGIN/lib'))
        self.library = home / 'lib/libnative.so.1'
        self.library.write_bytes(elf_fixtures.fixture((), '$ORIGIN', 'libnative.so.1', interpreter=None))
        self.launcher = self.bundle / 'NOH.AppDir/AppRun'
        self.launcher.write_bytes(b'#!/bin/sh\nexec "$(dirname "$0")/usr/lib/noh/noh-app" "$@"\n')
        for path in (self.app, self.library, self.launcher):
            path.chmod(0o755)
        self.materials = self.root / 'materials'
        self.materials.mkdir()
        (self.materials / 'LICENSE.txt').write_text('Synthetic fixture notice')
        self.manifest = {'schema_version': 2, 'version': '0.1.0', 'build': {
            'target': 'x86_64-unknown-linux-gnu', 'profile': 'release', 'features': ['gui', 'mcp'],
            'git_revision': COMMIT, 'git_dirty': False}}
        self.update_manifest()

    def update_manifest(self):
        self.manifest['sha256'] = {n: v['sha256'] for n, v in delivery.inventory(self.bundle).items()
                                   if n != 'manifest.json'}
        delivery.write(self.bundle / 'manifest.json', self.manifest)

    def seal(self):
        output = self.root / 'assets'
        delivery.seal(self.bundle, self.materials, output, 'linux-x64', COMMIT)
        return output

    def test_linux_archive_preserves_executable_modes_and_closure(self):
        output = self.seal()
        record = delivery.verify_envelope(output)
        self.assertEqual(record['platform'], 'linux-x64')
        self.assertEqual(set(record['assets']), {'NOH-0.1.0-linux-x64.zip', 'NOH-0.1.0-linux-x64-materials.zip'})

    def test_missing_library_blocks_seal_even_with_consistent_manifest(self):
        self.library.unlink()
        self.update_manifest()
        with self.assertRaisesRegex(ValueError, 'Unresolved'):
            self.seal()
        self.assertFalse((self.root / 'assets').exists())

    def test_unexecutable_launcher_is_rejected_even_with_consistent_hashes(self):
        self.launcher.chmod(0o644)
        with self.assertRaisesRegex(ValueError, 'AppRun lost'):
            self.seal()

    def test_tampered_zip_mode_is_rejected_after_updating_outer_checksums(self):
        output = self.seal()
        archive = output / 'NOH-0.1.0-linux-x64.zip'
        with zipfile.ZipFile(archive) as zipped:
            contents = [(info, zipped.read(info)) for info in zipped.infolist()]
        with zipfile.ZipFile(archive, 'w') as zipped:
            for info, data in contents:
                if info.filename.endswith('/noh-app'):
                    info.external_attr = 0o100644 << 16
                zipped.writestr(info, data)
        record = delivery.read(output / 'DELIVERY.json')
        record['assets'][archive.name] = {'sha256': delivery.digest(archive), 'size': archive.stat().st_size}
        delivery.write(output / 'DELIVERY.json', record)
        (output / 'SHA256SUMS').write_text(''.join(
            f'{delivery.digest(p)}  {p.name}\n' for p in sorted(output.iterdir()) if p.name != 'SHA256SUMS'))
        with self.assertRaisesRegex(ValueError, 'ELF lost'):
            delivery.verify_envelope(output)


if __name__ == '__main__':
    unittest.main()
