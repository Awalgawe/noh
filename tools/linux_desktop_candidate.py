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
import secrets
import shlex
import shutil
import subprocess
import tarfile
import time
import wave
import zipfile

from delivery import ROOT, COMMIT, LIMIT, digest, inventory, read, write, zip_members
from linux_matrix import CHECKS, MATRIX, unpack_candidate

APT_RUNTIME_PACKAGES = ['pulseaudio-utils', 'alsa-utils', 'pciutils', 'python3',
                        'ffmpeg', 'libmpv2', 'libasound2-plugins', 'libgl1-mesa-dri', 'libegl1',
                        'libxkbcommon-x11-0', 'libxcursor1', 'libxrandr2', 'libxi6', 'libxinerama1',
                        'libstdc++6', 'fonts-dejavu-core', 'binutils', 'espeak-ng', 'zenity', 'xdg-utils', 'mpv']


def case_inputs(name):
    row = next((r for r in read(MATRIX)['required_desktops'] if r['id'] == name), None)
    image = read(ROOT / 'assets/linux-desktop-images.json')['images'].get(name)
    if not row or not image or row['desktop'] not in ('GNOME', 'KDE', 'Cinnamon'):
        raise ValueError('This provisioning adapter supports only configured desktop cases')
    if (image['algorithm'] not in ('sha256', 'sha512') or not image['url'].startswith('https://')
            or not re.fullmatch('[0-9a-f]{' + str(64 if image['algorithm'] == 'sha256' else 128) + '}', image['digest'])):
        raise ValueError('Desktop image must have a pinned HTTPS URL and supported digest')
    if image.get('format', 'qcow2') not in ('qcow2', 'iso'):
        raise ValueError('Unsupported desktop image format')
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
    if not args.envelope:
        # These are owned download/unpack intermediates beneath the new output
        # directory. Keep the verified payload and provenance; never remove a
        # caller-supplied local envelope. Mint's installer needs the disk space.
        shutil.rmtree(payload)
        shutil.rmtree(envelope)
        (args.output / 'transport.zip').unlink()
    print(json.dumps({k: v for k, v in identity.items() if k != 'bundle_files'}))


