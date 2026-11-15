# Changelog

User-visible changes are recorded here. Published versions and their downloads
are available on [GitHub Releases](https://github.com/Awalgawe/noh/releases).
This changelog starts with the initial public source version; earlier private
builds are not public releases.

## 0.1.6

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.1.6).

### Captions

- Draw captions with a GPU effect engine, in the preview and in the export.
  Six effects are built in: Classic (the default, which reproduces the former
  captions), Simple karaoke, Falling words, Sung sweep, Breath and Type block.
- Choose an effect and its settings in a style panel beside the monitor. The
  five effects other than Classic can draw with a font installed on the
  computer, chosen in a list that shows each font in its own type; a missing
  font is reported and replaced by the effect's own typeface.
- Wipe karaoke captions syllable by syllable, with syllables proposed from
  language packs.
- Report a caption that leaves the safe area or that is too wide, instead of
  cutting a word between letters.
- Choose a caption effect from the command line and from the MCP server.
- Without OpenGL 3.3, Classic falls back to the former FFmpeg captions and
  the other effects are refused with a message.

### Lyrics and subtitles

- Edit the subtitles in the lyrics lane, place the words of a line by hand,
  and keep Whisper's word times beside the subtitles.
- Keep every cue in a lyrics file beside the `.srt`. Edits are written as they
  are made; NOH asks once before modifying an `.srt`, and asks which file is
  the reference when the two differ. Read-only files are left untouched.
- Choose the Whisper model, with an automatic default that prefers the most
  accurate model present. The lyrics remember which model produced them.
- Keep the lyrics when Whisper's last segment runs past the end of the song.

### Export and playback

- Encode composed project exports and caption burns with NVENC when available.
- Use one colour matrix per export, read untagged video as players do, and
  show a colour matrix conflict in the simple view.
- Move the playhead only from the time ruler.
- On Linux with FFmpeg 6.1, say when FFmpeg draws caption colours in the
  older standard.

### Interface

- Choose the interface theme in Options: system, dark or light.

### MCP

- Serve tool schemas without the `uint` formats that JSON Schema does not
  define.

### Dependencies

- Update the dependencies, which clears
  five vulnerabilities and two unsound advisories reported by the RustSec
  audit. rmcp moves to 3.10.0 and ZIP to 9.5.1.

### Linux desktops tested

- openSUSE Tumbleweed is now tested with KDE on Wayland: its main repository
  no longer has the package of the KDE X11 session. KDE on X11 is no longer tested; X11 remains tested with
  GNOME on Debian 13 and Cinnamon on Mint 22.3.

### Known issues

- On Debian 13 with GNOME, in 3 of 23 automated test runs made while
  preparing this release, the preview's picture stopped updating after about
  one second. In the one occurrence that was recorded in detail, the player
  reported the end of the clip at that moment. A possible cause is being
  investigated: the preview ending when its audio queue is momentarily empty;
  the same condition is already in 0.1.5. It is not known whether a real computer
  is affected.
- In the desktop test run of this release, Debian 13 passed on a second
  attempt after that failure, and Mint 22.3 passed on a second attempt after
  its first one timed out downloading its test image, before any test started.
- The automated tests use software rendering and virtual audio. No real
  graphics card, sound card, NVENC or CUDA device was tested.

## 0.1.5 — 2026-10-17, Windows packages added 2026-10-18

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.1.5).
The original Linux archives and tag are preserved. The Windows build has its
own recorded commit, differing only in release documentation and policy;
each platform's materials archive contains its actual compiled sources.

- Fix native Wayland picture, video and WAV drops, while preserving clipboard
  integration, cancellation, path validation and imports through the file chooser.
- Update ZIP to 9.0.0 for its upstream security fixes. Preserve Windows archive
  member and traversal validation.
- Include Windows Minimal, Standard, Complete and Web installers, the complete
  portable archive and corresponding sources for version 0.1.5.
- Correct the installation guide for the host-owned Linux media runtime and
  link the seven-desktop qualification results.

## 0.1.4 — Linux package

