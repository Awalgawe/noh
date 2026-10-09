# NOH

1. If this folder is still inside a ZIP, right-click the ZIP and choose **Extract All**.
2. Open the extracted folder and double-click **noh.exe**.
3. Add a clip or photo and a WAV soundtrack, preview, then export.

The application includes its media and subtitle tools. On Windows, local
transcription also needs the **Microsoft Visual C++ v14 Redistributable (x64)**.
Install it once from [Microsoft](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist)
if it is missing. NOH does not include Microsoft's installer or runtime DLLs.
The tested runtime is 14.51.36247.0; use that version or a newer compatible v14
runtime. Playback, export and existing SRT subtitles do not need this speech
prerequisite. Once the runtime is installed, transcription works offline.
See the [installation guide](docs/INSTALLING.md) for moving, updating and removing NOH.

- **bin/** — media engine, local speech runtime and models, command-line tools.
- **docs/** — [user guide](docs/APP.md), [CLI guide](docs/CLI.md) and technical documentation.
- **licenses/** — bundled components' licenses and font notices.
- **manifest.json** — build identity and checksums for the complete portable folder.

Keep these folders together when moving or copying NOH.

NOH's code is licensed under GNU GPLv3 (`GPL-3.0-only`); the full text is in
`licenses/NOH-LICENSE.txt`. Third-party components retain their own licenses.
The separate Whisper speech tool's NVIDIA CUDA files remain under the original
CUDA terms in `licenses/`; NOH's GPL license does not relicense them. Preserve
those terms when redistributing the complete package. Corresponding sources
and build materials are provided beside the binaries on
[GitHub Releases](https://github.com/Awalgawe/noh/releases).

For command-line use, open a terminal and run `bin/noh-cli.exe --help`.
The optional MCP server is `bin/noh-mcp.exe`; see [MCP setup](docs/MCP.md).