def debian_provisioning(row, image):
    gdm_config = {'ubuntu': '/etc/gdm3/custom.conf', 'debian': '/etc/gdm3/daemon.conf'}[image['os_id']]
    packages = ['gnome-session', 'gdm3', *APT_RUNTIME_PACKAGES]
    packages += ['pipewire-audio'] if row['audio'] == 'pipewire' else ['pulseaudio', 'gnome-session-xsession', 'xserver-xorg']
    commands = []
    if image['os_id'] == 'ubuntu':
        # Let the distro select its complete image/modules; Ubuntu 26.04 no
        # longer has the older linux-modules-extra package split. Reboot next.
        packages.append('linux-image-generic')
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
            ['dnf', 'install', '-y', '--allowerasing', 'ffmpeg', 'mpv-libs', 'mpv', 'python3',
             'gnome-session-wayland-session', 'gdm', 'nautilus', 'xdg-desktop-portal-gnome',
             'kernel', 'kernel-modules', 'kernel-modules-extra',
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
            ['zypper', '--non-interactive', 'install', '--no-recommends', '--', 'kernel-default', '-kernel-default-base',
             'python3', 'ffmpeg-8', 'libmpv2', 'mpv', 'plasma6-desktop', 'plasma6-session-x11', 'sddm',
             'xorg-x11-server', 'pipewire', 'pipewire-pulseaudio', 'pipewire-alsa', 'wireplumber',
             'pulseaudio-utils', 'alsa-utils', 'Mesa-dri', 'Mesa-libEGL1', 'libglvnd', 'libxkbcommon-x11-0',
             'libXcursor1', 'libXrandr2', 'libXi6', 'libXinerama1', 'dejavu-fonts',
             'pciutils', 'binutils', 'espeak-ng', 'zenity', 'xdg-utils'],
            ['zypper', '--non-interactive', 'addrepo', '--refresh', '--priority', '90',
             'https://ftp.gwdg.de/pub/linux/misc/packman/suse/openSUSE_Tumbleweed/Essentials/', 'packman-essentials'],
            ['zypper', '--non-interactive', '--gpg-auto-import-keys', 'refresh', 'packman-essentials'],
            ['zypper', '--non-interactive', 'dist-upgrade', '--from', 'packman-essentials', '--allow-vendor-change'],
            # SUSE's SDDM patch replaces Autologin.User with this sysconfig key,
            # even when it is empty. Use its distribution-supported preference.
            ['sed', '-i', 's/^DISPLAYMANAGER_AUTOLOGIN=.*/DISPLAYMANAGER_AUTOLOGIN="noh"/', '/etc/sysconfig/displaymanager'],
            ['grep', '-qx', 'DISPLAYMANAGER_AUTOLOGIN="noh"', '/etc/sysconfig/displaymanager'],
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
    debian_family = image['os_id'] in ('ubuntu', 'debian', 'linuxmint')
    if image['os_id'] == 'linuxmint':
        packages = ['lightdm', 'cinnamon', 'pipewire-audio', *APT_RUNTIME_PACKAGES]
        commands = []
        files = [{'path': '/etc/lightdm/lightdm.conf.d/90-noh-test.conf', 'content':
                  '[Seat:*]\nautologin-user=noh\nautologin-user-timeout=0\n'
                  'autologin-session=cinnamon\nuser-session=cinnamon\n'}]
        manager = 'lightdm'
    else:
        packages, commands, files, manager = (debian_provisioning(row, image) if debian_family
                                            else rpm_arch_provisioning(row, image))
    if not image.get('reboot_after_provisioning'):
        commands += [['modprobe', 'virtio_gpu'], ['modprobe', 'snd_hda_intel']]
    if row['desktop'] == 'GNOME':
        commands += [['install', '-d', '-o', 'noh', '-g', 'noh', '/home/noh/.config'],
                     ['sh', '-c', 'touch /home/noh/.config/gnome-initial-setup-done; chown noh:noh /home/noh/.config/gnome-initial-setup-done']]
    if not debian_family:
        # The SUSE base selects display-manager-legacy. Replace its alias only
        # when selecting our explicit SDDM manager on this disposable guest.
        commands += [['systemctl', 'enable', *(['--force'] if image['os_id'] == 'opensuse-tumbleweed' else []), manager]]
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
              'ssh_pwauth': False, 'package_update': True,
              'write_files': files, 'runcmd': ['set -eu', *commands]}
    if packages:
        config['packages'] = packages
    if image['os_id'] == 'fedora':
        # The cloud image reports a hostname-setting failure during init-local.
        # No test relies on a custom name: retain the image's hostname instead.
        del config['hostname']
        config['preserve_hostname'] = True
    args.output.write_text('#cloud-config\n' + json.dumps(config, indent=2) + '\n')
    write(args.output.with_suffix('.image.json'), image)


