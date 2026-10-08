# Release readiness

Publishing the source repository and distributing a supported application are
separate steps. No public binary release is currently qualified.

## Prepared delivery chain

The checked-in workflows are preparatory. Windows compilation and synthetic
delivery controls have passed on GitHub; this does not qualify a portable release.

| Workflow | Trigger and output | Authority |
| --- | --- | --- |
| `pr.yml` | PRs and main pushes; select affected checks on Windows, Linux, macOS ARM64 and Intel. Shared/unknown changes select all platforms. Documentation-only changes avoid native builds. | Read-only tests and one shared RustSec audit, followed by the required `PR checks` aggregate. No distributions. |
| `release.yml` | Manual dispatch on main: `verify` (default) tests portable packages without uploading them; `build` uploads reviewed candidates; `draft` prepares an independently qualified draft. Windows x64, macOS ARM64 or both. | Preflight before expensive work. Write token only in the draft branch; no automatic publication. |
| `delivery-candidates.yml` | Reusable Release build engine; direct dispatch remains private-only. PRs run synthetic delivery controls. | Read-only token; optional Mac signing uses separate protected steps. Artifact upload requires reviewed redistribution materials. |
| `delivery-draft.yml` | Reusable Release verifier and draft creator; direct dispatch remains private-only. | Exact run/attempt/commit and artifact verification; independently reviewed inert draft upload. |
| `rust-audit.yml` | Reusable audit, manifest/lockfile push changes and manual dispatch. No NOH compilation. | Vulnerabilities, unsoundness advisories and acquisition errors fail the job. Public diagnostic uploads are optional. |
| `native.yml` | Source/native verification. | Development evidence, not public packaging. |
| `update-release.yml` | Private controlled update trial. | Separate prototype; public updates remain disabled. |

Native builds and standard tests have passed for all three operating systems,
including both Mac architectures. That evidence does not qualify an installable
package. Linux and macOS Intel have no delivery packager and are excluded from
Release targets. Windows Setup installers remain a separate private prototype;
the Release entrypoint prepares portable ZIPs. The new entrypoint has been
checked locally, but not executed on hosted runners.

The build driver is `tools/delivery-build.ps1`; the acquisition, source and ZIP
controls are in `tools/delivery.py`. Standard runners are `windows-2025` x64 and
`macos-26` Apple Silicon. They use Rust 1.98.1, the locked Cargo graph, the
existing Windows GNU/LLD configuration, without the updater feature. The default
`compile-only` mode runs `cargo build --release --locked --features gui,mcp --bins`
and retains executables only on the disposable runner. Windows then runs the full
standard Rust unit, GUI, CLI, media and MCP integration suite against the locked
FFmpeg export backend. Tests use the same release profile to reuse compiled
dependencies, run media cases sequentially and require media prerequisites rather
than silently skipping them. Explicit opt-in hardware, benchmark and real speech
recognition tests remain ignored. No preview runtime or speech models are acquired
and no application binary artifacts are uploaded. The FFmpeg helper fetches only
the export packages already identified by the native lock, verifies archive/file
hashes and PE dependency closure, and checks the required encoders/libass filter.

`portable` with the default `upload_candidate=false` can acquire, build, test and
seal a complete candidate without uploading binaries or materials. This enables
disposable preparation checks before redistribution review is complete. Selecting
`upload_candidate=true` requires that review before acquisition starts. Failure
diagnostics remain bounded in both modes. No hosted portable preparation run has occurred.

`sign_macos=false` remains the default. An explicit main-branch portable Mac run
can use the protected `noh-macos-signing` environment and the prepared Developer
ID/notarization helper. This privately submits software to Apple after owner
approval; see [MACOS_SIGNING.md](MACOS_SIGNING.md) for exact inputs and native
checks. Unsigned jobs receive no signing credentials. No real signing/notarization
or environment setup has run in this preparation.

`portable` uses `cargo dev build --gui --mcp`. Both modes reject existing `dist/`
or maintainer inputs. Portable mode passes
speech/libmpv/FFmpeg paths explicitly, then runs `cargo dev verify --media --mcp`
against the packaged release workers. Python and Rust are build tools only.

