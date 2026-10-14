# Linux 0.1.3 exact-byte qualification

The Windows and Linux applications were produced by the same [Release build](https://github.com/Awalgawe/noh/actions/runs/38481645547),
attempt 1, from `6b8c8054b93169fd455d86d16af8aeb92fec1a0e`. This report covers the exact
linux x64 portable and corresponding materials. Publication and Windows
installer wrappers are separately checked; a future build does not inherit this acceptance.

The full [evidence record](release-evidence/linux-0.1.3.json) is bound in
`assets/delivery-policy.json` to DELIVERY.json SHA-256
`0bbf87d9eba04b1506f12a36ea504a5c9daf626198b7c112dab7a466a0a9c4ec`. The original CI manifest retains its `unqualified`
producer status; the separate exact-byte record defines the reviewed scope.

| Asset | Bytes | SHA-256 |
| --- | ---: | --- |
| `NOH-0.1.3-linux-x64-materials.zip` | 1160387102 | `ecd64a1cacf8f7b9cbe2e501da115340820ebfcb682a66078c7fefc932616d47` |
| `NOH-0.1.3-linux-x64.zip` | 590398412 | `d6c20c821ecc60d97f25e756bc0738a3e3eef7edbdcd5492368bdf771523a927` |

CI completed 474 tests, zero failures and 12 existing explicit
ignores across 22 suites. The shared RustSec audit and delivery
controls passed. Downloaded artifact digests, archive inventories, source
identity and native import closure were checked before native acceptance.

## Native desktop and media checks

Fresh unprivileged extraction, relocation to a path with spaces and removal of a
separate test extraction passed; external synthetic user data was preserved.
GUI, CLI and MCP report the same clean release identity. All 561 package files
remain unchanged. CLI media checks cover export with fades, full decoding,
caption burn, a vertical short and refusal to overwrite an existing output.
CPU transcription produced the expected synthetic English sentence in an
isolated network namespace with only loopback available.

The unchanged extracted CI GUI was also operated with real X11 input, without
capture-mode variables or injected application state. The recorded 73 actions
and observations cover:

- Zenity multi-file import (video, accented-name image, WAV and SRT) and chooser
  cancellation without a false error.
- Actual Play/Pause and seeking, rendered frames and measured WSLg audio output.
- Sung-language selection, transcription cancellation and successful retry.
- Opening generated lyrics in Mousepad, typing and saving a correction, then
  seeing that correction in NOH's preview while the editor stayed open.
- Two actual exports, MPV video opening and PCManFM folder opening.
- Native file-manager image drag-and-drop, TXT refusal and subsequent export
  success with the Open buttons restored. The second output used a -2 suffix
  and the original SHA-256 was preserved; both outputs decode completely.

Broader UI cases from the prior Critic-approved journey are explicitly reused:
short preview/export, export/preview cancellation, reordering, settings and
seven-language/narrow-window checks. Git comparison proves unchanged application
implementation and tools from approved e7c3f15 to this release; Cargo.toml changes
only the package version. Those broader cases were not all rerun on the CI bytes.
The new exact-byte interaction summary and its private evidence-manifest digest distinguish
directly repeated cases from that inherited evidence.

## Host setup and an observed WSLg audio incident

The fresh isolated desktop profile had no SRT default application. NOH correctly
reported the failed opening. Assigning installed Mousepad to application/x-subrip
in that test profile restored the actual button-to-editor workflow. The README
documents the required chooser and default applications; they are host components.

During external-player testing, WSLg PulseAudio stopped responding: MPV stalled
at audio initialization and independent pactl info timed out. The verified host
service did not exit on SIGTERM; its supervisor restarted it after SIGKILL.
The audio service then responded, MPV displayed the export and responded to
seek/close commands, and NOH Play/Pause passed again. This recovery changed no
package file. It does not fix or establish the cause of the underlying WSLg
issue. The event occurred after an earlier sink-monitor recording, without
proof that the recording caused it. The service remained responsive after tests.

## Correspondence and limits

All 343 packaged NOH source files match raw Git blobs at the CI source commit.
25,334 material files are byte-identical to the approved Linux 0.1.2 baseline;
only the application source, build environment and ELF report changed. All 228
ELF images have no unresolved direct import in the declared closure. Changed
root package metadata preserves the 486 third-party Cargo records and the
reviewed native/compiler/source inputs.

Acceptance is limited to Ubuntu 24.04.5 x86_64, glibc 2.39, under WSL 2/WSLg,
Mesa llvmpipe software rendering and PulseAudio. It is not a fresh-OS test or
qualification of other distributions, physical Linux graphics/audio hardware,
GPU transcription or human listening. Windows Explorer-to-WSLg dropping remains
unsupported; native Linux file-manager dropping was tested. This is an unsigned
AppDir ZIP with CPU speech, not an AppImage or automatic-update distribution.
All test windows and the owned desktop control processes were closed.
