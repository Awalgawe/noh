# Release readiness

Linux 0.1.4 changes to host-owned media dependencies. Its candidate job tests
one sealed archive across six pinned distribution bases and records exact test
images/packages. Seven separately reviewed desktop cases, including Mint with
Cinnamon, remain required before qualification. See [Linux validation](LINUX_VALIDATION.md)
for scope, commands, evidence and the exact-byte release gate.

Publishing the source repository and distributing a supported application are
separate steps. Windows 0.1.0 portable and its separately built installer have
distinct qualification reports and share the same application bytes.

## Prepared delivery chain

PR validation, portable preparation and installer packaging use distinct stages.
Qualification applies to the exact tested artifacts, not to every future build.
Only **PR validation** and **Release** are manual entrypoints. The five other
workflow files are reusable jobs called by those entrypoints; they cannot be
dispatched independently and do not start additional push or PR runs.

| Workflow | Trigger and output | Authority |
| --- | --- | --- |
| `pr.yml` | PRs and main pushes; select affected checks on Windows, Linux, macOS ARM64 and Intel. Shared/unknown changes select all platforms. Documentation-only changes, and changes limited to the release tooling listed in `tools/ci_scope.py`, avoid native builds. | Read-only tests and one shared RustSec audit, followed by the required `PR checks` aggregate. Windows and Linux also draw the caption effects with Mesa's software OpenGL. No distributions. |
| `release.yml` | Manual dispatch on main: `verify` (default) tests portable packages without uploading them; `build` uploads candidates; `installer` wraps the Windows portable; `rehearse-publish` and `publish` gather and verify a build, its installers and its Linux desktop run; `acceptance` tests the published files; `promote` makes the accepted pre-release the latest version; `draft` is the former route. The default `windows-linux` target selects Windows x64 and Linux x64; individual targets and `all` (including macOS ARM64) are available. | Preflight before expensive work. Write tokens only in the staging, `publish`, `acceptance` (report attachment only; that job runs no downloaded file) and `promote` jobs. `publish` publishes a pre-release; only `promote` changes the latest release. The jobs that run installers or the application hold no token. |
| `windows-installers.yml` | Reusable Windows installer stage called by Release on main. Produces Minimal, Standard and Complete offline installers plus a Web installer using the same wizard. Tests exact profile contents, installation, repair, cancellation, profile mismatch rejection, an upgrade from the last published version (`assets/upgrade-baseline.lock.json`) and removal before creating an unpublished review draft. | Reuses the released application bytes; records the wrapper commit and run separately. The installed interface starts with Mesa's software OpenGL, placed beside it for the test and removed before uninstalling. |
| `delivery-candidates.yml` | Reusable Release build engine. Synthetic delivery controls also run inside PR validation. | Read-only token; optional Mac signing uses separate protected steps. Artifact upload requires reviewed redistribution materials. |
| `delivery-draft.yml` | Reusable Release verifier and draft creator. | Exact run/attempt/commit and artifact verification; independently reviewed inert draft upload. |
| `rust-audit.yml` | Reusable audit called by PR validation and Release. No NOH compilation. | Vulnerabilities, unsoundness advisories and acquisition errors fail the job. Public diagnostic uploads are optional. |
| `native.yml` | Reusable Linux and macOS verification called by PR validation. | Both Mac architectures remain covered; development evidence, not public packaging. |

The five Windows measurement workflows and the private `windows-setup.yml` and
`update-release.yml` prototypes have been retired. Their sources and past runs
remain in Git/Actions history; public automatic updates remain disabled. The
current Windows installer workflow preserves the three content profiles and web
selection in ordinary per-user public installers. It does not enable the private
updater/controller protocol. Minimal includes the application, CLI and MCP but
no FFmpeg, preview runtime or speech files. Standard adds FFmpeg and preview;
Complete adds Whisper and models. The Web installer embeds Minimal and downloads
the selected component archives from the official release, verifies their
compiled-in SHA-256 with progress and cancellation, and installs them in the same
wizard. It explains and cleans up its temporary storage. Existing installations keep their profile and folder;
the selected content can grow without uninstalling. Minimal setup and the installed
`noh-components.exe` helper share the acquisition/extraction code. The GUI starts
that helper from its missing-resource panel and rechecks availability afterwards.
Profile templates and content registration preserve additions during repair.
User-created files survive removal; downgrades and incompatible helper targets are rejected.

