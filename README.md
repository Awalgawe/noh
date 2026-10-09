<p align="center">
  <img src="assets/noh-icon.png" width="88" alt="NOH">
</p>

<h1 align="center">Your clips and photos, set to music.</h1>

<p align="center">
  Arrange your media, add a song, preview the result and export a video.<br>
  NOH repeats your sequence to match the soundtrack, on one desktop screen.
</p>

<p align="center">
  <a href="#download-and-install">Download &amp; install</a> ·
  <a href="docs/APP.md">First video</a> ·
  <a href="https://github.com/Awalgawe/noh/releases">Releases</a> ·
  <a href="docs/CHANGELOG.md">Changelog</a> ·
  <a href="docs/BUILDING.md">Build from source</a>
</p>

![NOH showing a two-clip project, a soundtrack waveform and the export controls](docs/images/noh-desktop.png)

<p align="center"><sub>The Windows app with an illustrative project. Demo media was created for this screenshot.</sub></p>

## From a folder of media to a video

1. **Add your clips and photos.** Put them in the order you want.
2. **Drop in a WAV soundtrack.** Preview how the sequence fits the music.
3. **Export the video.** Or select a moment on the waveform for a vertical short.

### A few useful things, kept together

- **See and hear the result** in the built-in preview before exporting.
- **Add lyrics or captions** from an SRT file, or generate a local transcript and review it.
- **Keep compatible footage intact.** Video is copied without recompression where the composition allows it.
- **Work locally.** Your media is processed on your computer. The interface supports English, French, German, Spanish, Japanese, Korean and Simplified Chinese.

NOH is focused on repeating visual sequences, music and short extracts. It is not a general-purpose multitrack editor.

## Download and install

[GitHub Releases](https://github.com/Awalgawe/noh/releases) is the download page
for published versions, release notes and package checksums. **No ready-to-use
application release is available yet.** The [changelog](docs/CHANGELOG.md) tracks
the first public version in preparation.

Windows x64 and macOS Apple Silicon portable packages are being prepared and
validated. Linux and Intel Mac currently require a [source build](docs/BUILDING.md).
Source builds and native CI tests pass on all four targets; installable packages
still need their own validation. macOS signing and notarization remain incomplete.
Use each published release's notes for its supported systems and minimum versions.

Already have a complete portable build?

- **Windows:** right-click the ZIP, choose **Extract All**, then open `noh.exe`
  inside the extracted folder. Keep its `bin`, `docs` and `licenses` folders together.
- **Mac:** extract the portable ZIP, move `NOH.app` to **Applications**, then open it.
  The current development build is not yet approved for ordinary macOS distribution.

Complete portable builds include the media tools and speech models. Windows local
transcription requires the Microsoft Visual C++ v14 x64 runtime supplied by Microsoft. Follow the [installation guide](docs/INSTALLING.md)
for setup, updates and removal, then [make your first video](docs/APP.md#make-your-first-video).
Developers can [build from source](docs/BUILDING.md).

## Learn more

[Desktop guide](docs/APP.md) · [Command line](docs/CLI.md) ·
[Local MCP server](docs/MCP.md) · [Development](docs/DEVELOPMENT.md)

[Contributing](CONTRIBUTING.md) · [Security reports](SECURITY.md)

NOH's code is licensed under [GNU GPLv3](LICENSE) (`GPL-3.0-only`). Fonts and
third-party components retain their own licenses; see [credits and notices](docs/THIRD-PARTY.md).
