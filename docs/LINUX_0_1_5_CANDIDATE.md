# Linux 0.1.5 candidate and qualification

The native Wayland file-drop correction was merged through
[PR 26](https://github.com/Awalgawe/noh/pull/26) after required CI passed.
The companion clipboard integration and ZIP 9 security update are included.
[PR 27](https://github.com/Awalgawe/noh/pull/27) restored an unavailable pinned
openSUSE test image and added a preflight check before compilation.

## Exact candidate

- Compiled source: `1e2bbbd94c578115d45a750ab61493f7a1e48cae`.
- [Release run 39036065018, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39036065018): successful Linux build stage.
- Artifact: `delivery-linux-x64`, ID `12102976514`.
- Authenticated transport SHA-256: `ff4de48a0e1a64fc3505f58da88a5024364616076cdce3a2950070c8545e0919`.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `NOH-0.1.5-linux-x64.zip` | 484763457 | `3933b2ac380e8699472525fa9b6d42174514494f43d1941ddc5d823e7fc0fae4` |
| `NOH-0.1.5-linux-x64-materials.zip` | 365754866 | `5ac4c23960e66ea68ee236794c9cfaea5c160a48d4bee7c2ce77f07b1140b13b` |
| `DELIVERY.json` | 4174222 | `ecc86c5b9563cf35230e3e2453de989bd2237e8cf7fe5ef82db3e234cb63fa7f` |

The authenticated raw transport, every archive member and executable mode were
verified locally. The 49 bundle files and 24353 material files retain complete
inventories. All 381 NOH source files match the canonical archive of the compiled
commit. The 486 vendored crates match their Cargo checksum manifests and exact
file membership. Original notices for the three changed dependencies match the
approved input snapshot. Unchanged native and vendor material maps retain their
previously reviewed provenance. The [CI receipt](release-evidence/linux-0.1.5-ci.json)
records the verification scope and limitations.

## Container and source checks

The release build reports 477 passing Cargo tests and 12 explicit ignores across
22 suites. Audit and delivery controls passed. The same sealed files passed all
six container cases: Ubuntu 24.04 and 26.04, Debian 13, Fedora 44, openSUSE
Tumbleweed and Arch. Their GUI captures visibly render the synthetic video;
package inventories, probe receipts, exact executable hashes and matrix identity
were checked against the [container report](release-evidence/linux-0.1.5-runtime.json).

These checks cover startup, host media/graphics/audio libraries, import, preview,
export/decode/no-overwrite and offline CPU transcription. Containers use software
graphics and synthetic audio. Native desktop qualification is separate.

## Native desktop observations

The same candidate passed seven disposable QEMU/KVM desktop cases in
[desktop run 39037706317](https://github.com/Awalgawe/noh/actions/runs/39037706317).
Candidate and harness identities are recorded separately, although this run uses
the same commit for both. Each case verifies the full payload, provisions the
named desktop, removes external default routes for runtime checks, records
NOH-only virtual sound-card output, and retains actual chooser, cancellation,
import and exported-file opening screens for review.

| Distribution | Desktop / display / audio | Observed result |
| --- | --- | --- |
| Ubuntu 24.04 | GNOME / Wayland / PipeWire | Passed, including native file drop |
| Ubuntu 26.04 | GNOME / Wayland / PipeWire | Passed, including native file drop |
| Debian 13 | GNOME / X11 / PulseAudio | Passed |
| Fedora 44 | GNOME / Wayland / PipeWire | Passed, including native file drop |
| openSUSE Tumbleweed | KDE / X11 / PipeWire | Passed |
| Arch Linux | KDE / Wayland / PipeWire | Passed, including native file drop |
| Linux Mint 22.3 | Cinnamon / X11 / PipeWire | Passed |

The [desktop matrix](release-evidence/linux-0.1.5-desktops.json) binds each
case to its detailed report, installed package versions and retained evidence
hashes. All chooser/cancel/import, runtime video, external-player and four
Wayland hover/import sequences were visually inspected. Each Wayland import
adds the image and video to the chooser-imported clip, loads `drop sound.wav`,
and reaches Ready to export. Its still frame at time zero is black with the
default fade-in; the separate playback captures show rendered moving video.
The native logs show `text/uri-list` receive and finish requests. Newer system
libwayland logs use `#` object delimiters, while Ubuntu 24.04 uses `@`; both
forms were checked. Captured virtual sound-card output contains the expected
440 Hz NOH tone; the external player has audio disabled.

The four Wayland sessions additionally receive PNG, MP4 and WAV file URIs from a
separate native GTK process using actual QEMU pointer input. They retain hover
and import screens and both protocol logs; no input events are injected into
egui. The three X11 cases retain their chooser and output checks. The separate
native regression probe covers cancelled and rejected drops, clipboard round
trips, and X11-to-X11 import; its scope is documented in
[WAYLAND_FILE_DROPS.md](WAYLAND_FILE_DROPS.md).

Desktop observations are assembled only after inspecting their actual captures.
This report does not treat a successful CI job alone as visual acceptance.
QEMU results do not qualify physical GPUs, audio hardware or human listening.
Cross-protocol XWayland bridges remain outside the qualification scope.

The local release-control checks also passed: 63 delivery tests, 8 container
matrix tests and 8 desktop guard tests, with one explicit platform skip.

## Review and publication boundary

Source review v3 approved `5f134416275961ab31a1839f1378a368e59f0937` and pipeline
review v4 approved `44fa9dad3f3d4ee96b1f785a39143146bca6fdbf` in the independent
Critic chat `01a14995-743f-7c03-b385-9b1f3a98f3c8`. Their trees were merged
unchanged after required CI passed. Final-byte qualification requires a separate
OK on the exact frozen policy, evidence and [release notes](LINUX_0_1_5_RELEASE.md).

Publication uses the owner's authorized manual release route with the unchanged
authenticated CI archives, after `tools/delivery.py qualify` succeeds and the
reviewed qualification commit is merged through required CI. This does not claim
execution of the optional protected-environment draft workflow. No environment
protection is changed or disabled. The release tag points to the compiled source
commit above; the later evidence commit does not require rebuilding the package.
The original delivery record stays byte-identical and is attached publicly as
`linux-x64-DELIVERY.json`; publication checksums use that attachment name.

The first attempted release, run `39032946173`, failed because the old openSUSE
container manifest was unavailable. It uploaded no candidate. Its binaries and
partial matrix are not used for this release.
