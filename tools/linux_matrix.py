#!/usr/bin/env python3
"""Test one sealed Linux candidate in pinned, disposable distribution images."""
import argparse
from contextlib import ExitStack
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile

from delivery import ROOT, digest, read, relative, verify_envelope, write

MATRIX = ROOT / 'assets/linux-validation-matrix.json'
CHECKS = {'installation', 'startup', 'import', 'video_audio_preview', 'export', 'transcription'}


def validate_result(value, record, delivery_hash, desktop=False):
    matrix = read(MATRIX)
    expected = {row['id']: row for row in (matrix['required_desktops'] if desktop else cases())}
    if (value.get('schema_version') != 1 or value.get('status') != 'passed'
            or value.get('delivery_sha256') != delivery_hash
            or value.get('matrix_sha256') != digest(MATRIX)
            or set(value.get('cases', {})) != set(expected)):
        raise ValueError('Linux qualification lacks exact candidate/matrix coverage')
    names = ('usr/lib/noh/noh-app', 'usr/lib/noh/bin/noh-cli',
             'usr/lib/noh/bin/noh-mcp', 'usr/lib/noh/bin/speech/whisper-cli')
    hashes = {name: record['bundle_files']['NOH.AppDir/' + name]['sha256'] for name in names}
    for name, row in value['cases'].items():
        if (row.get('status') != 'passed' or row.get('elf_sha256') != hashes
                or row.get('checks') != {check: 'passed' for check in CHECKS}):
            raise ValueError(f'Linux case has missing checks or different executables: {name}')
        environment = row.get('environment', {})
        if any(environment.get(key) != val for key, val in expected[name].items()):
            raise ValueError(f'Linux case environment differs from the agreed matrix: {name}')
        if desktop:
            if (value.get('scope') != 'desktop-integration' or environment.get('machine_kind') not in ('physical', 'virtual-machine')
                    or not all(environment.get(key) for key in ('os_image_identity', 'gpu', 'audio_device', 'tested_at', 'reviewer'))
                    or not row.get('evidence')):
                raise ValueError(f'Linux desktop evidence is incomplete: {name}')
            for evidence in row['evidence']:
                path = ROOT / str(relative(evidence['path']))
                if not path.is_file() or digest(path) != evidence.get('sha256'):
                    raise ValueError(f'Missing or changed Linux desktop observations: {name}')
        elif (value.get('scope') != 'container-software-rendering'
              or not re.fullmatch(r'(?:sha256:)?[a-f0-9]{64}', environment.get('prepared_image_id', ''))):
            raise ValueError(f'Linux container image identity is missing: {name}')


def cases():
    value = read(MATRIX)
    rows = value['containers']
    if value['schema_version'] != 1 or len({row['id'] for row in rows}) != len(rows):
        raise ValueError('Invalid Linux matrix identity')
    for row in rows:
        if (not re.fullmatch(r'[a-z0-9.-]+', row['id'])
                or not re.fullmatch(r'[a-z0-9./-]+@sha256:[a-f0-9]{64}', row['image'])
                or row['display'] not in ('x11', 'wayland')
                or row['audio'] not in ('pipewire', 'pulseaudio')):
            raise ValueError('Linux matrix needs pinned images and known display/audio providers')
    return rows


def prepare(engine, row, output):
    folder = output / row['id']
    folder.mkdir()
    recipe = (ROOT / 'tools/linux-test-install.sh').read_bytes().replace(b'\r\n', b'\n')
    key = hashlib.sha256(json.dumps(row, sort_keys=True).encode() + recipe).hexdigest()
    tag = f'localhost/noh-linux-test:{row["id"]}-{key[:16]}'
    inspect = subprocess.run([engine, 'image', 'inspect', tag], capture_output=True, timeout=30)
    if inspect.returncode:
        with tempfile.TemporaryDirectory(prefix='noh-linux-image-') as tmp:
            context = Path(tmp)
            (context / 'install.sh').write_bytes(recipe)
            (context / 'Dockerfile').write_text(
                f'FROM {row["image"]}\nCOPY install.sh /opt/noh/install.sh\n'
                f'RUN NOH_CONTAINER_BUILD=1 sh /opt/noh/install.sh {row["id"]}\n'
                f'LABEL noh.test.inputs="{key}"\n', encoding='utf-8')
            with (folder / 'install.log').open('wb') as log:
                subprocess.run([engine, 'build', '--tag', tag, str(context)], check=True,
                               stdout=log, stderr=subprocess.STDOUT, timeout=1200)
        inspect = subprocess.run([engine, 'image', 'inspect', tag], check=True,
                                 capture_output=True, timeout=30)
    image = json.loads(inspect.stdout)[0]
    if image['Config'].get('Labels', {}).get('noh.test.inputs') != key:
        raise ValueError('Cached Linux test image has different setup inputs')
    image_id = image.get('Id', image.get('ID'))
    name = f'noh-package-inventory-{key[:16]}'
    try:
        with (folder / 'packages.txt').open('wb') as log:
            subprocess.run([engine, 'run', '--rm', '--name', name, '--network=none', image_id,
                            'cat', '/opt/noh/packages.txt'], check=True, stdout=log, timeout=120)
    finally:
        subprocess.run([engine, 'rm', '--force', name], capture_output=True, timeout=30)
    result = {**row, 'prepared_image_id': image_id, 'prepared_image_created': image['Created'],
              'recipe_sha256': key, 'packages_sha256': digest(folder / 'packages.txt'),
              'prepared_at': datetime.now(timezone.utc).isoformat(),
              'package_repositories': 'Distribution repositories at image preparation time; exact installed versions are recorded.'}
    write(folder / 'environment.json', result)
    return result


