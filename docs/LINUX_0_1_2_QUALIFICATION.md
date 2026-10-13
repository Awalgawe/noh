# Linux 0.1.2 AppDir validation

The Linux x86_64 package was built by the ordinary
[Release workflow, run 38417732392](https://github.com/Awalgawe/noh/actions/runs/38417732392),
attempt 1, from public commit `b3863e80128340a13efa108e5c7347a50d932f05`.
Its application version is 0.1.2. This adds a Linux distribution without replacing
the previously published Windows executables or installer assets.

The exact artifact evidence is in
[linux-0.1.2.json](release-evidence/linux-0.1.2.json). Acceptance is bound to
`DELIVERY.json` SHA-256
`13e42879d8f0ce1016650eb765299fbdf1d2a422ed1845f49aa5775c2bb8d128`.
The producer record remains unchanged with its original `unqualified` status;
the separate reviewed policy records the acceptance of those exact bytes.

| Asset | Bytes | SHA-256 |
| --- | ---: | --- |
| `NOH-0.1.2-linux-x64.zip` | 590399188 | `dfc0806b22733a38f73d7ce00d734c010ffd1450572c4ce9aae503811c2c9904` |
| `NOH-0.1.2-linux-x64-materials.zip` | 1160374566 | `c0086e4515f7fd42d8a6256a930a218fbfa15f2f11dbc0ed54e5d3cfda6bf6b0` |

## Verification

The CI build completed 470 tests with no failures and 12 explicit opt-in tests
ignored, across 22 test suites. The run also passed redistribution controls,
the shared RustSec audit, Linux input/package tests and the ELF dependency audit.
The downloaded GitHub artifact digest, every archive member, executable mode,
source identity and native import closure were checked before execution.

The package was extracted without administrator privileges, moved to a path
containing spaces, and run from outside its directory with isolated XDG settings.
GUI, CLI and MCP reported the same clean release build identity. A separate fresh
extraction was removed while an adjacent synthetic user-media file was preserved.
All 561 packaged files remained byte-identical after the tests.

Synthetic media checks covered encoded export with fades, refusal to overwrite an
existing output, decoding the output, caption burn and a vertical short. Real GUI
playback rendered frames and opened an audio output without a warning; its first
frame arrived in about 204 ms. A GUI export completed and its output decoded
without errors. The two application captures were visually inspected at 1200x850.
All GUI processes exited after capture.

CPU Whisper and Silero transcribed the synthetic speech exactly as
"This is a test of local speech recognition." The process ran as the ordinary
test user in an isolated Linux network namespace with only the loopback interface.
No download or network access was available to the transcription process.

The final materials contain all reviewed native inputs and notices. All 340 files
in the NOH source archive match the Git objects at the build commit. All 486 Rust
crate notice records match the previously approved complete graph, and the native
Rust/compiler supplements are the inputs approved in
[LINUX_REDISTRIBUTION.md](LINUX_REDISTRIBUTION.md). The final Whisper receipt's
41 compiler commands satisfy the reviewed x86-64/SSE2 CPU baseline.

## Tested environment and limits

Local execution used Ubuntu 24.04.5 x86_64, glibc 2.39, under WSL 2 / WSLg on an
Intel Core i7-4770S. Rendering used Mesa 25.2.8 llvmpipe with OpenGL 4.5, and audio
used the WSLg PulseAudio 17 RDPSink. The first playback exposed a host setup gap:
ALSA's default referenced a nonexistent hardware card. Installing Ubuntu's supplied
`99-pulseaudio-default.conf.example` as the test user's `.asoundrc` connected it to
the existing PulseAudio server. No package file changed; the repeated playback
then opened audio successfully. Linux desktops must supply a working default
audio device and graphics session.

This establishes the recorded Ubuntu/WSLg and hosted Ubuntu behavior. It does not
claim a freshly installed OS, physical Linux GPU testing, human listening
confirmation, or compatibility with every distribution or graphics driver.
The package is a complete AppDir ZIP, not an AppImage or distribution-specific
installer. It is unsigned, uses CPU speech, and has no automatic updates.
