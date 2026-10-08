# Build NOH from source

These instructions are for developers. People using a complete build do not
need Rust or a terminal. Public installers/downloads are not available yet.
If you already have a portable ZIP, use the [installation guide](INSTALLING.md).

## Requirements

- Rust 1.95 or newer and Cargo.
- FFmpeg with the encoders/filters needed by your media. Portable packaging uses
  the pinned FFmpeg 7.1 family and requires libass; Windows selects 7.1.1-6.
- For the desktop: libmpv and the platform's native GUI development libraries.
- For bundled transcription: the pinned speech runtime and model files.

Cargo downloads dependencies on the first build. Add `--offline` only when
they are cached. Run commands from the repository root so Cargo loads its
platform linker configuration. `CARGO_HOME`, `RUSTUP_HOME` and `CARGO_TARGET_DIR`
are respected.

## Windows

The tested toolchain is `stable-x86_64-pc-windows-gnu`, with the repository's LLD
linker configuration. Update-enabled builds also require a compatible C compiler
and archiver for native dependencies (the qualification runner uses LLVM/MinGW).

For a CLI-only source build:

```powershell
cargo build --locked --release --bin noh
.\target\release\noh.exe --help
```

Select FFmpeg with `--ffmpeg` or `NOH_FFMPEG` if it is not available through normal
discovery. Automatic discovery checks the executable's bundle first, then
absolute PATH directories. It excludes the working directory and its `bin`
subdirectory; it does not use relative PATH entries. Explicit engine choices
must point to a trusted executable.

For a complete GUI portable, first prepare these local inputs:

- The complete export folder from `assets/windows-native.lock.json`, with FFmpeg
  supplied by an explicit path; keep all pinned DLLs beside it.
- The complete preview folder from that lock, with libmpv selected by `NOH_LIBMPV`.
  Preview and export folders must remain separate because some DLL bytes differ.
- Original native notices selected by `NOH_NATIVE_NOTICES`, and NOH's original
  Rust notices selected by `NOH_RUST_NOTICES` for delivery preparation.
- The files pinned in `assets/speech-bundle.json`, supplied
  as a prepared speech folder. An earlier portable is usable only if it contains
  every currently pinned file; the old bundle lacks the four Visual C++ DLLs.

```powershell
$env:NOH_LIBMPV = 'C:\tools\native\preview\libmpv-2.dll'
$env:NOH_NATIVE_NOTICES = 'C:\tools\materials\native-notices'
$env:NOH_RUST_NOTICES = 'C:\tools\materials\rust-notices'
cargo dev build --gui --mcp --ffmpeg C:\tools\native\export\ffmpeg.exe --speech C:\tools\noh-speech
.\dist\noh\noh.exe
```

The packaging command verifies the pinned resources; it does not download them.
A prepared flat speech folder contains the manifest's filenames. An organized
portable keeps runtime/models in `bin/speech` and notices in `licenses`.
After the first complete build, packaging can reuse the resources in `dist/noh`.
Close that app before replacing its portable folder.

The public **Release** workflow offers explicit `verify`, `build` and `draft`
stages; see [RELEASING.md](RELEASING.md). `verify` tests portable packages without
uploading them. The reusable `delivery-candidates.yml` engine also retains a
private-only direct dispatch, which requires a successful Rust dependency audit
and defaults to `compile-only`.
It compiles GUI, CLI and MCP executables with `cargo build --release --locked
--features gui,mcp --bins`. Windows also runs the complete standard Rust unit and
integration suite using the same release profile and the locked FFmpeg export
backend. Media cases run sequentially and missing prerequisites fail the job.
Explicit opt-in hardware, benchmark and real recognition tests remain ignored.
No preview runtime or speech models are acquired, and executables are not packaged
or uploaded. Rust 1.98.1 is fixed by the delivery policy; Windows uses the existing
GNU/LLD configuration. Runner tests do not qualify clean-user installation.

The Windows Setup release workflow and its exact-byte native acceptance are
described in [UPDATE_RELEASE.md](UPDATE_RELEASE.md). For reusable lessons on
reducing CI build time, cache invalidation, prebuilt images and evidence reuse,
see [CI_PLAYBOOK.md](CI_PLAYBOOK.md).

`rust-audit.yml` also runs independently on Cargo manifest/lockfile changes,
audit-workflow changes and manual dispatch. It uses cargo-audit 0.22.2, installed
with its own locked dependencies and cached separately from application builds.
Each run fetches the current RustSec advisory database and scans every package
in Cargo.lock, including optional, test and other-platform dependencies. Known
vulnerabilities and unsoundness advisories fail the job; maintenance and yanked
package warnings remain visible. No advisories are ignored. Database/network
failures also fail the job. Audit logs, the lockfile checksum, auditor version
and available advisory database revision are retained for seven days.

To run the same check locally, install the pinned tool and run:

