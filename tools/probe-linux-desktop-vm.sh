#!/usr/bin/env bash
# Native observations of an existing candidate; independent review remains required.
set -euo pipefail
umask 077

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
evidence="$root/logs/linux-desktop-probe"
case_id=${NOH_DESKTOP_CASE:?Select an approved desktop case}
payload=${NOH_CANDIDATE_PAYLOAD:?Provide the verified candidate payload}
test -f "$payload"
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

ssh-keygen -q -t ed25519 -N '' -f "$work/key"
python3 "$root/tools/linux_desktop_candidate.py" cloud-config --case "$case_id" \
    --public-key "$work/key.pub" --output "$work/user-data"
cp "$work/user-data.image.json" "$evidence/image-identity.json"
mapfile -t image_spec < <(python3 - "$work/user-data.image.json" <<'PY'
import json, sys
image = json.load(open(sys.argv[1]))
for name in ('url', 'algorithm', 'digest'):
    print(image[name])
print('true' if image.get('reboot_after_provisioning') else 'false')
PY
)
test "${#image_spec[@]}" = 4
curl --fail --location --retry 2 --max-time 300 "${image_spec[0]}" -o "$work/base.img"
printf '%s  %s\n' "${image_spec[2]}" "$work/base.img" | "${image_spec[1]}sum" --check
qemu-img create -f qcow2 -F qcow2 -b "$work/base.img" "$work/guest.qcow2" 16G
printf 'instance-id: noh-desktop-probe\nlocal-hostname: noh-desktop-probe\n' > "$work/meta-data"
cloud-localds "$work/seed.img" "$work/user-data" "$work/meta-data"

# QEMU, cloud-init and SSH provide the lifecycle; no container or nested compositor.
# VNC and SSH are loopback-only. The monitor is a private Unix socket.
start_guest() {
    qemu-system-x86_64 -enable-kvm -machine q35 -cpu host -smp 3 -m 6144 \
    -drive "file=$work/guest.qcow2,format=qcow2,if=virtio" \
    -drive "file=$work/seed.img,format=raw,if=virtio,readonly=on" \
    -device virtio-vga,xres=1280,yres=1024 -device qemu-xhci -device usb-tablet \
    -device intel-hda -device hda-output,audiodev=sound \
    -audiodev "wav,id=sound,path=$evidence/virtual-audio.wav" \
    -netdev user,id=net,hostfwd=tcp:127.0.0.1:2222-:22 -device virtio-net-pci,netdev=net \
    -display vnc=127.0.0.1:9 -serial "file:$evidence/serial.log" \
    -qmp "unix:$work/qmp.sock,server=on,wait=off" \
    -no-reboot > "$evidence/qemu.log" 2>&1 &
    qemu_pid=$!
}
start_guest
transport_args=(-i "$work/key" -o BatchMode=yes -o ConnectTimeout=5
    -o StrictHostKeyChecking=accept-new -o "UserKnownHostsFile=$work/known-hosts")
ssh_args=(-p 2222 "${transport_args[@]}")
wait_for_ssh() {
    for ((i=0; i<60; i++)); do
        kill -0 "$qemu_pid" || { cat "$evidence/qemu.log"; return 1; }
        if ssh "${ssh_args[@]}" noh@127.0.0.1 true 2>/dev/null; then return 0; fi
        sleep 5
    done
    echo 'Guest SSH did not become ready within five minutes.'
    return 1
}
wait_for_ssh

# One bounded remote wait; save diagnostics even if provisioning failed.
cloud_status=0
timeout 20m ssh "${ssh_args[@]}" noh@127.0.0.1 'sudo cloud-init status --wait --long' \
    > "$evidence/cloud-init-status.txt" 2>&1 || cloud_status=$?
ssh "${ssh_args[@]}" noh@127.0.0.1 'sudo cat /var/log/cloud-init-output.log' \
    > "$evidence/cloud-init-output.log"
test "$cloud_status" = 0 || { cat "$evidence/cloud-init-status.txt"; exit "$cloud_status"; }
if [[ "${image_spec[3]}" = true ]]; then
    # Boot the installed kernel after provisioning its complete graphics/audio
    # modules. Preserve first-boot diagnostics; never test a mismatched module tree.
    ssh "${ssh_args[@]}" noh@127.0.0.1 'sudo systemctl poweroff --no-block'
    timeout 90s tail --pid="$qemu_pid" -f /dev/null
    wait "$qemu_pid"
    qemu_pid=
    mv "$evidence/serial.log" "$evidence/provisioning-serial.log"
    mv "$evidence/qemu.log" "$evidence/provisioning-qemu.log"
    mv "$evidence/virtual-audio.wav" "$evidence/provisioning-audio.wav"
    start_guest
    wait_for_ssh
    timeout 3m ssh "${ssh_args[@]}" noh@127.0.0.1 'sudo cloud-init status --wait --long' \
        > "$evidence/cloud-init-after-reboot.txt" 2>&1
fi
guest_status=0
scp -P 2222 "${transport_args[@]}" "$payload" noh@127.0.0.1:payload.tar
ssh "${ssh_args[@]}" noh@127.0.0.1 'mkdir noh-candidate && tar -xf payload.tar -C noh-candidate'
timeout 12m ssh "${ssh_args[@]}" noh@127.0.0.1 \
    "python3 /home/noh/noh-candidate/tools/linux_desktop_candidate.py guest --case '$case_id' --output /home/noh/noh-evidence" \
    > "$evidence/guest.log" 2>&1 || guest_status=$?
scp -r -P 2222 "${transport_args[@]}" noh@127.0.0.1:noh-evidence "$evidence/runtime" || guest_status=1
ssh "${ssh_args[@]}" noh@127.0.0.1 \
    'if command -v dpkg-query >/dev/null; then dpkg-query -W; elif command -v rpm >/dev/null; then rpm -qa; else pacman -Q; fi' \
    > "$evidence/packages.txt"

# QMP captures the actual guest scanout, independent of the application's capture mode.
python3 - "$work/qmp.sock" "$evidence/desktop.ppm" <<'PY'
import json, socket, sys, time
with socket.socket(socket.AF_UNIX) as connection:
    connection.settimeout(15)
    connection.connect(sys.argv[1])
    stream = connection.makefile('rwb', buffering=0)
    json.loads(stream.readline())
    for command in ({'execute': 'qmp_capabilities'},
                    {'execute': 'human-monitor-command', 'arguments': {'command-line': 'sendkey esc'}},
                    {'execute': 'screendump', 'arguments': {'filename': sys.argv[2]}}):
        stream.write(json.dumps(command).encode() + b'\n')
        while True:
            reply = json.loads(stream.readline())
            if 'error' in reply:
                raise RuntimeError(reply)
            if 'return' in reply:
                break
        time.sleep(.5)
PY
kill "$qemu_pid"
wait "$qemu_pid" || true
qemu_pid=
test "$guest_status" = 0 || { cat "$evidence/guest.log"; exit "$guest_status"; }
python3 "$root/tools/linux_desktop_candidate.py" audio-report \
    --input "$evidence/virtual-audio.wav" --output "$evidence/application-audio.json"
echo 'Native candidate checks completed; independent screen/audio review and the full matrix remain required.'
