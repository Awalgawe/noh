## Download NOH 0.1.5 for Windows and Linux

This release fixes native Wayland file drops reported in [issue 25](https://github.com/Awalgawe/noh/issues/25). Windows packages have now been added to the same release; the original Linux downloads and tag are unchanged.

### Windows x64

**[Web installer](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-windows-x64-Web-Setup.exe)** is the recommended download. It installs NOH and downloads the components for your chosen profile. Standard is the default.

| Offline installer | Included content |
| --- | --- |
| [Standard](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-windows-x64-Standard-Setup.exe) | NOH, media tools and preview support. |
| [Complete](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-windows-x64-Complete-Setup.exe) | Standard plus local automatic transcription and models. |
| [Minimal](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-windows-x64-Minimal-Setup.exe) | NOH only; optional components can be added later. |

The **[complete portable ZIP](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-windows-x64.zip)** needs no installation: extract it entirely and open `noh.exe`. For an existing installation, run the new installer; the tested 0.1.3 Complete upgrade preserves optional components and user-created files, including when using Minimal.

Automatic transcription requires Microsoft's v14 x64 runtime. If missing, the installer offers a download directly from Microsoft. Preview, export and existing subtitles work without it. Windows packages are unsigned; no Authenticode or SmartScreen reputation is claimed.

### Linux x86_64

**[Linux portable ZIP](https://github.com/Awalgawe/noh/releases/download/v0.1.5/NOH-0.1.5-linux-x64.zip)** includes the GUI, CLI, MCP server and offline CPU transcription models. Use a desktop with glibc 2.39 or newer and install its FFmpeg and libmpv packages first. FFmpeg must provide `libx264`, `aac` and `pcm_s16le`; libmpv must provide `libmpv.so.2`.

Extract the complete archive into a fresh directory, preserve executable permissions and run `./NOH.AppDir/AppRun`. Do not overlay an older AppDir containing private shared libraries. See the [installation guide](https://github.com/Awalgawe/noh/blob/main/docs/INSTALLING.md#linux-x86_64) for distribution packages and desktop setup.

### Changes

- Import pictures, videos and WAV files dropped from native Wayland applications. Clipboard integration, paths with spaces and accents, cancellation and batch validation are preserved.
- Update ZIP to 9.0.0 for upstream security fixes, preserving Windows archive-member and traversal checks.
- Provide Windows Minimal, Standard, Complete and Web installers and the complete portable package for 0.1.5.

### Validation and provenance

The published `v0.1.5` tag remains at `1e2bbbd94c578115d45a750ab61493f7a1e48cae`, the Linux compiled source from [run 39036065018, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39036065018). The unchanged Linux candidate passed 477 Cargo tests, six container cases and seven QEMU desktop cases, including actual file drops on four native Wayland desktops. [Linux results and limits](https://github.com/Awalgawe/noh/blob/main/docs/LINUX_0_1_5_CANDIDATE.md).

Windows was compiled from `a7a3e2cc0a4c337ed88699b0573f1689d0a5b1cc` in [run 39082899722, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39082899722). Its diff from the tag contains only release documentation, evidence and delivery-policy records. Its corresponding-source archive contains that actual Windows revision. The exact package passed 482 Cargo tests, native preview/audio-device checks, CPU/CUDA transcription, and 84 GUI capture cases. [Windows results and limits](https://github.com/Awalgawe/noh/blob/main/docs/WINDOWS_0_1_5_QUALIFICATION.md).

Installers wrap those same Windows application bytes, from [run 39084998885, attempt 1](https://github.com/Awalgawe/noh/actions/runs/39084998885), also at `a7a3e2cc0a4c337ed88699b0573f1689d0a5b1cc`. All three profiles passed local GUI/CLI and removal checks on Windows 10 x64 22H2, build 19045, plus the 0.1.3 upgrade check. Hosted install/reinstall/addition/repair/rejection checks passed; hosted GUI was unavailable because that runner lacks OpenGL 2.0. [Installer evidence](https://github.com/Awalgawe/noh/blob/main/docs/WINDOWS_0_1_5_INSTALLERS.md).

Windows checks used an existing host with the Microsoft runtime already installed, not a fresh OS. Linux desktops used virtual graphics/audio. No human listening, universal hardware compatibility, cross-protocol XWayland bridge, or fresh Microsoft prerequisite installation is claimed. Automatic updates remain disabled. macOS requires a source build.

### Checksums and sources

Verify downloads against **[SHA256SUMS](https://github.com/Awalgawe/noh/releases/download/v0.1.5/SHA256SUMS)**. Files named `-materials.zip` and `-installer-sources.zip` contain corresponding sources, original notices and build materials; they are not application downloads. The `windows-x64-DELIVERY.json` and `linux-x64-DELIVERY.json` files preserve original CI inventories; exact-byte qualification is recorded separately in the linked reports. `INSTALLER-PROVENANCE.json` records both platform revisions and wrapper identities, and `COMPONENTS.json` identifies the downloadable runtime archives.

NOH is GPL-3.0-only. Included third-party components retain their own licences and notices.
