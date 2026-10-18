# Windows 0.1.5 redistribution inputs

The 0.1.5 publication initially omitted Windows. This input review prepares the
missing Windows x64 portable and its Minimal, Standard, Complete and Web
installers. It does not qualify binaries or installer wrappers before they are
built and tested.

The baseline is the qualified Windows 0.1.3 source
`6b8c8054b93169fd455d86d16af8aeb92fec1a0e`, with the material review in
[RELEASE_0_1_3_INPUTS.md](RELEASE_0_1_3_INPUTS.md) and its referenced Windows
source, notice and CUDA records. The [input receipt](release-evidence/windows-0.1.5-inputs.json)
binds the current Windows inputs and the reused reviews by SHA-256.

Of 486 third-party Cargo package records, 482 are identical to Windows 0.1.3.
The four changed records retain their dependency lists:

- rustls 0.23.46 replaces 0.23.45. Its original Apache, ISC and MIT notices
  and pinned source archive were checked in the
  [0.1.4 input review](release-evidence/linux-0.1.4-rustls-inputs.json).
- winit 0.30.13 and egui-winit 0.36.2 use the immutable Wayland backports
  recorded in the [0.1.5 source review](release-evidence/linux-0.1.5-inputs.json).
  The original Apache and MIT notices accompany the Git sources. The obsolete
  egui-winit registry notice supplement is removed; other supplements remain.
- zip 9.0.0 replaces 8.6.0, with the same MIT notice and six dependencies.
  Its pinned source archive and original notice were also checked in that
  0.1.5 review. Windows archive validation was adapted and passed required CI.

These Rust source and notice checks concern the complete vendored graph and
apply to the same inputs on Windows. They do not transfer Linux runtime
qualification to Windows. The existing dispatch 0.2.0 declared-license
supplement remains explicitly distinguished from an original upstream notice;
that dependency is macOS-only.

Every other Windows redistribution input is unchanged: the native binary/source
locks, preview runtime, speech recipes/models, compiler source, runtime and
notice collectors, CUDA terms and NOH licence. Windows native materials still
require exact correspondence in the newly built archive. The new materials
must also contain their own complete NOH source tree and the current vendor
graph; previous Windows application bytes are not reused.

The existing Release workflow will build Windows once from the reviewed main
commit. Its installer stage reuses that exact portable. Native preview/audio,
CPU/CUDA speech, export, download protection, installer profiles and component
acquisition require exact-byte evidence and independent review before adding
assets to the existing release.

The published `v0.1.5` tag and Linux archives remain unchanged. The Windows
build commit will be a descendant containing only release evidence/policy
changes relative to the tagged application sources. The final review must
verify that restriction and explicitly publish the Windows commit, build run
and corresponding source archive alongside the existing Linux provenance.
The original tag must not be presented as the Windows build's Git revision.
