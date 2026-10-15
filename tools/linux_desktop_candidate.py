#!/usr/bin/env python3
"""Bind native desktop observations to one verified CI candidate, without rebuilding."""
import argparse
import array
from datetime import datetime, timezone
import importlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import time
import wave
import zipfile

from delivery import ROOT, COMMIT, LIMIT, digest, inventory, read, write, zip_members
from linux_matrix import CHECKS, MATRIX, unpack_candidate


def case_inputs(name):
    row = next((r for r in read(MATRIX)['required_desktops'] if r['id'] == name), None)
    image = read(ROOT / 'assets/linux-desktop-images.json')['images'].get(name)
    if not row or not image or row['desktop'] not in ('GNOME', 'KDE'):
        raise ValueError('This provisioning adapter supports only configured GNOME/KDE cases')
    if (image['algorithm'] not in ('sha256', 'sha512') or not image['url'].startswith('https://')
            or not re.fullmatch('[0-9a-f]{' + str(64 if image['algorithm'] == 'sha256' else 128) + '}', image['digest'])):
        raise ValueError('Desktop image must have a pinned HTTPS URL and supported digest')
    return row, image


def acquire(args, destination):
    """Reuse the release consumer's provenance gates, before executing any payload."""
    source = importlib.import_module('delivery-release')
    run = source.api(args.repository, f'actions/runs/{args.run}')
    source.validate_run(run, args.repository, args.workflow, args.commit, args.attempt)
    comparison = source.api(args.repository, f'compare/{args.commit}...main')
    if comparison['status'] not in ('ahead', 'identical'):
        raise ValueError('Candidate source is not retained on trusted main')
    jobs = source.api(args.repository, f'actions/runs/{args.run}/attempts/{args.attempt}/jobs?per_page=100')
    source.validate_jobs(jobs, ['linux-x64'], run['path'])
    artifacts = source.validate_artifacts(
        source.api(args.repository, f'actions/runs/{args.run}/artifacts?per_page=100'),
        ['linux-x64'], f'rust-audit-{args.run}-{args.attempt}', f'linux-validation-{args.run}-{args.attempt}')
    artifact, = artifacts
    archive_path = destination / 'transport.zip'
    with archive_path.open('xb') as output:
        subprocess.run(['gh', 'api', f'repos/{args.repository}/actions/artifacts/{artifact["id"]}/zip'],
                       check=True, stdout=output, timeout=600)
    if 'sha256:' + digest(archive_path) != artifact['digest']:
        raise ValueError('Candidate transport differs from the immutable GitHub artifact digest')
    envelope = destination / 'envelope'
    envelope.mkdir()
    with zipfile.ZipFile(archive_path) as archive:
        zip_members(archive)
        if sum(m.file_size for m in archive.infolist()) > 5 * LIMIT:
            raise ValueError('Candidate transport expansion exceeds the release bound')
        archive.extractall(envelope)
    write(destination / 'source-run.json', run)
    write(destination / 'source-artifact.json', artifact)
    return envelope


def prepare(args):
    if not COMMIT.fullmatch(args.commit) or min(args.run, args.attempt, args.workflow) < 1:
        raise ValueError('Exact positive source run/attempt/workflow and commit are required')
    args.output.mkdir(parents=True, exist_ok=False)
    envelope = args.envelope.resolve() if args.envelope else acquire(args, args.output)
    payload = args.output / 'payload'
    payload.mkdir()
    record, archive_hash = unpack_candidate(envelope, payload)
    if (record['source_commit'] != args.commit or record['run_id'] != str(args.run)
            or record['run_attempt'] != str(args.attempt)):
        raise ValueError('Verified package identity differs from the requested CI source')
    identity = {'source_commit': args.commit, 'run_id': args.run, 'run_attempt': args.attempt,
                'candidate_sha256': archive_hash, 'delivery_sha256': digest(envelope / 'DELIVERY.json'),
                'matrix_sha256': digest(MATRIX), 'test_harness_commit': os.environ.get('GITHUB_SHA'),
                'bundle_files': record['bundle_files']}
    write(payload / 'CANDIDATE.json', identity)
    # Only source helpers and explicit test contracts cross into the guest, never credentials.
    (payload / 'tools').mkdir()
    for path in (ROOT / 'tools').glob('*.py'):
        shutil.copy2(path, payload / 'tools' / path.name)
    (payload / 'assets').mkdir()
    for name in ('linux-host.json', 'linux-validation-matrix.json', 'linux-desktop-images.json'):
        shutil.copy2(ROOT / 'assets' / name, payload / 'assets' / name)
    with tarfile.open(args.output / 'payload.tar', 'w') as archive:
        for path in sorted(payload.iterdir()):
            archive.add(path, arcname=path.name)
    write(args.output / 'CANDIDATE.json', identity)
    print(json.dumps({k: v for k, v in identity.items() if k != 'bundle_files'}))