The installer stage does not run Cargo. All profiles reuse the qualified
portable's application executables and retain its notices and source-material
correspondence. Each has a derived manifest recording its actual inventory;
application source identity is distinct from the wrapper/workflow commit.
Public download instructions are updated only after exact artifact acceptance
and publication. The original complete installer remains the currently qualified
public download until that acceptance finishes.

Native builds and standard tests have passed for all three operating systems,
including both Mac architectures. That evidence does not qualify an installable
package. Linux x64 now has an AppDir packager and an Ubuntu 24.04 Release job;
Linux 0.1.3 has [exact-byte qualification](LINUX_0_1_3_QUALIFICATION.md) within its
recorded Ubuntu/WSLg limits. Future artifacts require their own qualification. macOS Intel remains
excluded from Release targets. The public Windows installer stage has passed on hosted runners;
the older private update/install workflows are retired. See the
[installer report](WINDOWS_0_1_0_INSTALLER.md) for the exact run and local acceptance.

The build driver is `tools/delivery-build.ps1`; the acquisition, source and ZIP
controls are in `tools/delivery.py`. Standard runners are `windows-2025` x64,
`ubuntu-24.04` x64 and `macos-26` Apple Silicon. They use Rust 1.98.1, the locked Cargo graph, the
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
diagnostics remain bounded in both modes. Hosted Windows and Linux portable runs
have completed; acceptance applies only to the recorded exact artifacts.

`sign_macos=false` remains the default. An explicit main-branch portable Mac run
can use the protected `noh-macos-signing` environment and the prepared Developer
ID/notarization helper. This privately submits software to Apple after owner
approval; see [MACOS_SIGNING.md](MACOS_SIGNING.md) for exact inputs and native
checks. Unsigned jobs receive no signing credentials. No real signing/notarization
or environment setup has run in this preparation.

`portable` uses `cargo dev build --gui --mcp`, after `delivery.py acquire-datasets`
and `build_syllable_dicts.py --inputs` have built the language packs it reads from
`NOH_LANGUAGES` (Windows and Linux). Both modes reject existing `dist/`
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


Candidate assets are named `NOH-<version>-windows-x64.zip`,
`NOH-<version>-linux-x64.zip` or `NOH-<version>-macos-arm64.zip`,
with a separate `-materials.zip`, `DELIVERY.json`
and `SHA256SUMS`. The JSON explicitly says `unqualified`; it records exact Git
identity, target/features/profile, run/attempt, complete file hashes, archive
hashes, download sizes and extracted bytes. Files of 2 GiB or more are refused.
ZIP verification rejects traversal, duplicate/colliding paths, symlinks, extra
files and altered contents. These assets are developer candidates, not advertised
downloads. Source materials preserve the NOH Git tree, locked vendored Rust
crates/notices, generated license metadata, native materials and build environment.
Rust notices are generated from the complete vendor graph before packaging and
copied into the Windows, Linux and macOS packages. Missing original texts remain
visible in `RUST-NOTICE-INVENTORY.json`; no license choice or clearance is inferred.
The same input folder includes the original generated Rust standard-library HTML
notice. Its pinned hash, compiler release/commit and installed rustc component
manifest are checked before packaging; the shared packager rechecks that hash.
This is original notice material, not a redistribution attestation. Native Mac
component correspondence and complete compiler/native notices remain unreviewed.

