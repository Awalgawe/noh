# Linux desktop VM feasibility

The manual/branch `Linux desktop VM feasibility` workflow tests whether a standard
Ubuntu GitHub runner can host an actual Ubuntu 24.04 GNOME Wayland desktop with
PipeWire. It does not compile, modify, download or qualify NOH, change the approved
matrix, or publish a release. Existing candidate and container/WSLg evidence stay
valid. A successful run is a prerequisite for extending desktop acceptance using
the exact existing candidate, not a qualification result itself.

The guest uses Canonical's dated 20260926 cloud image with its published SHA-256,
cloud-init's NoCloud seed, distribution GNOME/GDM/PipeWire packages, QEMU/KVM's
virtual GPU and HDA audio, and a normal GDM automatic login. Installed package
versions are retained; package repositories are resolved at preparation time.
KVM absence, failed provisioning, a missing real Wayland session or missing audio
services fail the probe. Software graphics and virtual audio cannot establish
physical-device behavior or human listening quality.

The run is limited to 35 minutes, with a 30-minute process timeout and an owned
QEMU cleanup trap. SSH/VNC bind only to loopback; QMP uses a private Unix socket.
The ephemeral SSH key, seed and VM disks are never uploaded. Evidence includes
runner/guest identity, package versions, boot/provisioning logs, actual guest
scanout and a synthetic tone recorded by QEMU. Screen and audio inspection are
required before claiming that this route is usable. No host desktop is controlled.

This small probe uses existing QEMU, cloud-init and OpenSSH commands. Quickemu's
documented Linux flow requires ordinary interactive installation; the existing
EtchDroid action mainly wraps QEMU lifecycle/recording and does not provision a
desktop. Direct invocation keeps this bounded experiment inspectable without a
third-party action dependency or a new general VM framework.

References checked on 2026-10-15:

- [GitHub runner virtualization limits](https://docs.github.com/en/actions/concepts/runners/github-hosted-runners): nested VMs are experimental and unsupported.
- [Canonical image and checksums](https://cloud-images.ubuntu.com/noble/20260926/).
- [cloud-init NoCloud](https://docs.cloud-init.io/en/latest/reference/datasources/nocloud.html).
- [GNOME automatic login](https://help.gnome.org/system-admin-guide/login-automatic.html).
- [QEMU invocation](https://www.qemu.org/docs/master/system/invocation.html).
- [Quickemu Linux setup](https://github.com/quickemu-project/quickemu/wiki/02-Create-Linux-virtual-machines).
- [EtchDroid QEMU/KVM action](https://github.com/etchdroid/qemu-kvm-action).
