# Linux AppDir redistribution inputs

This record describes the inputs to the first Linux x86_64 AppDir, not acceptance
of a final executable or support for every Linux distribution. The baseline is
Ubuntu 24.04 and Rust 1.98.1. Automatic updates and GPU speech backends are disabled.

## Native correspondence and original notices

`assets/linux-native.lock.json` has SHA-256
`a07f044a85979021e43f140c649e1d675185edbb48bd0e868ce3bbacb58e3060`.
It records 278 binary-package versions, 237 source-package versions and 755 source
archives. Its 517 selected runtime/notice files identify the exact binary package,
archive member and SHA-256. Packages that contribute only common license texts
are included in these counts; the counts do not describe linked libraries.

The binary and source URLs are HTTPS Ubuntu archive/security pool URLs, with
the archived Rust 1.75 compiler obtained from its official Launchpad build and
source publication. Source
records retain the exact source version and archive sizes/hashes, including the
original tarballs, packaging/patch tarballs and `.dsc` files. Acquisition verifies
every archive before use and copies complete source archives without rewriting
them. The output retains each original repository source record. It does not
substitute a newer upstream checkout for the source of an older Ubuntu binary.

The application retains 276 original package copyright documents under
`usr/lib/noh/licenses/native` and the referenced common terms under
`usr/lib/noh/licenses/common`. The corresponding source archives remain in the
separate materials asset. The full notice documents, including file-specific
exceptions, take precedence over a short license-name inventory. Their format is
described by the [Debian copyright specification](https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/).

The FFmpeg package identifies its normal binary build as GPL-2-or-later and
explicitly excludes its optional non-redistributable configuration. The libmpv
package identifies its binary combination as GPL-3-or-later because of
libsmbclient. NOH is GPL-3.0-only; preserve the upstream later-version grants and
all original notices. LGPL components remain replaceable shared libraries.
Their complete corresponding sources and packaging patches accompany the binaries.
The packager changes ELF search paths only; it does not recompile Ubuntu libraries.
Original hashes are in `LINUX-INPUTS.json`, relocated hashes in `ELF-IMPORTS.json`.
glibc and graphics-driver libraries are host prerequisites and are not redistributed.

Selected notice details that must remain visible:

- FreeType uses its original FTL notice, including the authors' 1996–2023 copyright.
  The README shipped beside the AppDir explicitly states: "This software is based
  in part on the work of the FreeType Team." No endorsement by FreeType or other
  upstream authors is claimed.
- libva's `Expat-advertising` and libXv's “All Rights Reserved” headings accompany
  express redistribution permissions and restrictions on using the authors' names
  in advertising. Their full texts are retained, not replaced by generic MIT text.
- xz/liblzma's `noderivs` entry applies to the verbatim GPL/LGPL license documents.
  Those documents and the source archives are retained without textual changes.
- libcom-err's `lib/support/profile.*` declaration offers GPL as an alternative
  to `MIT-US-export`; retain and use the GPL alternative. D-Bus and GLib declarations
  that offer GPL/LGPL alternatives to AFL are retained under those GNU alternatives.
  These choices do not relicense unrelated files in their complete source archives.

## Speech runtime and models

Whisper.cpp 1.9.4 comes from the exact upstream archive with SHA-256
`57e280cee375ab02425b806ad5146b99f6eb9357e3c2b31357c8a6af2e2e44ae`.
The full archive, Linux build recipe and compiler-command receipt accompany the
package. The build uses the x86-64/SSE2 CPU baseline and disables AVX, BMI2 and
GPU backends. The MIT notices of the ggml authors, OpenAI's multilingual Small
model and Silero VAD remain in the speech folder. Models and their notices retain
the exact existing `assets/speech-bundle.json` hashes. No CUDA redistribution is
part of this Linux package.

## Rust sources and notices

The Rust embedded in the Ubuntu libraries is separate from NOH's own Cargo graph.
The locked `native_rust_builds` section retains the official Ubuntu build logs and
`.buildinfo` records for rav1e 0.7.1-2 and librsvg 2.58.0+dfsg-1build1. Their output
package hashes match the native binary inputs. The rav1e log identifies 70 exact
dependency crate/version pairs in addition to rav1e itself; the lock maps each
to its exact Ubuntu development package, original copyright document and complete
source archives. This is a conservative build/test dependency set, not a claim
that every entry is linked into the runtime. It includes v_frame 0.3.7.

Librsvg's Debian source archive already contains its vendored crates under
`debian/missing-sources`; its original package copyright document covers them.
Both producer build records identify Rust and libstd packages version
`1.75.0+dfsg0ubuntu1-0ubuntu1`. The corresponding Ubuntu compiler source, packaging
patches, vendored dependencies and original copyright document are now included.
Its signed source descriptor pins the original archive to SHA-256
`a33de6e0dcaf237f00ee12b1664f61ae5f319431bbff3f7965dc1c404d633687`;
the official compiler build's `.changes` pins the compiler package used to obtain
that notice. These materials cover the embedded standard-library correspondence;
NOH's Rust 1.98.1 archive is not used as a substitute for these older sources.

For the added native Rust dependencies, use MIT/Expat where offered as an
alternative to Apache or Unlicense; preserve independent BSD, Zlib, Unicode and
other file-specific grants. Crossbeam's array implementation uses its BSD-2-Clause
alternative. Its CC-BY-SA example and BSD tests remain unchanged in the complete
source archive with their original notices. Minimal-lexical's Go-derived and
Sun-derived files retain their separate BSD and Sun/SunSoft/SunPro notices.
Av1-grain retains both BSD-2-Clause and the AOM patent license, including its
distribution and reciprocal patent conditions. The compiler archive remains
unmodified with the original notices for its source, vendored crates, fonts and
documentation; those source-only components are not copied as runtime executables.

All 486 vendored crate records match the previously reviewed Windows inventory,
including file hashes, declarations and selected alternatives. The aggregate's
different byte hash on Linux reflects the ordering of egui font-notice entries;
comparison by crate and notice-file path confirms that no notice is missing or
replaced. The existing [Rust notice decisions](RUST_NOTICE_DECISIONS.md) cover this
complete graph, including unused targets and tests. The original-text limitation
for dispatch 0.2.0 remains explicit; its declared-license treatment is unchanged.

The Linux rustc component identifies version 1.98.1 and source commit
`48a229ceaefd4985c50990b14116b6d856af0985`. Its original standard-library copyright
HTML matches the pinned SHA-256
`68129500b616d5838629e68f55ff3aed5e096dacf60ce9eb41bbe599a563afa6`.
The complete compiler source archive has SHA-256
`be1816e7f6c40abb90245ad6e024bed2a7e88d7dda4561e4d5470207df616b9f`.
The candidate also contains its own exact NOH Git source archive and Cargo.lock.

This record is submitted for independent redistribution review. It does not clear
the policy gate until an exact-input review record is added. Each final CI package
still requires separate qualification and unchanged-byte publication checks.