def debian_provisioning(row, image):
    gdm_config = {'ubuntu': '/etc/gdm3/custom.conf', 'debian': '/etc/gdm3/daemon.conf'}[image['os_id']]
    packages = ['gnome-session', 'gdm3', 'pulseaudio-utils', 'alsa-utils', 'pciutils', 'python3',
                'ffmpeg', 'libmpv2', 'libasound2-plugins', 'libgl1-mesa-dri', 'libegl1',
                'libxkbcommon-x11-0', 'libxcursor1', 'libxrandr2', 'libxi6', 'libxinerama1',
                'libstdc++6', 'fonts-dejavu-core', 'binutils', 'espeak-ng', 'zenity', 'xdg-utils']
    packages += ['pipewire-audio'] if row['audio'] == 'pipewire' else ['pulseaudio', 'gnome-session-xsession', 'xserver-xorg']
    commands = []
    if image['os_id'] == 'ubuntu':
        commands.append(['sh', '-c', 'DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends linux-modules-extra-$(uname -r)'])
    files = [{'path': gdm_config, 'content':
              '[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=noh\n'
              f'WaylandEnable={str(row["display"] == "wayland").lower()}\n'
              f'DefaultSession={image["session"]}\n'}]
    return packages, commands, files, 'gdm3'


def rpm_arch_provisioning(row, image):
    """Use the same distribution codec providers as the sealed container matrix."""
    if image['os_id'] == 'fedora':
        commands = [
            ['dnf', 'install', '-y', 'https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-44.noarch.rpm'],
            ['dnf', 'install', '-y', '--allowerasing', 'ffmpeg', 'mpv-libs', 'python3',
             'gnome-session-wayland-session', 'gdm', 'kernel', 'kernel-modules', 'kernel-modules-extra',
             'pipewire', 'pipewire-pulseaudio', 'pipewire-alsa', 'wireplumber', 'pulseaudio-utils',
             'alsa-utils', 'mesa-dri-drivers', 'mesa-libEGL', 'mesa-libGL', 'libxkbcommon-x11',
             'libXcursor', 'libXrandr', 'libXi', 'libXinerama', 'libstdc++', 'dejavu-sans-fonts',
             'pciutils', 'binutils', 'espeak-ng', 'zenity', 'xdg-utils'],
        ]
        files = [{'path': '/etc/gdm/custom.conf', 'content':
                  '[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=noh\n'
                  f'WaylandEnable=true\nDefaultSession={image["session"]}\n'}]
        return [], commands, files, 'gdm'
    if image['os_id'] == 'opensuse-tumbleweed':
        commands = [
            ['zypper', '--non-interactive', 'install', '--no-recommends', 'kernel-default',
             'python3', 'ffmpeg-8', 'libmpv2', 'plasma6-desktop', 'plasma6-session-x11', 'sddm',
             'xorg-x11-server', 'pipewire', 'pipewire-pulseaudio', 'pipewire-alsa', 'wireplumber',
             'pulseaudio-utils', 'alsa-utils', 'Mesa-dri', 'Mesa-libEGL1', 'libglvnd', 'libxkbcommon-x11-0',
             'libXcursor1', 'libXrandr2', 'libXi6', 'libXinerama1', 'dejavu-fonts',
             'pciutils', 'binutils', 'espeak-ng', 'zenity', 'xdg-utils'],
            ['zypper', '--non-interactive', 'addrepo', '--refresh', '--priority', '90',
             'https://ftp.gwdg.de/pub/linux/misc/packman/suse/openSUSE_Tumbleweed/Essentials/', 'packman-essentials'],
            ['zypper', '--non-interactive', '--gpg-auto-import-keys', 'refresh', 'packman-essentials'],
            ['zypper', '--non-interactive', 'dist-upgrade', '--from', 'packman-essentials', '--allow-vendor-change'],
        ]
    elif image['os_id'] == 'arch':
        commands = [['pacman', '-Syu', '--noconfirm', '--needed', 'linux', 'ffmpeg', 'mpv', 'python',
                     'plasma-desktop', 'plasma-workspace', 'sddm', 'xorg-xwayland',
                     'pipewire', 'pipewire-audio', 'pipewire-pulse', 'pipewire-alsa', 'wireplumber',
                     'libpulse', 'alsa-utils', 'mesa', 'libglvnd', 'libxkbcommon-x11',
                     'libxcursor', 'libxrandr', 'libxi', 'libxinerama', 'ttf-dejavu',
                     'pciutils', 'binutils', 'espeak-ng', 'zenity', 'xdg-utils']]
    else:
        raise ValueError('Unsupported desktop distribution')
    files = [{'path': '/etc/sddm.conf.d/noh-test.conf', 'content':
              f'[Autologin]\nUser=noh\nSession={image["session"]}\nRelogin=false\n'}]
    return [], commands, files, 'sddm'