Windows portable builds check normal/delay PE imports before media tests,
sealing and final-envelope acceptance. `PE-IMPORTS.json` records image hashes,
co-located DLL providers, Windows/API-set classifications and the external
NVIDIA driver dependency. Whisper's four Microsoft Visual C++ DLL imports are an explicit external x64 v14
prerequisite. Users obtain that runtime directly from Microsoft. The application
and materials archives exclude the DLLs, installer and CAB payloads. The PE audit
allows this exception only for the exact locked images under `bin/speech`; it
never uses the maintainer's PATH or System32 to satisfy an undeclared dependency.
Installed and missing-prerequisite behavior remains part of native acceptance.


Candidate assets are named `NOH-<version>-windows-x64.zip` or
`NOH-<version>-macos-arm64.zip`, with a separate `-materials.zip`, `DELIVERY.json`
and `SHA256SUMS`. The JSON explicitly says `unqualified`; it records exact Git
identity, target/features/profile, run/attempt, complete file hashes, archive
hashes, download sizes and extracted bytes. Files of 2 GiB or more are refused.
ZIP verification rejects traversal, duplicate/colliding paths, symlinks, extra
files and altered contents. These assets are developer candidates, not advertised
downloads. Source materials preserve the NOH Git tree, locked vendored Rust
crates/notices, generated license metadata, native materials and build environment.
Rust notices are generated from the complete vendor graph before packaging and
copied through the shared Windows/macOS packager. Missing original texts remain
visible in `RUST-NOTICE-INVENTORY.json`; no license choice or clearance is inferred.
The same input folder includes the original generated Rust standard-library HTML
notice. Its pinned hash, compiler release/commit and installed rustc component
manifest are checked before packaging; the shared packager rechecks that hash.
This is original notice material, not a redistribution attestation. Native Mac
component correspondence and complete compiler/native notices remain unreviewed.

Artifacts expire after seven days (independent staging after three). Caches are
not uploaded. Compression is disabled for artifact uploads of already compressed
ZIPs. There is no persistent Cargo/download cache, large-runtime PR upload or
automatic publication. Source/bottle/vendor staging can consume substantial
runner disk space; actual hosted-runner capacity/time must be measured.

## Unresolved delivery gates

Local preparation evidence and remaining activation work:

| Requirement | Local evidence | Remaining work |
| --- | --- | --- |
| Windows x64 / Apple Silicon build chain | Candidate driver, compile-only default, exact Rust/runtime locks; delivery workflows are covered by actionlint. | Actual clean hosted execution, runner capacity and native Mac dependency/minimum-OS checks. |
| Dependency traceability | Fixed MSYS2 URLs and hashes; 112 signed binary/source pairs with matching recipes; seven complete native Git repositories and 653 native Rust crate archives; export/preview closures kept separate. Historical acquisitions retained. | Review static/header coverage, native toolchain correspondence and dynamically loaded components; qualify actual final runtime behavior. |
| Sources and notices | Complete 486-crate NOH vendor inventory, original notices and content-match proof; generated standard-library HTML; original native package/source notices; all locked Mac sources/resources/patches collected. | Missing dispatch text and 26 native crate texts; native/toolchain correspondence and independent review of attribution, license alternatives, CUDA redistribution terms and the external Microsoft runtime prerequisite. |
| Package and qualification controls | Synthetic rejection controls, focused shared-packager checks, exact-file ZIP/envelope checks, separate closed redistribution/qualification registries. | Qualification records for exact final hosted package bytes; dynamically loaded dependencies and clean downloaded-file installation behavior. |
| Mac signing | Explicit protected signing choice, separate credential steps, inside-out signing, notarization/stapling and final-ZIP controls reviewed with simulated commands. | Owner Apple inputs/environment, native commands and clean Mac Gatekeeper/audio/GPU/media acceptance. |
| Installation and missing resources | Public installation/troubleshooting guides; local Windows resource behavior validated with synthetic fixtures and captured GUI. | Rehearse the actual final download/install/remove path on clean Windows/Mac profiles. |

Portable artifact uploads deliberately stop at the redistribution gate while
matching materials are missing. The Release `verify` stage remains available without uploads. Local checks prove the controls above; they do
not prove that a clean public CI runner can currently deliver a complete package.
Resolve the engineering/material gaps before activating portable distribution.

The authoritative list is `assets/delivery-policy.json`. It contains no
qualification records. Its unresolved lists cannot be cleared by candidate
output, a workflow input or a publication token.

