# NOH for Linux x86_64

Extract the complete archive, keep its folders together, and run
`NOH.AppDir/AppRun`. The folder can be moved, including to paths with spaces.
An x86_64 desktop with glibc 2.39 or newer is required (Ubuntu 24.04 baseline).
The desktop supplies its X11/Wayland session, graphics drivers, fonts and audio
server. FFmpeg, libmpv, their private libraries, CPU Whisper Small and Silero VAD
are included. Transcription works offline. Automatic updates are disabled.

CLI: `NOH.AppDir/usr/lib/noh/bin/noh-cli`
MCP: `NOH.AppDir/usr/lib/noh/bin/noh-mcp`
Guides and original notices: `NOH.AppDir/usr/lib/noh/docs` and `licenses`.
`manifest.json` covers every packaged file; `ELF-IMPORTS.json` records native
imports and the required host libraries. Corresponding sources and build
materials are supplied in the separate materials archive.

No administrator access is needed. Remove the extracted folder to uninstall.
This does not delete media or settings in your user profile.

This software is based in part on the work of the FreeType Team.
The original FreeType notice is in
`NOH.AppDir/usr/lib/noh/licenses/native/libfreetype6/copyright`.

NOH's packager modifies the Ubuntu native binaries' ELF library search paths
so they resolve the private libraries inside this folder (Linux packaging
introduced on 2026-10-13). `LINUX-INPUTS.json` retains original hashes and
`ELF-IMPORTS.json` records the relocated files. The corresponding source and
packaging scripts are in the materials archive.
