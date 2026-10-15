# Linux desktop candidate checks

This branch/manual workflow tests an existing sealed Linux candidate in actual
QEMU/KVM desktop VMs. It never recompiles or publishes NOH. The first adapter
covers Ubuntu 24.04 and 26.04 GNOME/Wayland/PipeWire plus Debian 13
GNOME/X11/PulseAudio. These are three rows of the unchanged seven-row matrix in
`assets/linux-validation-matrix.json`; Fedora, openSUSE, Arch and Mint remain
required. A subset or a successful shell exit cannot qualify the release.

The initial infrastructure-only experiment succeeded in run
[38771109225](https://github.com/Awalgawe/noh/actions/runs/38771109225), using source
`2d4e8bb106d0f68fd3261d242c203b12d3148ddf`. Its GNOME scanout and synthetic virtual
audio were independently reviewed. That run did not execute NOH. The earlier
failed run remains recorded; the SSH user manager started before audio packages
were installed, requiring a reload/start of their vendor units.

## Candidate identity and execution

The workflow defaults to the exact 0.1.4 candidate from Release run 38757764365,
attempt 1, source `51c5c29214986d453db5499a8b386ccf6c971743`. Manual inputs may name
another exact successful Linux Release. The test consumer reuses the release
consumer's authenticated repository, workflow, branch, commit, attempt, job and
artifact checks. It verifies GitHub's outer artifact digest, the complete delivery
envelope/materials and all package members before preparing a guest payload.
The original candidate identity and separate test-harness commit are retained.

Only source helpers, explicit contracts and the same candidate files enter the
guest. The GitHub token stays on the runner. All candidate files are checked
before and after execution; no overwrite, rebuild or repackaging of the delivery
archive occurs. Package prerequisites are installed from distribution repositories;
their exact versions, dated base-image identities and kernel/device records are
retained. `assets/linux-desktop-images.json` pins official images by SHA-256 or
SHA-512. No VM disk, cloud seed or private SSH key is uploaded.

Each case requires the selected OS, a real GDM login on seat0, GNOME, the selected
Wayland/X11 session, the selected PipeWire/PulseAudio server and a virtual PCI
audio sink. A null sink is refused. External IPv4/IPv6 default routes are removed
after provisioning. GDM settings use Ubuntu's `/etc/gdm3/custom.conf` or Debian's
`/etc/gdm3/daemon.conf`, matching the distribution package. The directly connected
SSH transport remains available. The shared host probe then checks ALSA before
graphics/libmpv before default PCM,
actual GUI import/video/audio preview, export/full decode/no overwrite and packaged
CPU transcription from synthetic speech. Its explicit GUI capture harness is
retained; it is not described as manual chooser or pointer interaction.

A separate ordinary AppRun startup, without capture injection, remains visible
for a QMP scanout. QEMU records the application's preview audio through its virtual
HDA output. Unlike the earlier infrastructure experiment, no separate tone player
runs: the audio check requires a quarter-second window dominated by the preview's
440 Hz signal. Raw audio, the application capture/report, normal desktop scanout,
package inventory and diagnostics are retained for independent inspection.

The workflow has read-only repository/actions access, loopback-only SSH/VNC,
private QMP and an owned-process cleanup trap. Each command is limited to 35
minutes and each job to 45 minutes. Failed guest checks still retain available
screen/package evidence and remain failures. Successful observations are marked
`awaiting-review`; this workflow never writes a final qualification policy record.
The release gate still requires all seven exact-bound cases and independent review.
Virtual graphics/audio do not establish physical-device or human-listening quality.
The earlier WSLg audio incident remains a separate unresolved observation.

## References and reuse

Checked on 2026-10-15:

- [GitHub runner virtualization limits](https://docs.github.com/en/actions/concepts/runners/github-hosted-runners): nested VMs remain experimental and unsupported.
- [Ubuntu 24.04 image/checksums](https://cloud-images.ubuntu.com/noble/20260926/) and [Ubuntu 26.04](https://cloud-images.ubuntu.com/resolute/20260927/).
- [Debian 13 dated generic image/checksums](https://cloud.debian.org/images/cloud/trixie/20261001-2618/).
- [Debian's GNOME Xorg session package](https://packages.debian.org/trixie/gnome-session-xsession).
- [cloud-init NoCloud](https://docs.cloud-init.io/en/latest/reference/datasources/nocloud.html), [GNOME automatic login](https://help.gnome.org/system-admin-guide/login-automatic.html) and [QEMU invocation](https://www.qemu.org/docs/master/system/invocation.html).

Existing QEMU, cloud-init and OpenSSH commands provide the VM lifecycle; the NOH
helpers only bind candidate identity, distribution setup and application evidence.
Quickemu's documented Linux flow requires ordinary interactive installation, and
EtchDroid's action wraps lifecycle/recording without desktop provisioning. The
bounded direct invocation avoids an additional action or general VM framework.
