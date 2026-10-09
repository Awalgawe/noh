# Windows distribution review

This review covers the fixed Windows x64 inputs in the delivery, speech, native
and Rust notice locks. It does not qualify an application binary or claim a
bit-identical rebuild of third-party binaries. Changes to these inputs require
another review. Package testing and unsigned-download behavior are separate.

## Source access and native components

The application is GPL-3.0-only. The release must offer its exact source tree,
Cargo.lock, all 486 vendored crates and the build/packaging instructions beside
the binary download, at no charge. The materials archive also contains the 112
MSYS2 paired source packages: upstream archives or complete bare Git repositories,
producer PKGBUILDs, patches and metadata. These are the source revisions used by
the producer, established by matching each recipe hash to the signed binary's
BUILDINFO. The seven Git sources were checked for complete objects, no external
alternates and no missing submodules/LFS. The four native Cargo graphs include
all 653 original checksum-pinned crate archives. SpeexDSP's static/header build
input is included although it is not itself a PE import.

This implements source access with the binaries under GPLv3 section 6(d),
rather than asking recipients to contact the author for sources. The materials
are part of the release and must remain downloadable as long as the binaries.
GPL/LGPL/MPL source notices and original copyright headers remain intact.
No proprietary limitation is imposed on modifying or replacing these libraries.
The native DLLs remain ordinary, replaceable files in their two original runtime
folders; the app and sources contain no installation-key or signature lock.

The export FFmpeg build is configured for GPLv3 and is rejected if `--enable-nonfree`
is present. The preview libmpv closure is distributed under the GPLv3 alternative
permitted by its GPL-2.0-or-later grant. GPL-2.0-or-later native dependencies use
their GPLv3 option. LGPL libraries retain their notices and complete sources;
LGPL 2.1 section 3 also permits the GPLv3 option where required by the combined
work. No GPL-2.0-only option is selected for a library linked into this closure.

The ambiguous producer labels have been checked against original source grants:
jbigkit `libjbig/jbig_ar.c` permits GPLv2 or later; uchardet
`src/nsUniversalDetector.cpp` offers GPLv2-or-later/LGPLv2.1-or-later alongside
MPL 1.1. The retained headers make these alternatives explicit. x264, x265,
xvidcore, libdvdcss and libdvdnav also permit GPLv2 or later in their sources.
The lz4 DLL uses the BSD-2-Clause library terms; the GPLv2 command-line program
is not part of the runtime. Cairo uses its LGPL alternative. Freetype uses its
GPLv2-or-later alternative. Zstd uses BSD-3-Clause. Original alternative texts
are retained even where one license is selected.

Other native terms are preserved as supplied: MIT, BSD, ISC, Zlib, Apache-2.0,
PSF-2.0, MPL-2.0, CC0, WTFPL and the original permissive bzip2/fontconfig/GSM/
libpng/libmodplug terms. MPL libraries remain unmodified and their complete
source archives accompany the binaries. GCC runtime DLLs retain the GPL terms
and GCC Runtime Library Exception 3.1; NOH uses GNU Rust/LLD and does not modify
or redistribute a compiler executable as part of the application. The original
GCC and mingw-w64 sources/notices are included in the paired source materials.
Generic build tools (autotools, compiler, CMake, Ninja and pkg-config) are not
application components. No claim is made that their complete toolchain binaries
are necessary to exercise the application's source rights.

All 112 native package identities now have materialized notices. Ninety-nine
original source notice pins preserve original bytes, including 26 packages
whose binary archive omitted a standalone notice. Four x264/rtmpdump notices
were read from the exact retained Git commits; their source/archive hashes and
original paths are pinned. The notice folder travels with the application.
Optional VapourSynth Python scripts, frei0r plugins and GLib modules are not
included or advertised as supported NOH features.

## Rust crates and standard libraries

The entire 486/653 source inventories are retained, including unused platform
and test dependencies. This superset avoids inferring legal coverage solely from
a PE import list or feature graph. Each crate's original terms, applicable
NOTICE files and retrieved upstream copyright notices accompany the application.
[RUST_NOTICE_DECISIONS.md](RUST_NOTICE_DECISIONS.md) records the individually
reviewed omissions, original source correspondence and declaration supplements.
The difflib 0.4.0 supplement uses a complete published-content match, not an
invented historical publication commit. The MIT-only declaration cases retain
the publisher's original authors, metadata and complete source; template fields
in the separately labelled standard terms do not invent a copyright notice.

