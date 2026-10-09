# Third-party components

NOH's own code is licensed under GNU GPLv3 (`GPL-3.0-only`). The licenses below
apply to their respective third-party components; distributing them together
does not replace their original license terms. Application packages include
NOH's license as `licenses/NOH-LICENSE.txt`.

## Persistent video preview

The Windows candidate uses MSYS2 libmpv **0.40.0-4**, dynamically loaded through
`libloading` 0.8.9 (ISC). The producer declares `GPL-2.0-or-later`; its linked
FFmpeg build declares GPLv3 or later. This replaces the historical upstream CI
LGPL runtime. Original license alternatives and the full combined component
inventory are covered by the exact-input Windows material review in
[WINDOWS_REDISTRIBUTION.md](WINDOWS_REDISTRIBUTION.md).

`assets/preview-runtime.json` and `assets/windows-native.lock.json` pin the exact
binary/source packages, patches, recipe and runtime file hashes. The portable
keeps the preview closure separate and replaceable in `bin/preview`. Its Windows
loader resolves dependencies from that DLL folder and Windows default system
directories. Packaging checks the selected producer hashes; the application
does not restrict modified compatible libraries. Original mpv Copyright and
license texts are extracted from its paired source archive into `licenses/native`.

## Bundled local speech recognition

The Windows GUI portable includes the qualified whisper.cpp b5130 Windows x64
CUDA 11.8 runtime (MIT), OpenAI Whisper multilingual Small model (MIT) and Silero
VAD v6.2.0 (MIT). `assets/speech-bundle.json` pins their upstream URLs and each
included file's SHA-256, including the license notices. NVIDIA CUDA runtime and
cuBLAS 11.11.3.6 libraries are bundled under the NVIDIA CUDA EULA, recorded in
`CUDA-RUNTIME-LICENSE.txt`. The separately downloaded official cuBLAS archive
fills the upstream Whisper ZIP's missing dependency; both archive hashes and
the individual DLL hashes are pinned. The development
packager verifies the complete local speech bundle before publishing it and
copies this inventory as `bin/speech/SPEECH-BUNDLE.json`; the outer manifest
covers it too. The inventory keys retain upstream filenames; runtime/models
are in `bin/speech/` and the `*-LICENSE.txt` notices are in `licenses/`.
Executables, libraries and model weights remain outside Git. No media upload,
startup model download or additional Python runtime is needed. The Windows
speech tool requires the external Microsoft Visual C++ v14 x64 runtime. The
ordinary installer offers its direct Microsoft download when missing; Microsoft's
installer and DLLs are not redistributed in NOH packages.

The delivery acquisition script extracts NVIDIA's full 61,498-byte license from
the checksum-pinned cuBLAS archive, not from a reformatted web notice. The two
model license URLs identify immutable upstream commits; their existing file
hashes are unchanged. The reviewed CUDA distribution basis is recorded separately
in [WINDOWS_REDISTRIBUTION.md](WINDOWS_REDISTRIBUTION.md).

## Rust dependencies

`Cargo.lock` pins the current dependency versions. Their individual licenses
continue to apply. The reviewed Windows materials include the complete locked
NOH/native graphs and additional Rust standard-library dependency sources and
notices; see [RUST_NOTICE_DECISIONS.md](RUST_NOTICE_DECISIONS.md).

## Windows installer

