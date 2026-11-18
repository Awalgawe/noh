# Download and install NOH

Download NOH for Windows or Linux x86_64 from the
[latest GitHub release](https://github.com/Awalgawe/noh/releases/latest).
For Windows, choose **`NOH-<version>-windows-x64-Web-Setup.exe`** for guided installation.
It includes NOH and downloads only the optional components for your chosen
profile. Download, verification and installation stay in the same wizard.

| Profile | Content | Offline installer filename |
| --- | --- | --- |
| Standard | NOH, media tools and preview for editing and exporting. | `NOH-<version>-windows-x64-Standard-Setup.exe` |
| Complete | Standard plus automatic transcription and its local models. | `NOH-<version>-windows-x64-Complete-Setup.exe` |
| Minimal | NOH only, with optional content available during setup or later. | `NOH-<version>-windows-x64-Minimal-Setup.exe` |

An offline installer includes its profile's content. Selecting more content needs
an Internet connection. Minimal needs media tools before preview and export are
available. Neither profile installation nor component downloads require Rust or Python.

For a complete portable copy, choose `NOH-<version>-windows-x64.zip`. The separate
`-materials.zip`, `-installer-sources.zip` and GitHub's **Source code** downloads
contain sources and build materials. The media/speech archives are downloaded
automatically by the installers; you do not need to extract them manually.

Read the release notes for tested Windows versions and package checksums.
Automatic updating is not enabled. Allow space for the download, temporary
extraction and installed files, plus your media and exports.

macOS currently requires a [source build](BUILDING.md). The experimental
Mac portable is not signed or notarized and is not offered as a supported download.

## Windows

### Guided installation

1. Open the web installer or your chosen offline installer and select your language.
2. Choose **Standard** for editing, **Complete** for automatic transcription too,
   or **Minimal** to add these components later.
3. Keep the proposed installation folder or choose another writable folder.
4. For Complete, if the Microsoft prerequisite is missing, the wizard offers its
   official download. Microsoft's installer presents its own licence and may
   request administrator approval. You can leave this optional step for later.
5. Finish installation, then open **NOH** from the Start menu. A desktop shortcut
   is optional.

NOH installs for your Windows account without requiring administrator rights.
Download, verification and extraction progress are shown and can be cancelled.
NOH verifies downloaded content before installing it. The confirmation page shows
the temporary folder used for downloads and extraction; the wizard removes its
own temporary files after completion, failure or normal cancellation. Archives
you placed beside the installer yourself are kept. Once the required components
are installed, editing and local transcription work without an Internet connection.

### Add or repair components later

In an installed copy of NOH, choose **Resolve missing resources**, then
**Add or repair components**. The wizard offers the missing media tools or
transcription content. Choose the desired profile and follow its download and
installation progress. You can also rerun an offline or web installer for the
same release to add content.

You do not need to uninstall NOH. Already installed content and user files are
preserved; selecting a smaller profile does not remove components. Failed downloads
leave the existing installation unchanged, so you can retry. Active-download
cancellation and subsequent retry were checked with the 0.1.3 Windows release;
see its notes for the tested environment and remaining limits.
Avoid starting this while editing a session you need to keep: the wizard may
need NOH closed to replace files. Restart NOH to enable newly added preview support.

The in-app component installer belongs to installed copies. To repair a portable
copy, extract its complete ZIP again, or select existing tool/model paths in Settings.

### Portable copy

1. Save the complete portable ZIP to your computer.
2. Right-click it and choose **Extract All**. Choose a folder you can write to,
   such as a NOH folder inside your user folder, and wait for extraction to finish.
3. Open the extracted folder and double-click **noh.exe**. Launch it from this
   folder, not from inside the ZIP.

Keep **bin**, **docs** and **licenses** beside `noh.exe`. Moving just the executable
leaves its required files behind. You can create a shortcut to `noh.exe` for
later launches. NOH's portable package has no installation wizard.

Microsoft describes the extraction steps in its
[ZIP guide](https://support.microsoft.com/en-us/windows/experience/storage-filemanagement/zip-and-unzip-files).

For local transcription in a portable copy, use **Download the Microsoft
component** in NOH's missing-components settings, open the downloaded installer,
then choose **Check again**. You can also install the **Microsoft Visual C++ v14 Redistributable
(x64)** from [Microsoft](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).
The package is tested with version **14.51.36247.0**; use this or a newer compatible
v14 runtime. Older versions are not qualified by this package. Microsoft's
installer may request administrator approval. If you cancel it, preview, export
and existing SRT subtitles remain available; install the prerequisite before
retrying generated transcription. Keep NOH closed while installing or repairing it.

NOH does not redistribute the Microsoft installer or its DLLs. A fresh computer
needs a one-time download and installation of this prerequisite. Once installed,
local transcription uses the included models without a network connection.
Do not download individual DLLs from third-party sites.

The Windows binary is not Authenticode-signed. Windows may show an unknown-publisher
or SmartScreen warning. Check that the download came from the official release
and verify its SHA-256 against `SHA256SUMS`; keep system security protections
enabled. Signing and download reputation are separate from the package checksums.

## Linux x86_64

1. Install your distribution's FFmpeg and libmpv packages. FFmpeg must provide
   `libx264`, `aac` and `pcm_s16le`; libmpv must provide `libmpv.so.2`.
2. Download `NOH-<version>-linux-x64.zip` from the official release and check its
   SHA-256 against **`SHA256SUMS`** in that release.
3. Extract the whole archive into a fresh directory with a tool that preserves
   Unix executable permissions. Do not overlay an older AppDir containing private
   shared libraries.
4. Open a terminal in the extracted folder and run `./NOH.AppDir/AppRun`.

Keep the complete `NOH.AppDir` folder together. It can be moved, including to a
path with spaces. The AppDir itself needs no administrator installation, Rust
or Python. CPU Whisper Small and Silero VAD are included for offline
transcription. Media tools, preview libraries, graphics and audio come from the
host. The separate `-materials.zip` contains corresponding sources and notices.

The baseline is Ubuntu 24.04 x86_64 with glibc 2.39. Follow the
[package README](../assets/linux-README.md) for distribution prerequisites;
the same instructions are included inside the archive. A compatible glibc
version alone does not establish desktop compatibility.

Your desktop must provide a working audio output and an XDG Desktop Portal file
chooser, or `zenity` on `PATH`. Minimal systems may lack both. On Ubuntu:

```sh
sudo apt-get update
sudo apt-get install --no-install-recommends ffmpeg libmpv2 zenity xdg-utils
```

Opening an exported video, its folder or a lyrics file uses `xdg-open` and the
host's default applications. Set a video player, file manager and text editor
for `.srt` files. For example, a minimal Ubuntu installation can use:

```sh
sudo apt-get install --no-install-recommends pcmanfm mpv mousepad
```

Choose the SRT editor through the file manager's **Open With** settings if lyrics
open in a video player. Saving a correction lets NOH reload lyrics for preview
and export.

Version 0.1.5 was tested on Ubuntu 24.04 and 26.04 GNOME, Debian 13 GNOME,
Fedora 44 GNOME, openSUSE Tumbleweed KDE, Arch Linux KDE and Linux Mint 22.3
Cinnamon. Tests covered chooser import, playback/audio, export and offline
transcription. Native Wayland file drops passed on the four Wayland desktops.
See the [candidate report](LINUX_0_1_5_CANDIDATE.md) for exact package identities
and environments. These are QEMU virtual graphics/audio results, not physical
hardware compatibility or human listening tests. General XWayland bridging is
outside the tested scope.

Linux 0.1.3 bundled libraries could conflict with Fedora's graphics and audio
stack. Version 0.1.5 uses the host runtime and must be extracted into a fresh
folder. In WSLg, Windows Explorer-to-Linux drag-and-drop remains unsupported
([Microsoft tracking issue](https://github.com/microsoft/wslg/issues/84)). Use
**Add files** instead; press **Ctrl+L** in the Linux chooser and enter
`/mnt/c/Users/` or `/mnt/d/` to browse Windows drives. WSLg may also require an
ALSA default-device mapping to its PulseAudio server.

The Linux package is unsigned and does not update itself; NOH only says when a
new version exists. To update, extract the new version into a new folder. To remove it, close NOH and delete
its extracted folder, first keeping any media or exports saved inside it.
Files elsewhere and separately stored preferences are retained.

## First launch

NOH follows your system language when supported. Use the sliders button in the
upper-right corner to choose another language.

1. Add a short video or a PNG/JPEG photo.
2. Add a WAV soundtrack.
3. Press **Play**, choose a destination, then **Export the video**.

See the [desktop guide](APP.md#make-your-first-video) for arranging media,
captions and vertical shorts. Keep the original media in place while working.
The editing session is not automatically restored after closing the app.

### A more accurate speech model

NOH ships Whisper Small. To transcribe lyrics with another whisper.cpp model, put
its `ggml-<size>.bin` file (for example `ggml-large-v3.bin` or
`ggml-medium.bin`) in the speech folder:

- Windows: `bin\speech` in the NOH folder, installed or portable;
- Linux: `NOH.AppDir/usr/lib/noh/bin/speech`.

Choose the model in **Settings**, with the Whisper menu on the **Lyrics** row:
**Automatic**, one of the models in the speech folder, or **Other file…**. The choice is kept
between sessions. The lyrics menu names the model it uses.

**Automatic** takes the most accurate model that runs at a usable speed on this
computer, in this order: large-v3, large-v3-turbo, large-v2, large-v1, large,
medium, small, base, tiny (an `.en` model after its multilingual one, a
quantized file such as `ggml-large-v3-q5_0.bin` after its full-size one). A
large model is taken only on Windows with an NVIDIA GPU whose memory holds the
model plus 2 GiB, or when no other model is installed. If a large model fails
there (for example, out of GPU memory because another program uses it), NOH
says so and transcribes again once with the next model.

A large model chosen by hand also works without such a GPU, but much more
slowly: the menu says so. On Linux transcription always runs on the CPU.

## Update or remove NOH

**Installed copy on Windows.** From the version that introduces it, NOH updates
itself. Once a day, and when you choose **Settings > Updates > Check now**, it asks
GitHub for the latest published release. When a newer one exists, NOH downloads
its Minimal installer and only the optional components whose files changed, and
checks the size and SHA-256 of each file against what GitHub declares for the
release. You can keep working meanwhile. Then either choose **Restart to update**,
or simply close NOH: the installation runs silently and keeps your content
profile, your settings and the files you added. Both switches in
**Settings > Updates** can turn this off. NOH does not install while another of
its programs runs from the installation folder (another NOH window, a
command-line export, the MCP server, FFmpeg or Whisper): close it first.

Copies installed before that version do not look for updates: install the new
release once by hand, as below; later versions then arrive by themselves.

The downloads are kept in `%LOCALAPPDATA%\noh\updates\<version>` until NOH starts
again in the new version. If an update fails, **Settings > Updates** says so and
opens the installation log. If NOH no longer starts, run the installer kept in
that folder, or the installer of the release page.

NOH reaches GitHub through Windows' `curl.exe`, which ignores the Windows proxy
settings: behind a proxy configured only there, checks fail and you update by
hand. The Windows installers are not Authenticode-signed (see above); the update
trusts GitHub's HTTPS and the digests it publishes.

**Portable copies and Linux** are only told that a new version exists, with a
link to its release page (on Linux, only when `/usr/bin/curl` is present). To
update a portable copy, close NOH and extract the complete new package into a new
folder. Check that it opens before removing the previous copy. Keep any media or
exports you saved inside the old folder before deleting it.

To update an installed copy by hand, close NOH and run the new release's
installer. To remove it, open Windows **Settings > Apps**, select **NOH**, then
choose **Uninstall**. User-created files in the installation folder are
preserved; keep your media and exports in your own folders. The update
preferences and downloads, like the language preference, are stored separately
and are not removed with the application.

To remove the portable copy, close NOH and delete its folder. Media and exports saved elsewhere remain in place.
The small language-preference file and `startup.log` are stored separately and
are not removed with the application.

## If it does not open

- **A DLL or media tool is missing:** for an installed copy, use **Add or repair
  components** if NOH opens, or rerun its matching installer if it does not. For
  a portable copy, extract the complete ZIP again and keep its files together.
  Do not download individual replacement DLLs from an unrelated website.
- **Only source files are present:** you downloaded a source archive. The Windows
  application package contains `noh.exe`.
- **The system blocks the app:** check the package's release notes and reported
  platform support. The current Mac development bundle is not a supported public
  download.
- **The app opens but export is unavailable:** follow the message in the bottom
  bar or the [desktop troubleshooting guide](APP.md#if-something-does-not-work).
- **Nothing appears, or an error message names the window:** NOH writes what
  happened to a small file, `startup.log`, each time it starts. The window needs
  OpenGL 2.0 or newer, and caption effects need OpenGL 3.3: check that the
  graphics card's own driver is installed, not a generic one. If that does not
  help, send the file with your report.

### The start-up file

`startup.log` is replaced at each start. It holds the NOH version, the system,
the time, each step of the start, the OpenGL vendor, renderer and version once
the window is open, and the error if the start failed. It is in the folder of
the language preference:

- Windows: `%APPDATA%\noh\startup.log`
- Linux: `$XDG_CONFIG_HOME/noh/startup.log`, usually `~/.config/noh/startup.log`
- macOS: `~/Library/Application Support/noh/startup.log`

When NOH closes at once without a message, the last line of the file is the
step it was on.

If there is no `startup.log`, or its time is that of an earlier start, NOH did
not reach its first step: either the system did not run it, or NOH could not
write in that folder. On Windows, the system or security software can refuse to
run a program ("Access is denied"): look in **Windows Security > Protection
history** for a block of `noh.exe`. NOH's files carry no Authenticode
signature, as stated under [Portable copy](#portable-copy); do not disable
security settings to get round a block.

## If a feature is unavailable

NOH checks media tools and speech files in the background. Missing tools,
models or runtime libraries appear in a warning with **Resolve missing
resources**. Open it to see the affected feature, expected file path and repair
instructions. In an installed copy, choose **Add or repair components**. For a portable copy,
restore the complete package or select the tool/model paths in Options. Then
press **Check again**. Restart NOH after restoring the fast-preview
library. On Windows, transcription diagnostics also link to Microsoft's official
Visual C++ runtime information.

Features with available dependencies remain usable. For example, a missing speech
model prevents transcription; an existing SRT can still be attached. These checks
verify file availability and tool startup, not the contents of a model or every
possible media operation. An operation that fails later reports its error.
