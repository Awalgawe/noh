# Windows 0.1.1 redistribution inputs

This supplements [the Windows material review](WINDOWS_REDISTRIBUTION.md).
It records reuse of third-party evidence, not qualification of a new application
binary, installer, download or release.

The baseline is public commit `bb5463af39a9071aa6d9f90797960e7e684014c8`.
Its `Cargo.lock` SHA-256 is
`2f588955258d2df69728115ad06ae3fc4bb4dcbdb3be39337cc83772c5fa3030`.
The new lock SHA-256 is
`985fbef9ddfa5b8df7f12ed7211122c40c16992635ea34af0e33fdbd9de66305`.
Parsing both TOML documents and normalizing only the root `noh` package version
from 0.1.1 to 0.1.0 produces equal documents. All 486 third-party package records,
versions, source URLs, checksums and dependency edges are unchanged.

The GPL-3.0-only application licence, native/speech/preview locks, Rust notice
lock/generator, Windows runtime generator and Rust standard-library source pin
remain byte-identical to the baseline authority. The native source/notice,
CUDA-term and Rust source/notice evidence in the original review therefore
applies to these same third-party inputs. The 0.1.1 materials must still contain
the new NOH Git source and matching Cargo.lock; the old application's source ZIP
does not substitute for the source of the new executable.

The installer additionally uses Inno's `is7z-x64.dll` 26.02 to extract ZIPs.
Its exact binary and source revisions are pinned in
`assets/installer-extraction.lock.json`. The profile/maintenance source assets
include both the library source and the complete Inno Setup 7.1.0 source archive,
with original notices and the LGPL-2.1 text. The
[reconstruction procedure](INSTALLER_REBUILD.md) describes replacing the library
and rebuilding with a personal Inno key; official publisher keys are not required.
This is the LGPL section 6a material route, not a claim that the embedded library
can be replaced in-place in a running official installer. Native PE import
inspection of the pinned decoder finds no unprovided runtime dependency.

These source/notice additions do not authorize bundling Microsoft's optional
speech prerequisite. Its installer and four DLLs remain excluded from application,
component and NOH material payloads. Microsoft supplies that prerequisite directly
to the end user. Full native behavior and final-byte publication still require
their own independent qualification.
