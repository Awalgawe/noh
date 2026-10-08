# Faster CI and reliable releases: a reusable playbook

This guide captures lessons from NOH that can be applied to another project.
Keep the method; replace the paths, platforms, keys and acceptance criteria.
Research date: 2026-10-06. Provider features and prices must be checked again
before choosing infrastructure. Contributor checks are in [DEVELOPMENT.md](DEVELOPMENT.md).

## Validate PRs without producing a release

`PR validation` (`.github/workflows/pr.yml`) runs on PRs, on pushes to `main`
and `codex/setup-release`, and by manual dispatch. It tests the PR merge checkout.
Its first job validates the platform selector and runs synthetic script controls
before any application compilation. The complete local Git diff includes deleted
files and both sides of renames; unavailable comparisons select every platform.

| Changed inputs | Native validation |
| --- | --- |
| Only README, AGENTS or Markdown under docs | No application build |
| Explicitly listed Windows tools and locks | Windows QA |
| Unix verification script or native workflow | Linux and macOS, both Mac architectures |
| Rust, Cargo, shared tools, embedded resources, languages, routing or unknown inputs | Windows, Linux and both Mac architectures |

Windows reuses `windows-qa.ps1`, the `qa` profile, matching GUI/CLI/MCP/update
workers and the existing synthetic media tests. Linux and macOS reuse the native
suite, including update tests and existing GUI captures. No path produces release
profiles, signed Setup packages or installers. The Windows job acquires only the
pinned compiler and test FFmpeg, without speech models or the delivery runtime.

The stable final check is **PR checks**. It succeeds only when the selector and
every selected job succeed; unselected jobs must be skipped. The workflow itself
has no path filter, so documentation PRs also get a completed check. Configure
this check in repository branch protection when adopting the workflow; adding the
file does not change branch protection. A newer run cancels obsolete work for
the same PR. Shared audits are performed once by the orchestrator.
PR and native diagnostic artifact uploads are best effort: exhausted artifact storage must not turn
successful tests into a failed code check. Compilation, tests and audit failures
still fail the gate. When upload fails, inspect the workflow logs; native capture
review still needs accessible images before claiming visual acceptance.

Caches retain compatible Cargo dependencies and verified tool/runtime downloads.
PR caches remain scoped by GitHub to their merge ref; pushes to the base branches
populate reusable caches for later PRs. A first run can still compile dependencies.
Changed Rust code and test executables still need compilation; this workflow does
not claim independent compilation of every application feature or a measured time
saving before its hosted run. Release/native installation qualification remains a
separate, explicitly selected delivery operation.