def cloud_config(args):
    row, image = case_inputs(args.case)
    debian_family = image['os_id'] in ('ubuntu', 'debian')
    packages, commands, files, manager = (debian_provisioning(row, image) if debian_family
                                        else rpm_arch_provisioning(row, image))
    if not image.get('reboot_after_provisioning'):
        commands += [['modprobe', 'virtio_gpu'], ['modprobe', 'snd_hda_intel']]
    if row['desktop'] == 'GNOME':
        commands += [['install', '-d', '-o', 'noh', '-g', 'noh', '/home/noh/.config'],
                     ['sh', '-c', 'touch /home/noh/.config/gnome-initial-setup-done; chown noh:noh /home/noh/.config/gnome-initial-setup-done']]
    if not debian_family:
        commands += [['systemctl', 'enable', manager]]
    commands += [['systemctl', 'set-default', 'graphical.target']]
    if not image.get('reboot_after_provisioning'):
        commands += [['systemctl', 'restart', manager]]
    user = {'name': 'noh', 'uid': 1000, 'shell': '/bin/bash',
            'sudo': 'ALL=(ALL) NOPASSWD:ALL', 'lock_passwd': True,
            'ssh_authorized_keys': [args.public_key.read_text().strip()]}
    if debian_family:
        user['groups'] = ['adm', 'sudo', 'audio', 'video', 'render']
    # Other images need no distro-specific supplementary groups: the active
    # logind seat grants device ACLs to the same user running the application.
    config = {'hostname': 'noh-desktop-test',
              'users': [user],
              'ssh_pwauth': False, 'package_update': True, 'packages': packages,
              'write_files': files, 'runcmd': commands}
    args.output.write_text('#cloud-config\n' + json.dumps(config, indent=2) + '\n')
    write(args.output.with_suffix('.image.json'), image)


def command(arguments, timeout=30):
    return subprocess.run(arguments, check=True, text=True, capture_output=True, timeout=timeout).stdout


def desktop_environment(manager_text, shell_bytes):
    """Use the actual desktop process when a legacy Xorg session omits manager imports."""
    names = ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_CURRENT_DESKTOP')
    manager = dict(line.split('=', 1) for line in manager_text.splitlines() if '=' in line)
    shell = dict(item.decode().split('=', 1) for item in shell_bytes.split(b'\0') if b'=' in item)
    return {name: shell.get(name, manager.get(name)) for name in names if name in shell or name in manager}


def verify_bundle(root, identity):
    expected = identity['bundle_files']
    for name, spec in expected.items():
        path = root / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size != spec['size'] or digest(path) != spec['sha256']:
            raise ValueError(f'Candidate file changed in the guest: {name}')
    if inventory(root / 'NOH.AppDir') != {k.removeprefix('NOH.AppDir/'): v for k, v in expected.items() if k.startswith('NOH.AppDir/')}:
        raise ValueError('Candidate AppDir contains missing or additional files')


