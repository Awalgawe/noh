> Historical notes for the initial Linux publication. The current
> [Windows and Linux release notes](RELEASE_0_1_5.md) supersede the download scope
> below. Existing Linux archives and the tag remain unchanged.

## Download NOH 0.1.5 for Linux

**[Linux x64 portable ZIP](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-linux-x64.zip) (484.8 MB)** includes the GUI, CLI, MCP server and offline CPU transcription models.

This release fixes native Wayland file drops reported in [issue 25](https://github.com/Awalgawe/noh/issues/25). **[Windows installers remain available in 0.1.3](https://github.com/Awalgawe/noh/releases/tag/v0.1.3).**

### Changes

- Pictures, videos and WAV files can be dragged into NOH from native Wayland applications. The import preview, cancellation and batch validation are preserved.
- Wayland clipboard integration is preserved alongside file drops, including paths with spaces and accented characters.
- The ZIP dependency is updated to 9.0.0 to address upstream security advisories. Windows update/archive path checks are preserved and covered by CI.

### Installation and upgrade

An x86_64 Linux desktop with glibc 2.39 or newer is required. Install the distribution's FFmpeg and libmpv packages before launching NOH. FFmpeg must provide `libx264`, `aac` and `pcm_s16le`; libmpv must provide `libmpv.so.2`. Graphics, audio and desktop libraries come from the host. CPU Whisper Small and Silero VAD are included for offline transcription.

Follow the [Linux package README](https://github.com/Awalgawe/noh/blob/1e2bbbd94c578115d45a750ab61493f7a1e48cae/assets/linux-README.md) for prerequisites and desktop setup. Those instructions are also included at `NOH.AppDir/usr/lib/noh/assets/linux-README.md` and take precedence over the older general installation guide.

Extract the complete ZIP into a fresh directory, preserve executable permissions, and launch `./NOH.AppDir/AppRun`. Verify downloads against `SHA256SUMS`. In particular, do not overlay an older AppDir containing private shared libraries.

### Validation

The archives are the unchanged files from [Release run 39036065018, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39036065018), built from `1e2bbbd94c578115d45a750ab61493f7a1e48cae`. The `v0.1.5` tag identifies that compiled source commit. The fix was integrated through [PR 26](https://github.com/Awalgawe/noh/pull/26) after required CI passed on Linux, Windows and both macOS architectures.

The candidate passed 477 Cargo tests, with 12 explicit ignores, six distribution container cases and seven desktop cases: Ubuntu 24.04 and 26.04 GNOME, Debian 13 GNOME, Fedora 44 GNOME, openSUSE Tumbleweed KDE, Arch Linux KDE, and Linux Mint 22.3 Cinnamon. The four Wayland desktops passed actual PNG/MP4/WAV file-drop checks. All seven passed chooser/cancellation/import, preview video and recorded application audio, export/decode/no-overwrite, offline transcription and exported-file opening checks.

The [candidate report](https://github.com/Awalgawe/noh/blob/main/docs/LINUX_0_1_5_CANDIDATE.md) records exact archive hashes, native desktop observations and the independent review requirement. Desktop evidence uses QEMU virtual graphics and audio; it does not establish physical-device compatibility or human listening acceptance. Cross-protocol XWayland drag-and-drop bridges are outside the qualification scope. Packages remain unsigned and automatic updates are disabled.

### Sources and checksums

`NOH-0.1.5-linux-x64-materials.zip` contains corresponding application and third-party sources, notices and build materials. `linux-x64-DELIVERY.json` preserves the original CI inventory and provenance; its candidate status is superseded only within the recorded exact-byte qualification scope. `SHA256SUMS` covers all three accompanying files.

NOH is GPL-3.0-only. Included third-party components retain their own licences and notices.