See GitHub's [workflow trigger and required-check behavior](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)
and [cache access restrictions](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#restrictions-for-accessing-a-cache).

### Hosted PR qualification, 2026-10-08

[PR 2](https://github.com/Awalgawe/noh/pull/2) targets `codex/setup-release`.
The initial full PR qualification used application/test commit
`c9d340a730db9178ebff6faefab465d42d2c1322`, in
[run 37777472449](https://github.com/Awalgawe/noh/actions/runs/37777472449).
At that checkpoint, the pipeline blocked on real failures and the PR was **not
fully validated or merged**. No distribution was produced.

| Selected check | Result | Observed job duration |
| --- | --- | --- |
| Routing and synthetic script controls | Pass | 7s |
| Shared RustSec audit | Pass | 15s |
| Windows QA | 567 passed, 26 existing ignores | 13m34s |
| Linux | Fails the VFR final-duration warning assertion in `caption_geometry` | 6m10s |
| macOS ARM | 516 tests pass, 12 existing ignores; two of four GUI captures fail | 5m58s |
| macOS Intel | 516 tests pass, 12 existing ignores; captures pass | 23m33s |
| PR checks | Correctly fails because selected platforms failed | 7s |

The Windows Cargo cache was an exact hit. Recorded worker compilation took
103.90s, test compilation 197.53s and test execution 350.53s. These are phase
observations, not a controlled speedup against the historical release workflow.
The earlier cold Windows job in run 37772349225 passed in 31m49s, with worker
compilation taking 825.54s. No extra run was launched merely to measure a cache.

The first run, 37771923153, stopped before application compilation because the
artifact quota rejected the audit-log upload. PR diagnostic uploads are now
best effort; actual audit/test failures remain blocking. Run 37772349225 then
exposed an unconsumed headless texture delta and an Intel cancellation-test hang.
The test-only fixes clear the delta and bound loopback fixture writes, with a
new cleanup check. The final run passes those tests on Unix. An extracted
standard-library-only fixture was also exercised locally; both bounded and
unbounded variants passed on Windows, so that local control does not reproduce
the former Intel hang.

The initial Linux failure was a fixture that assumed an unknown final-frame
packet duration; the observed FFmpeg 6 backend instead supplied a duration. The
fixture now uses sub-millisecond nominal packets with explicit VFR timestamps
and asserts the zero-duration precondition before retaining every original warning,
frame-count and presentation-time assertion. A focused Windows QA test passed;
[diagnostic run 37789927499](https://github.com/Awalgawe/noh/actions/runs/37789927499)
at `20e6f2e044e82d0b93b6e58e6d9afc63cc56f4fc` also passed it with Linux FFmpeg 6.1.1.

That diagnostic passed 516 functional tests (12 existing ignores) and all four
captures on Mac ARM in 5m18s. Its earlier large-window failures reported 980x656
instead of 980x850. The workflow now sets the ARM runner desktop to 1920x1080
unscaled, verifies the effective mode before compilation, and preserves the
requested capture sizes and readiness checks. Both desktop inventories are logged.

Linux passed 514 functional tests (12 existing ignores) in the same diagnostic;
the job then failed on all four captures because `libxkbcommon-x11.so.0` was missing.
The workflow installs the corresponding `libxkbcommon-x11-0` runtime package.
This package addition changes a runner prerequisite, not the application or its
capture assertions. Its acceptance, like that of the complete PR, requires a
successful final **PR checks** run; a targeted dispatch does not supply that gate.

Native manual dispatch offers `all`, `linux`, `mac-arm`, `mac-intel` and
`linux-mac-arm` choices for bounded diagnosis. PR and reusable-workflow calls
always retain all three Unix runners. Native audit/capture archives are optional
when artifact storage is full; actual audit, build, test and capture failures remain
blocking, and bounded capture errors are printed in job logs.

Local receipts and downloaded logs are retained under `.mcp-dev/ci-cost/pr-validation/`,
including each run's terminal record and per-job logs. Existing Windows/Intel
results, targeted follow-ups and complete PR validation are separate evidence.
Automated captures do not establish manual visual or physical-audio acceptance.
GitHub returned HTTP 403 for branch protection and rulesets on this private
repository, requiring GitHub Pro or public visibility. No subscription or repository
visibility change was made; artifact quota recovery does not remove that restriction.
The current merge candidate's checks are available on [PR 2](https://github.com/Awalgawe/noh/pull/2).

## Measure the expensive stages first

Export step start/end times from the CI provider and retain command durations.
Separate provisioning, downloads, dependency compilation, application compilation,
tests, packaging, upload and native acceptance. A failed run is useful evidence,
but its partial packaging time is not the duration of a successful release.

NOH's [run 37432984735](https://github.com/Awalgawe/noh/actions/runs/37432984735)
provides this baseline:

| Stage | Observed time | Interpretation |
| --- | ---: | --- |
| Compiler/packager preparation | 78 s | Repeated tool downloads are avoidable, but are not the largest cost. |
| Compile workers and run tests | 1,375 s | Includes compilation and 557 passing application tests plus six release-tool tests. Do not call the whole duration test execution. |
| Acquire full runtime and source materials | 414 s | Cache pinned downloads; retain extraction and verification. |
| Release A build and partial packaging | 1,045 s | Stopped during Standard preparation; version B was not built. |

The subsequent successful [run 37439140827](https://github.com/Awalgawe/noh/actions/runs/37439140827)
reused those application tests with the bounded provenance check described below.
It spent 77 s preparing tools, 560 s acquiring runtimes, 419 s compiling/testing the
corrected release tool, and 1,702 s building A/B and sealing all six profile packages.
These are provider step durations, not isolated compiler timings. That run predates
the four-layer cache changes, so it is not a warm-cache measurement.

The first successful B-only automation run,
[37469621614](https://github.com/Awalgawe/noh/actions/runs/37469621614), took
**62m20s** at application commit `944525b`. All four installers and ten assets were
verified in the private, unpublished draft. Native acceptance of these new bytes
remains pending; the accepted 0.1.1 baseline is unchanged.

| Stage | Observed time |
| --- | ---: |
| Candidate job, including the stages below | 54m38s |
| Restore Rust / producer tools / Cargo dependencies | 6s / 5s / 22s |
| Prepare compiler and packager / fetch Cargo inputs | 23s / 28s |
| QA workers, test compilation and execution | 19m44s |
| Acquire full runtime / save runtime download cache | 6m36s / 1m54s |
| Build B, prepare profiles, seal packages | 24m05s |
| Upload candidate artifact / diagnostics | 22s / 2s |
| Installer job, including the stages below | 5m57s |
| Download authenticated candidate / prepare and cache Inno | 57s / 9s |
| Build three offline wrappers / web wrapper | 23s / 4s |
| Stage six payload/envelope assets | 2m29s |
| Retain exact installers / upload four wrappers and verify all assets | 18s / 1m18s |

Totals include orchestration overhead; the short preflight (6s) and audit (19s)
run concurrently. Nested rows must not be added to their enclosing job totals.
The candidate artifact is 1,587,009,670 bytes. The QA block contains worker
compilation (2m21s), test compilation (4m57s), 559 passing tests (402.70s summed
execution), a separate release-tool test compilation (4m17s) and six passing
release-tool tests (0.31s), plus FFmpeg preparation and command overhead.
The B build contains QA helper compilation (4m19s), release compilation (13m01s)
and bundle preparation. Packaging takes Complete 186.43s, Standard 29.48s and
Minimal 6.97s; the remainder includes source freezing, hashes and signatures.

The logs show an exact Cargo cache hit of 330,263,629 compressed bytes from the
previous failed QA run, followed by `Cache up-to-date.` at job end. QA dependencies
were reused, but release dependencies compiled in this successful run were never
saved: the immutable exact-hit entry was already considered complete. Rust and
tool archives were warm; the runtime download cache was cold. This is not a
fully warm release measurement and does not demonstrate an overall speedup.
The correction below needs one fill on the next necessary run and a comparable
subsequent run to measure its benefit. Do not launch a release just to benchmark.

Detailed evidence is retained in the run's job logs and small diagnostic artifacts;
the local verified receipt is `.mcp-dev/setup-ci/37469621614/automation-report.json`.

### Correct redundant compiler configurations before another release

Functional delivery success does not complete the CI performance work. The next
investigation found two avoidable sources of compilation in that measured run:

- `updates`-only release-tool tests and a helper with no optional features used
  different dependency graphs from the `gui,mcp,updates` QA suite. The release
  workflow now includes both examples in one test command and compiles both
  helpers with the same feature set. All six release-tool tests remain required.
- A source snapshot beneath the checkout carries another `.cargo/config.toml`.
  Cargo [merges configuration arrays from parent directories](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure),
  so the LLD rustflags occurred twice in the snapshot. A small actual Cargo probe
  reproduced this, as did local dependency fingerprints. An explicit job-level
  `RUSTFLAGS` keeps one identical LLD flag in both trees; `linker = "rust-lld"`
  remains configured. A conflicting encoded flags environment is refused.

The runner still compiles the development helper inside its source snapshot and
executes that exact helper immediately afterward. Its embedded manifest directory
therefore remains correct. The new Windows QA script reuses the existing bounded
process runner to record FFmpeg preparation, workers, test compilation and actual
test execution separately. Production also records helper compilation separately
from release compilation/bundling. No test is removed to improve a timing number.

The explicit `verify_build_graph` dispatch validates these changes without creating
a release. It restores the historical QA cache read-only, with the old environment
only during the restore action; every compiler invocation uses the corrected
explicit flags. The old root artifacts already used the same single flag. The
comparison requires an exact cache hit and otherwise stops before compilation.
It never writes the partial reference into the production v2 cache. It runs QA,
builds the verifier and a versioned snapshot helper, and invokes only `--help` on
the helper. Full runtime acquisition, signing, packaging, release assets and
installer production are skipped. The local snapshot's version is not a new
release or a qualification claim. Measurements from this mode establish phase
costs only; they are not a measured total for a complete release.

The targeted [run 37481473432](https://github.com/Awalgawe/noh/actions/runs/37481473432)
at `a079532` passed all 565 tests with the same 26 existing ignored tests. Its
candidate job took 18m07s, excluding release production by design. The reference
Cargo cache was an exact hit and was not saved or modified.

| Comparable phase | Previous run | Corrected run |
| --- | ---: | ---: |
| Compile tests, including the release tool | 297s + 257s | 289.48s |
| Compile the snapshot development helper | 259s | 104.38s |
| Build the verifier executable | 17.07s | 4.82s |

The first two rows represent about seven minutes less compilation in this
comparison. Helper diagnostics show 279 fresh dependencies and only NOH rebuilt,
with its manifest directory in the snapshot and a single LLD flag in all three
actual compiler commands. This validates dependency reuse rather than inferring
it from the cache hit. Workers took 158.91s, test execution 362.40s, test runtime
preparation 4.17s and formatting 2.47s. These QA phases total 817.44s, compared with
the previous 1,184s enclosing QA step; that broader difference also includes
download, test execution and orchestration variation. Do not attribute it all to
compiler changes. The local verified report and authenticated diagnostics are in
`.mcp-dev/ci-cost/37481473432/`. No full-release total or production v2 warm-cache
benefit has been measured by this targeted run.

## Decide what the change actually invalidates

Apply this rule to every stage, not just dependency installation. Before starting
work, record the changed inputs, the outputs that depend on them and the existing
evidence that remains valid. A new Git commit alone is not a reason to repeat all
release operations. Reused binaries retain their original embedded source identity.

| Change | Required work | Reuse when still valid |
| --- | --- | --- |
| Documentation or release notes | Review the text and links; regenerate a package only if its shipped content changed. | Existing binaries and functional/native results. |
| Inno wizard, translations or wrapper builder | Check script syntax, build the affected final wrapper and exercise changed installation paths. | Signed profile payloads and their application/update acceptance. |
| Packaging layout/profile selection | Run producer checks on a real bundle, regenerate/sign changed packages, accept those final packages. | Compiled application only when its complete build inputs and embedded configuration are unchanged. |
| Runtime/model lock | Fetch only changed objects, revalidate hashes, repackage affected profiles and exercise their runtime paths. | Unchanged downloads and unaffected profile evidence; assess embedded resource/build-identity inputs before reusing NOH. |
| Application, embedded resources, trust/feed settings, version or compiler flags | Build affected targets/configurations and run affected tests; validate final release output. | Compatible dependencies and external assets; unchanged independent outputs. |
| Tests or validation scripts | Build/run changed checks against the correctly identified application; renew the affected evidence. | Existing release candidate if those edits did not change the application being qualified. |
| Cache or workflow orchestration | Lint/review the workflow; measure the next required execution. | Existing application candidate and its evidence unless the execution semantics changed. |

For NOH, `windows-setup.yml` is dispatched explicitly with the new version.
It builds and signs only B, retains the exact candidate, then calls
`windows-installers.yml` to build all four wrappers and prepare a private draft.
Ordinary pushes do not start another expensive release build. Native PR checks
remain separate. The installer workflow can also be dispatched with a successful
producer run, attempt and commit; it does not install Rust or compile NOH.
An installer failure can reuse a successful producer even if the overall run
failed. Artifact creation time must fall within that exact successful job attempt.

Path filters save whole runs; Cargo's dependency graph saves compilation inside a
required run. Keep one compatible target directory, avoid `cargo clean`, and do not
change flags or target paths gratuitously. See the [Cargo build cache](https://doc.rust-lang.org/cargo/reference/build-cache.html).
GitHub path filters can leave required checks pending; do not make a filtered-out
workflow the sole unconditional merge gate. See [workflow triggers](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow).

There are deliberate limits today. NOH's build identity includes all `tools`,
`tests`, `assets` and `locales` inputs, plus the Git revision/dirty state and build
options. Some changes outside the Rust application therefore still invalidate a
new build's identity. Do not remove provenance inputs or pretend an older binary
was rebuilt from HEAD just to get a cache hit. The wrapper already records its own
source separately from the accepted application commit. General cross-commit
application reuse would require a separately reviewed compilation-input contract.

Application compilation and signed profile packaging still share one producer
job. A failure inside that job can lose unretained intermediates. The subsequent
installer job consumes an authenticated artifact, so an installer or draft failure
does not invalidate the successful producer. Keep boundaries only where they
remove a measured repeated cost, rather than splitting every command into a job.

The producer's `-NewVersionOnly` mode builds B once and packages its three profiles.
A is a genuinely older accepted release. `assets/setup-baseline.lock.json` pins the
private repository/release/asset IDs, sizes, hashes and original candidate catalog.
Six existing payload/envelope assets are reused without uploading them again. A
small support ZIP retains the original catalog, keys, source inventory, B release
descriptors, verifier and historical bootstrap. The catalog still lists its old A;
only the explicit historical B subset is required during acquisition/acceptance.
Never rewrite that catalog or rebuild/re-sign the old application.

Release assets do not use the CI artifact expiry timer, but can still be deleted.
The pre-build check verifies private release identity and every retained API digest;
it fails on missing/changed assets without rebuilding A. This checks availability,
not a fresh download of the 1.5 GB payload set. Actual historical acquisition uses
`python tools/setup_delivery.py baseline --lock assets/setup-baseline.lock.json
--output <new-directory>` and hashes every downloaded byte. Native acceptance then
uses `update-setup-clients.ps1 -Stage Accept` with the exact previous directory,
commit and catalog digest. `-ValidateInputsOnly` tests the input contract without
claiming signature verification or a native installation. Keep older baselines
when supported direct upgrades need them; rotation requires accepted new evidence.

## Private draft automation and recovery

Before dispatch, the local operator uses the already authorized GitHub credentials
with `setup_delivery.py slot --commit <full-commit> --version <version> --create
--receipt <receipt.json>` to prepare an empty private draft at the exact source
commit. A small CI preflight checks both that slot and the historical baseline
before the expensive producer. Only the preflight and installer jobs receive
`contents:write`; the producer remains read-only and gets its signing key separately.

This separation matters: GitHub requires push access to see draft releases, and
creating/editing a release whose target modifies workflows relative to the default
branch requires Workflows:write, which GITHUB_TOKEN cannot receive. CI therefore
only reads the prepared draft and uploads assets. It never creates, edits or
publishes a release, never changes its target to bypass that restriction, and never
stores a personal access token. An absent or mismatched slot fails before building.

The installer job first builds/authenticates the three offline wrappers, then stages
the six signed payload/envelope assets in an explicitly unqualified private draft.
Their GitHub asset API IDs feed the web wrapper. Credentials remain runtime-only.
All four final installers and manifests are retained as a CI artifact *before*
final upload. The uploader reuses only existing assets with the same name, size and
SHA256, refuses different bytes, and never deletes or replaces an asset. The final
receipt checks all ten assets, `draft=true`, `published_at=null`, source and tag.
Notes are static and explicitly say staging may be incomplete until the final CI
receipt exists; native acceptance remains pending. No notes PATCH occurs in CI.
The commit/version marker identifies the slot, while the receipt separately binds
the exact run, attempt, producer job, artifact digest, catalog and installer bytes.
See the [GitHub release API](https://docs.github.com/en/rest/releases/releases) and
[asset API](https://docs.github.com/en/rest/releases/assets).

For a partial final upload, reuse the retained installer bytes and `draft-plan.json`
with `setup_delivery.py draft --finish`; do not rebuild already uploaded wrappers
and assume their bytes are identical. The producer artifact/provenance and original
wrapper commit must still match. A conflicting draft is a real refusal, not a reason
to overwrite it. The existing qualified-draft script remains a separate path.
Neither CI success nor the previous release's qualification proves native acceptance
for a new version: the automated draft explicitly records it as pending.

Inno Setup 7.1.0 is pinned to its official archive SHA256 and checked with
Authenticode before installation on the disposable runner. Cache only that archive;
verify it on every restore. The existing builder verifies compiler pins again.
Its [command-line compiler](https://jrsoftware.org/ishelp/topic_compilercmdline.htm)
produces the same four wrappers without compiling the application.

Do not infer a 50% time saving. In run 37439140827, Cargo reported release compilation
of 10m22s for A and 4m42s for B, after QA helper compilation of 3m07s and 43s respectively.
B already benefited from A's compiled dependencies. Producing B alone still needs
those dependencies from compilation or a compatible cache. Complete packaging took
2m31s for A and 2m38s for B; Standard took 17/22s and Minimal 4/5s. These substep times
come from that run's authenticated diagnostics and exclude other hashing/copying work.

## Separate four reusable layers

| Layer | Cache identity | On every run |
| --- | --- | --- |
| Exact toolchain | OS image family, architecture, compiler version, components | Select the pinned version and verify it is usable. |
| Producer tools | Platform plus tool lock and preparation script hashes | Rehash archives and extract from the verified archives. |
| Language dependencies | Compiler/target, dependency locks, manifests, flags and build configuration | Invoke the build tool; let it decide what can be reused. |
| Native runtimes/models | All referenced locks and acquisition scripts | Check each downloaded object's pinned hash, then extract and validate. |

Do not put an entire home directory or temporary directory in a cache. Explicit
paths exclude credentials, signing keys, user data and unrelated caches. Keep
published artifacts separate: a cache hit is a build optimization, not release
provenance or proof that an executable passed acceptance.

NOH implements these layers in
[windows-setup.yml](../.github/workflows/windows-setup.yml). Its Rust dependency
cache covers the common `target` directory used by both `qa` and `release` and
by the frozen source snapshots. The cache action excludes workspace products;
NOH is rebuilt with its current version, trust configuration and source fingerprint.
LLVM-MinGW, .NET and Velopack cache only archives. The current extraction marker
is not an integrity attestation for a restored executable tree.

Save a verified download/tool cache immediately after its successful preparation.
Where the pipeline has a separate completed-compilation boundary, save reusable
dependencies before packaging. NOH's current combined producer does not expose
that boundary: its dependency cache now saves only after a successful job. This
deliberate compromise avoids freezing an incomplete cache after early QA failure,
but a failure on the first fill can still discard newly compiled dependencies.
Restrict cache writes to trusted release-development triggers. Dependency caches
are executable inputs and must not be populated by untrusted changes and then
consumed by a signing job.

Cache entries are immutable. If the first save followed an early failure, an exact
hit may retain only part of the dependencies; inspect actual compilation, not just
the hit flag. This happened in run 37469621614. NOH now uses a stable v2 dependency
cache generation and disables `cache-on-failure`; it does not rotate keys per run
or commit, clear unrelated caches, or rerun the completed release for this fix.

Read the pinned action's implementation when configuration behaves unexpectedly:
in rust-cache v2.9.2, [`shared-key` replaces `key`](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/config.ts).
Supplying both silently ignored NOH's custom tool/policy hash. The v2 configuration
puts that hash directly in `shared-key`. An
[exact restore hit](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/restore.ts)
also prevents the post action from enriching the same cache entry. A successful
restore step is not evidence that all expensive build outputs were cached.

Keep QA before release to fail early. The two profiles intentionally differ, so
reversing them cannot share compiled artifacts across their compiler settings.
Historical runs built the verifier and development helper with different
features from the GUI/MCP tests. The Windows producer now compiles both with
`gui,mcp,updates` in the QA graph. It compiles the development helper once before
injecting release options, then passes `build --source-root <snapshot>`.
Both Cargo commands and packaging use that explicit root. Omitting the option
still selects the helper's compiled-in `CARGO_MANIFEST_DIR`, so a changed working
directory alone is insufficient. The producer preserves the helper hash and
frozen source inventory; the helper rejects mismatched embedded speech/preview
descriptions. Explicit-root packaging is restricted to Windows because the Mac
packager still embeds its package version. The hosted helper phases took100.732s
for the old snapshot build and6.207s for reuse; the later diagnostic path assertion
failed and was corrected/validated locally. See the precise evidence scope in
[CI_PERFORMANCE_RESEARCH.md](CI_PERFORMANCE_RESEARCH.md).

Use immutable action commits. For Rust, NOH pins
[rust-cache v2.9.2](https://github.com/Swatinem/rust-cache/releases/tag/v2.9.2),
which keys dependencies from the Rust environment and supports excluding workspace
crates. It also removes incremental artifacts from the saved cache. Application
rebuilds and final linking still take time; caching cannot remove all compilation.

## Validate the cache, including its failure paths

Record these fields on the next required v2 fill and warm runs. The measured v1
run above exposed an incomplete exact hit and cannot substitute for this comparison:

| Measurement | Cold run | Warm run |
| --- | --- | --- |
| Commit, runner image, toolchain and feature set | Pending | Pending |
| Exact/fallback cache hit for each layer | Pending | Pending |
| Compressed bytes and restore/save duration | Pending | Pending |
| Dependency/application compilation duration | Pending | Pending |
| Total job duration and remaining failures | Pending | Pending |

Check that a missing cache still builds, a changed lock invalidates the appropriate
layer, corrupt pinned downloads fail verification, and an application edit rebuilds
the application. An earlier successful package must never replace a current build
because a cache key matched. Verify that a packaging failure still leaves reusable
dependencies for the next run. Reuse existing hash/cancellation tests when their
code is unchanged; add a test only for a newly uncovered failure mode.

Do not invalidate verified raw archives solely because extraction code changed.
NOH's E5 comparison reproduced the actual production miss, prepared test FFmpeg
and the full runtime, then restored the compatible legacy bytes under a key
derived from six locked input files and an explicit download-layout version.
The fallback is allowed only for that exact audited lock fingerprint; layout
changes require a version bump and a new compatibility decision. Every fetch
still checks its SHA-256. Record the matched key and refuse unintended prefix
matches, including when the cache action reports a fallback rather than an exact
primary hit.

In [run37559773345](https://github.com/Awalgawe/noh/actions/runs/37559773345),
the modified segment fell from525.615s to197.163s, including51s restore,9s policy
overhead and73s full cache promotion, with zero save cost charged to baseline.
The net5m28s saving passed the declared120s/20% criterion;114 test files and1,182
full-runtime files matched. The job itself cost789s plus2.52GB of cold downloads
and one2.47GB retained cache. This demonstrates the observed transition, not a
speedup for every unchanged future run or for the entire release pipeline.
Both policies can hit after their first successful fill. Charge migration costs
before selection and distinguish them from steady-state behavior.

Compare saved build time with cache transfer/cleanup time. Large overlapping caches
can cost more than they save. GitHub currently defaults to 10 GB per repository
and evicts entries unused for seven days; increasing storage beyond the default
can incur charges. Inspect size and eviction before changing limits. See the
[cache reference](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching).

## When a prebuilt image is worthwhile

A prebuilt image can contain stable compilers, SDKs and libraries. It still needs
versioning, refreshes, integrity checks and a cold rebuild recipe. Never bake
signing keys, repository credentials or user data into it. Compare its generation,
storage and maintenance costs with the setup time it actually removes.

GitHub currently supports custom Windows x64 images through larger runners for
Team/Enterprise organizations. That is a different infrastructure choice from a
standard `windows-2025` runner in a personal repository. Image storage is billed
separately. See [custom images](https://docs.github.com/en/actions/how-tos/manage-runners/larger-runners/use-custom-images).

For NOH, dependency/download caches are the first step. Consider an image only if
measured residual provisioning cost justifies the platform and operational cost.
A self-hosted runner is another option when its maintenance, isolation and uptime
are already owned; do not introduce an always-on machine just to avoid a small
installation step. A Linux container does not reproduce Windows installer or
native GUI behavior.

## Build once, package several profiles, qualify the delivered bytes

Compile one application version, then derive each complete content profile from
the verified bundle. Sign the profile identity as well as its inventory. Update,
retention and repair must preserve that identity. Removing files after signing
breaks the inventory and repair may restore unwanted content.

Use the actual producer manifest as the packaging contract. NOH initially added
an FFprobe requirement that its real bundle did not satisfy; a synthetic fixture
containing FFprobe hid the mistake. A cheap profile-preparation trial on an already
qualified real bundle caught and verified the correction without rebuilding NOH.

Bind native acceptance to the CI commit, original artifact digest, independent
signing trust, package digest and final installer digest. A syntax-only installer
is not an installation proof. Test the real download, cancellation, failed access,
installation, restart, repair and removal paths on the final executable.

Preserve a genuinely older application/guardian for upgrade compatibility. Rebuilding
both A and B from new code does not prove that installed older code understands B.
One historical Complete A-to-B case can replace a redundant rebuilt Complete case
when other profile cases exercise the new guardian and the final offline installer
tests fresh Complete initialization. Record why the coverage is sufficient.

## Reuse evidence with an explicit boundary

When application source is unchanged and only packaging or documentation changed,
reuse the relevant successful tests. Bind reuse to a reviewed source diff, exact
workflow/run/attempt/step and authenticated diagnostic artifact. Retest the changed
producer and qualify its new outputs. A whole run can fail after a successful test
step; report both facts separately.

NOH's temporary exception is intentionally narrow and expires after one immediate
repair commit. Do not turn it into a blanket skip switch. A broad, durable evidence
reuse system needs its own invalidation design and should not be improvised during
a release repair.

Hosted Windows CI can run elevated while an ordinary desktop installer must refuse
elevation. Tests should always exercise profile/version logic and assert the public
entry point's elevation refusal when appropriate. Do not weaken production checks
to make the runner pass. Likewise, an isolated local test may lack named-pipe access;
diagnose the specific access error before changing the implementation or its timeout.

## Keep the collaboration cheaper than the work

A UI test failure can expose a real asynchronous race. After typing a new output
name, Enter may arrive before its filesystem check finishes. Keep a pending focus
request until the button becomes enabled, cancel it on newer user input, and test
the pending state deterministically. Waiting before Enter would hide that case.

Implement one coherent change, request a bounded peer review, run the affected
checks, then use CI for the complete required validation. The reviewer should
inspect the diff and evidence rather than repeat every build. Correct concrete
findings and stop adding checks once the agreed acceptance criteria are met.

Before a long operation, establish one process handle, bounded timeout, saved logs
and a terminal result. Put necessary polling in that process; do not repeatedly
read logs or delegate an agent just to wait. Chain already-approved mechanical
steps after successful prerequisites and stop on the first failure. Report useful
changes and preserve the active handle across handoffs.

## Copy to the next project

Validate cancellation through the actual installer UI. Inno Setup 7.1.0
[`ExecAndLogOutput`](https://jrsoftware.org/ishelp/topic_isxfunc_execandlogoutput.htm)
pumps messages but deliberately disables existing windows during the child wait.
A working controller cancellation flag alone therefore does not prove that the
user can request cancellation. NOH restores input once, only on the download
progress page, with navigation disabled; installation retains the normal lock.
A silent local CONNECT proxy can verify cancellation before any progress output
without rebuilding or replacing the signed application. Keep that test's process
environment isolated and record that the proxy actually received the request.

1. Identify the delivery artifact, target platforms and acceptance boundary.
2. Measure one representative required run and locate its largest costs.
3. Write down the four cache layers, their exact paths and invalidation inputs.
4. Reuse the existing package/build tools and preserve their verification.
5. Record an artifact identity that survives download, installation and review.
6. Make a small local reproduction before retrying a costly failed CI stage.
7. Measure the next required cold/warm runs; keep only demonstrated improvements.
8. Record residual costs and decide whether a prebuilt image is justified.