def unpack_candidate(envelope, destination):
    record = verify_envelope(envelope)
    if record['platform'] != 'linux-x64':
        raise ValueError('Linux matrix requires a Linux delivery')
    archive = envelope / f'NOH-{record["version"]}-linux-x64.zip'
    with zipfile.ZipFile(archive) as zipped:
        # verify_envelope already rejects traversal, links, collisions and hashes.
        zipped.extractall(destination)
        for member in zipped.infolist():
            if not member.is_dir():
                (destination / member.filename).chmod((member.external_attr >> 16) & 0o777)
    return record, digest(archive)


def run_case(engine, row, output, bundle, speech):
    folder = output / row['id']
    evidence = folder / 'probe'
    evidence.mkdir(mode=0o777)
    evidence.chmod(0o777)
    name = f'noh-matrix-{row["id"]}-{hashlib.sha256(str(output).encode()).hexdigest()[:8]}'
    command = [engine, 'run', '--name', name, '--rm', '--network=none', '--user', '1000:1000',
               '--shm-size=256m', '-v', f'{ROOT}:/work:ro', '-v', f'{bundle}:/candidate:ro',
               '-v', f'{speech}:/speech.wav:ro', '-v', f'{evidence}:/evidence:rw',
               row['prepared_image_id'], 'dbus-run-session', '--', 'python3',
               '/work/tools/linux_matrix_session.py', '--display', row['display'], '--audio', row['audio']]
    try:
        with (folder / 'runtime.log').open('wb') as log:
            subprocess.run(command, check=True, stdout=log, stderr=subprocess.STDOUT, timeout=600)
    finally:
        # Only this uniquely named disposable container is eligible for cleanup.
        subprocess.run([engine, 'rm', '--force', name], capture_output=True, timeout=30)
    report = read(evidence / 'result.json')
    if not report.get('gui') or not report.get('transcription') or report.get('scope') != 'host-runtime-smoke':
        raise ValueError('Linux matrix probe omitted GUI or transcription checks')
    return {'status': 'passed', 'probe_sha256': digest(evidence / 'result.json'),
            'elf_sha256': report['elf_sha256'], 'checks': {check: 'passed' for check in CHECKS}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', choices=('docker', 'podman'), default='docker')
    parser.add_argument('--envelope', type=Path)
    parser.add_argument('--speech-fixture', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--prepare-only', action='store_true')
    parser.add_argument('--cases', nargs='+')
    args = parser.parse_args()
    selected = cases()
    if args.cases:
        if not set(args.cases).issubset({r['id'] for r in selected}):
            parser.error('Unknown matrix case')
        selected = [r for r in selected if r['id'] in args.cases]
    if not args.prepare_only and (not args.envelope or not args.speech_fixture):
        parser.error('Runtime checks require a sealed envelope and synthetic speech fixture')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {'schema_version': 1, 'scope': 'container-software-rendering',
              'matrix_sha256': digest(MATRIX), 'cases': {}, 'status': 'incomplete',
              'desktop_qualification': False, 'physical_hardware_qualification': False}
    with ExitStack() as stack:
        if not args.prepare_only:
            # Reject malformed candidates before spending time on image preparation.
            bundle = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix='noh-candidate-')))
            bundle.chmod(0o755)
            record, archive_hash = unpack_candidate(args.envelope.resolve(), bundle)
            result.update(delivery_sha256=digest(args.envelope / 'DELIVERY.json'),
                          candidate_sha256=archive_hash, source_commit=record['source_commit'],
                          run_id=record['run_id'], run_attempt=record['run_attempt'],
                          speech_fixture_sha256=digest(args.speech_fixture))
        with ThreadPoolExecutor(max_workers=2) as workers:
            futures = {row['id']: workers.submit(prepare, args.engine, row, output) for row in selected}
            for case_id, future in futures.items():
                try:
                    result['cases'][case_id] = {'environment': future.result(), 'status': 'prepared'}
                except (ValueError, OSError, subprocess.SubprocessError) as error:
                    result['cases'][case_id] = {'status': 'failed', 'error': str(error)}
                    print(case_id, str(error), flush=True)
                print(case_id, result['cases'][case_id]['status'], flush=True)
        if not args.prepare_only:
            for row in result['cases'].values():
                if row['status'] != 'prepared':
                    continue
                try:
                    row.update(run_case(args.engine, row['environment'], output, bundle,
                                        args.speech_fixture.resolve()))
                except (ValueError, OSError, subprocess.SubprocessError) as error:
                    row.update(status='failed', error=str(error))
                print(row['environment']['id'], row['status'], flush=True)
    expected = 'prepared' if args.prepare_only else 'passed'
    passed = all(row['status'] == expected for row in result['cases'].values())
    result['status'] = expected if passed else 'failed'
    write(output / 'MATRIX.json', result)
    if not passed:
        raise SystemExit('Linux matrix failed; inspect the per-case logs')


if __name__ == '__main__':
    main()