Public Actions artifacts are themselves downloadable distributions. Therefore
`redistribution_unresolved` and the empty `redistribution_records` also block
**binary artifact uploads**, separately from final installation qualification.
Without reviewed redistribution, runs may build privately on the disposable
runner but retain only bounded failure diagnostics; no application/material ZIP
is uploaded. A redistribution record must match the exact hashes of Cargo.lock,
NOH's license and the platform's authoritative runtime/model/recipe inputs.
It requires reviewed SHA-pinned evidence for `native_corresponding_sources`,
`native_notices`, `rust_sources_notices` and, on Windows, `cuda_terms`. It must
describe actual complete corresponding materials and notice coverage, not a
provisional checklist. The draft verifier checks this authority again.
When upload is requested, the gate runs before portable acquisition/build to
avoid expensive known-blocked uploads. It does not block `compile-only` or
`portable` with `upload_candidate=false`, whose executables are not distributed.

- **Windows acquisition:** MSYS2 libmpv 0.40.0-4 and FFmpeg 7.1.1-6 replace the
  historical rolling libmpv/Gyan inputs. Every copied file has one pinned package
  member and SHA. The two native folders remain isolated. Production acquisition
  was exercised locally; clean hosted execution remains external. The earlier
  libmpv 404 and Gyan byte correspondence are retained as historical provenance.
- **Corresponding materials:** 112 signed binary/source pairs have matching
  PKGBUILD/BUILDINFO recipe hashes; original sources and patches are retained.
  Seven bare Git repositories pass offline full object checks. Four native Cargo
  graphs provide 653 pinned registry archives. Review static/header dependencies,
  actual linked features, native Rust 1.87/1.88/1.89 standard-library terms and all
  attribution/license alternatives. The native notice report identifies 26
  missing original crate texts, including unused-platform/test dependencies;
  NOH's separate graph still lacks dispatch's original text. Review Whisper/CUDA
  redistribution terms and the external Microsoft runtime prerequisite. These inventories do not clear public
  upload. Exact binary reproduction is not itself a prerequisite for source
  correspondence.
- **Mac inputs:** the frozen Homebrew inventory covers 114 formula recipes and
  bottle hashes, 46 stable resources and 22 patches. The source collector preserves
  SHA-pinned archives, exact Git trees and external/local/embedded patches. Its
  locked sources/resources/patches have all been collected locally. The output
  remains `unreviewed-source-materials`; exact bottle source/build/license
  correspondence requires separate evidence. Git submodules
  are rejected pending explicit pinned inputs. No Homebrew or Mac job has run here.
  `freeze-homebrew.py` emits intermediate `incomplete-source-inputs`: its API data
  must be enriched from the exact recipes before the collector accepts it.
- **Final platform bytes:** verify PE imports (including delay imports) and
  Mach-O closure, minimum OS, offline speech, media/export and installed behavior.
  Runner success is not a clean-user installation test, GPU/audio test,
  downloaded-file protection check or notarization result.

This is incomplete delivery preparation. No public binary is qualified and no
Windows/Mac candidate from these workflows exists yet. Old local portable
packages do not qualify the current source.

## Future maintainer activation

1. Resolve the engineering gates above and review the locked URLs/hashes and
   full notices/materials. Test changed runtimes using synthetic media. Run
   `python tools/test-delivery.py` and actionlint on all release/delivery workflows.
   Complete independent redistribution review and its exact-input record before
   allowing any public binary artifact. Local candidates remain private while
   those materials are incomplete.
2. Only after separately authorized repository creation/publication, enable
   Actions on the public repository. Standard runners are free for public
   repositories; artifact storage remains a separate quota. Apply main protection
   after the initial public commit. This preparation performs none of those actions.
3. Dispatch **Release** from main with `stage=verify` and the intended target
   first. This builds and tests complete portable candidates on disposable
   runners without uploading application/material archives. After redistribution
   review, dispatch `stage=build` to retain candidates. Save the successful run
   ID, attempt, commit and artifact IDs/hashes. Inspect full test logs, final
   manifests, ZIP sizes and materials; perform clean installation checks on those
   exact bytes. `target=all` means Windows x64 and macOS ARM64 only. Enable
   `sign_macos` after configuring the protected signing environment. An unsigned
   Mac candidate cannot pass the final signing qualification. `build` and `draft`
   fail in preflight while redistribution evidence is missing.
4. Commit independently reviewed qualification evidence under a public-safe
   evidence path. Each record in `qualification_records` binds `platform`,
   `source_commit` and `delivery_sha256` (the final `DELIVERY.json` file hash).
   Its `evidence` object must contain SHA-256-pinned files for
   `corresponding_sources`, `rust_notices`, `native_import_closure`, `clean_install`,
   `download_protection`, `audio_gpu`, `offline_transcription`, `media_export`.
   Mac also requires `developer_id`, `notarization`, `stapling`. Evidence is an
   actual reviewed report/material inventory, not an invented pass or Boolean.
   Clear a platform's unresolved items only after their evidence is established.
