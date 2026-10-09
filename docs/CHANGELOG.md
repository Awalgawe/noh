# Changelog

User-visible changes are recorded here. Published versions and their downloads
are available on [GitHub Releases](https://github.com/Awalgawe/noh/releases).
This changelog starts with the initial public source version; earlier private
builds are not public releases.

## 0.1.1 — Unreleased

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