def mint_preseed(args):
    """Use Mint's Ubiquity installer on the disposable guest's only disk."""
    _, image = case_inputs('mint-22.3')
    if image.get('installation_method') != 'ubiquity-preseed':
        raise ValueError('Mint installer identity is not configured')
    public_key = args.public_key.read_text().strip()
    if not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/=]+(?: [^\r\n]+)?', public_key):
        raise ValueError('Mint bootstrap requires the generated Ed25519 public key')
    public_key = ' '.join(public_key.split()[:2])  # The SSH comment is not authentication data.
    # The network used to download the preseed is inherited as an external
    # connection without working DNS. Activate ordinary NetworkManager DHCP
    # once the live desktop is running, before the installer fetches packages.
    # enp0s4 is the sole NIC in this adapter's fixed Q35 device layout.
    network = '; '.join([
        'exec >/dev/ttyS0 2>&1', 'set -e',
        'export PAGER=cat',
        "trap 'echo NOH_INSTALL_NETWORK_EXIT=$?' 0",
        'echo NOH_INSTALL_NETWORK_BEGIN',
        'nmcli device show enp0s4',
        'nmcli connection add type ethernet ifname enp0s4 con-name noh-installer ipv4.method auto ipv6.method disabled',
        'nmcli --wait 60 connection up id noh-installer',
        'nmcli device show enp0s4',
        'getent ahostsv4 packages.linuxmint.com',
        'getent ahostsv4 archive.ubuntu.com',
        'echo NOH_INSTALL_NETWORK_COMPLETE',
    ])
    # Debconf's File database decodes literal backslash-n; Ubiquity's GET then
    # truncates at that newline. Keep every preseed command free of that escape.
    # Ubiquity ignores the success hook's exit status. Retain its actual status
    # on serial, and make SSH bootstrap independent of first-boot cloud-init so
    # that its failure can be diagnosed through the existing transport.
    in_target = 'in-target --pass-stdout env DEBIAN_FRONTEND=noninteractive'
    finalize = '; '.join([
        'exec >/dev/ttyS0 2>&1', 'set -ex',
        "trap 'echo NOH_INSTALL_FINALIZE_EXIT=$?' 0",
        'echo NOH_INSTALL_FINALIZE_BEGIN',
        f'{in_target} apt-get update',
        f'{in_target} apt-get install -y --no-install-recommends openssh-server cloud-init',
        'rm -f /target/etc/cloud/cloud-init.disabled',
        'install -d -m 700 /target/home/noh/.ssh',
        "printf '%s' " + shlex.quote(public_key) + ' > /target/home/noh/.ssh/authorized_keys',
        'echo >> /target/home/noh/.ssh/authorized_keys',
        'chmod 600 /target/home/noh/.ssh/authorized_keys',
        f'{in_target} chown noh:noh /home/noh/.ssh /home/noh/.ssh/authorized_keys',
        "echo 'PasswordAuthentication no' > /target/etc/ssh/sshd_config.d/90-noh-test.conf",
        f'{in_target} systemctl enable ssh',
        'install -d /target/etc/default/grub.d',
        "echo 'GRUB_CMDLINE_LINUX=\"console=tty0 console=ttyS0,115200n8\"' > /target/etc/default/grub.d/90-noh-probe.cfg",
        f'{in_target} update-grub',
        'touch /target/var/lib/noh-unattended-install-complete',
        'echo NOH_INSTALL_FINALIZE_COMPLETE',
    ])
    # The random test password is never printed or used for SSH. Installer
    # bootstrap and NoCloud use the same sole public key and disable SSH passwords.
    password_hash = subprocess.run(['openssl', 'passwd', '-6', '-stdin'], check=True,
                                   input=secrets.token_urlsafe(32) + '\n', text=True,
                                   capture_output=True).stdout.strip()
    args.output.write_text(f'''d-i debian-installer/locale string en_US.UTF-8
ubiquity localechooser/supported-locales multiselect en_US.UTF-8
ubiquity languagechooser/language-name select English
ubiquity countrychooser/shortlist select US
d-i keyboard-configuration/layoutcode string us
d-i keyboard-configuration/modelcode string pc105
d-i console-setup/ask_detect boolean false
d-i netcfg/get_hostname string noh-mint-test
d-i netcfg/get_domain string local
d-i time/zone string UTC
d-i clock-setup/utc boolean true
d-i partman/early_command string {network}
d-i partman-auto/disk string /dev/vda
d-i partman-auto/method string regular
d-i partman-auto/choose_recipe select atomic
d-i partman-lvm/device_remove_lvm boolean true
d-i partman-md/device_remove_md boolean true
d-i partman-partitioning/confirm_write_new_label boolean true
d-i partman/choose_partition select finish
d-i partman/confirm boolean true
d-i partman/confirm_nooverwrite boolean true
d-i passwd/user-fullname string NOH Test
d-i passwd/username string noh
d-i passwd/user-password-crypted password {password_hash}
d-i passwd/auto-login boolean true
d-i user-setup/encrypt-home boolean false
d-i grub-installer/only_debian boolean true
d-i grub-installer/bootdev string /dev/vda
ubiquity ubiquity/download_updates boolean false
ubiquity ubiquity/use_nonfree boolean false
ubiquity ubiquity/poweroff boolean true
ubiquity ubiquity/reboot boolean false
ubiquity ubiquity/reboot_on_failure boolean false
ubiquity ubiquity/automation_failure_command string echo NOH_INSTALL_INTERACTION_REQUIRED >/dev/ttyS0
ubiquity ubiquity/failure_command string echo NOH_INSTALL_FAILED >/dev/ttyS0
ubiquity ubiquity/success_command string {finalize}
''')
    args.output.chmod(0o600)


