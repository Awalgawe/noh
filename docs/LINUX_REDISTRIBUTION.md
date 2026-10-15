# Linux distribution architecture

## Host-owned media runtime (unreleased, 2026-10-15)

Decision: keep the relocatable AppDir as a delivery container for NOH and its
CPU speech executable/models, and obtain **all** dynamically linked media,
graphics and audio libraries from the distribution. FFmpeg is a host process;
libmpv is loaded by its host SONAME. No shared library is shipped. The GUI, CLI
and MCP share the existing discovery path. Linux discovery now accepts the
explicit distribution PATH directory even when cwd overlaps it. Media processing
and GUI-thread behavior are unchanged.
This changes installation requirements: the next package is not self-contained.

### Why the inherited exclusion fix was insufficient

The Fedora 44 report identifies host Mesa binding to an older bundled Wayland,
an ALSA plugin-directory override incompatible with the host configuration, and
host PipeWire's ALSA plugin binding to an older libasound/libpipewire after
libmpv loads. Removing those three libraries fixes examples, not ownership.
An independent Ubuntu graph inspection found further overlap through XCB
(libbsd/libmd), Mesa (LLVM/ICU/libxml2), PulseAudio (audio codecs), and ALSA
plugins that themselves use FFmpeg. Those overlapping graphs cannot be called
private merely because dependency collection stops at selected SONAMEs.
Evidence is retained in the ignored pair register and inspection script.

The [pkg2appimage exclusion policy](https://github.com/AppImageCommunity/pkg2appimage/blob/master/excludelist)
is a useful established approach for constrained AppImages, but explicitly
records continuing exceptions and GLib conflicts. NOH's general Ubuntu
FFmpeg/libmpv builds import too much of the desktop ecosystem to establish a
small, stable private closure by extending that list. The 65-file removal and
38-test probe inherited by this task are superseded; they do not validate this
architecture or qualify Fedora.

### Alternatives and project constraints

| Approach | Fit for NOH | Decision |
| --- | --- | --- |
| Distribution-owned dependencies, as in native DEB/RPM packages | One provider owns libraries, drivers, plugins and their updates. Existing NOH runtime discovery already supports it. Requires installed media packages and codec capability checks. | Selected ownership model. Keep the existing AppDir envelope; adding DEB/RPM installers is unnecessary to correct library ownership. |
| Flatpak with a maintained runtime and GL extensions | Controls the runtime/library graph and avoids assuming arbitrary desktop ABI compatibility. Requires a new build manifest, media modules, permissions/portals, delivery and CLI/MCP integration qualification. | Appropriate future self-contained distribution, but larger than this correction. |
| Private AppDir media graph plus a host exclusion list | Retains the current installation experience only if the graph really is separable. Observed host ALSA/PulseAudio/Mesa dependencies overlap the bundled media graph. | Rejected for the current general-purpose native builds. |
| Prefer every host SONAME through a global library path | Can recover the reported desktop but changes child-process resolution and mixes codecs/providers opportunistically. | Diagnostic workaround only. |

References: [Debian shared-library dependency policy](https://www.debian.org/doc/debian-policy/ch-sharedlibs.html),
[Flatpak dependency model](https://docs.flatpak.org/en/latest/dependencies.html),
and [Flatpak GL extensions](https://docs.flatpak.org/en/latest/extension.html).
The selection above is a project-specific inference from those mechanisms and
the measured overlapping graph, not an upstream guarantee for NOH.

### Enforced contract

`assets/linux-host.json` declares x86_64/glibc 2.39, FFmpeg 6 or newer with
libx264/AAC/PCM encoders, libmpv client API major 2, and distribution package
names. `tools/linux_host.py` checks the actual provider capabilities before
packaging. `HOST-REQUIREMENTS.json` is copied into the package and materials.
The launcher sets neither ALSA_PLUGIN_DIR nor LD_LIBRARY_PATH.

The package contains exactly four ELF images: GUI, CLI, MCP and CPU Whisper.
The ELF audit rejects any extra ELF, SONAME provider, private dependency or
RPATH/RUNPATH, even for an unknown future library name. The producer and seal
both enforce that boundary; inert verification also requires the exact embedded
host contract and its declared path, even if all archive hashes are recomputed.
Future incompatible contracts must retain an explicit historical verifier and
use a new distribution marker. Verification of historical manifests keeps its historical generic ELF
policy. A new executable or dynamic speech dependency needs an explicit review.
No per-distribution library-preference script is introduced.

Native lock/acquisition files remain unchanged as historical 0.1.3 provenance;
the new delivery path no longer uses or distributes their runtime. CPU Whisper,
its exact source/build receipt, models/notices, NOH/Rust sources and notices
retain their existing provenance checks. No native package is relabeled as
NOH's source. Existing redistribution/release approval records cannot authorize
this new input contract: a future publication still needs fresh exact-input and
final-package acceptance. No release policy record is changed here.

### Initial architecture probes (v1)

The current local checks are packaging probes using 0.1.3 compiled application
code, with private RUNPATH metadata removed and input/output hashes recorded.
They are not a newly compiled candidate or final archive qualification.
`tools/probe-linux-host.py` creates an EGL/OpenGL context without a window,
loads ALSA before libmpv, initializes libmpv, then opens the host default PCM
and writes 960 silent frames. It records loaded paths and worker identities,
uses host FFmpeg through a real NOH CLI re-encoding export, decodes that output,
and verifies no-overwrite behavior. `--loader-only` labels partial coverage and
does not count as the export check.

Ubuntu 24.04 under WSLg passes the Wayland/software-rendering and audio/export
probe with FFmpeg 6.1.1 and libmpv API 2.2. A Fedora 44 container also passes:
glibc 2.43, Wayland 1.26.0, Mesa 26.2.4/llvmpipe LLVM 22.1.8, ALSA 1.2.16.1,
PipeWire 1.6.9, libmpv API 2.5 and FFmpeg 8.1.3 from RPM Fusion free. The initial
Fedora `ffmpeg-free` installation correctly fails the libx264 requirement.
Both successful probes perform an actual NOH CLI export, decode and
no-overwrite check using host discovery from a normal temporary work directory.
A Fedora container using the WSLg Wayland socket and a synthetic
PipeWire sink is not GNOME, physical GPU/audio hardware, human listening or
interactive GUI acceptance. Before publication, qualify the final archive on
the complete [Linux validation matrix](LINUX_VALIDATION.md) with real preview,
export, file selection and audio output. Do not infer universal distribution support
from the glibc floor or from this loader probe.

The Fedora image is the official amd64 manifest
`registry.fedoraproject.org/fedora@sha256:6fb67bf40d42d33f9805620b7fc04741df3ce328a8804308dfe5d186253f3f81`.
Package versions, mapped libraries, compiled identities, probe input/output hashes
and setup logs are in `logs/linux-host-libraries/`. Setup is disposable and
does not install Fedora packages or enable repositories on the user's desktop.
The v1 probe exposed a discovery regression from `/`: excluding cwd/bin hid
the host's `/usr/bin/ffmpeg` through merged `/bin`. Review correctly rejected
working around it with another cwd. Version 0.1.4 keeps explicit `/usr/bin` or
`/bin` PATH entries eligible on Linux, while rejecting implicit relative paths
and arbitrary cwd/bin entries and retaining Windows behavior. The maintained
export probe now runs from `/` without NOH_FFMPEG. A setup race between PipeWire and WirePlumber was also
corrected by waiting for the server and default sink, with a process timeout.

## Historical 0.1.3 redistribution record

The following record describes the previously shipped bundled native runtime.
It is retained for correspondence and is not the new host-runtime inventory.


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