Artifacts expire after seven days (independent staging after three). Compression
is disabled for artifact uploads of already compressed ZIPs. There is no
large-runtime PR upload or automatic publication. Source/bottle/vendor staging can
consume substantial runner disk space; actual hosted-runner capacity/time must be
measured.

The Linux build restores two caches. Only a Release run on main writes them, a
`verify` run included. GitHub drops a cache that nothing has read for seven days,
and the least recently read ones once the repository holds more than 10 GB: they
usually serve the builds of one release, not the first build of the next. The first
holds downloads: each file is compared with its pinned SHA-256 before every use, and
the folders restored beside them are removed before the build, so Whisper is compiled
again each time. The second holds compiled Rust dependencies, which are used as
restored: nothing compares them with anything. The Windows build restores no cache.

## Current review boundary

### Linux delivery preparation

`Release` selects `windows-linux` by default. Its two independent candidate jobs
use Windows 2025 and Ubuntu 24.04; `all` additionally includes macOS ARM64.
The same selection is checked when staging the exact build run, so a missing,
failed or skipped Linux job cannot produce a combined Windows/Linux draft.
The Windows-only installer stage still requires `windows-x64` explicitly.

The Linux job checks host FFmpeg/libmpv against `assets/linux-host.json`, builds
the pinned CPU Whisper recipe, and calls
`cargo dev build --gui --mcp --portable --speech ... --ffmpeg /usr/bin/ffmpeg`.
The AppDir contains GUI/CLI/MCP and CPU speech/models. It contains no shared
libraries and no private ELF search paths. The host owns the entire media and
desktop runtime. The old native lock/acquisition tool is historical provenance,
not an input to new packages. The declared
qualification baseline is Ubuntu 24.04 x86_64, glibc 2.39. A compatible glibc
version alone does not establish acceptance on another distribution.

`ELF-IMPORTS.json` records the complete ELF import closure. Seal and inert archive
verification repeat that audit; the latter also checks ELF/AppRun executable
permissions. The new manifest kind also rejects any extra ELF or private
provider and requires the exact host contract. Source materials include the
Whisper source/build receipt, Rust vendor sources and original notices; the
host package manager supplies the media packages and their sources/notices.
Linux redistribution and 0.1.3 CI-byte qualification have exact-hash records in
`assets/delivery-policy.json`. See [redistribution inputs](LINUX_REDISTRIBUTION.md)
and the [final-byte report](LINUX_0_1_3_QUALIFICATION.md). Changed inputs or a new
CI delivery invalidate that acceptance; local QA builds do not clear the gates.
The host-runtime architecture needs a fresh exact-input redistribution review;
the existing published-package authority is deliberately not reused.

Linux desktop acceptance must also exercise **Add files** with an actual host
file chooser and confirm that selected media enters the application. Record the
XDG Desktop Portal backend or `zenity` prerequisite. Test native file-manager
drag-and-drop separately when available; supplying fixture paths directly does
not cover either interaction, and Windows Explorer-to-WSLg drops are unsupported.

### Existing platform evidence

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
exact downloaded-byte attachment/Defender checks. The
[installer report](WINDOWS_0_1_0_INSTALLER.md) adds wrapper and installation acceptance.
Mac material review, native packaging and
signing/notarization remain unresolved; Windows acceptance does not clear Mac gates.

Public Actions binary artifacts are distributions. Before uploading them, the
redistribution record must match the exact hashes of Cargo.lock, NOH's license,
the runtime/model/recipe locks and notice collectors. It binds SHA-pinned evidence
for `native_corresponding_sources`, `native_notices`, `rust_sources_notices` and
Windows `cuda_terms`. Changed inputs invalidate that authority. A workflow input
or publication token cannot clear missing evidence. Preflight checks this before
an expensive retained build. The non-uploading `verify` mode remains available
for investigations; it does not produce a reusable public package.

## Version numbers

