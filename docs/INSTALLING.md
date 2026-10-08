# Download and install NOH

**Public downloads are not available yet.** The instructions below apply if you
have received a complete portable build. A supported download will need a
published application package and release notes identifying compatible systems.

## Get the application

When downloads are available, choose the application ZIP for your operating
system from the release's **Assets** list. GitHub's **Source code** archives and
**Code → Download ZIP** contain source files; they do not contain a ready-to-run app.
There is no public `.msi`, setup wizard or `.dmg` download. The Windows setup
wizard described below is a private candidate.

| Computer | Current package status |
| --- | --- |
| Windows with an Intel or AMD 64-bit processor | A complete portable build has been tested locally; no public release yet. |
| Mac with Apple silicon | A portable development build has been tested locally. The current bundle requires macOS 26.6.2 or newer and is not notarized. |
| Intel Mac, Windows on Arm, Linux | No qualified ready-to-use download. |

Complete portable builds contain their media tools and local speech models.
They do not require Rust, Python or a separate FFmpeg installation. Allow space
for both the ZIP and its extracted contents, plus your source media and exports.
Exact download and installed sizes belong in each release's notes.

## Windows

### Installer profiles (private candidate)

The private Windows installer offers three content profiles from the
same application build. Public downloads are still unavailable.

| Profile | Included content |
| --- | --- |
| Minimal | NOH, CLI and MCP. Configure external media tools in Options; preview/export/transcription require their respective resources. |
| Standard (recommended) | NOH, FFmpeg and the complete preview runtime. Attach existing subtitles; generated transcription is unavailable without separately configured speech resources. |
| Complete | Standard plus the qualified speech runtime and models for offline transcription. |

The web installer downloads one complete signed profile package. An offline
installer embeds the same package and needs no network for first installation.
The private web trial requires runtime GitHub access; no access token is embedded
in the installer. The private trial does not establish public download support.

Installation is per user in `%LOCALAPPDATA%\NOH`, without elevation. Updates and
repairs preserve the chosen profile. An existing installation cannot be silently
switched to another profile. Reopening the installer repairs that profile, including
when the application folder is missing; an older installer refuses a downgrade.
Downloading can be cancelled. Once application installation begins, wait for it to
finish. Failure shows an error and keeps a log under `NOH\installer-logs`.
The application owns the uninstall entry; the outer wizard adds no second uninstaller.

Uninstalling removes the application, its registration and its Start menu shortcut.
It retains `%LOCALAPPDATA%\NOH\.noh-update` and `installer-logs`. Recovery state
includes the installation identity, executable recovery tools and downloaded
packages; its size depends on the profile and update history and can be substantial.
While that identity remains, reinstalling preserves the profile and version rules.
There is currently no automatic purge of this recovery history.

The portable ZIP instructions below describe the existing portable delivery.

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

For local transcription, install the **Microsoft Visual C++ v14 Redistributable
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

## Mac with Apple silicon

These steps describe the **self-contained portable** ZIP. A developer's local
Homebrew-dependent build is a different package; see the [build guide](BUILDING.md).

1. Double-click the ZIP in Finder to extract it.
2. Move **NOH.app** to **Applications**, or another folder where you keep apps.
3. Open **NOH.app** in Finder. Its required tools and models are inside the app.

The current development bundle is not Developer ID-signed or notarized. A
downloaded copy may be blocked by macOS; the ordinary download-and-open path
has not been qualified. A public Mac release must resolve this before being
presented as ready to install. These instructions do not require disabling macOS
security settings. Apple's [installation guide](https://support.apple.com/guide/mac-help/install-and-uninstall-other-apps-mh35835/mac)
explains how apps from outside the App Store are handled.

## First launch

NOH follows your system language when supported. Use the sliders button in the
upper-right corner to choose another language.

1. Add a short video or a PNG/JPEG photo.
2. Add a WAV soundtrack.
3. Press **Play**, choose a destination, then **Export the video**.

See the [desktop guide](APP.md#make-your-first-video) for arranging media,
captions and vertical shorts. Keep the original media in place while working.
The editing session is not automatically restored after closing the app.

## Update or remove NOH

Automatic update installation is disabled in ordinary builds. To update a
portable copy, close NOH and extract the complete new package into a new folder.
Check that it opens before removing the previous copy. Keep any media or exports
you saved inside the old folder before deleting it.

To remove NOH, close it and delete its portable folder on Windows, or move
`NOH.app` to the Bin on Mac. Media and exports saved elsewhere remain in place.
The small language-preference file is stored separately and is not removed with
the application.

## If it does not open

- **A DLL or media tool is missing:** extract the entire ZIP again and keep all
  bundled files together. Do not download individual replacement DLLs from an
  unrelated website.
- **Only source files are present:** you downloaded a source archive. An
  application package contains `noh.exe` or `NOH.app`.
- **The system blocks the app:** check the package's release notes and reported
  platform support. The current Mac development bundle is not a supported public
  download.
- **The app opens but export is unavailable:** follow the message in the bottom
  bar or the [desktop troubleshooting guide](APP.md#if-something-does-not-work).

## If a feature is unavailable

NOH checks media tools and speech files in the background. Missing tools,
models or runtime libraries appear in a warning with **Resolve missing
resources**. Open it to see the affected feature, expected file path and repair
instructions. Restore the complete package, or select the tool/model paths in
Options, then press **Check again**. Restart NOH after restoring the fast-preview
library. On Windows, transcription diagnostics also link to Microsoft's official
Visual C++ runtime information.

Features with available dependencies remain usable. For example, a missing speech
model prevents transcription; an existing SRT can still be attached. These checks
verify file availability and tool startup, not the contents of a model or every
possible media operation. An operation that fails later reports its error.
