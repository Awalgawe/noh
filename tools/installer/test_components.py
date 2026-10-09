"""Component archive boundaries and tamper rejection using synthetic payloads."""
import copy
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import public_components as content


class ComponentArchives(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bundle = self.root / 'bundle'
        self.archives = self.root / 'archives'
        for name in ('noh.exe', 'bin/noh-cli.exe', 'bin/noh-mcp.exe', 'bin/ffmpeg.exe',
                     'bin/preview/libmpv-2.dll', 'bin/speech/whisper-cli.exe',
                     'bin/speech/models/model.bin', 'licenses/NOTICE.txt', 'manifest.json'):
            path = self.bundle / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name, encoding='utf-8')

    def prepare(self):
        with patch.object(content.delivery, 'verify_bundle', return_value={'version': '0.1.1'}), \
                patch.object(content.delivery, 'authorize_redistribution'):
            return content.prepare(self.bundle, 'a' * 40, self.archives)

    def test_archives_have_only_their_runtime_and_original_notices(self):
        catalog = self.prepare()
        self.assertEqual(set(catalog['groups']['media']['files']), {'bin/ffmpeg.exe', 'bin/preview/libmpv-2.dll'})
        self.assertEqual(set(catalog['groups']['speech']['files']),
                         {'bin/speech/whisper-cli.exe', 'bin/speech/models/model.bin'})
        for item in catalog['groups'].values():
            self.assertIn('licenses/NOTICE.txt', item['archive_files'])
            self.assertNotIn('noh.exe', item['archive_files'])
        content.validate(catalog, self.archives)
        stamps = {p.name: p.stat().st_mtime_ns for p in self.archives.glob('*.zip')}
        self.assertEqual(catalog, self.prepare())
        self.assertEqual(stamps, {p.name: p.stat().st_mtime_ns for p in self.archives.glob('*.zip')})

    def test_changed_download_is_rejected(self):
        catalog = self.prepare()
        path = self.archives / catalog['groups']['media']['name']
        path.write_bytes(path.read_bytes() + b'corrupt')
        with self.assertRaisesRegex(ValueError, 'archive changed'):
            content.validate(catalog, self.archives)

    def test_catalog_cannot_smuggle_app_or_mix_runtime_groups(self):
        catalog = self.prepare()
        for name in ('noh.exe', 'bin/speech/unexpected.exe'):
            changed = copy.deepcopy(catalog)
            changed['groups']['media']['archive_files'][name] = {'sha256': 'a' * 64, 'size': 1}
            with self.assertRaisesRegex(ValueError, 'Unexpected component'):
                content.validate(changed, self.archives)

    def test_inventory_identity_is_bound_to_the_asset_name(self):
        catalog = self.prepare()
        catalog['groups']['speech']['inventory_sha256'] = 'a' * 64
        with self.assertRaisesRegex(ValueError, 'inventory identity differs'):
            content.validate(catalog, self.archives)

    def test_inno_catalog_rejects_source_injection(self):
        catalog = self.prepare()
        catalog['groups']['media']['files']["bin/evil';.dll"] = {'sha256': 'a' * 64, 'size': 1}
        with self.assertRaisesRegex(ValueError, 'Unsafe component path'):
            content.script(catalog)


if __name__ == '__main__':
    unittest.main()
