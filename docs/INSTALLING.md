# Download and install NOH

Download NOH for Windows x64 from the
[latest GitHub release](https://github.com/Awalgawe/noh/releases/latest).
Choose **`NOH-<version>-windows-x64-Setup.exe`** for guided installation, or
`NOH-<version>-windows-x64.zip` for a portable copy. The separate `-materials.zip`,
`-installer-sources.zip` and GitHub's **Source code** downloads contain sources and build materials.

Read the release notes for tested Windows versions and package checksums.
Both Windows downloads include the application, media
libraries and local speech models. It does not require Rust, Python or a
separate FFmpeg installation. Automatic updating is not enabled.
Allow space for the download and installed files, plus your media/exports.

macOS and Linux currently require a [source build](BUILDING.md). The experimental
Mac portable is not signed or notarized and is not offered as a supported download.

## Windows

### Guided installation

1. Open **NOH-<version>-windows-x64-Setup.exe** and choose your language.
2. Keep the proposed installation folder or choose another writable folder.
3. If the Microsoft component for automatic subtitles is missing, the wizard
   offers to download it. Microsoft's installer presents its own licence and may
   request administrator approval. You can leave this optional component for later.
4. Finish installation, then open **NOH** from the Start menu. A desktop shortcut
   is optional.

NOH installs for your Windows account without requiring administrator rights.
It includes the same application, media tools and speech models as the portable ZIP.
The optional Microsoft download is separate; it is skipped when a compatible
runtime is already present. NOH itself does not need an Internet connection for editing.

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

For an installed copy, close NOH and run the new release's installer. To remove
it, open Windows **Settings > Apps**, select **NOH**, then choose **Uninstall**.
User-created files in the installation folder are preserved; keep your media
and exports in your own folders.

To remove the portable copy, close NOH and delete its folder. Media and exports saved elsewhere remain in place.
The small language-preference file is stored separately and is not removed with
the application.

## If it does not open

- **A DLL or media tool is missing:** extract the entire ZIP again and keep all
  bundled files together. Do not download individual replacement DLLs from an
  unrelated website.
- **Only source files are present:** you downloaded a source archive. The Windows
  application package contains `noh.exe`.
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