The optional ordinary Windows installer uses **Inno Setup 7.1.0**, copyright
1997-2026 Jordan Russell and portions copyright 2000-2026 Martijn Laan.
Its [original licence](https://jrsoftware.org/files/is/license.txt) permits use
and redistribution with its notices retained.
The installer keeps the producer's notices and installs the original licence
as `licenses/INNO-SETUP-LICENSE.txt`. Wrapper sources and the same licence are
provided alongside the release. See [Inno Setup](https://jrsoftware.org/isinfo.php)
and its [source code](https://github.com/jrsoftware/issrc/tree/is-7_1_0).

Candidate source materials include `cargo vendor --locked --offline` output for
the complete lockfile and a generated `RUST-INVENTORY.json` with version/source
and SPDX metadata. The inventory is built from every vendored manifest and
verified against Cargo.lock, independently of active features. Original license,
copyright and font notices are checked against the vendor file checksums.
`assets/rust-notices.lock.json` adds exact-commit, SHA-pinned upstream texts for
crates whose registry archives omit them; acquisition also verifies Git blob IDs.
The original texts and provenance are aggregated in `rust-notices/`, copied to
`licenses/rust` before package manifests and macOS signatures are created.

The local complete vendor inventory has 486 crates. `difflib` 0.4.0 omits VCS
metadata and license text from its registry archive. Its original manifest and
all six Rust files exactly match a pinned upstream tree that contains a MIT
license. The supplement preserves that original text, including the Kevin B.
Knapp attribution, and the full tree evidence. This content match does not
identify the historical publication commit or establish complete attribution/
license applicability. Those questions remain for independent review.
`dispatch` 0.2.0 still has no full notice text in its recorded Git tree.
The report covers all vendored crates, including target/build/test-only ones;
it does not infer which crates are linked into a particular binary. Standard
library/toolchain and native components are recorded separately. Maintainer review
must resolve these texts, attribution coverage and applicable license alternatives;
metadata or presence of a text alone does not clear the release gate.

The build inputs also pin the Rust 1.98.1 source archive from the official Rust
distribution server, including the statically linked standard-library sources
and original notices. Its exact SHA-256 is in `assets/delivery-policy.json`.
The crate inventory does not cover those components. CI also preserves the
generated `COPYRIGHT-library.html` shipped in the installed rustc component and
copies it to `licenses/rust/RUST-STANDARD-LIBRARY-COPYRIGHT.html`. The collector
checks Rust 1.98.1, its exact source commit, the component's file manifest and the
pinned original HTML hash. The notice inventory records its provenance separately.
This retains original third-party terms without choosing license alternatives or
establishing all target-specific correspondence. The pinned notice was verified
on Windows; a different native component notice stops the Mac candidate pending
explicit review. Full compiler/LLVM/build-tool notices and native runtime coverage
remain separate review items.

The optional MCP adapter uses the official `rmcp` 3.4.1 and `rmcp-macros` 3.5.0
crates (Apache-2.0), together with Tokio, tokio-util and schema dependencies.
These are compiled dependencies; running the portable server does not require
Cargo, Node.js, Python or a separately installed SDK. The dependency inventory
above remains a public-release prerequisite.

Timed PNG/JPEG support uses `image` 0.25.10 and `png` 0.18.1 (each MIT OR
Apache-2.0) for PNG/EXIF metadata. Their versions were already pinned in the
desktop dependency graph; they are now direct core dependencies with image's
default features disabled. Pixel decoding/rendering remains in FFmpeg.

Caption layout also uses the already-pinned `skrifa` 0.44.0 for font coverage
and metrics, and `unicode-segmentation` 1.13.3 for grapheme/word boundaries.
These are now direct runtime dependencies; no font toolkit installation is
needed to render subtitles. The complete dependency notice review above still
applies before public distribution.

## Embedded audio playback

The embedded desktop player uses Rodio 0.22.2 (MIT OR Apache-2.0), CPAL 0.17.3
(Apache-2.0), and Symphonia 0.5.5 (MPL-2.0) for the native audio device and WAV
decoding. AAC and clip mixes use the existing FFmpeg engine through bounded PCM
pipes. These optional GUI dependencies are compiled into the application;
no system audio package or external player is installed.

## Fonts and font licenses

`assets/NOHCJK.otf` is derived from Noto Sans CJK and redistributed under
the SIL Open Font License 1.1. It uses the family name NOH CJK; see
`assets/NOTICE.md` and `assets/OFL.txt`.

`assets/NotoSans-SemiBold.ttf` is the unmodified Noto Sans SemiBold face under
the SIL Open Font License 1.1. Its pinned upstream source and SHA-256 are in
`assets/NOTICE.md`; `assets/NotoSans-OFL.txt` is included in the portable bundle
as `licenses/FONT-NOTO-SANS-LICENSE.txt`. Both fonts are embedded; startup downloads none.
`assets/NotoSans-Regular.ttf` is the unmodified Noto Sans Regular face of the same
family and license, with provenance in `assets/NOTICE.md`; it is embedded as the
body text face. Its license is the same packaged `FONT-NOTO-SANS-LICENSE.txt`.
`assets/Jost-ExtraBold.ttf` is the weight-800 instance of Jost under
the SIL Open Font License 1.1, embedded for the NOH wordmark only; provenance
in `assets/NOTICE.md`, license packaged as `licenses/FONT-JOST-LICENSE.txt`.

## FFmpeg

FFmpeg is an external executable, not a Rust dependency. The build scripts take
an explicit local binary (or `NOH_FFMPEG`/PATH) and never fetch or install it.
The portable build records its license using `ffmpeg -L`.

The local macOS `.app` copies the selected native FFmpeg executable and its
license/configuration, while retaining its installed dynamic-library dependencies.
Use Homebrew `ffmpeg@7` for libass subtitle rendering and `mpv` for silent
scrubbing. These installations remain external to the app; the Windows speech
and preview bundles are not copied. This local package is not a standalone or
notarized public release. Its manifest records executable identity and hashes.

The optional macOS `--portable` build copies the complete non-system Mach-O
dependency closure of FFmpeg and libmpv into `Contents/Frameworks`. It rewrites
only the staged copies, preserving macOS system-library references, and applies
ad-hoc signatures from the nested code outward. `PORTABLE-RUNTIME.json` records
the original paths/hashes, relocated references, architecture and minimum OS.
Homebrew license/copyright files, installation receipts, SPDX SBOMs and available
build recipes for the copied formulae are preserved in `licenses/homebrew`.
The installed Homebrew libmpv and codecs have their own licensing configuration;
the historical Windows LGPL runtime qualification does not apply to these macOS binaries.
This development portable is not a public software release. The exact
corresponding-source and distribution inventory remains a public-release task.

Relocation and signature layout follow Apple's
[dynamic library guidance](https://developer.apple.com/library/archive/documentation/DeveloperTools/Conceptual/DynamicLibraries/100-Articles/DynamicLibraryDesignGuidelines.html)
and [nested code signing guidance](https://developer.apple.com/library/archive/technotes/tn2206/).
The full file hash manifest is stored alongside the signed app, avoiding a
circular hash/signature dependency. No signing identity or credential is used.

The macOS portable also includes native whisper.cpp 1.9.4 (MIT), built from its
SHA-256-pinned source archive with static GGML and embedded Metal shaders. It
uses Apple system frameworks for Metal and Accelerate, without Homebrew backend
plugins or NVIDIA DLLs. `tools/prepare-macos-speech.py` prepares this runtime and
the same checksum-pinned Small and Silero models recorded in
`assets/speech-bundle.json`. `Resources/speech/WHISPER-BUILD.json` records the original
executable hash, source URL/hash, architecture and CMake options; the surrounding
signed-file manifest records the packaged hash after signing. Runtime and model
MIT licenses are shipped beside the speech files. Speech remains local/offline.

The previous Windows runtime used FFmpeg 7.1 from Gyan's essentials build. Its
original acquisition was through imageio-ffmpeg 0.6.0. The official
[Gyan 7.1 release](https://github.com/GyanD/codexffmpeg/releases/tag/7.1) ZIP has
now been downloaded and checked: its FFmpeg executable is byte-identical to that
runtime. Its provenance is retained in the delivery lock's historical fields.
The README declares 34 external-library versions, including
Git revisions; it does not identify every enabled/transitive dependency.
The current Windows candidate uses MSYS2 FFmpeg **7.1.1-6** with its paired
sources, patches and original recipe. Its 113-image export closure lives in
`bin`; the 130-image preview closure lives in `bin/preview`. Eighteen same-name
DLLs differ between these producer builds, so their folders must stay separate.
The producer declares `GPL-3.0-or-later`. Public packaging must record the
exact binary provenance, checksums, build configuration and corresponding-source
obligations for that build; the `-L` notice alone is not a complete release process.

## Candidate delivery materials and remaining gaps

The Windows Whisper binaries import four Microsoft Visual C++ runtime DLLs:
`msvcp140.dll`, `vcruntime140.dll`, `vcruntime140_1.dll` and `vcomp140.dll`.
NOH's public package and source-material archives exclude these files, their CABs
and Microsoft's installer. Users obtain the x64 v14 runtime directly from
[Microsoft](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).
This is a separately installed speech prerequisite, not part of Windows itself.
The tested version is 14.51.36247.0; later compatible v14 versions are supported.
Whisper/CUDA binaries and models remain unchanged. Acquisition and PE auditing
bind the exception to exact locked speech image hashes; unrelated missing imports
still fail. Neither a registry entry nor an import audit proves working native
transcription. Installed and missing-runtime behavior must be tested.

`assets/delivery-windows.lock.json` binds the MSYS2 native lock and whisper.cpp
b5130 sources. The native lock contains 112 signed binary/source package pairs,
including the exact SpeexDSP build input absent from the PE import graph. Source
PKGBUILD hashes match the signed binaries' BUILDINFO records. Seven bundled bare
Git repositories were checked offline for complete objects and exact source
revisions. Four original native Cargo graphs contribute 653 checksum-pinned crate
archives and original available notices. These graphs include unused platforms
and tests; they do not establish which features were actually linked.

Original producer notices and 99 pinned source notices are copied to the
portable. Full paired source archives, recipes and patches are separate materials.
All 112 native package identities now have materialized notices, including the
26 packages whose binary archives omitted standalone terms. Twenty-six native
crate archives omit standalone notice files. Exact upstream root notices now
supplement thirteen; fourteen explicit per-archive declarations (including the
separate NOH dispatch dependency) are accompanied by pinned standard license
texts and their original Cargo metadata. The notice report distinguishes these
supplements from original texts. See [notice decisions](RUST_NOTICE_DECISIONS.md).
The exact four native Rust toolchain packages now supply their original generated
standard-library notices, matching rust-src archives and producer recipes.
The five standard-library Cargo.lock graphs also supply 92 distinct dependency
archives and their original notices, including the dependencies missing from the
producer notice output.
[Windows distribution review](WINDOWS_REDISTRIBUTION.md) records source coverage,
license alternatives, compiler exceptions and CUDA distribution conditions.
Final package behavior remains a separate qualification requirement. Source recipe
correspondence does not assert a bit-identical rebuild or redistribution clearance.
The native lock's `coverage_audit` names the remaining recipe-only build tools,
native Rust toolchain versions and optional data omitted by the runtime collector:
VapourSynth's Python standard library/scripts, external frei0r plugins and GLib/GIO
modules. Those optional producer features are outside the packaged NOH workflow.
Caption rendering through libass/fontconfig and ordinary MP4/image/audio operations
must be checked independently of their presence.

The original libmpv ZIP was downloaded successfully on 2026-09-30 and is retained
locally. Its SHA-256 matches the saved official GitHub asset digest, and its DLL
matches that historical runtime. `preview-runtime.json` retains the full mpv
commit, upstream run/attempt/job and asset ID. The rolling download URL returned
HTTP 404 on 2026-10-04. The current CI acquisition uses the fixed MSYS2 package
URLs and hashes; it no longer requests that rolling asset.

The exact [upstream build](https://github.com/mpv-player/mpv/actions/runs/36640285359)
used `ci/build-mingw64-full.sh`, a BtbN/FFmpeg-Builds container and several moving
Git branches. Its run metadata confirms success and the full source commit;
the job-log API returned HTTP 403 requiring upstream repository administrator
rights. An ordinary GitHub login is not established as sufficient. This is
historical provenance, rather than an input to the selected MSYS2 candidate. Retaining
the original binary proves acquisition and byte identity, not source closure.

`assets/delivery-macos.lock.json` fixes 114 Homebrew formula recipes and bottle
inputs against one checksum-pinned core snapshot. Acquisition verifies the
selected bottles. The source collector additionally preserves stable resources,
exact Git trees and every locked external/local/embedded patch, with SHA-256
records. Explicit GNU direct HTTPS download URLs retain the original recipe URL
and expected hash when the mirror selector redirects to HTTP. Collected materials
remain unreviewed; exact bottle build materials and license coverage remain
explicit unresolved gates. Homebrew receipts and SBOMs retained
by the portable packager are supplementary evidence, not a compliance attestation.

`assets/delivery-policy.json` records the reviewed Windows distribution materials
and the remaining native qualification gates. macOS materials remain unreviewed.
The draft verifier refuses unresolved platforms and requires independently
reviewed evidence bound to the exact candidate manifest hash. None of the new
inventories establishes that a public binary release is ready.

The independent redistribution registry additionally blocks public Actions
binary artifacts until the exact locked inputs have complete reviewed source
materials/notices. Unqualified labels alone do not authorize distribution.