def require_installed_mint(root_mount, installation_marker):
    if (not root_mount.split() or root_mount.split()[0] not in ('ext4', 'btrfs', 'xfs')
            or not installation_marker):
        raise ValueError('Mint must boot its installed disk; a live ISO cannot qualify this case')


def command(arguments, timeout=30):
    return subprocess.run(arguments, check=True, text=True, capture_output=True, timeout=timeout).stdout


def desktop_environment(manager_text, shell_bytes):
    """Use the actual desktop process when a legacy Xorg session omits manager imports."""
    names = ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_CURRENT_DESKTOP',
             'KDE_SESSION_VERSION', 'KDE_FULL_SESSION')
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
    if image.get('installation_method') == 'ubiquity-preseed':
        root_mount = command(['findmnt', '--noheadings', '--output', 'FSTYPE,SOURCE', '/']).strip()
        marker = Path('/var/lib/noh-unattended-install-complete').is_file()
        require_installed_mint(root_mount, marker)
        write(args.output / 'installation.json', {'method': 'ubiquity-preseed', 'root_mount': root_mount,
                                                 'installer_success_marker': marker})
    session = None
    login_service, shell_name, desktop_token = {
        'GNOME': ('gdm-autologin', 'gnome-shell', 'GNOME'),
        'KDE': ('sddm-autologin', 'plasmashell', 'KDE'),
        'Cinnamon': ('lightdm-autologin', 'cinnamon', 'X-Cinnamon'),
    }[row['desktop']]
    for _ in range(24):
        observed_sessions = []
        for line in command(['loginctl', 'list-sessions', '--no-legend']).splitlines():
            values = command(['loginctl', 'show-session', line.split()[0]])
            data = dict(item.split('=', 1) for item in values.splitlines() if '=' in item)
            observed_sessions.append(data)
            if (data.get('Name') == 'noh' and data.get('Type') == row['display'] and
                    data.get('Seat') == 'seat0' and data.get('Active') == 'yes' and
                    data.get('Service') == login_service and data.get('Remote') == 'no'):
                session = data
                break
        if session:
            break
        time.sleep(5)
    write(args.output / 'observed-sessions.json', observed_sessions)
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
    if desktop_token not in os.environ.get('XDG_CURRENT_DESKTOP', '').split(':'):
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
    initial_sinks = sinks
    command(['pactl', 'set-default-sink', sink['name']])
    # Disposable virtual hardware can start muted (Debian's fresh HDA profile).
    # Establish and record a normal audible device setting before app playback.
    command(['pactl', 'set-sink-mute', sink['name'], '0'])
    command(['pactl', 'set-sink-volume', sink['name'], '75%'])
    sinks = json.loads(command(['pactl', '--format=json', 'list', 'sinks']))
    sink = next(s for s in sinks if s['name'] == sink['name'])
    if sink.get('mute') is not False or not sink.get('volume') or any(
            channel.get('value', 0) <= 0 for channel in sink['volume'].values()):
        raise ValueError('Virtual-PCI output remains muted or has zero volume')
    audio_info = command(['pactl', 'info'])
    if ('PipeWire' in audio_info) != (row['audio'] == 'pipewire'):
        raise ValueError('The observed sound server differs from the approved case')
    write(args.output / 'session.json', {'session': session, 'manager': {k: os.environ.get(k) for k in
          ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE',
           'KDE_SESSION_VERSION', 'KDE_FULL_SESSION')},
          'initial_sinks': initial_sinks, 'sinks': sinks, 'default_sink': sink['name'],
          'audio_info': audio_info, 'os_release': actual_os})
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
                        '--graphics', 'wayland' if row['display'] == 'wayland' else 'surfaceless', '--gui', '--retain-media',
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
          'limits': ['Runtime GUI preview uses the explicit capture harness; separate QMP chooser/cancellation/import and desktop output opening require screen review.',
                     'QMP screen and recorded application audio still require independent inspection.',
                     'No physical-device or human-listening qualification; full seven-case matrix remains required.']})
    print('Native runtime checks passed; screen/audio review and full matrix coverage remain pending.')


