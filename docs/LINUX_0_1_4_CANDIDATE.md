# Linux 0.1.4 CI candidate

The Linux package is published in [release 0.1.4](https://github.com/Awalgawe/noh/releases/tag/v0.1.4).
All seven named desktops passed the observed CI checks. These checks did not cover
external drag-and-drop; see the [native Wayland defect and correction](WAYLAND_FILE_DROPS.md).
Independent approval applies to an exact policy/evidence commit; these tests do
not establish physical-hardware qualification.

## Exact build and files

- Source: `392b3a5799cd4848dd9f18310efe05a99e0e55c2` on `main`.
- [Release run 38784097315, attempt 1](https://github.com/Awalgawe/noh/actions/runs/38784097315): successful, Linux-only, build stage.
- Download the run's `delivery-linux-x64` artifact for the package, corresponding
  materials, `DELIVERY.json` and `SHA256SUMS`.
- The artifact is retained locally under
  `logs/linux-host-libraries/ci-38784097315/delivery-linux-x64/`.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `NOH-0.1.4-linux-x64.zip` | 484732269 | `493a3e4d7258ec282f7271edbf2c0f6b88d7f5ea197a5099719858a8080b2d18` |
| `NOH-0.1.4-linux-x64-materials.zip` | 365660280 | `d9421fb2a8913b677040d79ef5da90bfe1008f7a7541245546214756a34c3527` |
| `DELIVERY.json` | 4171848 | `13304be35c3f06b803a0c8a93d7041a933e9a9378b8bd1d4ea7688a402c8eff3` |

Extract into a fresh directory, install the distribution dependencies described
in the included installation guide, and run `NOH.AppDir/AppRun`. CPU transcription
is included; FFmpeg, libmpv and the graphics/audio stack come from the host.

## CI results and independent review

[PR 23](https://github.com/Awalgawe/noh/pull/23) integrated the Linux ALSA recovery
fix after [required CI](https://github.com/Awalgawe/noh/actions/runs/38781412565)
passed, including Linux, Windows and both macOS architectures.

The Linux Release reports 477 Cargo test passes and 12 explicit ignores across
22 suites. This count does not claim that opt-in physical-device tests ran.
The same sealed candidate passed all six container cases: Ubuntu 24.04 and
26.04, Debian 13, Fedora 44, openSUSE Tumbleweed and Arch Linux. Checks cover
dependency installation, startup identity, graphics/audio loading, media preview,
export/decode/no-overwrite and offline CPU transcription. These are software
graphics and synthetic audio checks, not full desktop or hardware tests.

Critic OK on v23 covers the exact candidate, corresponding sources/notices,
49 packaged files, 24339 material files, the canonical source archive and six
container results. Those unchanged results are reused without rebuilding.
The [CI receipt](release-evidence/linux-0.1.4-ci.json) and unchanged
[container matrix](release-evidence/linux-0.1.4-runtime.json) retain this evidence.

## Native desktop evidence

The same 49-file payload runs in disposable QEMU/KVM machines on GitHub Actions.
Each case uses a normal user in the actual named desktop session. The checks
cover fresh installation, startup, chooser/cancellation/import, rendered video
and recorded application audio, export/decode/no-overwrite, offline transcription
and visible opening of the exported file. No application rebuild is involved.

| Distribution | Desktop / display / audio | Observed result | CI run |
| --- | --- | --- | --- |
| Ubuntu 24.04 | GNOME / Wayland / PipeWire | Passed; Critic v29 reviewed | [38797609408](https://github.com/Awalgawe/noh/actions/runs/38797609408) |
| Debian 13 | GNOME / X11 / PulseAudio | Passed; Critic v29 reviewed | [38797609408](https://github.com/Awalgawe/noh/actions/runs/38797609408) |
| Ubuntu 26.04 | GNOME / Wayland / PipeWire | Passed; subject to exact final review | [38801535222](https://github.com/Awalgawe/noh/actions/runs/38801535222) |
| openSUSE Tumbleweed | KDE / X11 / PipeWire | Passed; subject to exact final review | [38801535222](https://github.com/Awalgawe/noh/actions/runs/38801535222) |
| Arch Linux | KDE / Wayland / PipeWire | Passed; subject to exact final review | [38801535222](https://github.com/Awalgawe/noh/actions/runs/38801535222) |
| Fedora 44 | GNOME / Wayland / PipeWire | Passed; subject to exact final review | [38812080615](https://github.com/Awalgawe/noh/actions/runs/38812080615) |
| Linux Mint 22.3 | Cinnamon / X11 / PipeWire | Passed; subject to exact final review | [38826016237](https://github.com/Awalgawe/noh/actions/runs/38826016237) |

The [desktop matrix](release-evidence/linux-0.1.4-desktops.json) binds each case to
its exact candidate, dated image/checksum, session, device, test time and retained
observations. Individual reports bind runtime results, actual UI screens, exported
video and application-only PCM to their original CI artifacts. Package inventories
and kernel/device diagnostics remain in those artifacts. Rolling-distribution
results identify the tested images and installed packages; mutable repositories
are not a guarantee of future bit-for-bit environment reconstruction.

Mint run 38826016237 completed the ISO installation, booted the installed Mint
22.3 system and passed the Cinnamon/X11/PipeWire application checks. The native
chooser opened, cancellation returned cleanly, the imported clip appeared in the
preview, and the export opened visibly in the configured player. Application-only
virtual audio has a 440 Hz power fraction of 0.987 and peak 9675. Failed earlier
installer/provisioning attempts are not used as application evidence.

The proposed exact qualification is recorded in
[`assets/delivery-policy.json`](../assets/delivery-policy.json), review round
`linux-host-libraries-20261015-v30`. It reuses Critic v23 for the unchanged build
and v29 for the first two native cases. Independent review must approve the final
commit containing all seven observations and this policy before publication.

## Limits and publication boundary

These observations use virtual graphics and audio devices. They establish no
physical-GPU, physical-audio-device or human-listening qualification. The external
mpv MIME handler and software video output are explicit test-account preferences;
NOH retains its default renderer. Container checks remain distinct from native
desktop checks. The earlier WSLg audio observation is not silently marked resolved.

All seven native case records are present. Release authorization additionally
requires the Critic's approval of the exact final evidence and policy commit. No public release, draft or tag is
authorized by this report. The existing release gate remains enforced.