The [published Linux release](https://github.com/Awalgawe/noh/releases/tag/v0.1.4)
contains the host-runtime packaging correction below.

- Linux packages now require host FFmpeg and libmpv, and ship no shared
  libraries. This keeps the media, graphics and audio stack with the host's
  drivers/plugins and removes the forced ALSA plugin directory. CPU speech
  remains included. Install the declared host dependencies and extract into a
  fresh directory. The [candidate report](LINUX_0_1_4_CANDIDATE.md) records the
  seven desktop results, exact files and independent-review boundary.
- Find the Linux distribution's FFmpeg when launched from `/`, `/usr` or
  `/usr/bin`, while continuing to reject implicit working-directory lookup.
- Keep Linux preview audio active while ALSA recovers from a temporary buffer
  underrun, instead of permanently switching to silent preview. Device loss,
  recovery failures and a stalled audio clock still trigger silent fallback.
- Verify the installed-dependency contract in final archives and retain the
  linked Linux installation guide. Test one sealed candidate across the
  [Linux validation matrix](LINUX_VALIDATION.md), with separate desktop gates.
- Update the locked Rustls dependency to 0.23.46 for the four security fixes
  described in its [upstream release](https://github.com/rustls/rustls/releases/tag/v%2F0.23.46).

## 0.1.3 — 2026-10-14

Download the [Windows installers and Windows/Linux portable ZIPs](https://github.com/Awalgawe/noh/releases/tag/v0.1.3).
Both application packages come from the same CI build; Windows installers reuse
its application bytes. See the [Linux qualification report](LINUX_0_1_3_QUALIFICATION.md)
for actual desktop checks and the Ubuntu/WSLg limits.

- Treat cancelling the native file chooser as cancellation, without a false error.
- Keep the sung-language choices open inside the lyrics menu.
- Open Linux media, folders and SRT files with Linux default applications under
  WSL as well, and keep external launch waits off the interface thread.
- Translate opening confirmations and export/preview cancellation in all seven
  interface languages.
- Show export success and its Open buttons after a previous refused file drop.
- Document the Linux host file chooser and default-application prerequisites,
  including the Windows Explorer-to-WSLg drop limitation.

## Linux package addition — 2026-10-13

The [0.1.2 release](https://github.com/Awalgawe/noh/releases/tag/v0.1.2) now also
provides a complete Linux x86_64 AppDir ZIP with media tools and CPU transcription
models. Its CI-built bytes were validated on the recorded Ubuntu 24.04/WSLg
baseline, with software graphics and PulseAudio. See the
[qualification report](LINUX_0_1_2_QUALIFICATION.md) for limits and exact hashes.
Existing Windows assets are unchanged. The Release workflow selects Windows and
Linux together by default and retains individual platform targets.

## 0.1.2 — 2026-10-10

Download the [Windows installers and portable ZIP](https://github.com/Awalgawe/noh/releases/tag/v0.1.2).

- Use one Windows web installation wizard: embed the small application payload
  and download only the selected optional components, without launching another
  NOH installer.
- Show cancellable verification progress for large component archives and existing
  content. Display the temporary location before installation and clean owned
  downloads/extracted files on success, failure or normal cancellation.
- Prevent installation after cancelling extraction, and retain the existing
  content profile when rerunning Web setup.

## 0.1.1 — 2026-10-09

Download the [Windows installers and portable ZIP](https://github.com/Awalgawe/noh/releases/tag/v0.1.1).

- Restore Minimal, Standard and Complete Windows installers and a small web selector.
- Offer missing media tools and transcription downloads during setup and from
  NOH's resource settings. Existing installations can add or repair content
  without uninstalling, while preserving user files and already installed content.
- Keep downloads outside the GUI thread, check exact archive hashes and retain
  the seven interface languages. Restart NOH to initialize newly added preview support.
- Consolidate CI around PR validation and Release; remove obsolete measurement
  and private installer workflows. Package installers from an identified successful
  Release build without compiling the application again.

## 0.1.0 — 2026-10-09

The first public Windows x64 release, with an installer and a portable ZIP, is available from
[GitHub Releases](https://github.com/Awalgawe/noh/releases/tag/v0.1.0).

### Added

- A desktop workflow for ordering video clips and PNG/JPEG photos, repeating the
  sequence to match a WAV soundtrack, previewing the result and exporting a video.
- Waveform range selection and vertical short exports with fit or crop controls.
- SRT lyrics/captions and local transcription with the bundled speech runtime;
  generated text and timing can be reviewed before rendering it into a video.
- Background media processing, cancellation and protection against overwriting
  existing files.
- English, French, German, Spanish, Japanese, Korean and Simplified Chinese interfaces.
- A command-line interface and local MCP server sharing the desktop's media core.
- Public GPL-3.0-only sources, contribution guidance and private security reporting.

### Distribution

- Native CI validation on Windows x64, Linux, macOS Apple Silicon and Intel Mac.
- PR checks that select affected platforms and avoid native builds for documentation.
- A separate manual pipeline for package verification, candidate builds and release drafts.
- A Windows x64 portable package with bundled media tools and local speech models.
- A per-user Windows installer, Start-menu shortcut and uninstaller, built and
  tested through the standard Release pipeline from the qualified portable bytes.
- An optional direct Microsoft prerequisite download in the installation wizard.
- A direct Microsoft download action when the optional transcription prerequisite is missing.
- Corresponding native/Rust source archives, build recipes and original licence notices.

### Known limitations

- macOS and Linux packages are not available; these targets require source builds.
- macOS distribution signing and notarization are not complete.
- Automatic update installation is disabled in ordinary builds.
- The desktop does not restore the editing session after closing.
- Verified HDR tone mapping and end-to-end color management are not provided.

See the [desktop guide](APP.md) for supported workflows and the
[release readiness guide](RELEASING.md) for packaging status.