def open_output(args):
    """Use the actual desktop's MIME opener on the byte-identical CLI export."""
    observation = read(args.output / 'case.json')
    session = read(args.output / 'session.json')
    report = read(args.output / 'result.json')
    export = (args.output / 'media/composition.mp4').resolve()
    if digest(export) != report['synthetic_export_sha256']:
        raise ValueError('Retained export differs from the decoded/no-overwrite-tested output')
    app = Path(f'/proc/{observation["normal_startup_pid"]}/exe')
    if app.resolve(strict=True) != (ROOT / 'NOH.AppDir/usr/lib/noh/noh-app').resolve():
        raise ValueError('The ordinary candidate application is no longer running')
    chooser = subprocess.run(['pgrep', '-x', '-u', str(os.getuid()), 'zenity'], capture_output=True, timeout=10)
    if chooser.returncode != 1:
        raise ValueError('The Zenity fallback remains running, or its process state could not be checked')
    # rfd prefers the desktop portal. Absence of the Zenity fallback does not
    # establish chooser closure; all UI conclusions require the retained screens.
    os.environ.update({k: v for k, v in session['manager'].items() if v is not None})
    os.environ.update(XDG_RUNTIME_DIR=f'/run/user/{os.getuid()}',
                      DBUS_SESSION_BUS_ADDRESS=f'unix:path=/run/user/{os.getuid()}/bus')
    # Explicit test-account preferences, not a claim about a distro's defaults.
    # Disable external-player audio so only NOH can supply the recorded 440 Hz tone.
    config = Path.home() / '.config/mpv'
    config.mkdir(parents=True, exist_ok=True)
    # This QEMU GPU has no accelerated rendering. Keep the external viewer on
    # its documented shared-memory output; NOH's own rendering is unchanged.
    video_output = 'wlshm' if session['manager']['XDG_SESSION_TYPE'] == 'wayland' else 'x11'
    settings = f'audio=no\nkeep-open=yes\nvo={video_output}\nlog-file={args.output.resolve() / "output-player.log"}\n'
    with (config / 'mpv.conf').open('x') as file:
        file.write(settings)
    command(['xdg-mime', 'default', 'mpv.desktop', 'video/mp4'])
    association = command(['xdg-mime', 'query', 'default', 'video/mp4']).strip()
    if association != 'mpv.desktop':
        raise ValueError('The test account did not retain its chosen desktop video handler')
    with (args.output / 'xdg-open.log').open('x') as log:
        opener = subprocess.Popen(['xdg-open', str(export)], stdin=subprocess.DEVNULL,
                                  stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    player = None
    for _ in range(40):
        found = subprocess.run(['pgrep', '-x', '-u', str(os.getuid()), 'mpv'], text=True, capture_output=True, timeout=5)
        if found.returncode not in (0, 1):
            raise ValueError('Unable to inspect the desktop output player')
        for pid in found.stdout.split():
            arguments = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
            if any(str(export).encode() in argument for argument in arguments):
                player = {'pid': int(pid), 'arguments': [arg.decode() for arg in arguments if arg]}
        if player:
            break
        if opener.poll() not in (None, 0):
            raise ValueError('The ordinary desktop opener failed; see xdg-open.log')
        time.sleep(.25)
    if not player:
        raise ValueError('The desktop association did not launch a player for the exact output')
    time.sleep(3)
    player_log = (args.output / 'output-player.log').read_text()
    if 'VO: [' not in player_log:
        raise ValueError('The desktop player did not establish a video output')
    write(args.output / 'output-opening.json', {'status': 'awaiting-review', 'export_sha256': digest(export),
          'association': association, 'explicit_player_settings': settings, 'player': player,
          'opener_exit_code': opener.poll(), 'limits': ['Actual output screen still requires independent review.',
                                                      'External-player audio is disabled to preserve NOH-only tone evidence.']})


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
    p = sub.add_parser('mint-preseed')
    p.add_argument('--public-key', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=mint_preseed)
    p = sub.add_parser('guest')
    p.add_argument('--case', required=True)
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=guest)
    p = sub.add_parser('open-output')
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=open_output)
    p = sub.add_parser('audio-report')
    p.add_argument('--input', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.set_defaults(fn=audio_report)
    args = parser.parse_args()
    args.fn(args)


if __name__ == '__main__':
    main()
