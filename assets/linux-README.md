# NOH for Linux x86_64

Extract the complete archive, keep its folders together, and run
`NOH.AppDir/AppRun`. The folder can be moved, including to paths with spaces.
An x86_64 desktop with glibc 2.39 or newer is required (Ubuntu 24.04 baseline).
The distribution supplies FFmpeg, libmpv and the complete X11/Wayland,
graphics and audio stack. NOH ships no shared libraries. CPU Whisper Small
and Silero VAD are included; transcription works offline. Automatic updates
are disabled. This package requires installed host dependencies.

On Ubuntu 24.04, install the media and GUI requirements:

```sh
sudo apt-get install --no-install-recommends ffmpeg libmpv2 libasound2t64 \
  libegl1 libgl1 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
  libxkbcommon0 libxkbcommon-x11-0 libxcursor1 libxrandr2 libxi6 libxinerama1 libstdc++6
```

On Fedora 44, use `mpv-libs`, `pipewire-alsa`, the desktop graphics packages,
and an FFmpeg provider with the `libx264`, `aac` and `pcm_s16le` encoders.
Fedora's `ffmpeg-free` alone lacks `libx264`; a distribution-compatible full
codec provider is required for export (for example RPM Fusion's free FFmpeg
packages). Installing a third-party repository is a system administration
choice; NOH does not enable repositories or install system packages itself.
The exact dependency contract is `usr/lib/noh/HOST-REQUIREMENTS.json`.

The desktop must provide an XDG Desktop Portal file chooser or `zenity`.
Opening exported videos, folders and SRT lyrics uses `xdg-open` with Linux
default applications, including under WSL. Associate `.srt` with a text editor.
For a minimal Ubuntu desktop, install the missing host components, for example:

```sh
sudo apt-get install --no-install-recommends zenity xdg-utils pcmanfm mpv mousepad
```

Those host packages need administrator access; running the extracted NOH folder
does not. In WSLg, use the Linux file chooser or a Linux file manager to import
files. Windows files are accessible under `/mnt/c`; dragging from Windows
Explorer directly into WSLg is not supported.

CLI: `NOH.AppDir/usr/lib/noh/bin/noh-cli`
MCP: `NOH.AppDir/usr/lib/noh/bin/noh-mcp`
Guides and original notices: `NOH.AppDir/usr/lib/noh/docs` and `licenses`.
`manifest.json` covers every packaged file; `ELF-IMPORTS.json` records native
imports. `HOST-REQUIREMENTS.json` declares the external runtime. Sources and build
materials are supplied in the separate materials archive.

Remove the extracted folder to uninstall NOH.
This does not delete media or settings in your user profile.

Install into a new directory: do not copy this package over the older 0.1.3
AppDir, whose private libraries and launcher used a different runtime model.
The packager removes private ELF search paths. The system package manager owns
FFmpeg/libmpv and their notices, sources and security updates.
