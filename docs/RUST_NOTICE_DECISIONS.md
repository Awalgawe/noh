# Rust notice decisions for Windows distribution

The published crate archive, its checksum, original license declaration and
source notices are the evidence. A missing standalone LICENSE file does not by
itself mean that a publisher gave no license: Cargo permits an explicit SPDX
license expression instead of a license-file field. Conversely, a label is not
an automatic check of all copyright notices or distribution conditions.

We preserve all original files in the source materials. The application carries
an aggregate notice file and machine-readable inventory. The decisions below
are explicit per-archive records in `assets/rust-notices.lock.json`, not an
unbounded fallback for future dependencies.

## Original upstream supplements

Thirteen native crate archives omit a notice that exists in the root of their
upstream repository at the exact published VCS revision: anes 0.1.6, phf 0.10.1,
phf_codegen 0.10.0, phf_generator 0.10.0, phf_macros 0.11.2, phf_shared 0.10.0,
profiling and profiling-procmacros 1.0.16, valuable 0.1.1, wit-bindgen-rt 0.39.0,
zune-core 0.4.12 and zune-jpeg 0.4.14/0.4.20. Acquisition binds each original
text to its Git blob, SHA-256 and the crate's embedded VCS revision. The texts
are not taken from a moving branch or a different version.

## Explicit declarations without a retrieved standalone text

Fourteen exact archives have their original Cargo.toml reproduced with authors
and license expression, alongside a standard license text from the pinned SPDX
License List. These are clearly marked `standard-license-text`, not original
upstream copyright notices. We do not invent years, copyright holders or a
historical publication commit. Placeholders in the unmodified MIT standard text
are template fields, not claims about the package's authors. Original source
files and any notices they contain remain in the accompanying sources.

- Apache-2.0 is selected where explicitly offered: crc-catalog 2.4.0,
  fxhash 0.2.1, mac 0.1.1, servo_arc 0.3.0, and both winapi GNU import-library
  crates 0.4.0. The latter also contain the original copyright/license header in
  lib.rs. We preserve that source; we do not infer copyright dates from version
  publication dates.
- MIT is the publisher's declaration for block 0.1.6, crunchy 0.2.2,
  malloc_buf 0.0.6, objc-foundation 0.1.1, objc_id 0.1.1, simd_helpers 0.1.0
  and dispatch 0.2.0. Their declared authors are copied verbatim from the package.
- selectors 0.25.0 declares MPL-2.0 in Cargo.toml and explicitly applies it in
  the header of src/lib.rs, including the official Mozilla license URL. Its
  complete unmodified sources accompany the distribution.

The collector checks the archive checksum, metadata file checksum, exact authors
and expression, selected alternative, and standard-text hash. Unknown archives,
changed declarations, changed authors and substituted standard terms fail.
The inventory keeps `missing_original_notice_texts` even when a declared-license
supplement supplies the terms, so the provenance distinction remains visible.

## Scope

This covers the complete vendored source inventories: 486 NOH crates and 653
native crate archives, including dependencies for unused targets and tests.
It does not claim that every listed crate is linked into Windows binaries.
Standard-library/compiler notices and non-Rust native components are separate.
The generated status remains unreviewed until the corresponding evidence is
independently accepted. These notice decisions alone do not authorize publication.

The standard-library dependency supplement separately covers 92 distinct crate
versions from the five exact library Cargo.lock files (Rust 1.87, 1.88 twice,
1.89 and 1.98.1). It is a complete graph superset, including unused targets/tests.
Five archives need original upstream notices: compiler_builtins 0.1.152,
fortanix-sgx-abi 0.5.0/0.6.1, vex-sdk 0.27.1 and wasip1 1.0.0. These are pinned
to their recorded VCS base revision and original notice bytes. vex-sdk's published
VCS metadata says the tree was dirty: the retained upstream MIT notice agrees
with its published MIT declaration, but is not evidence of an identical source
tree. That limitation is carried in the inventory. compiler_builtins' combined
MIT/Apache/LLVM-exception requirements are retained together, not reduced to one
permissive alternative.

References:
- https://doc.rust-lang.org/cargo/reference/manifest.html#the-license-and-license-file-fields
- https://spdx.org/licenses/
- https://www.mozilla.org/en-US/MPL/2.0/