For OR choices, select Apache-2.0 when available, otherwise MIT, otherwise the
explicit permissive option. This includes the Apache alternative of an
Apache-2.0/GPL-2.0-only declaration. All operands of AND expressions remain
applicable, including BSD, ISC, Unicode, NCSA and font terms. Sole MPL-2.0 crates
are supplied with their complete unmodified source under MPL. The fonts remain
under OFL-1.1 and Ubuntu Font License, including their names and copyright
notices; they are not relicensed as NOH code. CDLA-Permissive-2.0 data retains its
terms. None of these choices removes notices or overrides a file-level license.

NOH's Rust 1.98.1 standard-library notice comes from the exact installed rustc
component, pinned to its version, commit, component manifest and original
COPYRIGHT-library.html hash. Its complete upstream source archive accompanies
the release materials. Native Rust 1.87.0-2, 1.88.0-2, 1.88.0-3 and 1.89.0-3 each
have an exact signed MSYS2 compiler/source-package pair. Acquisition retains
the producer's generated COPYRIGHT-library.html and full COPYRIGHT.html, checks
their hashes and verifies the recipe hash against both BUILDINFO records.
The matching rust-src archive, original recipe, patches and bootstrap settings
are included in the materials. The full upstream compiler source URL/hash is
also recorded. Compiler executables and the compiler/LLVM implementation sources
are not application payloads. Rust's dual MIT/Apache terms are preserved.

The producer's generated notice has an empty out-of-tree section for some
versions; it is not treated as proof of complete dependency coverage. We also
retain and verify each exact `library/Cargo.lock` against the original source
archive. Their full dependency graphs cover 42, 36, 36, 35 and 30 registry crates
respectively (92 distinct crate versions in the union). Every checksum-pinned
original crate archive accompanies the materials, and its notices are collected
separately in `rust-standard-libraries`. This includes object 0.37.1 for Rust
1.89 and covers optional features, build/test inputs and non-Windows targets
without claiming they are all linked. In-tree std components retain the producer
notices and the original source headers in the rust-src/full Rust source archives.
Graph omissions or changed lock/source/notice bytes fail acquisition.

## Whisper, models, CUDA and the Microsoft prerequisite

whisper.cpp b5130 is a separate MIT command-line program. Its exact source archive,
including the Windows CUDA build workflow and ggml source, accompanies the
materials. NOH invokes it using ordinary WAV input and transcript files; it is
not linked into the GPL application. Whisper and the Whisper/Silero models retain
their original MIT notices. Runtime, model and notice hashes are pinned.

The five unmodified NVIDIA DLLs are CUDA runtime, cuBLAS/cuBLASLt and
NVRTC/NVRTC-builtins. These component families are redistributables listed in
the [CUDA 11.8 EULA](https://docs.nvidia.com/cuda/archive/11.8.0/eula/index.html),
sections 1.1.1, 1.1.2 and 2.2. They provide acceleration to the separate Whisper
application, which provides speech transcription beyond the SDK's functionality.
The SDK files are private to that application's speech directory; they are not
offered as a standalone SDK. No NVIDIA driver, compiler, development tool or
modified NVIDIA sample is redistributed. The original CUDA license, including
third-party notices, is included unchanged. These files retain NVIDIA's terms,
including its restrictions; GPL rights in NOH do not relicense NVIDIA's SDK.
The application is not presented as endorsed by NVIDIA. Users who redistribute
the bundle must preserve and comply with the separate CUDA terms.

The four Microsoft Visual C++ runtime DLLs and Microsoft's installer are excluded
from the application and materials. Transcription requires a separately installed
x64 v14 runtime obtained directly from Microsoft. NOH does not exercise a Visual
Studio redistribution grant on the user's behalf. The tested version and link
are in the installation guide. The PE audit only allows these external imports
for the exact pinned speech images; a missing unrelated DLL still fails.

## Acceptance boundary

This review addresses the locked files, license choices, corresponding sources
and distribution layout. It is not a warranty about every legal jurisdiction,
codec patent or user's use of third-party media. No legal conclusion is inferred
from a successful build alone. Publication still requires the Actor/Critic review
of these exact materials and qualification of the final portable archive.