def guest(args):
    row, image = case_inputs(args.case)
    args.output.mkdir(parents=True, exist_ok=False)
    identity = read(ROOT / 'CANDIDATE.json')
    verify_bundle(ROOT, identity)
    actual_os = platform.freedesktop_os_release()
    if (actual_os.get('ID') != image['os_id'] or
            (image['version_id'] is not None and actual_os.get('VERSION_ID') != image['version_id'])):
        raise ValueError('Guest OS differs from the selected desktop case')
    if command(['systemd-detect-virt']).strip() != 'kvm':
        raise ValueError('This adapter requires a KVM guest, not a container or nested compositor')
    session = None
    login_service, shell_name = ('gdm-autologin', 'gnome-shell') if row['desktop'] == 'GNOME' else ('sddm-autologin', 'plasmashell')
    for _ in range(24):
        for line in command(['loginctl', 'list-sessions', '--no-legend']).splitlines():
            values = command(['loginctl', 'show-session', line.split()[0]])
            data = dict(item.split('=', 1) for item in values.splitlines() if '=' in item)
            if (data.get('Name') == 'noh' and data.get('Type') == row['display'] and
                    data.get('Seat') == 'seat0' and data.get('Active') == 'yes' and
                    data.get('Service') == login_service and data.get('Remote') == 'no'):
                session = data
                break
        if session:
            break
        time.sleep(5)
    if not session:
        raise ValueError('The required real display-manager desktop session did not start')
    shell_pid = command(['pgrep', '-x', '-u', str(os.getuid()), shell_name]).strip()
    if not re.fullmatch('[1-9][0-9]*', shell_pid):
        raise ValueError('A unique desktop shell owned by the desktop user is required')
    os.environ.update(XDG_RUNTIME_DIR=f'/run/user/{os.getuid()}',
                      DBUS_SESSION_BUS_ADDRESS=f'unix:path=/run/user/{os.getuid()}/bus')
    # A compositor with elevated scheduling capabilities may restrict procfs reads.
    shell_environment = subprocess.run(['sudo', 'cat', f'/proc/{shell_pid}/environ'], check=True,
                                       capture_output=True, timeout=30).stdout
    observed_environment = desktop_environment(command(['systemctl', '--user', 'show-environment']), shell_environment)
    write(args.output / 'desktop-environment.json', {'session': session, 'shell': shell_name, 'shell_pid': int(shell_pid),
                                                    'observed_environment': observed_environment})
    os.environ.update(observed_environment)
    os.environ['XDG_SESSION_TYPE'] = row['display']
    if row['desktop'] not in os.environ.get('XDG_CURRENT_DESKTOP', '').split(':'):
        raise ValueError('Selected desktop is absent from the actual desktop manager environment')
    command(['systemctl', '--user', 'daemon-reload'])
    services = ['pipewire', 'pipewire-pulse', 'wireplumber'] if row['audio'] == 'pipewire' else ['pulseaudio']
    command(['systemctl', '--user', 'start', *services])
    command(['systemctl', '--user', 'is-active', *services])
    sink = None
    for _ in range(40):
        sinks = json.loads(command(['pactl', '--format=json', 'list', 'sinks']))
        sink = next((s for s in sinks if s['name'].startswith('alsa_output.') and s.get('properties', {}).get('device.bus') == 'pci'), None)
        if sink:
            break
        time.sleep(.25)
    if not sink:
        raise ValueError('No real virtual-PCI audio sink appeared; a null sink cannot qualify this case')
    command(['pactl', 'set-default-sink', sink['name']])
    audio_info = command(['pactl', 'info'])
    if ('PipeWire' in audio_info) != (row['audio'] == 'pipewire'):
        raise ValueError('The observed sound server differs from the approved case')
    write(args.output / 'session.json', {'session': session, 'manager': {k: os.environ.get(k) for k in
          ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE')},
          'sinks': sinks, 'default_sink': sink['name'], 'audio_info': audio_info, 'os_release': actual_os})
    (args.output / 'hardware.txt').write_text(command(['lspci', '-nnk']) + command(['aplay', '-l']))
    speech = args.output / 'synthetic-speech.wav'
    command(['espeak-ng', '-v', 'en-us', '-s', '140', '-w', str(speech),
             'This is a synthetic speech sample. The video and audio tools work together.'])
    # Remove external routes after provisioning; SSH remains on the directly connected virtual LAN.
    for family in ('-4', '-6'):
        if command(['ip', family, 'route', 'show', 'default']).strip():
            command(['sudo', 'ip', family, 'route', 'del', 'default'])
        if command(['ip', family, 'route', 'show', 'default']).strip():
            raise ValueError('Guest still has an external default route during offline tests')
    (args.output / 'runtime-routes.txt').write_text(command(['ip', '-4', 'route']) + command(['ip', '-6', 'route']))
    with (args.output / 'runtime.log').open('w') as log:
        subprocess.run(['python3', str(ROOT / 'tools/probe-linux-host.py'), '--appdir', str(ROOT / 'NOH.AppDir'),
                        '--graphics', 'wayland' if row['display'] == 'wayland' else 'surfaceless', '--gui',
                        '--speech-fixture', str(speech), '--output', str(args.output / 'result.json')],
                       check=True, stdout=log, stderr=subprocess.STDOUT, timeout=600)
    report = read(args.output / 'result.json')
    expected_elf = {k.removeprefix('NOH.AppDir/'): v['sha256'] for k, v in identity['bundle_files'].items()
                    if k in ('NOH.AppDir/usr/lib/noh/noh-app', 'NOH.AppDir/usr/lib/noh/bin/noh-cli',
                             'NOH.AppDir/usr/lib/noh/bin/noh-mcp', 'NOH.AppDir/usr/lib/noh/bin/speech/whisper-cli')}
    if (report['elf_sha256'] != expected_elf or not report.get('gui') or not report.get('transcription')
            or report['scope'] != 'host-runtime-smoke'):
        raise ValueError('Native runtime evidence lacks the exact candidate or required checks')
    # Leave a normal, uninjected application window for independent QMP scanout inspection.
    with (args.output / 'startup.log').open('w') as log:
        app = subprocess.Popen([str(ROOT / 'NOH.AppDir/AppRun')], stdin=subprocess.DEVNULL,
                               stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    time.sleep(5)
    if app.poll() is not None:
        raise ValueError('Normal desktop application startup terminated before screen observation')
    verify_bundle(ROOT, identity)
    write(args.output / 'case.json', {'status': 'awaiting-review', 'scope': 'native-desktop-observation',
          'environment': {**row, 'machine_kind': 'virtual-machine', 'os_image_identity': image,
                          'gpu': 'QEMU virtio GPU; see hardware.txt and result.json renderer',
                          'audio_device': sink['name'], 'tested_at': datetime.now(timezone.utc).isoformat()},
          'elf_sha256': expected_elf, 'checks': {name: 'passed' for name in CHECKS},
          'candidate': {k: v for k, v in identity.items() if k != 'bundle_files'},
          'normal_startup_pid': app.pid,
          'limits': ['GUI import/preview uses the existing explicit capture harness; normal startup is separately observed.',
                     'QMP screen and recorded application audio still require independent inspection.',
                     'No physical-device or human-listening qualification; full seven-case matrix remains required.']})
    print('Native runtime checks passed; screen/audio review and full matrix coverage remain pending.')


def audio_report(args):
    with wave.open(str(args.input), 'rb') as stream:
        rate, channels, width, frames = stream.getframerate(), stream.getnchannels(), stream.getsampwidth(), stream.getnframes()
        if width != 2 or channels != 2 or rate not in (44100, 48000):
            raise ValueError('Unexpected QEMU audio format')
        samples = array.array('h', stream.readframes(frames))
    mono = samples[::channels]
    # Search quarter-second windows for the preview's known 440 Hz signal, not just any startup noise.
    size = rate // 4
    cosine = [math.cos(2 * math.pi * 440 * n / rate) for n in range(size)]
    sine = [math.sin(2 * math.pi * 440 * n / rate) for n in range(size)]
    best = 0
    for offset in range(0, len(mono) - size + 1, size):
        section = mono[offset:offset + size]
        power = sum(x * x for x in section)
        if power > size * 100 ** 2:
            c = sum(x * y for x, y in zip(section, cosine))
            s = sum(x * y for x, y in zip(section, sine))
            best = max(best, 2 * (c * c + s * s) / (size * power))
    value = {'sha256': digest(args.input), 'sample_rate': rate, 'frames': frames, 'channels': channels,
             'peak': max(map(abs, samples), default=0),
             'rms': math.sqrt(sum(x*x for x in samples) / len(samples)) if samples else 0,
             'best_quarter_second_440hz_power_fraction': best}
    write(args.output, value)
    if best < .8:
        raise ValueError('Recorded virtual audio does not establish the application preview tone')
    print(json.dumps(value))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('prepare')
    p.add_argument('--repository', default='Awalgawe/noh')
    p.add_argument('--workflow', type=int, required=True)
    p.add_argument('--run', type=int, required=True)
    p.add_argument('--attempt', type=int, required=True)
    p.add_argument('--commit', required=True)
    p.add_argument('--envelope', type=Path, help='Use a previously verified local envelope; CI must acquire authenticated artifacts')
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=prepare)
    p = sub.add_parser('cloud-config')
    p.add_argument('--case', required=True)
    p.add_argument('--public-key', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=cloud_config)
    p = sub.add_parser('guest')
    p.add_argument('--case', required=True)
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=guest)
    p = sub.add_parser('audio-report')
    p.add_argument('--input', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=audio_report)
    args = parser.parse_args()
    args.fn(args)


if __name__ == '__main__':
    main()
