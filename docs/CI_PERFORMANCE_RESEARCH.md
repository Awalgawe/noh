# NOH CI performance research

Research date: 2026-10-06. Repository examined: `codex/setup-release`,
`b5a52bd75365c5bff5a8576e592f8314c4ca1c27`.
Research status: paired review accepted on 2026-10-06. Implementation evidence
below is current on 2026-10-07; the original research and appended rounds retain
their historical context. All four implemented changes are retained after
paired review; runtime cache selection passed its net-cost criterion. Two-thread
media scheduling and ThinLTO are rejected. The v20 review accepted closure of
the original goal against the production path and the documented limits below.
Exact paired verdicts are recorded in `.mcp-dev/ci-research/pair/register.md`.

## Implemented changes and verified scope

| Change in the Windows delivery path | Same-run before / after | Measured scope and qualification |
| --- | --- | --- |
| Materialize only required native crate notices while hashing every member |111.500s /50.213s;61.287s saved (55.0%) | Warm full runtime acquisition, [run37547242943](https://github.com/Awalgawe/noh/actions/runs/37547242943). All1,182 output paths/sizes/hashes identical;42 delivery controls pass. |
| Reuse immutable synthetic media fixture bytes across tests |108.991s /103.232s;5.759s saved (5.3%) | Serial media suite on the same two-CPU hosted runner, [run37549357105](https://github.com/Awalgawe/noh/actions/runs/37549357105). Same14 media names; full required suite565 passed/26 existing ignores. |
| Compile one QA packaging helper and pass an explicit frozen Windows source root |100.732s /6.207s;94.526s saved (93.8%) | Helper compilation after matching QA workers, [run37550499119](https://github.com/Awalgawe/noh/actions/runs/37550499119). Both compiler phases completed; whole diagnostic failed on a subsequently fixed path assertion. Corrected actual root/version routing passed locally;15 helper tests cover unchanged helper Rust code. |
| Preserve compatible raw downloads across extraction-code edits |525.615s /197.163s;328.452s saved (62.49%) | Real cache-policy transition including both production consumers, restore, key/guard overhead and full promotion; [run37559773345](https://github.com/Awalgawe/noh/actions/runs/37559773345). Same1,182 full-runtime files and114 test files;973 checked requests per arm, no candidate network miss. |

These are independently measured phases, not a summed whole-workflow speedup.
The substantive work reductions are structural: native notice extraction avoids
30,282 temporary file writes in the scoped local inventory, and the packaging
helper no longer recompiles the versioned application library for each source
snapshot. Media fixture reuse removes54 repeated FFmpeg fixture subprocesses
from the seven callers, but its measured time saving is modest. The earlier
roughly seven-minute QA compilation gain from cache/feature/LLD fixes predates
this Actor/Critic implementation and is not counted as a new gain here.

The implementation commits are a8ced20 (runtime notices),7bdc234 (fixtures),
ed55f2f (explicit-root helper) and c8a8c0e (cache selection).
No application algorithm, GUI translation, production
test scheduling or release optimization profile changes. Shared media behavior,
cancellation, no-overwrite rules, GNU/LLD configuration, source/signature checks
and delivery acceptance gates remain. Two-thread media execution was rejected:
its additional13.768s saving did not reach the predeclared60s criterion.

Costs and limits remain explicit. The controlled comparisons use one hosted
pair each, with uncontrolled OS cache/order effects. The E3 diagnostic's overall
failure is retained; its corrected local controls do not turn it into a hosted
success or a packaging/native-acceptance result. Production Cargo-v2 warm-cache
and whole-release elapsed times are unmeasured. E5 closes the measured raw-cache
integration gap: a compatible input-only cache is now present and the guarded
production restore selects it. Its5m28s net saving applies to the observed
transition, not every unchanged future run. E1's warm algorithm comparison and
E5's same-code cache comparison are separate observations, not an additive
whole-workflow result.

E4 also rejected ThinLTO: warm compilation of all five executables took299.523s,
versus271.152s for complete LTO, with339 Fresh dependencies and none recompiled
in either measured arm. No release-profile change or additional ThinLTO-specific
acceptance is required. Source, process, artifact, cost and review details follow
in the implementation rounds below.

## Original read-only research context

Author: compilation/cache session `01a11269-184a-7430-b207-13a6a3d0c577`.
Reviewer: CI architecture session `01a11269-a66a-7df1-b89e-80ba2d284c62`.
Requested by the user in chat `01a10cfd-27f8-7772-9703-b536ccacb27b`.
The sessions exchanged findings and objections directly. No production code,
workflow, cache policy, release, draft, or installed tool was changed for this
study. No application build, CI dispatch, paid infrastructure, or desktop control
was used. Existing logs, source inspection, and public primary sources provide the
evidence. This document is the sole research deliverable and is not a commit.

## Decision in brief

The evidence supports a sequence of bounded improvements, not a new cache product
or a promised ten-minute release. First establish the corrected production cache's
warm behavior during necessary work. Then target remaining compilation and media
test scheduling, with runtime acquisition and transfer timings kept separate.
Retaining an already authenticated application for a wrapper-only repair offers
larger savings than optimizing the 27 seconds spent compiling four wrappers.

The first bounded experiments are runtime acquisition, release ThinLTO and media
test scheduling after their respective baseline measurements. Integration-test
consolidation is conditional on compiler-unit evidence. None is measured on NOH yet.
Nextest is a scheduling experiment with a six-minute execution budget, not a cure
for the several minutes spent compiling tests. More ambitious job parallelism,
helper extraction, runner changes, and cross-commit result reuse need additional
contracts and evidence before implementation.

Labels used below: **observed** means existing NOH source/log evidence;
**documented** means a cited upstream capability; **inference** is reasoning from
those facts; **estimate** is explicitly unmeasured; **experiment** is future work.

## What was actually measured

| Evidence | Scope | Observed result |
| --- | --- | --- |
| [Run 37469621614](https://github.com/Awalgawe/noh/actions/runs/37469621614), `944525b` | Successful B-only producer, four installers, private unpublished staging | 62m20 elapsed; candidate 54m38; installers 5m57; native acceptance still pending |
| [Run 37481473432](https://github.com/Awalgawe/noh/actions/runs/37481473432), `a079532` | Read-only historical QA cache, QA and snapshot helper validation | Candidate job 18m07; 565 passed, 26 existing ignored; no release build, packages, installers, or publication |
| Historical A/B run 37439140827 | Different, earlier producer | A release compilation 10m22; B 4m42 after A; useful evidence of reuse within one run, not a current warm-cache forecast |

Local evidence: `.mcp-dev/setup-ci/37469621614/automation-report.json`, original
`logs/delivery/github-run-37469621614/{RUN,JOBS}.json` and job logs;
`.mcp-dev/ci-cost/37481473432/report.json` and authenticated
`diagnostics/setup/*.{stdout,stderr,process}.log/json`. These ignored files stay
local. The diagnostic SHA-256 for the targeted run is
`b6e6e6b765b0384ab3321479a92fd17adfa33e37031e7be9cedc978aa52c4fd3`.

The full run used Rust 1.98.1, Windows GNU, LLD, LLVM-MinGW 20260922,
Velopack 1.2.161, .NET runtime 8.0.31, and Inno Setup 7.1.0. Its actual runner image
was `windows-2025-vs2026`, version `20260925.250.1`, runner agent 2.337.0,
region `canadaeast`. A runner label alone does not freeze that environment.

| Full-run phase | Seconds | Interpretation |
| --- | ---: | --- |
| Rust/tools/Cargo restore | 6 / 5 / 22 | Already small, not a reason alone to buy an image service |
| Compiler/packager preparation and Cargo fetch | 23 / 28 | Revalidation/extraction and downloads are different costs |
| QA enclosing step | 1,184 | Includes compilation, test runtime, and execution |
| Full runtime acquisition | 396 | Network, hashing, extraction, and checks mixed together |
| Runtime cache save | 114 | First-fill cost; cannot be charged to every warm run |
| Build B and seal profiles | 1,445 | Includes helper, release compilation, bundle work, hashes and signatures |
| Candidate artifact upload | 22 | 1,587,009,670-byte artifact, already compression level 0 |
| Installer job | 357 | Includes authentication, downloads, staging and verification |

Within the 1,445-second production step, helper compilation took 259 seconds,
release compilation 781 seconds, and Complete/Standard/Minimal packaging took
186.43/29.48/6.97 seconds. These nested times must not be added to the enclosing
step. Within the installer job, the actual offline wrappers took 23 seconds and
the web wrapper 4 seconds. Candidate acquisition took 57 seconds, payload staging
149 seconds, and final wrapper upload/verification 78 seconds.

The existing corrective work is complete, not a new recommendation:

- Cache v1 contained only QA dependencies after an early failure. Immutable exact
  hits prevented enriching it. Production v2 includes the tools/policy hash in
  `shared-key`, disables save-on-failure, and retains trusted-branch writes.
- Nested snapshots duplicated configuration-array LLD flags. Explicit job
  `RUSTFLAGS` now retain exactly one flag, preserving the configured LLD linker.
- QA, verifier and helper feature graphs are now aligned to `gui,mcp,updates`.

The targeted run measured test compilation at 289.48 seconds instead of
297 + 257, and helper compilation at 104.38 instead of 259. This is about seven
minutes less compilation across those two phases, with ordinary runner variation
still present. All 279 helper dependencies were Fresh; the three actual compiler
commands were NOH's build script, library, and development example. The helper
was compiled in the correct snapshot. Production v2 has not yet supplied a
measured warm full-release result. Subtracting seven minutes from 62m20 is not a
new full-run measurement.

## Critical path and the real Rust graph

Current ordering in `.github/workflows/windows-setup.yml`:

```mermaid
flowchart LR
    A[Audit and preflight] --> B[Tools and caches]
    B --> C[QA workers]
    C --> D[Compile test targets]
    D --> E[Execute tests]
    E --> F[Acquire full runtime]
    F --> G[Reuse QA helper with explicit snapshot root]
    G --> H[Release application]
    H --> I[Bundle and seal three profiles]
    I --> J[Authenticated candidate artifact]
    J --> K[Offline wrappers and payload staging]
    K --> L[Web wrapper and final asset verification]
```

Audit/preflight are concurrent prerequisites. QA deliberately gates the expensive
producer. Source freezing, verifier creation and cache operations add edges not
shown individually. Web installation metadata depends on the staged payload asset
IDs; these steps cannot all be made independent by removing `needs`.

NOH is one Cargo package, with a shared media library, five binary targets
(`noh`, `noh-app`, `noh-mcp`, guardian, repair), developer/release examples, and
19 automatically discovered integration-test targets. The observed successful
suite runs 27 harnesses: library + five binaries + 19 integration targets + two
examples. GUI source modules form a substantial binary target. Features add
eframe/rfd/audio, MCP/Tokio, and updater/network/cryptographic dependencies to the
shared graph. There is no evidence that FFmpeg, libmpv or speech models are compiled
by Cargo in this workflow: these are separately acquired pinned runtime inputs.
`ring` and resource generation do have native/build-script work.

Cargo legitimately builds the normal library and its test variant, normal workers
and test harnesses. A second `cargo test --no-run`/execution invocation showing a
2.33-second Fresh check is not another 289-second compilation. Integration tests
also require normal binaries through `CARGO_BIN_EXE_*`; deleting the explicit
workers command would not eliminate their build. It might change scheduling,
which must be measured separately. See [Cargo test target selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html)
and [Cargo targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests).

QA uses release inheritance, optimization 1, 16 codegen units, no cross-crate LTO,
and a manifest request for incremental compilation. Release uses size optimization,
fat LTO and one codegen unit. These are distinct compiled dependency artifacts.
Reordering the builds cannot make QA objects satisfy release requirements.
The observed CI environment sets `CARGO_INCREMENTAL=0`; actual helper compiler
commands contain no incremental flag. Local QA incremental behavior therefore
does not describe CI. Preserve local iteration behavior while measuring CI options.

`build.rs` computes identity over `src`, **all** `tools`, `tests`, `assets`,
`locales`, manifests/lock/config and build options. It also watches Git revision,
dirty state and relevant directories. It avoids rewriting unchanged generated
identity, but a changed identity still invalidates the shared library and its
dependents. Documentation is excluded from the compilation source digest yet can
alter Git revision/dirty state. Test-only and installer-tool edits can alter the
broad digest. This is intentional provenance, not proof of a slow hashing routine.
No per-build-script timing establishes that hashing itself consumes minutes.
Cargo's [build-script change detection](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection)
also watches directory contents; copying source trees and their mtimes matters.

The snapshot development helper currently links `noh`, uses its media/fixture
types and `find_ffmpeg`, and embeds `CARGO_MANIFEST_DIR`. Reusing a checkout helper
for a snapshot would select the wrong source. Simply moving the same code into
an `xtask` package leaves the library dependency intact. A useful separation would
accept an explicit source root, separate packaging from fixture/capture features,
and record the tool's own identity independently from the application. This is a
reviewed interface change, not a cache toggle.

## Compilation and cache choices

| Choice | Evidence and NOH implication | Decision |
| --- | --- | --- |
| Existing dependency cache | 279 Fresh helper dependencies; QA restore 22 seconds in the full run. v2 warm release is unknown. | Retain and measure first. Do not stack another full target cache on it. |
| Cache workspace crates | Pinned action supports it, but NOH's sole package changes broadly and source snapshots embed roots/version/options. Clean checkouts can invalidate fingerprints. | Conditional after identity/tool boundaries exist; not a blanket switch now. |
| Persist incremental artifacts | Default rust-cache cleanup excludes them and CI disables their production. Different from persistent local development. | Reject a claim that `incremental=true` alone speeds CI. A separate cache would need measured size, transfer and actual reused work. |
| sccache | Rust libraries can benefit across machines/paths; Rust compilation invoking a system linker is not cacheable. Incremental must be disabled. | Lower priority while dependencies are Fresh and executable links remain. Compare against, not just add to, dependency cache. |
| ThinLTO | Current release is fat LTO/cgu1 across five binaries; comparable projects use thin. | Highest-value compiler experiment after warm baseline; no gain percentage established. |
| More codegen units | Can expose compilation parallelism; may affect speed and size of final binaries. Two-CPU runner limits benefit. | Change separately after LTO experiment; do not confound both in the first trial. |
| Fewer integration harnesses | 19 binaries each link NOH; shared helpers are repeated modules. | Consolidate a small coherent group only if unit timing confirms significant repeated link/codegen cost. Keep every test. |
| Split core/application/tool crates | Could narrow invalidation and expose compilation concurrency; adds interfaces and generic/codegen boundaries. | Tool boundary first if justified; no wholesale workspace rewrite based only on source line counts. |

The pinned rust-cache [save implementation](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/save.ts)
short-circuits an exact complete key; its [cleanup](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/cleanup.ts)
retains dependency artifact directories, rather than a reusable application package.
Incremental cleanup and environment behavior are also described in its
[documentation](https://github.com/Swatinem/rust-cache). Cache completeness is
about what was actually compiled before the writer saved, not the action's green
restore step. A future save boundary after all required compilations but before
packaging could preserve a first fill on packaging failure. A second early QA
writer under the same immutable key would recreate the previous defect.

The [sccache Rust limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)
explicitly exclude bins, dylibs/cdylibs and proc macros that invoke the linker.
A hit rate over hundreds of tiny requests is not a saved-time metric. Require
cacheable-time coverage, miss reasons, bytes and end-to-end time. NOH's embedded
identity changes the main library too. Windows path normalization has received a
specific [maintainer fix, PR 2736, merged 2026-06-19](https://github.com/mozilla/sccache/pull/2736),
listed in [sccache 0.16.0 release notes](https://github.com/mozilla/sccache/releases/tag/v0.16.0).
Do not normalize away the helper's semantically meaningful source root to force a
hit. Keep compiler/native-tool identities in any cache contract.

For LTO, preserve Windows GNU/LLD. First vary only `lto=true` to `lto="thin"`,
retaining optimization `s`, cgu1 and strip settings; then test cgu16 only if still
useful. Compare all shipped executables, imports, startup, representative preview,
export and update/repair behavior, not merely compiler success. The main media
encoders live in external runtimes, so binary-size/latency tradeoffs could be
favorable, but NOH's own hashing, timeline and orchestration code also matters.
The [Cargo profile specification](https://doc.rust-lang.org/cargo/reference/profiles.html#lto)
distinguishes fat, thin, local thin (`false`) and off; they are not interchangeable.
No measured evidence attributes all 781 seconds to LLD or LTO. Stable
[`--timings`](https://doc.rust-lang.org/cargo/reference/timings.html) identifies
expensive compilation units and overlap, not a precise separate linker breakdown.

Integration consolidation must preserve unique tests and ignored fixtures, update
exact self-executable test paths, retain `NOH_*_EXE` overrides, and preserve temp
directory isolation. One huge binary can increase incremental rebuild work and
memory. It may also alter libtest concurrency, so do not combine topology changes
and a new test runner in one experiment. Cargo explicitly describes integration
tests as separately linked executables and supports organizing them into modules.

Unstable checksum freshness remains a watch item, not an adopted fix: the
[Cargo documentation](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#checksum-freshness)
still identifies the experimental switch and notes build-script file tracking
continues to use mtimes. Introducing nightly would not remove NOH's changing
embedded Git/option identity. Do not manipulate mtimes to conceal changed inputs.

## Test execution: bounded concurrency, not fewer assertions

The targeted run's 362.40-second execution command contains 357.42 seconds summed
across successful harnesses plus Cargo/process overhead. Its largest groups were:

| Harness | Passed | Seconds |
| --- | ---: | ---: |
| media | 14 | 158.60 |
| project | 7 | 32.47 |
| images | 6 | 31.06 |
| shared library | 292 | 28.21 |
| MCP | 10 | 23.98 |
| controller | 13 | 18.47 |
| captions | 6 | 14.36 |
| GUI | 166 | 12.18 |

This gives an absolute execution-only saving ceiling below 362.40 seconds, not an
attainable target. Media alone consumes 44% of the command. Parallelizing library
and GUI tests alone has at most about 40 seconds to recover. A zero-overhead model
with two equally loaded independent execution slots would take about 181 seconds;
FFmpeg threading, uneven durations, Windows process startup and exclusive resources
make that model optimistic. More hosts also add setup and transfer costs.

Classification must use operations and resource ownership, not filenames alone:

| Class | Examples/observations | Initial scheduling policy |
| --- | --- | --- |
| Pure deterministic logic | parsing, geometry, translations, state transitions | Concurrency allowed after global-state audit; no media prerequisite bypass |
| Isolated media integration | `tests/support` allocates a unique `TempDir` per fixture | Start with at most two test processes; measure CPU/RSS/child-process count and FFmpeg internal threads |
| Timing/cancellation/process-tree tests | sleeps, timeouts, inherited pipes, ignored self-executable fixtures | Serialize initially; verify no leaked descendants and unchanged deadlines |
| Updater locks/IPC | executable-relative leases and Job Objects; network fixtures often bind port 0 | Serialize conflicting resource groups; unique fixture roots do not prove every lock is independent |
| Native hardware/bench/recognition | Existing explicit ignored campaigns | Preserve their separate qualification and ignore reasons; do not silently run or remove them |

The official nextest release inspected was
[0.9.146, 2026-09-21, commit `8af696d`](https://github.com/nextest-rs/nextest/releases/tag/cargo-nextest-0.9.146).
Pin an actually qualified version/binary if an experiment is authorized. Its
[process-per-test explanation](https://nexte.st/docs/design/why-process-per-test/)
acknowledges both Windows spawn cost and lost in-memory sharing. Per-process
isolation does not isolate filesystem/IPC resources. Configure
[test groups](https://nexte.st/docs/configuration/test-groups/) for mutual exclusion;
[`threads-required`](https://nexte.st/docs/configuration/threads-required/) is
resource weighting and can reduce throughput. Start retries at zero so failures
cannot disappear into a flaky-success total.

A lower-complexity comparator is existing libtest with concurrency two only for
audited independent groups, keeping process/update tests serial. Nextest adds
inter-harness scheduling, per-test measurements and cancellation management, but
must beat that comparator after tool setup. Preserve the 565 passed + 26 ignored
inventory by stable names and reasons, including 14 development-tool and six
release-tool tests. Assert `NOH_MEDIA_TESTS=1` and exact worker/runtime identities.
The current explicit target list has no doctest step; do not claim nextest expands
coverage automatically. Its [documented doctest limitation](https://nexte.st/)
requires a separate Cargo command if doctests are part of future required coverage.

Nextest [archives](https://nexte.st/docs/ci-features/archiving/) and
[partitions](https://nexte.st/docs/ci-features/partitioning/) support build-once,
run-many. They do not themselves include NOH's source/runtime/environment proof.
Archives omit source by default; remapping paths is not a substitute for exact
worker/FFmpeg/DLL inputs. Try scheduling on one machine before cross-job sharding
for a six-minute workload. Do not compile the entire graph separately per shard.

## Comparable projects: what their actual code supports

Sources were read on 2026-10-06 and pinned through public unauthenticated GitHub
REST reads. Manifest versions identify inspected development trees, not a guarantee
of released versions. Source URLs and local SHA-256 values for the first four are
retained in `.mcp-dev/ci-research/public-compilation/sources.json`; immutable
references for the other four are recorded in
`.mcp-dev/ci-research/architecture-peer-notes.md`. No upstream workflow duration is used as a NOH
estimate. Differences in hardware, target and acceptance scope are explicit.

| Project and inspected files | Actual practice | Transferable lesson and limit |
| --- | --- | --- |
| [Alacritty CI](https://github.com/alacritty/alacritty/blob/29dc55375da817d53540f4caefbe708703c57b07/.github/workflows/ci.yml), [manifest](https://github.com/alacritty/alacritty/blob/29dc55375da817d53540f4caefbe708703c57b07/Cargo.toml), commit 2026-10-05, Rust minimum 1.85 | Windows/macOS Cargo tests, separate terminal feature coverage; workspace splits terminal/config/UI; release ThinLTO, incremental false; inspected CI file has no cache action | Desktop/windowing analogy supports a measured ThinLTO option and stable crate boundaries. It does not prove an elaborate cache is mandatory or reproduce NOH media/installer coverage. |
| [Helix build](https://github.com/helix-editor/helix/blob/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc/.github/workflows/build.yml), [manifest](https://github.com/helix-editor/helix/blob/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc/Cargo.toml), commit 2026-09-29, version 25.7.1/Rust 1.90 | Cross-OS workspace tests plus integration command; release thin, separate `opt` profile fat/cgu1; [setup action](https://github.com/helix-editor/helix/blob/ba40e547426b0f9896c8bdc699a4ab11f2b37dbc/.github/actions/rust-setup/action.yml) uses rust-cache 2.9.1 shared-key and a separate grammar cache keyed by OS/arch/languages manifest | Distinguish ordinary feedback from expensive distribution optimization and separate native generated inputs. Editor grammars and media runtimes have different validation. |
| [Nushell CI](https://github.com/nushell/nushell/blob/3b08da67d19de7982a6612bca1eea4009fee2f9b/.github/workflows/ci.yml), [manifest](https://github.com/nushell/nushell/blob/3b08da67d19de7982a6612bca1eea4009fee2f9b/Cargo.toml), commit 2026-10-06, version 0.116.2/Rust 1.97.1 | `ci` profile inherits dev with no debug info; release size optimization + thin; rust-cache 2.9.2 includes workspace crates; many smaller crates; Windows uses Incredibuild MSVC | Useful counterexample to a universal ban on workspace caches. Its stable crate graph, custom harness and runner economics are not NOH's single-package GNU/LLD graph. |
| [egui CI](https://github.com/emilk/egui/blob/35b9cbf27afd1756f5896bdd1325f155a415c054/.github/workflows/rust.yml), [manifest](https://github.com/emilk/egui/blob/35b9cbf27afd1756f5896bdd1325f155a415c054/Cargo.toml), commit 2026-10-06, version 0.36.2/Rust 1.95 | Same GUI version as NOH; rust-cache writes restricted to main; Windows Clippy and GPU tests on macOS; varied feature checks; custom release optimization comments mainly concern Wasm | Strong example of matching checks to platform capability and trusted cache writers. Its Wasm optimization/panic choices and macOS GPU testing cannot replace NOH's Windows acceptance. |
| [Zed release](https://github.com/zed-industries/zed/blob/73be6ac6b1975f428328bbf7fe185d3b671afec4/.github/workflows/release.yml), commit 2026-10-06 | Windows QA and bundles on self-hosted 32-vCPU runners; nextest and remote R2 sccache in test steps; retained checkout; gates before bundles and separate upload | Demonstrates artifact boundaries and combined test scheduling/cache use in a real desktop app. Persistent 32-core infrastructure is not evidence of a gain on NOH's two-core GNU runner. |
| [RustDesk Flutter build](https://github.com/rustdesk/rustdesk/blob/9f9585ce155a6558f0625eaf8fabfe8827c9a552/.github/workflows/flutter-build.yml), commit 2026-10-06 | Shared bridge artifact, separate Rust/vcpkg caches, pinned vcpkg revision, Windows MSVC and unsigned artifacts/signing stages | Separate expensive native inputs and their owners. Its Flutter engine acquisition does not establish NOH's integrity or acceptance contract. |
| [Lapce release](https://github.com/lapce/lapce/blob/f66ffaa95387794205c35864517de70958b9ea8d/.github/workflows/release.yml), commit 2026-09-30 | Windows release-lto then portable-feature build, grouped MSI/ZIP/proxy artifact, aggregate release job, one-day retention; no explicit Rust cache in inspected Windows job | Another counterexample to caching everything; useful artifact-stage separation. Moving toolchain and retention/re-tagging choices are unsuitable defaults for NOH's recovery needs. |
| [LosslessCut build](https://github.com/mifi/lossless-cut/blob/70f2663a7a7c995903701acd2f616d057f4fdc2f/.github/workflows/build.yml), commit 2026-09-30 | Native OS matrix, architecture-specific FFmpeg acquisition, Yarn cache, app packaging and screenshot/upload paths | Closest media workflow comparison separates runtime from app build. Electron compilation and screenshot checks do not replace NOH's media/update tests; nightly full builds are not automatically economical. |

## Runtime acquisition, packaging and transfer

The runtime cache saved in the full run was 2,470,779,528 compressed bytes.
The original job log records tar/zstd starting at 13:46:43.300 UTC, the first
upload report at 13:48:15.903, and final bytes at 13:48:32.590. About 92.6 seconds
precede reported upload; about 16.7 seconds separate the first report and final
bytes (17.6 seconds through the saved confirmation). The first interval includes
archive preparation/compression and any unreported overhead; it is not a precise
CPU profile. It refutes attributing the entire 114-second cache save to network.

`delivery.py::windows_inputs` and `windows_runtime.py::acquire` acquire and extract
sequentially. The native lock describes 112 packages, 653 native Rust crates,
130 preview and 113 export file entries. These are graph counts, not fresh network
requests on every run. The current 396-second acquisition measurement does not
separate object download, digest, archive extraction, copy and notice generation.

Two bounded experiments follow instrumentation:

- Prefetch missing immutable objects with two workers, then at most four if
  beneficial. Preserve HTTPS restrictions, expected hashes, unique temporary
  paths, atomic rename, bounded retry and failure propagation. Keep extraction
  sequential initially. Network prefetch during QA is conditional on measured
  contention; parallel decompression and FFmpeg on two CPUs can be slower.
- Examine one-pass or physical-order tar extraction. The code scans members,
  then reads selected files from seekable compressed archives. CPython 3.14.7
  [decompression streams](https://github.com/python/cpython/blob/v3.14.7/Lib/compression/_common/_streams.py#L113)
  rewind and decompress again for backward seeks; [tarfile](https://github.com/python/cpython/blob/v3.14.7/Lib/tarfile.py)
  and the [ZstdFile seek contract](https://github.com/python/cpython/blob/v3.14.0/Lib/compression/zstd/_zstdfile.py#L199)
  substantiate the mechanism. Whether NOH's selected member ordering makes it
  significant is unmeasured. Keep duplicate/path/link/member/hash checks and
  never expose an unfinished extraction as validated output.

Raw-object cache identity can be separated from derived-runtime identity: changing
validation code should rerun validation but need not redownload unchanged pinned
bytes. Consider a few component caches or content-addressed raw objects with
bounded fallback; every requested object must still pass its current digest.
Hundreds of individual Actions entries add transfer/lookup/maintenance overhead.
An extracted-tree artifact needs its own tool/lock/inventory/producer contract;
it is not interchangeable with a raw archive cache. This design must account for
eviction and first-fill compression as well as nominal hit rate.

Three profile packages already share one application build. Parallel packing can
at best overlap roughly the 36-second Standard+Minimal tail of the measured pack
commands; the Complete pack remains the long pole. Describe operations total
about 43 seconds; profile signatures total under two seconds. Optimizing those
has a bounded budget and must retain checks at trust boundaries. Profile-envelope
signing is not a claim of qualified Authenticode signing.

Candidate/installer artifact compression is already zero. Merging producer and
installer jobs could remove a 57-second receive/authentication boundary but loses
cheap independent wrapper retries and privilege separation. Retain the boundary.
Moving staging to Linux creates another download and failure edge; assess its net
price/time difference, not just cheaper Linux minutes. Existing exact-digest
upload resume should retain existing bytes rather than rebuild wrappers after a
transient upload failure.

## Runner options and total cost

Official offers checked on 2026-10-06 are not a quote or measured NOH throughput.
Public [owner metadata](https://api.github.com/users/Awalgawe) identifies Awalgawe
as `User`. Organization-only offerings are not a direct `runs-on` change for this
repository. No ownership/visibility migration is proposed.

| Option | Verified offer | Decision for NOH |
| --- | --- | --- |
| GitHub standard private Windows | Documented 2 CPU / 8 GB; $0.010/min beyond included allowance | Keep baseline. Public Windows offers 4 CPU / 16 GB, so public-project timings are not matched evidence. |
| GitHub larger Windows | 4 CPU $0.022; 8 CPU $0.042; 16 CPU $0.082/min | Team/Enterprise organizations only, no included minutes. Consider latency only after warm graph and eligibility change. |
| GitHub custom Windows x64 image | Available through larger runners with versioning/storage | Same eligibility barrier. Tool-only image saves little with current caches; image creation/update/storage need amortization. |
| Depot Windows 2025 | 2 CPU / 8 GB / 100 GB $0.008/min; 4 CPU $0.016; 8 CPU $0.032; Developer plan $20/month | Organizations only; no Windows Hyper-V or RAM-disk accelerator. Weighted allowances, fixed plan and overage require an exact quote. |
| Blacksmith Windows 2025 | Public beta; 2 CPU / 7 GB / 130 GB, smallest advertised $0.008/min | Organizations only; current docs supersede older unsupported-Windows FAQ. Validate image/tool differences and beta reliability. |
| Persistent dedicated self-hosted Windows | GitHub Actions service currently free; owner pays machine/license/operations | Warm target/runtime may help, but persistent state and release credentials need isolation. Do not use the user's daily workstation by default. |
| Ephemeral self-hosted Windows | One-job runner lifecycle supported | Better isolation, but provision/wipe/log retention plus external caches and Windows compute still cost. No universal price without region/provider/hours. |
| Linux cross-build to Windows GNU | Rust target supports cross-compilation with the suitable native toolchain | Windows extraction/helpers/packaging/acceptance remain. New host/provenance/transport contract needed; cannot replace Windows testing. |

Sources: [GitHub hardware](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[runner prices/eligibility](https://docs.github.com/en/billing/reference/actions-runner-pricing),
[billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions),
[custom images](https://docs.github.com/en/actions/how-tos/manage-runners/larger-runners/use-custom-images),
[self-hosted model](https://docs.github.com/en/actions/concepts/runners/self-hosted-runners),
[ephemeral lifecycle](https://docs.github.com/en/actions/reference/runners/self-hosted-runners),
[Windows GNU target](https://doc.rust-lang.org/rustc/platform-support/windows-gnu.html),
[Depot runner types](https://depot.dev/docs/github-actions/runner-types),
[Depot eligibility/cache](https://depot.dev/docs/github-actions/overview),
[Blacksmith types](https://docs.blacksmith.sh/blacksmith-runners/overview),
[eligibility](https://docs.blacksmith.sh/introduction/quickstart) and
[pricing](https://www.blacksmith.sh/pricing).

Calculated list-price example, not an invoice: rounding the full baseline's
Windows jobs to 55 + 6 minutes gives $0.61, plus about $0.012 for two sub-minute
Linux jobs, before storage. Included quota changes marginal cost. For otherwise
comparable jobs, larger 4/8/16-CPU Windows runners must be more than 2.2/4.2/8.2
times faster to lower variable compute cost. These ratios are break-even points,
not assertions about attainable scaling. Faster single-core/I/O hardware can
matter; only measurements establish the result. Human waiting can justify a
latency premium, but report it separately from infrastructure savings.

GitHub currently bills excess artifact storage at $0.25/GB-month and cache/image
storage at $0.07/GB-month. Default cache capacity is 10 GB with seven-day unused
eviction; capacity can be raised with paid opt-in. See the current
[cache reference](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching).
Measure runtime/Cargo cache competition before adding layers. Depot's shared
branch cache namespace needs explicit trusted-writer isolation; key names alone
are not access control. Do not copy provider speedup/half-price claims: their
hardware, allowance and comparison baseline differ from NOH. Current billing
documentation, rather than a superseded 2025 self-hosted pricing announcement,
governs the cost model.

## Change impact and proof reuse

Reusing a cache, reusing an executable, and reusing a test result are three
different operations. A cache must still be validated by Cargo/input hashes. An
executable retains its original build identity. A test receipt retains the exact
inputs and environment against which it passed. None may be relabeled as a fresh
HEAD build merely because only a wrapper was edited.

| Change | Appropriate path | Evidence invalidated / retained |
| --- | --- | --- |
| Application/media/GUI/update code or compiled translations/resources | Build affected QA graph, execute required tests, produce new release only when delivery is requested | Dependency cache may survive; application/test results depending on changed behavior do not; final native evidence binds new bytes |
| Tests/fixtures only | Compile and run changed test graph; retain unchanged accepted application as the explicit subject if the test interface permits | Current broad build fingerprint still changes on a new compile; no silent claim that HEAD's binary equals the old one |
| Installer wrapper/scripts only | Existing authenticated producer artifact -> rebuild relevant wrapper -> qualify its new behavior | Preserve original app commit/hash and tests; refresh wrapper source/version/manifest; current separate installer boundary already supports this |
| Runtime/model/native lock | Acquire and validate exact changed objects; regenerate affected profiles; run affected media/native checks | Application code may be reusable only if no embedded inputs changed; old runtime acceptance does not cover new binaries/models |
| Documentation only | Text/link/diff inspection | No new app build solely to refresh a Git commit label; packaged docs/notices may require re-inventory/repackaging; embedded resources remain code inputs |
| Cargo lock/toolchain/features/flags/build script | Explicit compiler-contract invalidation | Rebuild affected graph and tests; restore only compatible entries, never infer compatibility from equal filenames |

A durable result-reuse receipt should bind: repository and original source
commit/tree; an explicit versioned compilation-input manifest; test names and
fixture hashes; Cargo lock/features/profile/flags and effective build options;
Rust/native tools; worker/runtime digests; OS image and relevant environment;
command, test result/ignore inventory, run/attempt/job/step; authenticated artifact
digest and its producer authority. A reviewed source diff links later wrapper/docs
work to retained evidence. If any required field is absent or incompatible, run
the affected check. Security/advisory feeds are time-dependent, so retain a fresh
audit even for unchanged source. Trust configuration/channel/package/version
changes also invalidate embedded application identity.

This extends the existing narrowly scoped repair exception and therefore needs
separate design review. Do not expand that exception into a generic `skip-tests`
switch. Cache writers must remain trusted; signing must consume authenticated
artifacts with original producer identity. A successful old install is never the
native qualification of newly packaged bytes.

## Cold and warm behavior

| State | Expected work | What can falsify the assumption |
| --- | --- | --- |
| All caches absent | Download tools/crates/runtime; compile QA and release dependencies and NOH; full validation | Run must still succeed; missing cache is not missing required evidence |
| Downloads warm, compiled dependencies cold | Rehash/extract/check native archives; compile both profiles | Native hashing/extraction may still dominate despite zero network downloads |
| v2 dependency cache warm | Fresh compatible dependencies; rebuild NOH targets/options and final links | Actual Compiling list, feature/flags/profile/host mismatch, incomplete writer, eviction |
| Same job, unchanged source | Cargo can reuse just-built workers/tests/helper where contracts match | Snapshot source root/version/trust or generated identity changes |
| Wrapper-only with retained candidate | No Cargo release/QA rebuild; wrapper checks and authenticated transfer | Missing/expired/changed candidate artifact or incompatible wrapper inputs |

Do not manufacture a release to fill caches. Record the next necessary fill and
comparable subsequent warm run. Save/restore time and compressed bytes belong in
the result; keep cold and warm totals separately. Unchanged dependency graphs can
still be cold after eviction. Existing broad runtime key invalidation and verified
object reuse are candidates for refinement only after acquisition is decomposed.

## Phase budgets for measurement

These are observed investigation budgets from two different runs, not service
objectives and not additive components of a predicted release duration. The
targeted run omits production and uses a different source/cache context.

| Phase | Reference budget | Next question |
| --- | ---: | --- |
| Tools/cache restoration, setup and Cargo fetch | 1m24 in the full run | Is remaining preparation worth any image complexity? |
| Corrected QA, excluding snapshot helper | 13m37 in the targeted run | How much of compilation versus execution remains after safe scheduling? |
| Corrected snapshot helper | 1m44 in the targeted run | Does a separate tool contract amortize better than current recompilation? |
| Runtime acquisition / first cache fill | 6m36 + 1m54 in the full run | Separate cold network, warm extraction/hash, and cache archive preparation |
| Release compilation | 13m01 in the old partial-cache full run; warm v2 TBD | Measure warm dependencies before attributing the residual to LTO/linking |
| Three packs, descriptions and profile signatures | About 4m28 in the full run | Keep Complete long pole and authentication; measure repeated reads |
| Installer job including authenticated transfer/staging | 5m57 in the full run | Wrapper-only work should remain on this order of magnitude, excluding native qualification, while avoiding the producer entirely |
| Queue, orchestration and other command gaps | Record separately | Do not redistribute unexplained gaps into compiler or network savings |

The full-run elapsed total exceeds the sum of its two Windows jobs by 1m45;
concurrent preflight/audit and scheduling intervals need their own timeline.
Small-change success means avoiding unnecessary work and rerunning only checks
whose evidence was invalidated. Wrapper-only reuse already exists, while a new
docs/test/runtime classifier still needs its contract.

## Agreed priority and minimal measurement protocol

The ranking was accepted after the architecture review below. Estimates are
decision bounds, not additive promised savings.

| Priority | Proposal | Plausible scope / bound | Cost and stop condition |
| --- | --- | --- | --- |
| 1 | Measure corrected v2 and retain exact producer evidence on wrapper repair | Avoid repeating whole producer for wrapper-only work; existing seven-minute compilation gain must not be counted again | Low instrumentation effort; stop if proposed reuse cannot retain truthful identity |
| 2 | Instrument runtime acquisition, then bounded prefetch or sequential-pass extraction | Zero to some fraction of 396s cold acquisition; warm extraction and first-save preparation need separate measurement | Low-to-moderate targeted work; reject contention, digest/path validation regression or larger cache costs |
| 2 | ThinLTO comparison with same GNU/LLD toolchain | Targets remaining warm release compilation; 781s historical cold/mixed phase is only an upper contextual budget | Moderate validation; reject for meaningful output/performance regression or negligible net gain |
| 2 | Bounded media-test scheduling, libtest versus nextest | Execution <362s total; two-slot ideal roughly 181s before overhead; no effect on 448.39s QA compilation | Moderate classification effort; reject missing tests, skipped media, new flakes or descendant leaks |
| 3 | Selective integration-harness consolidation | Some fraction of 289.48s compile-tests; no per-link attribution yet | Moderate refactor; stop if timing shows no substantial repeated cost |
| 3 | Separate packaging helper from app/fixtures with explicit source root | Some fraction of 104.38s warm snapshot-helper phase, plus future invalidation reduction | Interface/provenance work; moving files alone is insufficient |
| Conditional | QA/release overlap or test sharding | Ideal overlap cannot save more than the smaller branch (currently QA 13m37s), before new overhead; warm release may be shorter | Duplicate setup, cache writes and wasted release work on QA failure; signing still waits for passing gates |
| Conditional | Workspace artifact cache / sccache | Depends on stable cacheable local libraries not present in current evidence | Extra transfers/storage; reject if backend costs exceed saved compile time |

Future measurement, requiring a separately authorized implementation phase:

1. On the next necessary baseline run, record image/hardware/tool versions,
   immutable action SHAs, graph/profile/flags/options, every cache's key/hit/bytes/
   restore/save time, dependency versus NOH units and existing process durations.
   Save Cargo `--timings` reports before cleanup. Read only known non-secret build
   inputs; do not dump the environment. Split runtime acquisition into network,
   hash, extraction and validation timings. Keep queue, job and total separately.
2. Start with one representative cold/fill observation and one compatible warm
   observation. Historical runs inform hypotheses; they are not a same-input A/B
   experiment. Do not call a second invocation in the same target directory a
   hosted warm-cache comparison. Report missing comparability explicitly.
3. For the selected optimization, change one variable. Use a bounded compile/test
   diagnostic without release publication; preserve separate experiment artifacts
   and cache identities so changed profiles cannot poison the production baseline.
   For test scheduling, reuse the exact compiled tests/workers/runtime and list
   every test before and after; collect per-test duration and peak resource use.
4. Repeat only when variability or a borderline result would change the decision.
   If a gain is smaller than observed runner variation, mark it inconclusive.
   No expanding benchmark matrix merely to obtain a favorable best run.
5. Proposed acceptance thresholds, to calibrate before running: identical required
   test inventory/results and no new flakes or weakened guarantees. For moderate
   new tool/topology complexity, seek at least 60 seconds saved in a repeated
   targeted phase or a material reduction in failed-run recovery cost. Accept
   smaller low-risk fixes if amortized benefit is clear. For large structural
   changes, require a material matched full-workflow improvement (for example
   10-15%) or a substantial reduction in repeated work across the actual mix of
   app/test/wrapper/runtime/docs changes, with maintenance payback. A 15% phase
   improvement is a diagnostic signal only: saving 16 seconds from a 104-second
   helper does not alone justify a workspace redesign. These are decision
   criteria, not predictions or a requirement to run extra benchmark releases.
6. Check final byte identity/provenance and all affected delivery guarantees.
   Compiler-profile changes require artifact/performance comparison; wrapper
   changes require wrapper qualification. A warm full-run total is claimed only
   after the complete required workflow actually passes. Define total economic
   cost as runner minutes + transfer/storage + maintenance + retries, including
   work discarded after failure.

## Alternatives rejected or deferred

| Alternative | Why it is not the next move | Reconsider only if |
| --- | --- | --- |
| Four parallel wrapper runners / three application builds for profiles | Only 27s wrapper work; profiles already share one build; extra runtimes/artifacts and runner minutes | Wrapper computation becomes a measured dominant phase |
| Universal nextest/sccache rollout | Neither solves the observed compiler, path and external-process constraints automatically | Controlled comparison demonstrates net savings and unchanged coverage |
| Cache an entire extracted runtime or final package as trusted state | Cache hit is not validation or producer provenance | Separate authenticated input/tool/inventory contract is designed and verified |
| Images solely for compiler/tool installation | Existing restore/setup is small; eligibility and maintenance add cost | Provisioning becomes a material repeated bottleneck |
| Replace GNU/LLD with MSVC, mold, or an unqualified linker | LLD is a project correctness requirement, not a cosmetic choice | A separate portability/correctness project is authorized; not a CI tuning shortcut |
| Linux-only validation / daily workstation CI | Cannot demonstrate Windows GUI/IPC/installer behavior; persistent personal state undermines clean qualification | Deliberate dedicated isolation and native validation contracts exist |
| Change repository visibility or ownership for cheaper runners | Out of scope and not a compilation improvement | User separately chooses that administrative change |
| Merge producer and installer jobs or move every upload to Linux | Loses cheap retry/privilege boundary or adds downloads; costs are not zero | Matched end-to-end/failure-cost model favors the change |
| Use upload-artifact v7 `archive:false` immediately | Feature requires one file; candidate is multi-file and current downloader expects ZIP; upload itself is 22s | A separate transport change produces enough net benefit to justify contract changes |
| Remove tests, enlarge timeouts to hide contention, or label old bytes as HEAD | Changes acceptance or provenance instead of improving performance | Never for a timing result |

The single-file transport capability is documented by the already pinned
[upload-artifact 7.0.1 inputs](https://github.com/actions/upload-artifact/tree/v7.0.1#inputs).
Retention also limits retries: NOH's candidate artifacts currently last seven
days, while GitHub allows [workflow/job reruns within 30 days](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs).
Rerun eligibility is not artifact availability. A missing retained artifact must
stop reuse; this study does not change retention or paid storage settings.

## Debate and residual uncertainty

| Exchange | Objection | Author response incorporated in the draft |
| --- | --- | --- |
| Runtime/installer architecture | Four wrappers are 27s; runtime acquisition 396s is not network-only; cache save spends 92.6s before upload reports | Rank runtime instrumentation ahead of broader topology; retain wrapper boundary and distinguish compression/extraction/transfer |
| Test execution | Library/GUI harnesses take 40s; nextest marketing cannot establish a six-minute saving | Focus bounded scheduling on media, preserve all self-executable fixtures and IPC/Job Object checks; use libtest as comparator |
| Cache/crate topology | Nushell caches workspace crates, but NOH's monolithic identity and snapshot-root helper invalidate them differently | Defer workspace-cache and xtask restructuring until a real input/interface contract; do not normalize meaningful paths |
| Hardware | Larger/vendor Windows offerings need organization ownership and can increase total price | Record current personal owner, eligibility barriers, break-even ratios, fixed fees and unexplored account quota |
| Success criterion | A 15% helper-phase saving may be only 16s and cannot justify a large refactor | Require full-workflow or change-mix economic benefit for structural changes; keep phase percentages diagnostic |
| Job overlap | QA/release parallelism reduces elapsed critical path without necessarily reducing runner-minutes | Model duplicated setup and wasted work on failed QA; gate signing/promotion on both results |

The architecture reviewer explicitly accepted the integrated recommendation,
ranking and adjusted success criteria on 2026-10-06, with no residual technical
disagreement. The final wording correction clarifies that invalidated checks must
be rerun. Both sessions retain the empirical uncertainties below; agreement is
on the decision method and priority, not on an unmeasured speedup.

Unresolved measurements: warm v2 release compilation; time attributable to fat
LTO versus codegen/link; test concurrency on two CPUs; runtime network versus
decompression/hash; transfer bottlenecks; actual account plan/quota;
and practical cross-commit identity boundaries. These are research limits, not
evidence of a particular speedup. No overall duration target is established.

The older cache section of `CI_PLAYBOOK.md` still describes pre-correction helper
features in one paragraph. The source at the examined HEAD, its opening measured
correction section and `STATUS.md` are authoritative for this study; the already
implemented feature/LLD corrections are not proposed again.

Research validation: nine downloaded public compilation-source snapshots match
their recorded SHA-256 values; pinned workflow/manifest contents and timing
arithmetic were checked; the new document passes whitespace/diff checks. Only
this uncommitted document is added to the tracked workspace. No application test
was rerun for a research-only change.

## Actor/Critic implementation plan v2 — 2026-10-07

This section starts the authorized implementation phase; the research conclusions
above are historical inputs, not completion of the performance goal. Actor:
`01a11356-7747-7083-93ef-933a05b8e388`; independent read-only Critic:
`01a11356-d08c-7fe2-abdb-792b5544af32`. The ignored round register is
`.mcp-dev/ci-research/pair/register.md`. Following the user's setup correction,
the Actor now also has an active Goal for the full objective; the coordinator
retains its coordination Goal. Activation does not establish performance success.

### Minimal current-state audit

- Workspace `<checkout>`, branch `codex/setup-release`, HEAD
  `b5a52bd75365c5bff5a8576e592f8314c4ca1c27`. At intake only this report was
  untracked. No Cargo, rustc, rust-lld, FFmpeg or NOH process was observed.
- `windows-setup.yml` is manual, builds B once, and already supports a diagnostic
  QA/helper run. The separate installer workflow already reuses an authenticated
  producer. Neither separation is a proposed new saving.
- Ordinary PRs use `native.yml` (three Unix platforms) and selected delivery
  controls. `delivery-candidates.yml` builds its actual candidate only on manual
  dispatch; its Windows compile-only mode still tests in release profile.
  Documentation-only changes do not automatically launch a full producer.
  Therefore changing those manual modes cannot be called an automatic PR gain.
- `tools/windows-qa.ps1` already aligns `gui,mcp,updates`, builds workers, compiles
  the explicit test inventory, then runs it serially. Cache v2 and the single LLD
  flag are implemented. The targeted run's 565 passes/26 ignores and 18m07 job
  exclude delivery. The full run's 62m20 remains the only complete baseline.
- Native acquisition scans each compressed archive before selected extraction.
  Selected runtime members are read in role/lock order, then notices. A file used
  by both preview and export can be read twice. This is a concrete candidate for
  reducing decompression work, but its cost is not measured yet.
- No production v2 warm result exists. No new release, native acceptance, public
  upload, paid service, signing change, profile change or broad cache replacement
  is needed merely to choose the first experiment.

### Experiments and decision gates

The first implementation candidate is runtime acquisition; the next is QA test
scheduling if the runtime probe is insufficient. Compiler/topology work is
conditional on measured residual cost. Stop each branch when its discriminating
result rejects the mechanism; do not implement all alternatives by default.

For E1, a representative `.tar.zst` mechanism probe **requires CPython 3.14 and
the actual standard-library `r:zst` decoder path used in CI**. The existing
Python-before-3.14 fallback fully decompresses to `BytesIO`; it can establish
functional output parity, but cannot confirm or refute repeated Zstandard
decompression on hosted CI. A result from that fallback leaves the CI mechanism
undecided. Prefer the observed hosted 3.14.7 patch version for the local probe;
record the exact patch version on both sides of every later hosted comparison.

| Experiment | Hypothesis and baseline | Changed input and measurement | Decision and bounded cost |
| --- | --- | --- | --- |
| E1: eliminate repeated archive reads | Existing 396s full acquisition includes network, extraction, hashes and notices; a warm cached local baseline can show whether ordering/duplicate reads dominate | Instrument existing acquisition boundaries and compressed-stream seeks without changing accepted bytes. Reuse pinned cached archives, forbid missing-object downloads for the initial probe. Compare existing order with physical member order/read-once, retaining full archive validation and every output hash. Record Python decoder, archive hashes, output inventories, elapsed/CPU and decompressed bytes/backward seeks. | First inspect cache availability and the largest representative packages plus native crate notices. Run one bounded baseline/prototype comparison, maximum 10 min per arm, no Cargo. Adopt a simple implementation only for identical validated outputs and a clear mechanism/time reduction; target at least 60s in a complete repeated acquisition phase for moderate complexity. A small local microprobe does not establish hosted savings. |
| E2: schedule independent media tests | The same compiled 27 harnesses took 362.40s serially; media alone 158.60s. Compilation cost is separate. | Audit test names for shared environment, locks, self-executable children and timing constraints; keep those serial. Compare existing libtest execution with two slots for audited isolated groups using the exact same workers, tests and FFmpeg. Record the complete names/results/ignore inventory, durations, peak process/RSS/CPU and descendant cleanup. | One serial/two-slot pair, no recompilation between arms, timeout 20 min each, retries zero, unchanged deadlines. Adopt only if every required test remains accounted for, no new failure/leak and at least 60s saved on a matched hosted phase. Try nextest only if the simpler comparator exposes an important inter-harness limitation. |
| E3: required-work routing | Some platform-independent tooling changes may currently trigger all native jobs; manual Windows feedback still compiles release tests. Existing manual delivery separation already avoids most ordinary release work. | Audit `verify-unix.sh`, workflow triggers and actual recent run mix. For app/test/runtime/docs/wrapper/compiler edits, map outputs and required gates. If a concrete redundant route remains, implement a conservative classifier with an unconditional summary gate and fail-closed unknown-path behavior; retain Windows/native tests wherever their inputs change. | Do this inexpensive source/control audit while E1 is reviewed. No generic skip-tests or cross-commit proof reuse. Require an exact representative change matrix showing omitted work is irrelevant, preserved gate behavior, and matched triggered-job evidence before claiming a whole-run saving. If routing is already appropriate, explicitly retain it. |
| E4: release ThinLTO, only after residual evidence | Fat LTO/cgu1 may dominate remaining production compilation, but the 781s historical phase includes cold dependencies | Preserve GNU/LLD, opt-level `s`, cgu1 and strip; vary LTO alone. Capture Cargo timings on the next necessary v2 fill/warm producer. A later isolated compiler experiment must keep the baseline cache read-only and record both dependency and NOH unit costs. | Do not launch two full releases for benchmarking. Require a reviewed experiment version before costly builds. Adoption needs meaningful net compilation savings plus final-byte startup, imports, media/preview/export and update/repair acceptance, size and performance comparison. A faster link log alone cannot qualify it. |

E1 may instead falsify the extraction hypothesis and identify network as the
remaining cost. In that case two-worker immutable-object prefetch is the next
single-variable variant, preserving HTTPS/hash checks, unique pending paths,
atomic completion, retry bounds and failure propagation. It requires cold-network
measurement; warm extraction is not a comparator. No overlap with QA until
contention is measured. No automatic move to four workers.

Integration-harness consolidation, helper extraction and QA/release overlap stay
conditional: capture unit timings first; preserve test paths, helper source-root
identity and all gates. A 104s helper phase does not justify a broad workspace
rewrite by itself. Runner purchases, repo migration and a cache stack are outside
this experiment sequence.

### Measurement, validation and completion contract

1. Before a run, record exact changed-file SHA-256 values/diff, base commit,
   command, tool versions, source/worker/runtime hashes, cache state and scope.
   The existing recorded-process helper owns timeout, stdout/stderr, exit status
   and terminal process JSON; reuse it instead of building another watcher.
   Before every heavy step, check for conflicting project workloads.
2. Keep raw probe evidence under `.mcp-dev/ci-cost/`; append results here and round
   decisions to the register. Record the live exec/session handle there. A known
   tool process is awaited directly. Hosted execution uses the existing
   `logs/delivery/github-inspect-ci.ps1 -Wait` observer once, saving terminal logs;
   no model polling and no repeated log reads while unchanged.
3. E1 requires synthetic malformed-archive/member/hash checks and exact real
   inventory parity, including original notices/materials and PE import/FFmpeg
   validation. E2 preserves all 565 required passes and 26 existing ignores by
   names/reasons, media-required environment, worker identities and cancellation.
   No release/native tests are substituted with syntax checks.
4. Submit implementation and local evidence as a new exact version for Critic
   review before a hosted dispatch. If a push is needed, prepare the reviewed
   concrete diff and proposed commit/push action for the coordinator's explicit
   authorization; a plan OK alone authorizes neither publication nor completion.
5. Hosted E1 should run old/new acquisition in one disposable Windows job with
   identical pinned downloads and tool/image context, separate fresh outputs and
   recorded order; network acquisition and warm extraction are separate results.
   No signing or app build is necessary for this scoped comparison. Validate the
   measured implementation's normal caller and failure propagation too.
6. Report local/hosted, cold/warm, phase/workflow, observed/projected separately.
   Compare summed runner time, cache/upload bytes and elapsed critical path;
   never add nested durations or describe 62m20 to 18m07 as a complete speedup.
   Repeat only if variability/borderline results could change the decision.
7. Finish the overall goal only with implemented, substantially reduced work/time,
   matching evidence for the claimed scope, preserved quality/delivery guarantees
   and final-version Critic OK. If only a phase improves, state that boundary and
   leave the broader question open; do not substitute an approved plan for success.

Current live processes: none. Next action: submit v2, establish the representative
Python decoder, audit available runtime archives and instrument the smallest E1
mechanism probe. No compile started. Critic M1 accepted: the fallback/CI decoder
distinction is now a decision prerequisite, not just a recorded environment field.

## Implementation v3: reduce native-notice temporary files — 2026-10-07

Plan v2 received independent Critic OK on report SHA256
`4da28264f097eafd11e9d5fd67e817a4e50b0c2d58f27730c73c73a01dd121e1`.
That approval covered the plan only. v3 adds the implementation, regression
controls, actual local evidence and a reviewable hosted experiment.

### Probe result and selected change

The four-largest-native-package probe used the same CPython 3.14.7 `r:zst`
decoder as the historical runner. Three packages had no redundant selected reads
removed by ordering; FFmpeg reduced 17 extract calls to nine and two decoder
rewinds to one, saving about 0.16s locally. This does not justify a broad ordering
refactor or refute every streaming alternative. Evidence:
`.mcp-dev/ci-cost/runtime-e1/probe.{py,json,process.json}`.
The official portable interpreter was obtained without a system installation;
its archive SHA256 and official source URL are in `python-provenance.json` there.

The more useful finding was work that the notice producer does not consume:
`native_rust_notices` materialized all crate source files into a temporary vendor
tree, then read notice files and deleted the tree. The complete original `.crate`
archives are already retained in native source materials. The revised function
still reads/hashes every regular member and keeps the full per-file checksum map,
but writes only Cargo metadata, VCS metadata, discovered notices/fonts, the
explicit `license-file`, and the complete sources of any crate requiring complete
upstream-content correspondence. It preserves the existing notice generator.
All members are validated, including unselected files. Explicit root-file and
file/parent collision refusals retain failures formerly exposed by extraction.

| Local notice subphase, same 653 pinned crates | Original | Final implementation |
| --- | ---: | ---: |
| Observed elapsed | 64.525s | 22.008s |
| `Path.write_bytes` calls | 32,737 | 2,455 |
| Bytes written through those calls | 574,307,834 | 7,509,183 |
| Final notice/material files | 3 | 3, byte-identical |
| Existing missing-notice records | 26 | Same 26 identities |

The deterministic reduction is 30,282 file writes and 566,798,651 temporary bytes.
These counters exclude JSON/text writes and final `copytree` writes; they are not
total disk traffic. The local elapsed difference is 42.517s (65.9%) for this
subphase only. This is a directional result: baseline ran first, OS filesystem
caches were uncontrolled, and an independent inventory-hash command could overlap
the final candidate invocation. An earlier candidate before the additional
topology refusals took 18.313s; the normal caller later spent 28.389s inside the
final notice function. Do not treat these differing contexts as repeated matched
measurements or extrapolate them to hosted minutes.

Evidence: `notices-baseline/measurement.json`, `notices-candidate/measurement.json`,
`notices-candidate-final/measurement.json`, their process records, and the unchanged
`measure-notices.py`, all under `.mcp-dev/ci-cost/runtime-e1/`. The records bind
the exact source hashes, interpreter, platform, crate-lock hash and output hashes.
Baseline and final inventories match, including the difflib source-correspondence
proof and both generated notice files. No notice gaps were hidden or removed.

### Validation and prepared hosted comparison

- The first broad local native baseline stopped on sandbox temporary-directory
  ACL refusal before notice generation. Its `baseline/measurement.json` is a
  failed partial result, not a successful acquisition or end-to-end baseline.
  Subsequent experiments used the normal Python temporary-file behavior outside
  the sandbox, with their temporary root inside the ignored experiment directory.
- All 42 delivery controls pass in 2.084s on the final code (39 existing plus
  three new tests with malformed-archive subcases). They cover notice selection,
  an explicit license path before the last-positioned manifest, all-source hashes,
  complete-correspondence retention and invalid unselected members. Existing
  supplement, hash, original-byte, PE closure and delivery controls remain active.
- The final code's normal `windows_runtime.acquire` caller succeeds on all cached
  native inputs in 47.972s, including 878 checked fetches and FFmpeg validation.
  Export and preview inventories match the original prefix; final notices match
  the successful original notice baseline. PE audit finds 113 export and 130
  preview images with zero missing imports. Evidence: `native-candidate/`,
  `native-candidate.process.json`, `pe-validation.json` inside that output,
  and `delivery-controls-final.{stderr.log,process.json}`.
- No Rust application, worker, signing, installer or GUI change was made; no
  application build or media test campaign was repeated for this acquisition
  change. Existing application acceptance is retained for its original bytes;
  a future rebuilt application keeps its own new broad source identity.
- `windows-runtime-measure.yml` is manual and private-branch restricted. It pins
  baseline `b5a52bd`, uses Python 3.14.7 and the baseline's exact existing raw cache
  read-only, and fails before acquisition if the cache is absent. Two sequential
  invocations of `measure-windows-runtime.py` run complete old/new acquisition on
  one Windows runner. Missing cached objects fail instead of downloading. Both
  arms have 600s process limits; the job has a 30-minute limit and shares the
  producer's concurrency group. There is no Rust, signing, package, cache save,
  release mutation or binary upload. Only JSON/log evidence is retained.
- The comparator requires equal interpreter/platform/harness, unchanged lock and
  acquisition dependencies, equal checked-fetch counts/bytes and exact complete
  output inventories. It records full acquisition and nested notice timing, not
  a predicted full release. Initial order is baseline then candidate; reverse
  only if variability or a borderline result could change the decision.
- Actionlint, Python syntax and diff checks pass. The hosted run has not been
  dispatched. Review/authorization is still required for the concrete commit/push
  that makes this new workflow available; no authorization is inferred from OK.

Remaining costs and scope: the production raw-runtime key still includes the
changed acquisition script, so its first post-change full run would miss the old
exact key. The diagnostic explicitly restores that old cache; this is not proof
of a first-run production saving. No cache migration or policy change is included
in v3. Ordinary PR routing remains unchanged after the audit: full release work
is already manual, Unix tests share their debug graph, and broad tools changes
can still trigger all three Unix platforms. E2 and compiler/residual-work choices
remain open. This local phase gain alone does not complete the CI goal.

Live processes: none. Next: independent v3 code/evidence/hosted-experiment review;
then route the exact reviewed commit/push proposal to the coordinator. The overall
Goal remains active; no new release or native acceptance is claimed.

## Review correction v4 and concrete hosted prerequisite — 2026-10-07

Critic returned REVISE v3 for M2: the new notice selector rejected `license-file`
forms such as `./legal/custom.txt` that the existing generator normalizes with
`PurePosixPath`. Accepted and fixed: use the generator's existing traversal,
absolute-path, backslash and colon validation, then normalize only the selection
key. The original metadata remains unchanged in the notice inventory. The
regression fixture uses `./legal//custom.txt` with a non-notice filename and checks
both retained original text and unchanged metadata. Added unsafe license-path
subcases keep the refusals explicit. All 42 controls pass in 2.397s;
`runtime-e1/delivery-controls-v4.{stderr.log,process.json}` records the result.
The existing 653-crate outputs/timings remain evidence for their exact recorded
source version; this narrow compatibility fix does not justify another full local
acquisition or timing campaign. No new timing claim is made for v4.

Read-only remote preflight confirms private repository 1406079938, default branch
`main`, `codex/setup-release` still at `b5a52bd`, no open PR for that branch and no
unfinished run in its latest 30 runs. The original raw runtime cache is available:
ID `8572635570`, size `2,470,779,528` bytes, key
`noh-runtime-v1-windows-x64-d88c3148333c2bdc3010ac0778a8920f2c165bf5fe17dba938f27600c32be8dd`.
Its last access was 2026-10-06T14:46:33.906755Z. Cache availability may change;
the job still fails closed on an exact miss. Receipt:
`.mcp-dev/ci-cost/runtime-e1/remote-preflight.json`.

The new diagnostic is not registered remotely. GitHub documents that
[manual dispatch requires the workflow on the default branch](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow).
Therefore v4 adds an explicit push trigger restricted to `codex/setup-release`
and changes of this exact diagnostic workflow file. The proposed reviewed push
will start one comparison through that event, without changing `main` or relying
on an unverified manual-dispatch route. It pins the observed existing cache key
directly rather than recomputing it from a nested checkout. Every object still
passes the current expected digest and both acquisitions remain read-only with
respect to cache writes.

Concrete action, pending final-version Critic OK and human authorization through
the coordinator: commit exactly the five files in `pair/v4-identity.json`, with
message `Reduce temporary native notice extraction and measure cached acquisition`,
then normally push `HEAD:refs/heads/codex/setup-release` to the existing private
origin. This starts one Windows measurement job (30-minute maximum, two 600s
process limits). It restores about 2.47GB of existing cache, checks controls,
compares acquisition, and uploads only logs/JSON with seven-day retention. It
does not create/sign/package/publish a release, compile NOH, write a cache, merge
or alter repository settings. Its compute/storage count against existing account
usage; no purchase is proposed. The observer will wait on the resulting run once
and save terminal status/logs before collecting its small evidence artifact.

Critic's v3 review accepted the other implementation evidence and experiment
structure, with the existing scope/measurement limits. v4 requires its own exact
OK; no commit, push or hosted execution has occurred.

## E2 extension: remove repeated fixture preparation — 2026-10-07

The local scheduling discriminator preserved all 14 media tests with the same
compiled binaries: serial 153.544s, two threads 97.675s on an eight-logical-CPU
host. The 55.869s difference is local only; it does not establish the proposed
hosted 60s adoption threshold. Evidence is in `.mcp-dev/ci-cost/media-e2/` and its
v5 identity. No parallel production scheduling is adopted from that result.

The source audit found a simpler independent reduction: seven `Cases::new`
callers in `tests/media/cli.rs` each regenerate the same eight input files and
validate rotation, costing nine FFmpeg invocations each. Cache the eight immutable
byte arrays in a process-local `OnceLock`, using the unchanged generator and
rotation assertion once, then write independent copies into each test's existing
`TempDir`. Never retain a static temporary directory. All test names, product
assertions, subprocess limits, output paths, and cleanup checks remain unchanged.
This removes 54 fixture-preparation FFmpeg invocations per full media harness;
all functional assertions still execute for each test.

Validation: reuse the already measured baseline and its retained worker/runtime
identities; compile the modified harness once and run the 14 tests serially and
with two threads. These local runs validate the new shared initialization and
per-test file isolation, but their timings remain directional. A hosted E2 job
will compile the required suite once, execute all tests with the existing serial
policy, and measure the media harness with two threads on the same binaries.
For a directly comparable fixture-only baseline, compile the original media CLI
test module from a8ced20 as an additional diagnostic harness in the same graph
as the changed module; both use the same final worker, library and FFmpeg.
Record the generated source, all binary hashes and exact test-name/results.
The hosted job must require the historical QA cache, use the existing pinned
GNU/LLD toolchain and runtime, avoid release production and cache writes, and
retain logs/JSON only. No scheduling policy changes before measured acceptance.

The user has now authorized the required commits, pushes and optional PR within
legality. Critic exact-version review remains required before hosted dispatch.
Current E1 run: 37547242943, source a8ced2028401a28e4fddd655395335ebedfdbaf8;
its existing observer is active. Hosted E1 outcome is not yet established here.

## Hosted E1 outcome and E2 preparation v9 — 2026-10-07

After explicit human authorization, the five approved v4 files were committed
and pushed as `a8ced2028401a28e4fddd655395335ebedfdbaf8`. The single filtered push
run [37547242943](https://github.com/Awalgawe/noh/actions/runs/37547242943) completed
successfully. Its job took 233s including setup, cache restore, both measurements,
controls and diagnostics. All 42 delivery controls passed in 2.033s.

| Same hosted runner, same cached inputs | Original | Candidate |
| --- | ---: | ---: |
| Full warm runtime acquisition | 111.500s | 50.213s |
| Native notices, nested within acquisition | 79.770s | 13.488s |
| Complete final inventory | 1,182 files | Same paths, sizes and SHA-256 values |

The observed acquisition difference is 61.287s (55.0%). The nested notice
reduction must not be added to it. Input/tool/source comparison and full inventory
parity passed in CI and were independently recomputed by the local collector.
The authenticated artifact SHA-256 is
`05e412db22b529218133d56f950a4a8dd271c72d211c8332ded07ec4008145a7`.
Evidence: `.mcp-dev/ci-cost/37547242943/{run,jobs,report}.json`, its `diagnostics/`,
and `logs/delivery/github-run-37547242943/job-112554059786.log`.

This is one baseline-first pair with uncontrolled OS caches, not a guaranteed
minimum saving or a new full-release duration. It meets the nominal 60s phase
criterion narrowly; the deterministic removal of 30,282 temporary file writes
and repeated local directional observations support retaining the small change.
No reverse run is claimed. Production raw-cache key invalidation remains a
separate cost: the existing key includes producer scripts and the diagnostic
explicitly reused the old raw cache. No first production-run saving is proven.

Critic approved v8 fixture code and plan, including the structural removal of
54 FFmpeg preparation invocations and preservation of per-test file ownership.
Local changed-harness compilation took 72.786s. Both local arms subsequently
passed all 14 tests, but the pair's final executable-identity guard failed.
Diagnosis: the discovery `cargo test -- --list` recompiled after diagnostic-script
edits changed the broad build fingerprint, after the initial hashes had been
recorded. The timed arms themselves report no compilation. Their times are not
accepted as a valid same-binary comparison; the raw failed experiment is retained
at `.mcp-dev/ci-cost/media-e2-fixtures/`. No timing rerun is needed before the
already planned hosted comparison. Functional pass evidence is scoped to the
unchanged v8 fixture source and the actual rebuilt local binaries.

The new `windows-media-measure.yml` and `measure-windows-media.ps1` prepare one
bounded hosted experiment. Before compilation, they generate an extra integration
entry using the original a8ced20 CLI test module and unchanged media support.
Both original/current harnesses are compiled in one graph against identical
current workers/library/runtime. This avoids a second worker/library compilation
solely to construct a fixture baseline. The generated diagnostic source is
recorded; its binaries are QA subjects, never release qualification evidence.

The three arms are original serial, candidate serial, and candidate two-thread.
Each discovers and executes exactly the same 14 names, uses zero retries and the
unchanged 60s child limits, and has a 600s enclosing timeout. Afterward the rest
of the original suite executes serially; combined with candidate serial it must
account for 565 passes and 26 existing ignores. Existing timing/IPC/updater tests
stay serial. Cargo recompilation after the measured binary hashes is refused.
Sampling records CPU observations, peak watched process counts/RSS and remaining
watched processes; it can miss short-lived processes and is not a complete
ancestry tracer. Preserve that limitation when interpreting its numbers.

The job requires the historical QA and raw runtime caches, retains the pinned
GNU/LLD toolchain, saves no cache, produces no release and uploads logs/JSON only
for seven days. Maximum job time is 45 minutes; each compilation limit is 30
minutes and the remaining suite limit is 20 minutes, bounded by that job limit.
PowerShell parsing, actionlint and diff checks pass. Exact-version Critic review
is required before the next commit/push triggers this job. No E2 hosted dispatch
or production scheduling adoption has occurred. Overall performance Goal remains
active; no native acceptance, release-profile gain or full end-to-end saving is
claimed.

## E2 prerequisite correction v10 — 2026-10-07

Run [37548794658](https://github.com/Awalgawe/noh/actions/runs/37548794658), at
7bdc23454076d797645edd71211ccc5575581600, stopped before any compilation or test.
All four caches hit, but `cargo fetch --locked --offline` could not find the
`base64` registry index entry. An exact compiled-dependency cache hit does not
prove the registry index is complete. The failed job consumed 173s, including
restoring the raw runtime; it produced no measurement artifact. Original run/job
records and the failure log remain under `logs/delivery/github-run-37548794658/`.

The correction follows the existing producer's dependency preparation: fetch
locked dependencies once with network access, restricted to the Windows GNU
target. All compilation and timed executions remain offline. This changes no
Rust version, lock, test, compiler flag, cache-save policy or arm. A new push is
required because the old attempt ran the old workflow; do not rerun that unchanged
failed attempt. Actionlint and diff checks pass on the correction.

The ignored media result collector is also prepared for review. It reuses the
approved v7 producer/attempt/artifact checks and exclusive fresh evidence folder,
then independently checks the three exact test-name lists, process results,
required 565/26 inventory, compiler contract, watched-process cleanup and timing
arithmetic. Syntax checked, not executed remotely. Collector path:
`.mcp-dev/ci-cost/media-e2/collect-ci.py`. No measurement or success is inferred
from the preparation failure.

## E3 implementation v11: one helper for explicit Windows source roots

The next eliminated operation is compilation of the development helper inside
fresh versioned source snapshots. The historical matched hosted observation was
104.38s for this phase even after feature/LLD unification. That time is a baseline
observation, not the new saving. The helper's compiled-in `CARGO_MANIFEST_DIR`
previously made simply reusing the checkout helper incorrect.

`dev build --source-root <path>` now canonicalizes and validates the selected root,
uses it for both Cargo build/metadata, and passes it to the existing packaging
implementation. Without this option, behavior retains the compiled-in root.
Explicit-root packaging is limited to Windows; macOS still embeds its package
version. GUI packaging also requires the selected speech/preview descriptions to
match the helper's embedded bytes. These are explicit failure boundaries, not
permission to silently package another source tree or runtime.

The Windows producer compiles the helper once in the already used QA feature
graph, before setting release build options. It checks the original source
inventory, copies the helper into the case's tools directory, records its hash
and source context, and rechecks that hash before each explicit-root invocation.
The existing frozen snapshot checks, staged executable identity checks, bundle
version checks, runtime hashes, signatures, three profiles and delivery gates
remain. No release compiler profile or application code changes.

Local evidence is under `.mcp-dev/ci-cost/helper-e3/`. The initial helper compiled
in 48.109s; this includes local invalidated library work and is not a CI saving.
That implementation built two real dependency-free Rust projects (0.8.7/0.8.8)
from paths containing spaces and Unicode, while invoked from the repository
root. Each resulting executable reported the selected version and source root.
An existing directory without Cargo.toml failed before building. These checks
used --no-bundle: they prove source selection, not a new NOH release/bundle.

Inspection then identified embedded speech/preview JSON and the Mac package
version. The additional compatibility refusals described above were implemented;
the new guard test and all existing helper tests pass: 15/15, zero ignored, in
0.17s after compilation. Earlier source-selection evidence remains scoped to its
recorded helper hash; those no-bundle code paths are unchanged by the guard.
No new whole-application test campaign is warranted for packaging-helper routing.

A 30-minute private Windows diagnostic is prepared for exact-version review. It
restores the existing QA/tool caches, fetches locked Windows dependency inputs,
and builds the same QA workers once. It then measures the reusable current helper
and the old bcf286a helper built in a version-0.1.3 snapshot, with identical flags,
features and target cache. The current helper runs first, so any OS cache advantage
favors the old snapshot helper; caches are nevertheless uncontrolled. It preserves
the reusable binary before the baseline replaces the normal output, executes the
two real synthetic source-root cases and invalid-root check, and runs all 15
helper tests. It downloads no media runtime, saves no cache and produces no NOH
release, bundle, installer, signature or public asset. Tiny diagnostic crates use
a separate target directory. Only JSON/logs are retained for seven days.

Artifacts: `windows-helper-measure.yml`, `measure-windows-helper.ps1`; ignored
collector `.mcp-dev/ci-cost/helper-e3/collect-ci.py` independently checks producer,
artifact, commands, case outputs, test count and timing arithmetic. Static
PowerShell/actionlint/format/diff checks pass; collector syntax passes. A clear
matched helper-phase reduction and successful routing/guard tests are required
before a gain is accepted. A hosted result and Critic review are still pending.
The full-release elapsed time and production v2 warm-cache result remain unmeasured.

The corrected E2 run is 37549357105 at bcf286a; its original observer remains
active. Do not create a duplicate observer or rerun its tests while waiting.

## E2 hosted result and next dominant-cost experiment v12 — 2026-10-07

The corrected [run37549357105](https://github.com/Awalgawe/noh/actions/runs/37549357105)
succeeded at bcf286a, attempt1, job112560888170. The authenticated artifact digest
is `c668569c95a306148a6a164b57d3238eb866d7f107fb898e2e7f8b3252c2bd89`;
original records and independently recomputed report are in
`.mcp-dev/ci-cost/37549357105/`. All565 required tests passed;26 existing tests
remained ignored. Each of the three media arms executed the same14 names.

| Same two-logical-CPU runner | Seconds |
| --- | ---: |
| Original fixtures, serial |108.991|
| Shared fixture bytes, serial |103.232|
| Shared fixture bytes, two threads |89.464|

Fixture reuse saved5.759s; adding a second thread saved13.768s. Their combined
19.527s is far below the proposed60s scheduling adoption threshold. Keep the
small deterministic fixture-work reduction, but reject production parallelism
on this evidence. The earlier eight-CPU local result did not transfer to this
runner. Watched FFmpeg peaks were1/1/2, sampled working-set peaks90.2/89.8/102.8MB;
the last arm had one unavailable process sample. No watched process remained.
These are sampled exact executable paths, not exhaustive ancestry/peak proofs.

The job took897s including setup, preparation and repeated measurement arms.
Workers89.125s, test compilation155.268s and remaining-suite146.691s are separate
observations; they are not comparable speedups against the earlier different
runner's158.91/289.48s. The raw cache/OS cache/order limits remain. E3 is pending
in run37550499119 at ed55f2f; its sole observer is active. Overall Goal remains
open: the measured media gain is too small to settle the full-CI objective.

### E4: evaluate Windows release ThinLTO without weakening delivery gates

The remaining major candidate is the historical781s release compilation phase.
Preserve opt-level=s, codegen-units=1, strip, locked dependencies, GNU/LLD, features,
source behavior, test inventory and all release gates. The single intended
compiler change is complete LTO versus ThinLTO. The production profile stays
unchanged until evidence and exact-version review support adoption.

Prepare one bounded Windows diagnostic sharing the existing concurrency group.
Use the current pinned toolchain and cached dependencies. Library-only builds
prime release dependencies separately for complete and ThinLTO. Then compile fresh copies of the same source with
ThinLTO and complete LTO. Keep the same application version; record distinct
nonsemantic benchmark comments in copied Cargo.toml files to force a fresh
workspace build without deleting unrelated caches. Each arm has its own source
and copied final executables; all arms share the target dependency cache. Record
full commands, effective LTO option, source hashes, rustc commands, compiled versus
fresh dependency names, elapsed/CPU context and sizes. Explicit single LLD flags
remain in force across nested roots.

Do not compare a cold dependency rebuild with a warm one. Require all non-NOH
dependencies to be reused in the measured arms. The cheap local probe confirms
that changing LTO recompiles dependencies, while library-only priming avoids
linking the final executables twice. If inputs
or reuse cannot be established, retain the failure/incomparable result rather
than announcing a gain. The candidate-selection criterion is at least20% and120s
saved on the matched warm release-build scope. This threshold and the Rust suite
do not authorize production adoption. Adoption also requires the previously
accepted final-byte startup, imports, media/preview/export and update/repair
acceptance, size and performance comparison, with no meaningful regression.

Validate the actual candidate release configuration with the full required Rust
suite, matching GUI/CLI/MCP workers, required FFmpeg environment, serial execution,
zero retries and original ignores. Compile test artifacts only once for that
candidate; verify the test-name inventory and every result. Query all five
candidate executables' build identities and preserve their hashes before/after.
The existing build-identity options already capture every CARGO_PROFILE_* value,
including release LTO; verify distinct option fingerprints instead of adding
redundant provenance code. Keep macOS/Linux release settings unchanged unless separately
qualified. No signing, bundle/installers, public release or native acceptance is
claimed by this experiment; their existing gates remain required.

Proposed bound: one hosted job,75-minute maximum; each release build or test
compilation30minutes, execution20minutes, all bounded by the job deadline.
Require sufficient disk space, preserve terminal logs and partial failures, save
no cache and upload diagnostics. If the compilation threshold selects a candidate,
retain both compared sets of executables in a separate private, three-day artifact
for final-byte qualification without recompiling them. These are experimental
bytes, never public release assets. Existing process/observer/collector
mechanisms provide completion. This is a plan only: no E4 profile change,
implementation, build or dispatch has occurred. Critic review precedes its
implementation review and hosted dispatch; no additional user approval is needed
under the current explicit autonomous authorization.

## E3 partial hosted result and E4 revised boundary v13 — 2026-10-07

E3 run37550499119 at ed55f2f failed in the diagnostic assertion after both
compilation arms succeeded. The helper correctly built the first requested
synthetic source; Cargo returned a canonical Windows path while the assertion
expected mixed separators. The authenticated partial artifact is retained in
`.mcp-dev/ci-cost/37550499119/`, digest
`aab74b4445157d4ba00dc407cb4d4afa12c0d577383a1a466e17bd7152477c38`.
The collector deliberately refuses to declare the failed job successful.

The completed reusable-helper phase took6.2066164s; the original snapshot helper
took100.7322971s, an observed94.5256807s reduction after the same QA workers.
This is a helper-phase measurement from a failed overall diagnostic, not a
successful full release or an integrated pipeline saving. Both use the same
features, QA profile, single LLD flags and target cache. The historical helper
must rebuild the versioned NOH library; the reusable helper uses that library
from QA. This avoided rebuild is the intended mechanism.

Canonicalize the expected root with GetFullPath. The exact corrected hosted
routing block was executed locally against the final helper, SHA256
`74701fe732a60817ffa001a78f647671216abadba756d706a501bf249554403f`.
Both actual tiny Cargo builds (0.8.7 and0.8.8, paths with spaces and Unicode)
and the invalid-root refusal passed. Evidence:
`.mcp-dev/ci-cost/helper-e3/final-validation/` and `validate-final.ps1`.
The previously reviewed15 helper tests cover unchanged Rust code, including
both embedded-contract mismatch refusals. Do not repeat the whole hosted build
just to rerun the corrected path assertion. No hosted success, packaging or
production warm-cache claim is inferred from the combined scoped evidence.

Critic v12 accepted E2 and raised M4: the compiler timing threshold cannot be the
production adoption gate. The E4 text now explicitly separates candidate
selection from all previously required final-byte validation and size/performance
comparison. Production release settings remain unchanged.

Two cheap local synthetic probes in `.mcp-dev/ci-cost/lto-e4/` establish the Cargo
mechanism, not NOH timing. Fat-to-Thin changes recompile the dependency. Separate
library-only priming gives Fresh dependency records in both subsequent binary
arms, avoiding two unnecessary full executable builds. The hosted diagnostic
must enforce that reuse for every dependency; any mismatch invalidates the pair.
Both compared executable sets will be retained privately only if the timing
threshold selects a candidate, enabling later final-byte checks without another
build. No public release, signing or installation is performed at this stage.

## E4 diagnostic implementation v14 — 2026-10-07

Prepared `windows-lto-measure.yml`, `measure-windows-lto.ps1` and ignored
`.mcp-dev/ci-cost/lto-e4/collect-ci.py`. The job uses the reviewed v13 plan, the
existing private branch/concurrency group and read-only pinned caches. Four
fresh archived source roots keep version0.1.0 and differ only by a documented
manifest comment: fat library priming, thin library priming, thin warm binaries,
fat warm binaries. No full executable builds are used for priming. Twelve GiB
free is required before each build; six GiB before candidate test compilation.

The runner rejects any compiled non-NOH dependency in the measured arms, unequal
dependency sets, missing/fresh workspace binaries, missing effective compiler
flags, differing worker identities or an untracked LTO option fingerprint. Each
arm copies all five binaries, recording hashes and sizes. The warm comparison
excludes dependency priming and setup; their costs remain separately recorded.
Thin runs before fat, with OS caches explicitly uncontrolled.

Only a timing-selected candidate incurs the full release Rust suite. Required
targets are checked against Cargo metadata; every executable's discovered test
names must match executed/ignored names. The expected current total is566 passed
and26 unchanged ignores, including the new helper contract test. Serial order,
media inputs, zero retries, bounded child processes and worker/hash checks remain.
Rejected timing candidates have no test-success claim and leave production
unchanged. Even a fully successful diagnostic only selects a candidate for the
separate final-byte/size/performance gates.

The workflow retains diagnostic logs/JSON for seven days. A selected candidate's
two sets of experimental executables are retained privately for three days,
including on a later test failure to permit diagnosis. The collector authenticates
run/attempt/source/path and artifact digests, recomputes dependency reuse, test
inventories and timing thresholds, and downloads selected bytes against recorded
hashes only after a successful required suite. Failed jobs retain authenticated
partial diagnostics and are not reported successful. Actionlint, PowerShell
parsing, Python syntax and diff checks pass. No E4 dispatch or production LTO
change has occurred; exact-version review is requested before the single push run.

v15 corrects review M5 before dispatch: the explicit fat profile override emits
`-C lto=fat`, while the first validator expected the shorthand `-C lto`. The
existing local probe logs reproduce that mismatch. Both corrected positive
checks and opposite-mode negative checks pass against those logs, without any
new compilation (`.mcp-dev/ci-cost/lto-e4/flag-validation.json`). Other v14
workflow, runner and collector behavior is unchanged.

## E4 first hosted attempt and parser correction v16 — 2026-10-07

[Run37553055598](https://github.com/Awalgawe/noh/actions/runs/37553055598),
commit7882820, attempt1/job112572823669, failed after successful fat-library
priming (326.1153981s). No ThinLTO or matched warm measurement was completed.
The parser incorrectly assumed that `-vv --message-format=json` produced only
JSON: Cargo also forwards prefixed build-script stdout. The actual stream has
341 compiler artifacts,40 script records,2,826 compiler messages, one successful
build-finished record and397 prefixed text lines. This is a harness failure,
not a compiler failure or evidence for either optimization.

Authenticated partial diagnostics remain in `.mcp-dev/ci-cost/37553055598/`;
artifact digest `db162c7ee26e45cf90535b7608cb1670779cbcb01318e7ee7fdf4fb5bd358bf2`.
The collector retained them and refused a success report. The original observer
is terminal. The correction consumes only Cargo's documented bracket-prefixed
script lines outside JSON, preserves the raw stream and requires exactly one
successful build-finished record. Malformed JSON, failed, missing and duplicate
completion records still fail. PowerShell runner and Python collector recover
the same341 artifacts from the real log and pass those positive/negative controls
without a new build. Evidence: `lto-e4/parser-controls/` and `validate-parser.ps1`.
Actionlint and syntax checks pass; corrected dispatch awaits exact review.

The subsequent qualification boundary is also concrete: these diagnostic workers
have no production update trust configuration. They can support startup/import/
media/performance comparison. If selected, update/repair acceptance will require
a later correctly configured delivery candidate, with its own byte identities;
the diagnostic cannot stand in for that gate. Production LTO remains unchanged.

## E4 measured decision and closure proposal v17 — 2026-10-07

The corrected [run37554235331](https://github.com/Awalgawe/noh/actions/runs/37554235331)
succeeded at e58ceb5, attempt1/job112576628819. The authenticated diagnostic
artifact digest is
`07edd1cc0c975e419f860a235b505387335a4bad0105fc35810e3be834be18e0`.
Raw records and the collector's independently recomputed result are preserved
in `.mcp-dev/ci-cost/37554235331/`.

| E4 scope on the same runner | Seconds | Dependency work |
| --- | ---: | --- |
| Complete-LTO library priming |406.070|339 dependencies compiled; excluded from the warm comparison|
| ThinLTO library priming |361.952|339 dependencies compiled; excluded from the warm comparison|
| ThinLTO five-worker warm build |299.523|339 Fresh, zero dependency compilation|
| Complete-LTO five-worker warm build |271.152|Same339 Fresh, zero dependency compilation|

ThinLTO was28.371s (10.46%) slower in this pair and did not meet either part of
the20%/120s candidate threshold. Reject it and retain the existing complete-LTO
release profile. Order was thin then fat, OS caches were uncontrolled, and this
single run does not establish universal compiler performance. It is sufficient
to decline the proposed change. The much older781s cold/mixed release phase is
context, not the baseline for a new510s improvement claim.

Each measured arm rebuilt all five workers with the required effective flags;
their build identities agreed within each arm and recorded distinct LTO option
fingerprints. The recorded GUI size also increased from20,782,592 to33,709,056
bytes under ThinLTO. No candidate was selected, so the conditional566/26 release
test stage and experimental-binary upload did not run. No test success, native
acceptance, packaging, signing or deployment is claimed for these experimental
bytes. Existing production QA and delivery gates are unchanged.

### Experiment cost and stopping decision

| Hosted diagnostic job | Result | Job wall time |
| --- | --- | ---: |
| E1 37547242943 |Successful controlled acquisition comparison|233s|
| E2 37548794658 |Missing registry input, before compilation|173s|
| E2 37549357105 |Successful fixture/scheduling comparison and required suite|897s|
| E3 37550499119 |Both compilation arms complete; later path assertion failed|352s|
| E4 37553055598 |Library primer complete; mixed-output parser failed|507s|
| E4 37554235331 |Successful controlled comparison; ThinLTO rejected|1,555s|

Total hosted job wall time was3,717s (61m57s), including failed attempts and
setup/cleanup. This excludes queueing, local work and agent usage; it is not a
billing estimate. Failures remain in the evidence, and no failed run was relabeled
successful. Tiny local controls decided the corrections before each rerun.

The retained changes remove repeated work:55% less warm acquisition time and
93.8% less helper compilation time on their measured scopes, plus the smaller
fixture reduction. These findings preserve outputs and relevant controls.
More parallel tests and ThinLTO did not satisfy their criteria and are not
adopted. However, these phase results do not yet prove the original goal at the
production CI scope. Coordinator and Critic rejected that proposed closure (M6).

The paired closure audit permits reuse of accepted proofs while their relevant
source inputs remain unchanged; it does not narrow the goal to those phases.
A whole-workflow saving and a production warm-v2-cache result remain unmeasured.
The next step is a read-only reconstruction of the actual critical path and its
cache effects, followed only by a discriminating integration observation where
needed. No repeated functional suite or automatic full release is required for
that analysis. The goal remains active.

## Production integration audit and next discriminating step v18 — 2026-10-07

M6 is accepted. A percentage within a phase does not by itself establish a
substantial reduction in the user's CI wait. The original goal remains active;
v17's source/evidence acceptance is distinct from goal completion.

The representative path is the existing Windows B-only producer, with
`verify_build_graph=false`: parallel audit/preflight prerequisites, candidate
tools/caches, QA workers and required tests, full runtime acquisition/cache save,
one new-version build/seal, candidate transport, then installers. No A rebuild
is introduced. Every adopted change lies on this sequential candidate path:
E2 changes fixture work inside QA, E1 changes full acquisition, and E3 changes
the helper compiled before the new-version build. They do not shorten audit,
signing, payload staging, wrappers or native acceptance.

The historical whole-run budget can be reconciled without a build. At944525b,
audit/preflight took19s/6s in parallel, candidate3,278s, installers357s; the
reported3,740s total includes86s outside these critical job durations. Within
candidate: QA1,184s, full acquisition396s, raw cache save114s, build/seal1,445s,
candidate upload22s; the remaining117s cover preparation, other steps and job
overhead. Build/seal includes helper259s and release compilation781s, plus
profile packaging186.43/29.48/6.97s and other work. None of those nested times is
added to its enclosing duration. The later a079532 QA-only run measured817.44s
of QA and104.38s helper work but omitted release, profiles and installers; it
does not replace the whole-run denominator. E4's271.152s warm release phase is
also an isolated reference, not a newly observed full production duration.

### Concrete integration defect: a code edit invalidates unchanged raw inputs

The current production raw-runtime key is
`noh-runtime-v1-windows-x64-7186d4d0c3aca22c8379645a342e33d3a3d1ceec5cbb39855854c45f68b2a685`.
The read-only GitHub cache inventory contains only the older2,470,779,528-byte
raw cache ending`d88c3148333c2bdc3010ac0778a8920f2c165bf5fe17dba938f27600c32be8dd`.
The production restore has no fallback, so its new key misses. The same inventory
still has no production-v2 Cargo cache; the historical QA-only cache is separate.

Reconstructing the ordered Windows hashFiles inputs reproduces the actual old
key at both944525b andb5a52bd. Only `tools/windows_runtime.py` changed the raw key.
All six locked-input files still have the exact hashes recorded by hosted E1.
Thus warm extraction gains were measured against available archives that the
actual new producer would not restore. Its additional network acquisition and
cache-save cost can offset those gains on the next real run. This is a concrete
missing integration observation, not a reason to repeat E1's notice comparison.
Evidence: `.mcp-dev/ci-cost/cache-key-reconstruction.json` and
`cache-integration-before.json`, plus the original job records.

### E5 proposed boundary: cache selection through validated acquisition

Use a Windows raw-cache v2 key based on the six locked inputs and an explicit
download-layout version, rather than extraction implementation files. Preserve
the existing archive/member validation; restored bytes never bypass fetch's
SHA-256 check. A one-time legacy fallback is permitted only when the six-file
fingerprint equals the measured unchanged input set (`a0cfdc77...dbc3eb48`). Any
lock change removes that fallback. Do not use a broad prefix across incompatible
locks. Bump the layout version when cache paths or selected downloads change
without a lock change; extraction-only edits should not redownload the same bytes.

Before adoption, review a bounded Windows diagnostic with the actual production
cache path and action versions. First require the reconstructed current key to
miss, then run the current production acquisition from the resulting empty cache.
Record fetched versus reused objects, byte counts, validated final inventory and
elapsed time. Preserve that owned directory, restore the compatible legacy cache
to the same production path, and repeat the identical producer code. Require a
real restore hit, zero missing candidate inputs and identical full output hashes.
This varies the cache-selection policy, not the notice algorithm or application.

Measure restore plus acquisition in each arm. If validated and selected, save
one new stable v2 cache through the normal trusted-branch action and include its
actual promotion time in the candidate's first-run cost. Do not save a redundant
baseline cache merely to time it. Use the conservative comparison charging zero
save time to baseline and the entire measured promotion cost to candidate; keep
steady-state savings separate. Proposed selection requires at least120s and20%
net improvement on that modified segment after this conservative charge. It does
not automatically authorize a whole-workflow speedup claim or goal closure.

Bound the job to30minutes, each acquisition to15minutes, require sufficient disk
for two owned cache/output copies, retain partial diagnostics, and stop before
Cargo, new release bytes, signing, installers or publication. Validate cache key
selection against every changed lock and retained content against the real fetch
checks. No full functional suite or repeated E1/E2/E3 benchmark is requested.
The expected2.5GB cold fetch is a real experiment cost; do not conceal it as a
free lookup. Exact implementation and experiment review precede dispatch. After
that observation, reassess the original goal against the reconciled production
path and any remaining material uncertainty; do not close it by changing scope.

### E5 implementation v19 — ready for exact review, not yet measured

The diagnostic now reproduces both cache consumers in production order: locked
FFmpeg preparation before QA, then full runtime acquisition. Both arms use the
same current code and shared raw cache; Rust compilation and tests between these
consumers are excluded. It checks the actual GitHub `hashFiles` baseline key
against the reconstructed key before starting. A lookup requires the compatible
legacy reference and refuses an already promoted transition before cold fetch.
The baseline must really miss and start empty; the candidate must restore the
exact compatible key and refuses every missing download. The production
`fetch()` hash check remains active for all calls. Both output inventories and
the ordered requested objects must match before any promotion.

The seven small synthetic controls exercise all six lock invalidations,
implementation-only stability, missing-lock refusal, output/source mismatches,
cold/warm contrast and the provisional threshold. The existing corrupt-cache
control from accepted E1 is reused for unchanged `fetch()`. Static workflow and
Python checks accompany exact review; no new application compilation is needed.

Prescreening only permits one trusted-branch v2 save. The collector authenticates
the completed job/artifact and confirms the new cache exists, then includes the
actual restore and save step durations in the final decision. It also charges
the entire key-resolution step and candidate identity guard to the candidate,
including the diagnostic's extra disk check. Baseline save remains charged zero.
GitHub action durations have one-second resolution; acquisition uses a monotonic
high-resolution timer. Diagnostic inventory reads, lookup, baseline preservation
and artifact transport are excluded from the modified production segment but
included in the reported whole experiment job cost. Require12GiB free disk;
preserve both owned outputs/caches and partial logs. No cache deletion is planned.

The read-only pre-experiment inventory totals3,406,376,621 bytes across six caches.
One new raw cache of the old size would raise that to5,877,156,149 bytes before
eviction; this is a storage estimate, not a quota or billing claim. The cold arm
also downloads about2.5GB. A successful action alone is insufficient: the final
criterion still requires120s and20% net improvement after promotion.

This measures the first compatible transition caused by the current extraction
edit. After an unchanged producer has saved its cache, either policy can be warm;
E5 does not establish a recurring saving on every run. The stable key additionally
avoids needless invalidation on future implementation-only edits, but their
frequency and total benefit are not measured. E1's warm acquisition improvement
is separate and must not be added again to this same-code comparison. The
production restore is prepared on the private branch for this candidate policy;
no full producer is dispatched before the E5 decision. Original-goal assessment
and the absent Cargo-v2 cache remain open after this implementation review.

## E5 hosted result and original-goal assessment v20 — 2026-10-07

Critic accepted the exact v19 implementation and collector before the single
push experiment. [Run37559773345](https://github.com/Awalgawe/noh/actions/runs/37559773345),
commit `c8a8c0e43e96645f495b56d4a02745ad28603aa0`, succeeded in789s. The approved
collector authenticated attempt1, job112594188774 and diagnostic SHA-256
`069ea1542551948d9e9af32b55c4ce59817c1f47feaac92ff343ecc4c6d3bf21`.
Evidence is retained under `.mcp-dev/ci-cost/37559773345/`; the original job log
is under `logs/delivery/github-run-37559773345/`.

| Modified production segment | Actual old policy | Compatible candidate |
| --- | ---: | ---: |
| Restore raw cache |1s, real miss|51s, exact guarded legacy fallback|
| Prepare locked test FFmpeg |111.332s|3.966s|
| Acquire and validate full runtime |413.266s|60.176s|
| Total timed acquisition, including inter-phase checks |524.615s|64.163s|
| Extra key resolution and identity guard charged |0s|9s|
| Cache save charged |0s, conservatively omitted|73s, actual full promotion|
| Compared total (do not add nested acquisition rows) |525.615s|197.163s|

The candidate saves328.4517744s (5m28s),62.49% of this segment after the complete
promotion charge, exceeding both120s and20% criteria. The114 test-runtime files
and1,182 full-runtime files match byte-for-byte. Each arm makes973 checked fetch
calls in the same order (85 during test preparation); baseline downloads888
objects totalling2,518,236,854 bytes and candidate downloads none. All cached
objects still pass the real production SHA-256 check. Seven hosted synthetic
controls pass; no Rust, media suite, release, signature or native acceptance is
claimed by E5. Their unchanged accepted evidence and production gates remain.

The stable cache really exists: ID8602275493, key
`noh-runtime-v2-windows-x64-a0cfdc77e4fba63d2b8c8e5110bac3b5fd3f96cdcd933c07ee8f7368dbc3eb48`,
2,470,773,699 bytes, created inside this job. Seven caches now total5,877,150,320
bytes; no old cache was deleted. E5 consumed789s of hosted job wall time. Across
all seven diagnostic attempts the total is4,506s (75m06s), including failures,
setup and teardown, excluding queueing, local work and model usage. This is not
a billing estimate. The cold2.52GB fetch and added2.47GB retained cache are costs.

### What this establishes at the original CI scope

The same production actions, path, current input locks and acquisition code now
demonstrate a material sequential-path saving with real cache selection and its
costs included. The original B-only path performs these consumers around its QA
work; no concurrent branch masks their sequential cost. Unlike the rejected v17
closure, this proof does not assume that the producer will restore an available
cache under a key it never actually requests. The exact policy is integrated and
the new cache is present. The other expensive stages remain on the path.

For scale only,328.45s is about8.8% of the historical3,740s B-only elapsed budget.
That ratio is not a measured whole-CI speedup, and subtracting it from62m20 would
not produce a verified new release duration. No sum of E1, E2, E3 and E5 is used
to claim one workflow saving. E1 remains the separately proven warm extraction
gain; E3 removes repeated helper compilation; E2 is a modest fixture improvement.
E5 and E1 concern the same acquisition stage under different comparisons;
their sum is not a measured workflow gain. Two-thread media scheduling and
ThinLTO remain rejected.

This is one cold-then-restored comparison; network and OS cache effects are not
controlled across repeated runs, and the diagnostic omits the intervening Rust/
test workload. Identical unchanged producers can subsequently hit either
policy's cache, so E5's5m28s is not a recurring saving on every run. The new key
avoids further invalidation for extraction-only edits, but their frequency is
unknown. Lock changes still invalidate the cache; selected bytes are never
accepted solely because a key matched. Cargo-v2 is still absent. Filling it and
measuring a fully warm production job belong to the next necessary delivery;
this work does not claim that separate benefit.

The v20 paired review verified all32 artifact hashes, found no open issue, accepted
the four changes and explicitly accepted closure of the original objective. M6
is closed: the real sequential-path reduction includes actual cache selection
and all new costs, with preserved guarantees and explicit limits. No indispensable
validation remains for these changes. No extra release is required merely to
obtain a total; the whole-workflow duration and recurring-gain limitations above
remain part of the accepted conclusion. This v21 documentation update records
that verdict without changing implementation, evidence or scope.
