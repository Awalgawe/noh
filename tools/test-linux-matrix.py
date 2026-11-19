"""Reject incomplete matrix evidence, falsely successful GUI preview checks and MCP jobs."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import unittest
import tempfile
import sys
import time
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


# A stand-in MCP server: newline-delimited JSON-RPC on stdio, with one way to misbehave per mode.
FAKE_MCP_SERVER = r'''
import json, os, sys, time
mode, record = sys.argv[1], sys.argv[2]
tools = ['noh_inspect', 'noh_diagnose', 'noh_job', 'noh_export', 'noh_preview']
if mode == 'extra_tool':
    tools.append('noh_added_later')
if mode == 'missing_tool':
    tools.remove('noh_diagnose')
jobs = {}
def reply(message_id, result):
    print(json.dumps({'jsonrpc': '2.0', 'id': message_id, 'result': result}), flush=True)
for line in sys.stdin:
    message = json.loads(line)
    method, params = message.get('method'), message.get('params', {})
    if 'id' not in message:
        continue
    if mode == 'silent':
        continue
    if method == 'initialize':
        with open(record, 'w') as output:
            json.dump({'environment': dict(os.environ), 'cwd': os.getcwd()}, output)
        version = '2024-11-05' if mode == 'old_protocol' else params['protocolVersion']
        print(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/message', 'params': {}}), flush=True)
        reply(message['id'], {'protocolVersion': version, 'serverInfo': {'name': 'fake', 'version': '1'},
                              'capabilities': {'tools': {}}})
    elif method == 'tools/list':
        reply(message['id'], {'tools': [{'name': name} for name in tools]})
    elif method == 'tools/call':
        name, arguments = params['name'], params['arguments']
        if name == 'noh_job':
            job = jobs[arguments['job_id']]
            reply(message['id'], {'structuredContent': job, 'isError': False})
            continue
        job_id = name + '-' + str(len(jobs))
        state = 'failed' if mode == 'job_fails' and name == 'noh_diagnose' else 'succeeded'
        result = None
        if name == 'noh_export':
            if mode != 'no_output':
                open(arguments['request']['output'], 'wb').write(b'exported')
            result = {'kind': 'export', 'output': arguments['request']['output'], 'duration': 3.0}
        jobs[job_id] = {'version': 1, 'job_id': job_id, 'state': state, 'revision': 2, 'changed': True,
                        'result': result, 'extra_field_added_later': True,
                        'error': {'code': 'fake'} if state == 'failed' else None}
        reply(message['id'], {'structuredContent': {'version': 1, 'job_id': job_id, 'state': 'running',
                                                    'revision': 1}, 'isError': False})
if mode == 'no_exit':
    time.sleep(60)
'''


class McpProbeTests(unittest.TestCase):
    def run_fake(self, mode, timeout=20):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            server = folder / 'server.py'
            server.write_text(FAKE_MCP_SERVER, encoding='utf-8')
            record, output = folder / 'record.json', folder / 'out.mp4'
            reduced = probe.mcp_environment
            def environment():
                # A Windows Python needs SYSTEMROOT to start; Linux, where the probe runs, does not.
                return reduced() | ({'SYSTEMROOT': os.environ['SYSTEMROOT']} if os.name == 'nt' else {})
            with mock.patch.dict(os.environ, {'NOH_FFMPEG': '/not/for/the/server', 'HOME': str(folder)}), \
                    mock.patch.object(probe, 'mcp_environment', environment):
                result = probe.mcp_job([sys.executable, '-I', server, mode, record], folder / 'clip.mp4',
                                       folder / 'song.wav', output, timeout=timeout)
            seen = json.loads(record.read_text(encoding='utf-8'))
            return result, seen, output.read_bytes()

    def test_a_real_job_succeeds_in_the_client_environment(self):
        result, seen, exported = self.run_fake('ok')
        self.assertEqual(exported, b'exported')
        self.assertEqual(result['exit_code'], 0)
        self.assertEqual(result['tools'], ['noh_diagnose', 'noh_export', 'noh_inspect', 'noh_job', 'noh_preview'])
        self.assertEqual(seen['environment']['PATH'], '/usr/bin:/bin')
        self.assertFalse([name for name in seen['environment'] if name.startswith('NOH_')])
        self.assertLessEqual({'HOME', 'PATH', 'LANG'}, set(seen['environment']))
        self.assertEqual(Path(seen['cwd']), Path('/').resolve())

    def test_a_newer_server_with_more_tools_and_fields_still_passes(self):
        result, _, _ = self.run_fake('extra_tool')
        self.assertIn('noh_added_later', result['tools'])

    def test_a_missing_tool_a_failed_job_or_a_missing_output_fails(self):
        for mode, message in (('missing_tool', 'does not list noh_diagnose'), ('job_fails', 'job ended failed'),
                              ('old_protocol', 'answered protocol'), ('no_output', 'without its output file')):
            with self.subTest(mode=mode), self.assertRaisesRegex(ValueError, message):
                self.run_fake(mode)

    def test_a_silent_or_lingering_server_is_bounded_and_stopped(self):
        for mode, message in (('silent', 'did not answer initialize'), ('no_exit', 'did not stop')):
            started = time.monotonic()
            with self.subTest(mode=mode), self.assertRaisesRegex(TimeoutError, message):
                self.run_fake(mode, timeout=3)
            self.assertLess(time.monotonic() - started, 20)

    def test_a_probe_report_needs_gui_transcription_and_mcp(self):
        report = {'scope': 'host-runtime-smoke', 'gui': {'a': 1}, 'transcription': {'cues': 1}, 'mcp': {'tools': []}}
        matrix.require_complete_probe(report)
        for part in ('gui', 'transcription', 'mcp'):
            with self.subTest(part=part), self.assertRaisesRegex(ValueError, 'omitted'):
                matrix.require_complete_probe({key: value for key, value in report.items() if key != part})
        with self.assertRaisesRegex(ValueError, 'omitted'):
            matrix.require_complete_probe(report | {'scope': 'loader-only'})


if __name__ == '__main__':
    unittest.main()
