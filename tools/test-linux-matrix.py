"""Reject incomplete matrix evidence and falsely successful GUI preview checks."""
import copy
import importlib.util
from pathlib import Path
import unittest
import tempfile
import sys
from unittest import mock

from delivery import digest, read
import linux_matrix as matrix

spec = importlib.util.spec_from_file_location('probe', Path(__file__).with_name('probe-linux-host.py'))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class MatrixTests(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.evidence_root = Path(folder.name)
        path = self.evidence_root / 'observations.txt'
        path.write_text('Synthetic fixture observations; not real qualification.')
        self.evidence = {'path': path.name, 'sha256': digest(path)}
        patch = mock.patch.object(matrix, 'ROOT', self.evidence_root)
        patch.start()
        self.addCleanup(patch.stop)

    def fixture(self, desktop=False):
        names = ('usr/lib/noh/noh-app', 'usr/lib/noh/bin/noh-cli',
                 'usr/lib/noh/bin/noh-mcp', 'usr/lib/noh/bin/speech/whisper-cli')
        hashes = {name: str(i) * 64 for i, name in enumerate(names)}
        record = {'bundle_files': {'NOH.AppDir/' + n: {'sha256': h} for n, h in hashes.items()}}
        rows = read(matrix.MATRIX)['required_desktops'] if desktop else matrix.cases()
        result = {'schema_version': 1, 'status': 'passed', 'delivery_sha256': 'a' * 64,
                  'matrix_sha256': digest(matrix.MATRIX),
                  'scope': 'desktop-integration' if desktop else 'container-software-rendering',
                  'cases': {}}
        for environment in rows:
            environment = environment | {'prepared_image_id': 'b' * 64}
            if desktop:
                environment.update(machine_kind='virtual-machine', os_image_identity='fixture image hash',
                                   gpu='virtual GPU', audio_device='virtual audio', tested_at='2026-10-15', reviewer='Fixture reviewer')
            result['cases'][environment['id']] = {'status': 'passed', 'elf_sha256': hashes.copy(),
                'checks': {check: 'passed' for check in matrix.CHECKS}, 'environment': environment,
                'evidence': [self.evidence]}
        return result, record

    def test_complete_container_and_desktop_evidence(self):
        for desktop in (False, True):
            value, record = self.fixture(desktop)
            matrix.validate_result(value, record, 'a' * 64, desktop)

    def test_candidate_change_or_missing_distribution_is_rejected(self):
        value, record = self.fixture()
        for key, replacement in [('delivery_sha256', 'b' * 64), ('matrix_sha256', 'b' * 64),
                                 ('status', 'prepared')]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                matrix.validate_result(value | {key: replacement}, record, 'a' * 64)
        value['cases'].pop('arch')
        with self.assertRaises(ValueError):
            matrix.validate_result(value, record, 'a' * 64)

    def test_bad_candidate_is_rejected_before_preparing_images(self):
        envelope = self.evidence_root / 'assets'
        envelope.mkdir()
        (envelope / 'DELIVERY.json').write_text('{}')
        argv = ['linux_matrix.py', '--envelope', str(envelope), '--speech-fixture',
                str(self.evidence_root / 'observations.txt'), '--output', str(self.evidence_root / 'output')]
        with mock.patch.object(sys, 'argv', argv), mock.patch.object(matrix, 'prepare') as prepare:
            with self.assertRaisesRegex(ValueError, 'Unsupported candidate record'):
                matrix.main()
            prepare.assert_not_called()

    def test_missing_runtime_checks_or_changed_executables_are_rejected(self):
        for mutation in ('checks', 'elf_sha256', 'status'):
            value, record = self.fixture()
            value['cases']['fedora-44'][mutation] = {} if mutation != 'status' else 'prepared'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                matrix.validate_result(value, record, 'a' * 64)

    def test_mutable_image_or_wrong_audio_provider_is_rejected(self):
        for key, replacement in [('image', 'ubuntu:latest'), ('audio', 'pulseaudio'),
                                 ('prepared_image_id', '')]:
            value, record = self.fixture()
            value['cases']['fedora-44']['environment'][key] = replacement
            with self.subTest(key=key), self.assertRaises(ValueError):
                matrix.validate_result(value, record, 'a' * 64)

    def test_containers_cannot_qualify_mint_or_real_desktops(self):
        value, record = self.fixture()
        with self.assertRaises(ValueError):
            matrix.validate_result(value, record, 'a' * 64, desktop=True)

    def test_desktop_observations_are_required_with_exact_hashes(self):
        value, record = self.fixture(True)
        (self.evidence_root / 'observations.txt').write_text('Changed observations')
        with self.assertRaisesRegex(ValueError, 'changed Linux desktop observations'):
            matrix.validate_result(value, record, 'a' * 64, desktop=True)
        value, record = self.fixture(True)
        value['cases']['mint-22.3']['environment']['machine_kind'] = 'container'
        with self.assertRaises(ValueError):
            matrix.validate_result(value, record, 'a' * 64, desktop=True)

    def test_gui_requires_real_decoded_video_and_audio(self):
        value = {'fixture': False, 'timed_out': False, 'capture_ready': True,
                 'project_terminal': 'finished', 'resource_issues': 0,
                 'mini_preview': {'explicit_ffmpeg': False, 'player': {'audio_available': True,
                                  'frame': {'position': 1.2, 'width': 128, 'height': 72}}}}
        probe.check_gui_report(value)
        for field, replacement in [('fixture', True), ('project_terminal', 'failed'), ('timed_out', True)]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.check_gui_report(value | {field: replacement})
        for field, replacement in [('audio_available', False), ('frame', {}), ('warning', 'No audio')]:
            changed = copy.deepcopy(value)
            changed['mini_preview']['player'][field] = replacement
            with self.subTest(field=field), self.assertRaises(ValueError):
                probe.check_gui_report(changed)


if __name__ == '__main__':
    unittest.main()
