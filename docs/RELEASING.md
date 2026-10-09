# Release readiness

Publishing the source repository and distributing a supported application are
separate steps. Windows 0.1.0 portable is qualified; its installer is tracked separately.

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

## Current review boundary

Windows redistribution materials have an exact-input reviewed record in
`assets/delivery-policy.json`. This covers 112 signed native binary/source
pairs, their original recipes/patches and notices, seven complete Git sources,
486 NOH crates, 653 native crates and 92 additional standard-library crate
versions across five locked graphs. The graphs overlap; these counts are not a
count of distinct libraries linked into the application. Original and declared
notice supplements, standard-library correspondence, license alternatives and
Whisper/CUDA conditions are documented in [WINDOWS_REDISTRIBUTION.md](WINDOWS_REDISTRIBUTION.md)
and [RUST_NOTICE_DECISIONS.md](RUST_NOTICE_DECISIONS.md).

Windows 0.1.0 portable qualification is recorded against the exact compiled
commit and DELIVERY digest in `assets/delivery-policy.json`. Its
[qualification report](WINDOWS_0_1_0_QUALIFICATION.md) covers build/source identity,
PE imports, fresh extraction/configuration, the external Microsoft prerequisite,
actual preview/GPU/audio-device operation, transcription, captions/export and
exact downloaded-byte attachment/Defender checks. The installer is excluded
pending interactive qualification. Mac material review, native packaging and
signing/notarization remain unresolved; Windows acceptance does not clear Mac gates.

Public Actions binary artifacts are distributions. Before uploading them, the
redistribution record must match the exact hashes of Cargo.lock, NOH's license,
the runtime/model/recipe locks and notice collectors. It binds SHA-pinned evidence
for `native_corresponding_sources`, `native_notices`, `rust_sources_notices` and
Windows `cuda_terms`. Changed inputs invalidate that authority. A workflow input
or publication token cannot clear missing evidence. Preflight checks this before
an expensive retained build. The non-uploading `verify` mode remains available
for investigations; it does not produce a reusable public package.

Both publication routes below require independent review of the final bytes.
A qualification record binds `platform`, `source_commit`, `delivery_sha256` and
SHA-pinned evidence for `corresponding_sources`, `rust_notices`,
`native_import_closure`, `clean_install`, `download_protection`, `audio_gpu`,
`offline_transcription` and `media_export`. Mac additionally requires
`developer_id`, `notarization` and `stapling`. Each evidence file must describe
actual observations and limits; a Boolean or invented pass is insufficient.
Clear remaining qualification items only after reviewing those exact results.

## Hosted pipeline publication

1. Merge the reviewed source/packaging change through protected PR checks.
   Windows material authority is already recorded for the current inputs;
   renew it if the locks or notice collectors change. Mac requires its own
   completed material review before selecting it.
2. Dispatch **Release** on main with `stage=build` and the intended target to
   retain one complete candidate build. Save the run ID, attempt, source commit,
   artifact IDs and hashes. Do not precede this with a disposable `verify` build
   unless a specific unresolved build question warrants it. `target=all` means
   Windows x64 and macOS ARM64, not Linux or Intel Mac.
3. Download and inspect the exact artifacts, perform native qualification and
   commit the independently reviewed evidence/qualification record. Keep the
   original application bytes and source commit. A metadata-only review does not
   require rebuilding that package.
4. Use `NOH_DELIVERY_WORKFLOW_ID` for **release.yml in the current repository**.
   The hosted `draft` route requires the `noh-public-draft` environment with an
   independent required reviewer, prevention of self-review and main-only
   deployments. Do not bypass that environment or claim a local build came from
   the hosted pipeline. Mac signing has separate protected environment inputs.
5. Dispatch `stage=draft` with the exact source run, attempt, commit and target.
   The verifier checks successful controls/audit/build jobs, artifact hashes,
   source ancestry and final qualification before staging inert assets. Its token
   can create a draft, not automatically publish it. Review the ordinary release
   title, notes, tag and assets, then publish only with the owner's authorization.

The repository is public and main protection is active. Hosted artifact storage
is a separate quota from runner minutes. The local route below can reuse the
verified inputs and avoids a second application build solely to upload bytes.

## Reviewed local Windows publication

