#!/usr/bin/env bash
# Infrastructure feasibility only: this does not qualify a NOH candidate.
set -euo pipefail
umask 077

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
evidence="$root/logs/linux-desktop-probe"
mkdir -p "$evidence"
exec > >(tee "$evidence/probe.log") 2>&1
work=$(mktemp -d "${RUNNER_TEMP:-/tmp}/noh-desktop-vm.XXXXXX")
qemu_pid=
cleanup() {
    result=$?
    trap - EXIT
    if [[ -n "$qemu_pid" ]] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid"
        wait "$qemu_pid" || true
    fi
    printf '%s\n' "$result" > "$evidence/exit-code.txt"
    # The disposable runner owns the image/seed/key; never upload them.
    exit "$result"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

date --utc --iso-8601=seconds > "$evidence/started-at.txt"
uname -a > "$evidence/runner.txt"
lscpu >> "$evidence/runner.txt"
df -h "$work" >> "$evidence/runner.txt"
test -c /dev/kvm || { echo 'KVM device is unavailable; no desktop VM was tested.'; exit 1; }
# Restrict the device to this disposable runner's user instead of making it world-writable.
sudo setfacl -m "u:$(id -un):rw" /dev/kvm
sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends qemu-system-x86 qemu-utils cloud-image-utils
qemu-system-x86_64 --version > "$evidence/qemu-version.txt"

image_url=https://cloud-images.ubuntu.com/noble/20260926/noble-server-cloudimg-amd64.img
image_sha256=6a81c37564db9b1ee84e141922625e1d7c5b389b99bb3c572e0243607d5bb4d2
printf '%s\n%s\n' "$image_url" "$image_sha256" > "$evidence/image-identity.txt"
curl --fail --location --retry 2 --max-time 300 "$image_url" -o "$work/base.img"
printf '%s  %s\n' "$image_sha256" "$work/base.img" | sha256sum --check
qemu-img create -f qcow2 -F qcow2 -b "$work/base.img" "$work/guest.qcow2" 16G
ssh-keygen -q -t ed25519 -N '' -f "$work/key"
public_key=$(cat "$work/key.pub")
cat > "$work/user-data" <<EOF
#cloud-config
hostname: noh-desktop-probe
users:
  - name: noh
    uid: 1000
    shell: /bin/bash
    groups: [adm, sudo, audio, video, render]
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: true
    ssh_authorized_keys: ['$public_key']
ssh_pwauth: false
package_update: true
packages:
  - gnome-session
  - gdm3
  - pipewire-audio
  - pulseaudio-utils
  - alsa-utils
  - pciutils
  - python3
write_files:
  - path: /etc/gdm3/custom.conf
    content: |
      [daemon]
      AutomaticLoginEnable=True
      AutomaticLogin=noh
      WaylandEnable=true
      DefaultSession=gnome.desktop
runcmd:
  - [sh, -c, 'DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends linux-modules-extra-\$(uname -r)']
  - [modprobe, virtio_gpu]
  - [modprobe, snd_hda_intel]
  - [install, -d, -o, noh, -g, noh, /home/noh/.config]
  - [sh, -c, 'touch /home/noh/.config/gnome-initial-setup-done; chown noh:noh /home/noh/.config/gnome-initial-setup-done']
  - [systemctl, set-default, graphical.target]
  - [systemctl, restart, gdm3]
EOF
printf 'instance-id: noh-desktop-probe\nlocal-hostname: noh-desktop-probe\n' > "$work/meta-data"
cloud-localds "$work/seed.img" "$work/user-data" "$work/meta-data"

# QEMU, cloud-init and SSH provide the lifecycle; no container or nested compositor.
# VNC and SSH are loopback-only. The monitor is a private Unix socket.
qemu-system-x86_64 -enable-kvm -machine q35 -cpu host -smp 3 -m 6144 \
    -drive "file=$work/guest.qcow2,format=qcow2,if=virtio" \
    -drive "file=$work/seed.img,format=raw,if=virtio,readonly=on" \
    -device virtio-vga -device qemu-xhci -device usb-tablet \
    -device intel-hda -device hda-output,audiodev=sound \
    -audiodev "wav,id=sound,path=$evidence/virtual-audio.wav" \
    -netdev user,id=net,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=net \
    -display vnc=127.0.0.1:9 -serial "file:$evidence/serial.log" \
    -qmp "unix:$work/qmp.sock,server=on,wait=off" \
    -no-reboot > "$evidence/qemu.log" 2>&1 &
qemu_pid=$!
ssh_args=(-i "$work/key" -p 2222 -o BatchMode=yes -o ConnectTimeout=5
    -o StrictHostKeyChecking=accept-new -o "UserKnownHostsFile=$work/known-hosts")
ready=false
for ((i=0; i<60; i++)); do
    kill -0 "$qemu_pid" || { cat "$evidence/qemu.log"; exit 1; }
    if ssh "${ssh_args[@]}" noh@127.0.0.1 true 2>/dev/null; then ready=true; break; fi
    sleep 5
done
"$ready" || { echo 'Guest SSH did not become ready within five minutes.'; exit 1; }

# One bounded remote wait; save diagnostics even if provisioning failed.
cloud_status=0
timeout 20m ssh "${ssh_args[@]}" noh@127.0.0.1 'sudo cloud-init status --wait --long' \
    > "$evidence/cloud-init-status.txt" 2>&1 || cloud_status=$?
ssh "${ssh_args[@]}" noh@127.0.0.1 'sudo cat /var/log/cloud-init-output.log' \
    > "$evidence/cloud-init-output.log"
test "$cloud_status" = 0 || { cat "$evidence/cloud-init-status.txt"; exit "$cloud_status"; }
guest_status=0
timeout 180s ssh "${ssh_args[@]}" noh@127.0.0.1 bash -s > "$evidence/guest.txt" 2>&1 <<'GUEST' || guest_status=$?
set -euo pipefail
cat /etc/os-release
uname -a
systemd-detect-virt
lspci -nnk
loginctl list-sessions
session=
for ((i=0; i<24; i++)); do
    while read -r candidate _; do
        if [[ $(loginctl show-session "$candidate" -p Name --value) = noh &&
              $(loginctl show-session "$candidate" -p Type --value) = wayland ]]; then
            session=$candidate
            break
        fi
    done < <(loginctl list-sessions --no-legend)
    [[ -n "$session" ]] && break
    sleep 5
done
test -n "$session"
loginctl show-session "$session"
pgrep -a -u "$(id -u)" gnome-shell
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
# SSH started this user manager before cloud-init installed desktop packages.
# Apply the newly installed vendor units to that already-running manager.
systemctl --user daemon-reload
systemctl --user start pipewire pipewire-pulse wireplumber
systemctl --user is-active pipewire pipewire-pulse wireplumber
pactl info
pactl list short sinks
aplay -l
python3 - <<'PY'
import math, struct, wave
with wave.open('/tmp/noh-desktop-tone.wav', 'wb') as output:
    output.setparams((1, 2, 48000, 0, 'NONE', 'not compressed'))
    output.writeframes(b''.join(struct.pack('<h', int(8000 * math.sin(2 * math.pi * 440 * n / 48000)))
                               for n in range(48000 * 2)))
PY
pw-play /tmp/noh-desktop-tone.wav
GUEST
ssh "${ssh_args[@]}" noh@127.0.0.1 "dpkg-query -W -f='\${binary:Package}\t\${Version}\n'" \
    > "$evidence/packages.txt"

# QMP captures the actual guest scanout, independent of the application's capture mode.
python3 - "$work/qmp.sock" "$evidence/desktop.ppm" <<'PY'
import json, socket, sys
with socket.socket(socket.AF_UNIX) as connection:
    connection.settimeout(15)
    connection.connect(sys.argv[1])
    stream = connection.makefile('rwb', buffering=0)
    json.loads(stream.readline())
    for command in ({'execute': 'qmp_capabilities'},
                    {'execute': 'screendump', 'arguments': {'filename': sys.argv[2]}}):
        stream.write(json.dumps(command).encode() + b'\n')
        while True:
            reply = json.loads(stream.readline())
            if 'error' in reply:
                raise RuntimeError(reply)
            if 'return' in reply:
                break
PY
test "$guest_status" = 0 || { cat "$evidence/guest.txt"; exit "$guest_status"; }
echo 'GNOME Wayland and PipeWire guest probe completed; inspect the screen and audio evidence.'
echo 'Infrastructure feasibility only. No NOH candidate or physical device was qualified.'
