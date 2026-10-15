# Linux desktop candidate checks

This branch/manual workflow tests an existing sealed Linux candidate in actual
QEMU/KVM desktop VMs. It never recompiles or publishes NOH. The first adapter
covers Ubuntu 24.04 and 26.04 GNOME/Wayland/PipeWire, Debian 13
GNOME/X11/PulseAudio, Fedora 44 GNOME/Wayland/PipeWire, openSUSE Tumbleweed
KDE/X11/PipeWire, Arch KDE/Wayland/PipeWire and installed Mint 22.3
Cinnamon/X11/PipeWire. All seven rows in `assets/linux-validation-matrix.json`
remain required. A subset or a successful shell exit cannot qualify the release.

The initial infrastructure-only experiment succeeded in run
[38771109225](https://github.com/Awalgawe/noh/actions/runs/38771109225), using source
`2d4e8bb106d0f68fd3261d242c203b12d3148ddf`. Its GNOME scanout and synthetic virtual
audio were independently reviewed. That run did not execute NOH. The earlier
failed run remains recorded; the SSH user manager started before audio packages
were installed, requiring a reload/start of their vendor units.

## Candidate identity and execution

The workflow defaults to the exact 0.1.4 candidate from Release run 38784097315,
attempt 1, source `392b3a5799cd4848dd9f18310efe05a99e0e55c2`. Manual inputs may name
another exact successful Linux Release. The test consumer reuses the release
consumer's authenticated repository, workflow, branch, commit, attempt, job and
artifact checks. It verifies GitHub's outer artifact digest, the complete delivery
envelope/materials and all package members before preparing a guest payload.
The original candidate identity and separate test-harness commit are retained.
The optional `suite` input selects all seven configured cases, the original three
(`baseline`), Fedora/openSUSE/Arch (`additional`) or Mint alone (`mint`) for bounded trials.
Selecting a subset does not change the final acceptance matrix.
The previous candidate from Release run 38757764365 reproduced a Debian ALSA underrun failure in
[run 38777802421](https://github.com/Awalgawe/noh/actions/runs/38777802421). It can
still support provisioning trials. The new default contains the audio recovery
correction; all seven desktops must validate these new candidate bytes.

Only source helpers, explicit contracts and the same candidate files enter the
guest. The GitHub token stays on the runner. All candidate files are checked
before and after execution; no overwrite, rebuild or repackaging of the delivery
archive occurs. Package prerequisites are installed from distribution repositories;
their exact versions, dated base-image identities and kernel/device records are
retained. `assets/linux-desktop-images.json` pins official images by SHA-256 or
SHA-512. No VM disk, cloud seed or private SSH key is uploaded.
After authenticated acquisition, owned download/unpack intermediates are removed
to leave space for the VM; the verified payload and source provenance remain.
Caller-supplied local envelopes are preserved.
Fedora uses RPM Fusion and openSUSE uses Packman Essentials for the same host codec
providers as the container checks. Arch performs a full package upgrade. Those
three guests shut down after provisioning and boot their installed kernel before
testing, so graphics/audio modules match the running kernel. First-boot logs and
audio are retained separately. openSUSE replaces `kernel-default-base` with the
complete `kernel-default` in one explicit Zypper transaction. Fedora retains the
image's hostname: a custom hostname is unnecessary for tests and the image reported
an early hostname-setting failure. The cloud configuration is schema-checked before
download and again inside the guest; empty package lists are omitted. Provisioning
stops on its first failing command, with full cloud-init diagnostics retained.
Rolling releases retain their actual OS metadata,
package versions and kernel in addition to the immutable base-image identity;
their evolving version string is not compared to a fixed release number. The
dated Arch and openSUSE filenames are pinned beneath rotating official mirror
directories: if an image expires, resolve and review a new available image
identity before retrying. Do not substitute an unpinned rolling alias. The
openSUSE 20261013 image expired during preparation; the reviewed replacement is
20261014 with its own checksum and matching base-package inventory.

Mint uses the official Cinnamon ISO and its Ubiquity installer, with a standard
preseed served only on runner loopback to the private QEMU network. It partitions
only the disposable VM's `/dev/vda`, creates the test user and installs SSH/cloud-init.
A random test password is hashed without printing it; the preseed is never uploaded.
The live installer receives no cloud seed. After successful installation powers
off, the ISO, kernel and initrd are detached and deleted; only the installed disk
and NoCloud seed boot for testing. Native checks require a persistent root filesystem,
the installer-success marker, Linux Mint 22.3 and a real LightDM Cinnamon X11 session.
An ISO boot or Ubuntu with Cinnamon cannot satisfy this case. Installer serial/audio
diagnostics remain separate from application evidence. Both QEMU and the private
seed server have owned-process cleanup and bounded waits. This adapter still needs
its first successful execution; configuration alone is not installation proof.

Each case requires the selected OS, a real GDM, SDDM or LightDM login on seat0, the selected desktop,
Wayland/X11 session, the selected PipeWire/PulseAudio server and a virtual PCI
audio sink. The virtual monitor advertises 1280x1024 so the existing 980x850
capture viewport fits with normal desktop decorations. Display/session variables
come from the actual user-owned desktop shell, with the user manager as a fallback;
the selected values are recorded before validation. A null sink is refused.
External IPv4/IPv6 default routes are removed
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
- [Fedora 44 official images](https://fedoraproject.org/releases.json) and [GNOME Wayland session package](https://packages.fedoraproject.org/pkgs/gnome-session/gnome-session-wayland-session/fedora-44.html).
- [openSUSE dated appliances](https://download.opensuse.org/tumbleweed/appliances/), [Plasma session package source](https://api.opensuse.org/public/source/openSUSE:Factory/plasma6-workspace/plasma6-workspace.spec) and [SDDM configuration](https://manpages.opensuse.org/Tumbleweed/sddm/sddm.conf.5.en.html).
- [Official Arch cloud-image recipe](https://github.com/archlinux/arch-boxes/blob/master/images/cloud-image.sh) and [image mirror](https://geo.mirror.pkgbuild.com/images/latest/).
- [Official Mint 22.3 Cinnamon ISO/checksums](https://linuxmint.com/edition.php?id=326), [Mint's Ubiquity preseed options](https://github.com/linuxmint/ubiquity/blob/master/debian/ubiquity.templates) and [an existing Packer preseed implementation](https://github.com/mrlesmithjr/packer-templates/blob/master/http/linuxmint/preseed.cfg). Only the installer mechanism is reused; obsolete server/kernel/package choices from the historical example are excluded.
- [cloud-init NoCloud](https://docs.cloud-init.io/en/latest/reference/datasources/nocloud.html), [GNOME automatic login](https://help.gnome.org/system-admin-guide/login-automatic.html) and [QEMU invocation](https://www.qemu.org/docs/master/system/invocation.html).

Existing QEMU, cloud-init and OpenSSH commands provide the VM lifecycle; the NOH
helpers only bind candidate identity, distribution setup and application evidence.
Quickemu's documented Linux flow requires ordinary interactive installation, and
EtchDroid's action wraps lifecycle/recording without desktop provisioning. The
bounded direct invocation avoids an additional action or general VM framework.