The first public Windows portable can be built locally from a clean public
commit after protected PR checks and the independent distribution-material
review. Use the existing Cargo release profile, GNU/LLD configuration and locked
inputs; reuse caches, build once and test those exact packaged workers. Do not
set GITHUB_ACTIONS locally or invent a hosted run identity. The disposable hosted
build driver remains restricted to hosted runners. A local seal records null
run/attempt fields and retains the actual build environment.

The Windows qualification scope is a fresh extraction with a new application
configuration and a PATH containing Windows system directories only, followed by
real preview, export and CPU/CUDA speech checks on the recorded Windows/GPU/audio
configuration. This is not a claim of a freshly provisioned Windows VM or human
listening tests. The external Microsoft runtime is present at the tested version;
its absence is covered by import restrictions and failure diagnostics, not by
uninstalling shared system components. State these limits in the evidence.

A downloaded archive must retain its published hashes and be checked under
normal Windows download protection. Authenticode signing/reputation are not
claimed for the unsigned portable; do not disable security settings. A blocking
security result must be reported and investigated before publication.

Seal the application and complete materials together. After the Critic accepts
the exact DELIVERY.json hash and native evidence, record the qualification against
that source commit, run the existing envelope/qualification checks, and upload
those unchanged assets to an ordinary GitHub release. The tag must identify the
compiled source commit. A subsequent documentation/qualification commit may add
the release link and evidence without rebuilding or relabelling that binary.
The hosted draft path keeps its separate run/protected-environment requirements.

The ordinary Windows installer is a separate wrapper around those exact portable
bytes. `tools/public-installer.py --bundle <portable-folder> --commit <source-sha>
--compiler <Inno-Setup-7.1.0-folder> --output <new-folder>` verifies the portable
manifest, input authority and compiler pins before generating its exact file list.
It does not rebuild NOH. Preserve `INSTALLER.json`, the wrapper source ZIP and
native install/uninstall evidence alongside the original delivery identity.
Qualify this additional executable independently before advertising it.

Use the existing **Release** workflow on main with `stage=installer` and
`target=windows-x64`. This calls the public job in `windows-installers.yml` for
the initial 0.1.0 release; there is no branch-specific build or publication path.
It verifies the existing release tag, portable ZIP and manifest hashes,
then compiles the checked-in wrapper with the pinned Inno compiler. The application
is not rebuilt. Its receipt distinguishes the application commit from the wrapper
workflow commit and run/attempt, and records the actual installer SHA-256.
The hosted test installs into a disposable path containing spaces, checks every
installed payload hash, launches the GUI and CLI, reinstalls, and uninstalls while
checking that a user-created file survives. Silent installation does not exercise
the Microsoft prerequisite wizard. Existing portable media qualification remains
valid for the unchanged installed application bytes.

A hosted runner without OpenGL 2.0 cannot validate GUI startup. The exact observed
renderer error is recorded as unavailable, never as a successful GUI check; other
launch errors still fail the job. In that case, validate the exact hosted installer
and installed GUI on a supported local Windows host before publication.

Successful output is retained in an unpublished review draft to avoid Actions
artifact-storage quotas. This job never publishes that draft or changes the live
release. After reviewing the exact results and any required interactive checks,
add those same tested bytes and wrapper sources to the ordinary release, together
with their checksum and provenance. Do not publish the temporary review draft.

The release sequence is fixed: reviewed commit on main, hosted build, tests of
that exact installer, independent review, then publication of those same bytes.
Local graphics checks supplement the hosted run using its downloaded installer;
they do not create a second release build. The wrapper runs as x64, matching the
application and the system runtime it detects, avoiding WOW64 path redirection.

The wizard installs per user, detects the tested Microsoft v14 x64 prerequisite,
and offers a direct download only when needed. It pins the Microsoft-signed
14.51.36247.0 installer by SHA-256, displays download progress/cancellation,
opens Microsoft's own licence/install UI, then checks the prerequisite again.
No Microsoft binary or installer is embedded or mirrored. Skipping the option
keeps preview/export/existing subtitles available. Silent NOH installation does
not launch the external installer; provisioning that prerequisite is a separate
administrator action. Native acceptance must distinguish a simulated missing
prerequisite from an actually absent system installation.