```powershell
cargo install cargo-audit --version 0.22.2 --locked --no-default-features
cargo audit --file Cargo.lock --deny unsound
```

This does not compile NOH or update Cargo.lock. It covers known Rust dependency
advisories; external FFmpeg, libmpv, CUDA and Microsoft runtimes need a separate
native-component vulnerability review. New advisories can affect unchanged
locks, so rerun the audit before delivery. No periodic audit is configured.

The delivery engine's `portable` mode acquires explicitly locked inputs from a fresh checkout.
Windows acquisition uses Python 3.14's built-in Zstandard reader for fixed MSYS2
binary/source packages, then verifies each selected runtime file. Release
`verify` uses `upload_candidate=false`; artifact upload additionally requires the
independent redistribution gate. A seeded cache is always rehashed. Local
acquisition does not establish clean hosted execution or redistribution clearance.
Candidate acquisition and source-material controls are described in
[RELEASING.md](RELEASING.md).

To collect the locked macOS dependency sources without installing Homebrew or
compiling NOH, use `python tools/delivery.py macos-sources --cache <cache-folder>
--output <new-materials-folder>`. This works on Windows too. The output contains
exact recipes, stable sources/resources and patches, with hashes and provenance;
it remains unreviewed. Resource names are encoded for portable filenames while
their original names are preserved. `tools/freeze-homebrew.py` produces an
intermediate inventory requiring source enrichment from the exact recipes before
this collector accepts it.

The pinned Windows speech inputs now include `msvcp140.dll`, `vcruntime140.dll`,
`vcruntime140_1.dll` and `vcomp140.dll`, extracted from the checksum-pinned
official Microsoft runtime. The helper extracts CAB members without executing
the installer; the installer/CAB/file hashes and original runtime-use license
document are recorded. Redistribution additionally requires the applicable
Visual Studio grant to be reviewed; it is not established by that runtime-use
EULA. The old local bundle lacks these DLLs, a gap hidden by development PCs.
`python tools/delivery.py audit-pe --bundle dist/noh --output
logs/delivery/PE-IMPORTS.json` inspects normal and delay imports without running
the images, records image hashes and rejects missing co-located DLLs. It does
not search PATH or System32 for application dependencies. Portable sealing and
envelope verification repeat this check. System/API-set classification does not
qualify minimum OS or loader behavior. NVIDIA GPU use requires the separately
installed driver; CPU fallback remains part of native acceptance.

The output includes the desktop launcher, CLI, optional MCP server, media
libraries, local speech runtime, models, user guides and notices. Keep the whole
folder together. `manifest.json` records file hashes and build identity.

## macOS

Install Xcode Command Line Tools, Rust, and the media tools:

```sh
brew install ffmpeg@7 mpv
cargo dev build --gui --mcp
open dist/NOH.app
```

This local app depends on the installed Homebrew libraries. It is built for the
current architecture, not as a universal binary. The packaging check rejects
unsupported FFmpeg versions or a missing libass subtitle renderer.

For a self-contained portable, prepare native Whisper and the pinned models
(Python 3.12+, CMake and Xcode Command Line Tools are needed for this step):

```sh
python3 tools/prepare-macos-speech.py
cargo dev build --gui --mcp --portable
```

The portable contains FFmpeg, libmpv, Whisper, the multilingual Small model and
Silero VAD. Later builds reuse the prepared resources; `--speech <folder>` can
select another preparation directory. The Small model adds about 466 MiB.

Apple Silicon builds produce `dist/NOH-macos-arm64/NOH.app` and its ZIP. The
current portable requires **macOS 26.6.2 or newer**. The actual minimum is derived
from all bundled libraries and recorded in the package. Signatures are ad hoc;
Developer ID signing and notarization are not configured. Intel distribution is
not yet validated. See [third-party notices](THIRD-PARTY.md) before distributing
any binaries.

The delivery workflow uses the standard `macos-26` Apple Silicon runner and a
frozen Homebrew core snapshot/bottle inventory, separate from this developer
Homebrew setup. That workflow has not run yet; its resulting minimum OS and
native behavior must be established by its actual output and installation tests.

## Linux

A desktop distribution is not yet validated. Native dependencies and a portable
packaging baseline still need qualification. The [development guide](DEVELOPMENT.md)
describes source checks; their success is not a ready-to-use Linux release.

## Build identity and validation

`noh --version` prints a short label; `noh --build-info` emits JSON with the
source/build fingerprint, compiler, target, profile and enabled features.
The GUI and MCP executable support the same queries. This describes the compiled
build, not the current checkout, and does not promise reproducible binaries.

Use [DEVELOPMENT.md](DEVELOPMENT.md) for checks and capture tooling,
[UPDATES.md](UPDATES.md) for the optional updater prototype, and
[RELEASING.md](RELEASING.md) for the separate public release requirements.