From 0.2.0, NOH follows [Semantic Versioning 2.0.0](https://semver.org/).
A version is `MAJOR.MINOR.PATCH`. While MAJOR is 0:

- **PATCH** (0.2.0 → 0.2.1): corrections only. Nothing in the list below is
  added, removed or changed.
- **MINOR** (0.2.x → 0.3.0): anything else — something is added to the list, or
  something in it changes or goes away.

What a change is measured against — the list as known on 2026-11-16, completed
when something else is found to be relied on:

- the command line of `noh`: commands, options, what it prints for other
  programs (`--json` output) and its exit codes;
- the MCP server: tool names and their schemas;
- the environment variables NOH documents, such as `NOH_FFMPEG`;
- the files exchanged with NOH: lyrics files (`.noh-lyrics.json`), the `.srt`
  files it reads and writes, preferences;
- the installers as people meet them: profiles and download file names;
- the supported systems and what must be installed on them.

When automatic updates are switched on, what an installed version reads from a
newer release joins the list. A file written only to help diagnose a problem,
such as `startup.log`, is not in it: its place is documented, its content may
change.

1.0.0 will mean the list is stable: from then on a breaking change needs a new
MAJOR. Until then a MINOR may break it, and the changelog says so.

The changelog's open section is headed `## Unreleased`. The release commit
chooses the number from what that section holds — only corrections, a PATCH;
anything else, a MINOR — and renames the heading. Versions up to 0.1.6 were
numbered before this rule and are not renumbered; 0.1.6 added functions and
would have been a MINOR. 0.2.0 is the first version numbered by the rule: a
MINOR, because it adds a second player for the montage, a cache of preview
copies on disk and documented environment variables: `NOH_PLAYER` in the
user's guide; `NOH_PROFILE`, `NOH_CACHE_DIR` and capture switches in the
developer's.

The "pre-release" of the publication below is GitHub's flag on the final number
while its files are tested. It is not a Semantic Versioning pre-release: a
number never carries a suffix.

## Publication from 0.1.6

Releases are built, published, tested and promoted on GitHub. No step runs on a
maintainer's machine, and each stage is one manual dispatch of **Release** on main.

Before a release, refresh the Arch pin of `assets/linux-desktop-images.json` in a pull
request if its address no longer answers: Arch keeps an image for some weeks, and the
desktop run of step 4 fails on an address that no longer exists. Check it on the day
of the desktop run, whatever the release commit pinned. A new pin states its address
and the digest its publisher gives; the probe checks that digest at download.
openSUSE Tumbleweed needs no refresh: its publisher keeps one snapshot at a time, so
each run takes the current one and checks the digest openSUSE signed
([LINUX_DESKTOP_PROBE.md](LINUX_DESKTOP_PROBE.md)).

Before the build, fetch the two syllable datasets as the build does, into an empty
folder: `python tools/delivery.py acquire-datasets --cache <empty folder> --output
<folder>`. The build stops when one of them cannot be fetched or no longer has its
pinned digest, and both happened on 2026-11-18: the first address of the English
archive was down for maintenance, which its mirrors in
`assets/syllable-sources.lock.json` now cover, and the publisher of the French one had
repacked it the day before. A changed archive is a changed redistribution input: its
new pin is reviewed and recorded before any build.

1. Merge the release commit through PR checks. It is written as if the version
   were already published: the changelog entry is `## <version>`, without
   "unreleased". The `publish` stage refuses an entry still marked unreleased.
   The stage reads the entry from main as it is when it runs, not from the built
   commit: the changelog is not in the packages, so a correction merged after the
   build reaches the release notes without building again. The notes then name the
   commit the text was read from; the tree under the tag keeps the older text.
2. `stage=build`, `target=windows-linux`: one build of that commit. Note the run
   ID, attempt and commit. When one platform fails, the run stops the other one:
   publication takes both packages from the same attempt, so the build is
   dispatched again. `stage=verify` lets every platform finish.
3. `stage=installer`, `target=windows-x64`, with that run, attempt and commit: the
   Windows installers wrap the same application bytes and are tested, including an
   upgrade from the last published version.
4. Dispatch **Linux desktop candidate checks** on main with `suite=all` and the same
   build run, attempt and commit: every required desktop starts the exact Linux
   package in a virtual machine. Failed desktops may be re-run in the same run; a
   new run must cover all of them. Its screens are kept as evidence; nobody
   reviews them.

   Re-run a failed desktop only after reading why it failed. Two intermittent
   failures are known on Debian 13 and may be re-run: the interface capture that
   never ends (`gui-timeout.json` is present; seen in 3 of 23 runs; the player
   reports the end of the clip after about one second, cause under investigation), and the file dialog left open
   ("The Zenity fallback remains running"; seen once, the test's keys did not reach
   the dialog). The release notes then name the desktop and the attempt that
   passed. Any other failure stops the release.
5. `stage=publish`, with the build run, attempt and commit, plus `installer_run_id`,
   `installer_attempt` and `desktop_run_id`: every file is checked against a digest
   recorded by those runs, then published unchanged under the final tag `v<version>`
   as a **pre-release** that is not the latest release. The stage creates a draft
   first; if anything fails while it is being filled, the stage deletes that draft,
   which holds nothing public and creates no tag, so the stage can be run again.
   If the last step refuses to publish the filled draft, the draft stays and
   nothing is public. Inspect it and delete it by hand before running the stage
   again: a new run would otherwise take that draft over, fail on the files each
   run produces anew, delete it and fail. It refuses an existing tag.
   `stage=rehearse-publish` takes the same inputs and does everything except the
   one request that publishes, including the checks that request is preceded by,
   then deletes the draft: nothing is published, and its draft carries a
   `rehearsal-…` tag, never the real one. Every change the stages make to a release
   names its tag and commit: GitHub replaces the tag of a draft changed without it
   by an `untagged-…` placeholder, and the first publication of 0.1.6 stopped on
   that. The
   published release carries `PUBLICATION.json`, the list of files and digests this
   run verified. The same list is kept as an artifact of the run before anything
   becomes public: acceptance and promotion rely on that artifact, not on how the
   run ends. If the stage fails after the release became public, do not run it
   again and do not delete anything: read its message. A version is lost only when
   the public state itself is wrong, for example a pre-release that became the
   latest release.
6. `stage=acceptance`, `tag=v<version>`: the published files are downloaded as a
   user downloads them, checked against `SHA256SUMS` and exercised on Windows and
   Linux. The result is attached to the release as `ACCEPTANCE.json`, the only file
   added after publication.
7. `stage=promote`, `tag=v<version>`: when the attached report passed for the exact
   files the publishing run verified, and comes from a successful acceptance run on
   main, the same release becomes the latest version. Nothing is rebuilt.
   From the version that introduces automatic updates, promotion also sends the
   release to every installed Windows copy within a day: NOH reads GitHub's
   latest release and installs its Minimal installer (`src/auto_update`). A
   pre-release or a draft is never offered.

Release files can be replaced after publication by anyone who can write to the
repository. Acceptance and promotion therefore do not trust the release's own
`SHA256SUMS`: they compare every file with the artifact of the publishing run, and
the acceptance report with the artifact of the acceptance run. The automatic
update cannot make that comparison: it trusts the digest GitHub reports for each
file of the latest release, so a file replaced after promotion reaches installed
copies.

Time limits come from artifact retention. Build packages and desktop results are
kept 7 days: the installer, desktop and publish stages must follow the build within
that time.
The publication and acceptance artifacts are kept 90 days: promote within that
time. Attaching `ACCEPTANCE.json` requires that immutable releases stay disabled
for this repository.

A defect found after publication is fixed in the next version number, a PATCH
unless the correction changes something in the list above. A published
tag is never moved and its files are never replaced.

Evidence lives beside the packages, not in this repository: `INSTALLER-TESTS.json`,
`installer-test-evidence.zip`, `INSTALLER-PROVENANCE.json` and `ACCEPTANCE.json` are
release files, and each states what was observed and its limits. Hosted runners have
no hardware GPU, audio device, NVENC or CUDA: none of these is exercised before a
release, and the release notes say so.

After a release is promoted, a pull request moves `assets/upgrade-baseline.lock.json`
to that release's Complete installer, so that the next version is tested as an
upgrade from it.

The two sections below describe the former route, in which a qualification record
was committed to this repository before a draft was prepared. They are kept for
reference and are not used to publish from 0.1.6. The redistribution gate above
still applies to every stage that handles packages.

## Former qualification record

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
   Windows x64, Linux x64 and macOS ARM64; Intel Mac remains excluded.
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
compiled source commit at initial publication. A subsequent documentation/qualification commit may add
the release link and evidence without rebuilding or relabelling that binary.
The hosted draft path keeps its separate run/protected-environment requirements.

When completing a published release with a missing platform, preserve the
published tag and existing binary/material archives. A later build from main
may supply the missing platform only if its full diff against the tag contains
release documentation/evidence and delivery-policy records exclusively. Verify
the exact diff during final review; any application, dependency, recipe or
workflow change requires a new release version. Publish each platform's actual
compiled commit, run/attempt, delivery digest and corresponding sources in the
release notes and provenance. Never substitute the original tag for a later
build's real revision. The new platform and its wrappers still require all
input, CI, native exact-byte and independent publication checks above.

The ordinary Windows installer is a separate wrapper around those exact portable
bytes. `tools/public_components.py` first derives the two runtime ZIPs and
`COMPONENTS.json` from the verified portable. `tools/public-installer.py` builds
the maintenance helper with `--maintenance --profile minimal`, then builds each
offline profile with `--profile <minimal|standard|complete> --helper <helper-exe>`.
Each invocation also requires `--bundle <portable-folder> --commit <source-sha>
--components <component-folder> --compiler <Inno-Setup-7.1.0-folder> --output <new-folder>`.
It verifies the portable manifest, content archives, input authority and compiler
pins before generating its exact file list. Build Web with the same
`tools/public-installer.py --web --profile minimal --helper <helper-exe>` and
the matching bundle, component catalogue, compiler and output arguments above.
Web acquires components through the shared wizard; it does not launch an offline installer.
It does not rebuild NOH. Preserve `INSTALLER.json`, the wrapper source ZIP and
native install/uninstall evidence alongside the original delivery identity.
Qualify this additional executable independently before advertising it.

Use the existing **Release** workflow on main with `stage=installer` and
`target=windows-x64`, plus the successful Release build's `source_run_id`,
`source_attempt` and `source_commit`. This calls the public job in
`windows-installers.yml`; there is no branch-specific build or publication path.
It verifies the source run, mandatory jobs, immutable artifact digest, portable
and corresponding materials before compiling the checked-in wrappers. The application
is not rebuilt. Its receipt distinguishes the application commit from the wrapper
workflow commit and run/attempt, and records the actual installer SHA-256.
The hosted test installs into a disposable path containing spaces, checks every
installed payload hash, launches the GUI and CLI, reinstalls, and uninstalls while
checking that a user-created file survives. It also adds media and speech to
Minimal, repairs missing tools, retains additions during a Minimal reinstall,
and rejects a corrupt component archive before changing the installation.
Silent installation does not exercise
the Microsoft prerequisite wizard. Existing portable media qualification remains
valid for the unchanged installed application bytes.

A hosted runner has no OpenGL 2.0. The installer stage therefore places Mesa's
software OpenGL beside the installed application, starts the interface, requires its
capture, then removes those two files before uninstalling. The archive is pinned in
`assets/software-opengl.lock.json`; it is test material and is never packaged. A
launch error fails the job. This shows that the interface starts, not how it behaves
on a hardware GPU. PR validation keeps the archive in the Actions cache; the installer
stage downloads it from its publisher on each run and cannot pass while that file is
unreachable.

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