5. Set repository variable `NOH_DELIVERY_WORKFLOW_ID` to the numeric ID of
   **`release.yml` in the recreated repository**, never an old repository ID.
   Configure environment `noh-public-draft` with an independent required reviewer,
   prevention of self-review and a main-only deployment branch policy. No PAT is
   needed; narrowly scoped `GITHUB_TOKEN` permissions are declared in the jobs.
6. Dispatch **Release** with `stage=draft`, the same `target`, and exact
   `source_run_id`, `source_attempt`, `source_commit` from the successful `build`.
   The verifier requires successful preflight, controls, RustSec audit and every
   selected candidate job. It accepts only the selected package artifacts plus an
   optional audit report bound to that run/attempt. It downloads only packages,
   checks GitHub archive digests, then rechecks ZIP inventories and independent
   qualification without executing assets. The protected write-token job hashes
   the staged files, refuses an existing tag and creates only a **draft**.
   Inspect all assets, qualification reports, version/OS/architecture/minimum-OS
   and size/checksum notes before separate human publication. Rebuild/requalify
   if signing or any final byte changes. Nothing publishes on push, tags or PRs.
7. After the real public application ZIP exists and installation is qualified,
   replace the README's unavailable-download message with a link to that exact
   Release asset. Keep Windows-first scope explicit while Mac gates are pending.

Windows Authenticode is not implemented here; any selected certificate/service
needs separate owner authorization. For Mac, an Apple Developer account/team,
Developer ID Application identity/private key and notarization credentials must
be supplied by the owner. No identities, secrets, paid services or certificates
are fabricated. Signing/notarization/stapling commands and final-ZIP round-trip
controls are prepared; owner configuration and their native acceptance remain Mac
engineering gates. This draft workflow cannot make an ad-hoc app qualified.

Runner and asset constraints:
[standard GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[release asset limits](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).

## Source repository

- Include NOH's GPLv3 license and retain the licenses and notices of third-party components.
- Include only public source, guides, tests and required assets. Exclude private
  media, keys, session reports, personal paths and diagnostic archives.
- Inspect the history and commit metadata that will be published, not just the
  latest tree. A deletion commit does not remove old file contents.
- Verify documentation links, screenshots and build/package inputs from a clean
  source tree. Do not rely on ignored files present on a maintainer's machine.

Release attestations and qualification records must identify the exact
source commit and build included in the distribution.

## Application distributions

Before advertising a ready-to-use download, establish:

- The exact supported operating systems, architectures and minimum versions.
- Native launch, media, transcription, export and failure behavior on each target.
- Resolution or an explicit release decision for the known limitations in
  [DEVELOPMENT.md](DEVELOPMENT.md#known-limitations).
- The complete license, notice, provenance and corresponding-source inventory
  for each bundled native component; see [THIRD-PARTY.md](THIRD-PARTY.md).
- Distribution signing, macOS notarization where applicable, package integrity
  and a documented installation/removal path.
- A versioned artifact whose build identity matches its source and file manifest.
- A complete application ZIP attached to the release, clearly separated from
  source archives, with its architecture, minimum OS, download size, extracted
  size and checksum recorded. Link to that actual asset from the README only
  after it exists and has passed the checks below.
- The corresponding source and build materials required by NOH's GPLv3 and by
  the exact native components included in the release.

Validate the [installation guide](INSTALLING.md) with the actual release ZIP
on a fresh user profile without development tools: extract, open, import media,
preview, export, close, reopen and remove the app. Include a path with spaces.
Check the platform's downloaded-file protections and signature behavior; a
successful launch from a developer's existing folder does not cover that path.

The current Windows portable and macOS Apple Silicon package have local native
validation. That does not establish public release readiness, Intel Mac support
or Linux distribution support. Do not replace these limits with generic
cross-platform or production-ready claims.

## Update feeds

The Windows updater is a controlled prototype. Ordinary installation remains
disabled. Its publication workflow defaults to verification and has an empty
independent qualification registry. Source visibility must not enable publishing
or installation. See [UPDATES.md](UPDATES.md),
[UPDATE_RELEASE.md](UPDATE_RELEASE.md) and [UPDATE_NATIVE_GATES.md](UPDATE_NATIVE_GATES.md).
