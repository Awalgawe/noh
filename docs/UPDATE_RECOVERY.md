# Windows update qualification and repair

This is a developer procedure for disposable installations. It does not update
ordinary NOH builds or publish releases. The controlled Windows GUI A-to-B cycle
and external manual repair have local qualification; public installation,
automatic rollback and power-loss durability remain unqualified.

## Official Setup experiment (C1/C2, 2026-10-05)

`pwsh -NoProfile -File tools/update-setup-probe.ps1` explores whether the pinned
Velopack Setup can replace NOH's custom repair implementation. Run sequentially,
without an elevated Windows token. It builds only QA qualification tools and a
tiny Rust client, uses cached Velopack 1.2.161, and downloads nothing. The complete
packager archive is pinned by SHA-256; its extracted tool/vendor files are checked
against that archive. Each run creates a unique ignored case under
`.mcp-dev/update-setup-probe`, an ordinary installed fixture, and one unique HKCU
Uninstall key. Cleanup removes only that key after checking its InstallLocation.
The installed files, fault copies, process records and terminal result are retained.
A sandbox that disallows this HKCU write cannot complete ordinary installation;
access outside that sandbox is distinct from Windows administrator elevation.

The following matrix defines outcomes independently of the candidate mechanism.
Existing updater tests remain baseline evidence for their original code paths;
they do not qualify this new path automatically.

| Required outcome and actor | Existing baseline | Accepted synthetic evidence / remaining boundary |
| --- | --- | --- |
| Signed identity, target, version, channel and exact bytes; origin/cache writer without the signing key | `VerifiedRelease`, streamed `verify_package`, negative release tests | Explicit local `noh-setup-probe-v1` Ed25519 contract and digest checks; invalid signature and changed Setup rejected before launch; A/B identities verified in C2s. Production metadata still accepts only `.nupkg`; this does not qualify remote eligibility, downgrades or freshness. |
| Protect verified input through consumption; same-user writer can attempt Setup overwrite and package-parent rename, but cannot replace the trusted probe or its keys | `ProtectedFile`, copied handles, Windows supervisor tests | Write/parent-rename attempts before launch and during the install hook, with cwd outside the package directory; job membership, image/token snapshot and natural drain. C2a and C2s exercise guardian loss at the hook and after root rename. No additional substitution capability is inferred beyond these attempts and unchanged primitive tests. |
| Obtain consent and exclude work; clients cooperate with the shared activity protocol | GUI/CLI/MCP runtime leases and guarded handoff | Explicit fixture consent; active client veto before Setup, competing client denied during Setup, client admitted afterwards. A single synthetic client represents the shared protocol; actual GUI/CLI/MCP integration remains future work. |
| Preserve projects and offline model availability | Existing synthetic-media acceptance and delivered bundle checks | External user-data sentinel unchanged and a bundled synthetic model restored byte-for-byte. No model storage redesign or full-size performance result. |
| Recover when the installed application is broken | Delivered external manual repair, qualified only in the earlier controlled path | Retained authenticated Setup repairs missing `current` (C1), helper/stub loss and guardian loss (C2a). C2s separately proves full-Setup A-to-B replacement/restart and post-rename repair. Normal helper apply is stopped, not passed. |
| Control privileges; ordinary non-admin owner | Existing guard privilege checks | Runner refuses elevation; actual Setup/job consumer tokens sampled at the paused hook. C2s observes root-rename refusal under a specific changed parent ACL, followed by exact ACL restoration and repair. Other ACL failures and packages with installable prerequisites remain outside qualification. |
| Fail or recover predictably under process/I/O interruption | Primitive guardian-death tests; no general automatic rollback guarantee | C2a establishes consumer termination after guardian loss; C2s interrupts after root rename, before new application/hook, then repairs by rerunning Setup. Backup A persists. Automatic rollback, general orphan cleanup and power-loss durability remain unqualified. |

The threat model does not grant arbitrary control of the verifier, keys and all
application executables. Authenticity is not proof that a server disclosed the
newest release. The marker/lease directory is coordination, not an access-control
boundary; its identity and lifecycle would need production integration.

Responsibility comparison for this candidate:

- **Delegate:** package extraction, installed-file publication, replacement of a
  damaged installation and engine housekeeping to official Setup. The probe does
  none of those operations; moving `current` is explicit fault injection only.
- **Retain:** trusted metadata and streaming hash verification, consent, client
  exclusion, protected supervised launch, result checks, retained recovery inputs.
- **Add/adapt:** a stable coordination anchor outside the replaceable root, a
  separately designed Setup authentication/publication contract, and integration
  of that anchor into each real client. The experimental envelope is not a
  production format or an Authenticode signature.
- **Not yet removed:** all existing production/prototype repair code remains.
  This experiment measures feasibility, not migration savings or release readiness.

`result.json`, per-attempt JSON, Setup logs, binary hashes, source inventory and
packager inventory identify each run. The probe records its embedded build identity.
Failed attempts remain evidence: the first rejected `.exe` using the production
`.nupkg` contract; the second reached the install hook but failed at the sandboxed
HKCU write. Neither is a repair pass. C1 review requires a complete corrected run
before C2 begins. No production installation gate is changed by this experiment.

Corrected C1 run: `case-6647c7c113734fd698f347480b883895/result.json`, **passed**
in 57.05 seconds including compilation. Initial installation and missing-current
repair both returned zero, drained their jobs naturally, restored fixture/model
bytes and allowed a subsequent client to start. Repair itself took 0.60 seconds.
The hook snapshot identified Setup, the fixture and Windows `conhost.exe`, all
without elevation. Invalid signature, modified bytes, absent consent and an
active client were refused before Setup launch. Prelaunch and hook-time input
mutation attempts were denied; the user-data sentinel was unchanged and the
unique HKCU key was removed. These are synthetic C1 results, not full-NOH or
normal-update qualification. Independent review compared the source/binary hashes,
installed bytes, negative reports, actual repair log and HKCU cleanup, and both
participants explicitly approved C1 and progression to C2 on 2026-10-05. The seven
inventoried C1 sources were retained in that case's `sources` directory after
verifying they still matched the original inventory.

### C2a: missing engine and guardian loss

Use the same command with `-Stage C2a`. This creates a fresh fixture, establishes
the C1 baseline, moves its helper and execution stub into retained fault evidence,
and requires Setup to restore both original hashes. A second repair pauses at the
actual install hook, after extraction. The runner retains OS process handles for
the observed job members before killing **only** its own guardian. The supervisor's
job closes and terminates those consumers. The runner waits on the retained handles,
records available exit codes, checks released protections/client admission, and
then reruns the retained Setup to verify recovery. This is not the separate
post-root-rename interruption scenario and does not demonstrate automatic rollback.

Corrected run `case-002859d25ba24ad7a42a167a2644f4a4/result.json` passed in 37.39 s
including compilation. The helper/stub hashes were restored; all three observed
consumers signalled termination after guardian loss; Setup reexecution restored
and verified the payload/model; user data remained unchanged; HKCU cleanup passed.
Consumer exit codes on forced job closure were zero: they establish process
termination here, **not successful installation**. Only the later normally
completed repair is an installation pass. The killed guardian's own JSON remains
empty; its atomic hook checkpoint and the owning runner's terminal result are the
evidence for that deliberate fault.

The earlier `case-9b51cf5f3a5d40a1866da5283e49a7cf` is not accepted termination
evidence: its Process objects had not retained OS handles before the fault, and
consumer exit codes were unavailable. Peer review required explicit SafeHandle
retention before identity checks and fault injection, which the corrected run uses.
Independent review of the corrected source snapshots, hashes, reports and cleanup
explicitly approved C2a; both participants agreed to proceed.

The normal helper variant has a separate unresolved privilege path:
[`apply_windows_impl.rs`](https://github.com/velopack/velopack/blob/92d6a1c91716729d449034df5c50307dcce39493/src/bins/src/commands/apply_windows_impl.rs)
relaunches with `runas` if its root is unwritable. The pinned public CLI has no
identified no-elevation switch. An earlier NOH write preflight does not prevent
later permission changes. This source-level finding stops qualification of that
variant under the present contract; no UAC experiment was run and the existing
prototype is not a qualified fallback. The next distinct C2s candidate uses the
full authenticated Setup for both version replacement and repair, with no runtime
dependencies. Its permission refusal, A-to-B restart and post-rename recovery
require their own evidence. Models remain bundled; project data stays external.

### C2s permissions: full Setup candidate

`-Stage C2s-permissions` pauses the authenticated/protected supervisor before
launching Setup. A non-inherited Deny ACE for the current user's SID temporarily
removes CreateDirectories and DeleteSubdirectoriesAndFiles on the **disposable
case parent**. This targets root replacement; denying writes only inside the old
installation would not necessarily prevent its replacement. Original and changed
SDDL are retained, and the original ACL is restored in `finally`.

`case-fa87e43a0a804f30b11f449b4814223e/result.json` passed the expected-refusal
scenario in 42.43 s including compilation. Setup started, its root-rename attempts
returned access denied, and it exited 1 before reaching the install hook or the
post-rename preparation phase. The three sampled installed-file hashes stayed
unchanged and its job was empty at exit. After ACL restoration, another official
Setup repair completed normally. `acl-restoration-check.json` additionally records
exact equality of restored and original SDDL. HKCU cleanup passed.

This is an observed rename refusal with unchanged sampled application bytes,
not a completed installation under those denied permissions. The pinned Setup
path and this fixture's empty runtime dependencies support the bounded privilege
analysis; no claim covers packages that install prerequisite runtimes or every
possible ACL failure. The normal helper variant remains unqualified.

### C2s A-to-B replacement and restart

`-Stage C2s-version` first establishes the permission refusal/recovery, then
packages separately compiled fixture versions 1.0.0 and 1.0.1 with the same
identity. Setup B replaces installed A. The new executable acquires the shared
lease and writes its compile-time version and actual PID; the supervisor checks
both, in addition to the installed payload/model hashes and Setup/job exits.
The runner checks the Windows installation record moves from A to B. This is a
full-Setup replacement followed by an explicit supervised client launch, not
`Update.exe apply` or an installer-initiated restart.

`case-9a6f58ce615e4f39907e92578edb6160/result.json` passed in 46.68 s including
build. Setup B returned zero, its job drained, and the launched client reported
1.0.1 with the expected PID. The bundled model and external data sentinel remained
intact. The strengthened permission cleanup used a retained Setup process handle
and checked exact ACL restoration. No interrupted-upgrade result is implied.

### C2s interruption after the root rename

`-Stage C2s-interruption` establishes the previous scenarios, resets the disposable
installation to A using its authenticated Setup, then begins a second A-to-B
replacement. A small C# observer, compiled from the inventoried source snapshot,
uses a real filesystem rename event. It checks the saved root and old fixture,
retains the identified Setup's OS handle, observes whether a new fixture exists,
and kills only the guardian. After both processes terminate, the runner requires
the saved A/model hashes, no new fixture and no install-hook marker. A missed
window is inconclusive. Neither the observer nor NOH moves/publishes engine files.

`case-aa45ce439f664bb4a44d569373145d30/result.json` passed in 49.04 s including
build. `post-rename-observation.json` records the saved root/old fixture present,
new root already present, but no new application fixture at observation or after
termination; the hook was not reached. The official log had reached extraction
of Update.exe. Thus this is **after root rename, before the new application
fixture/hook**, not before every new engine file. The old A and model hashes
remained intact. Rerunning authenticated Setup B completed and launched a real
1.0.1 client with a matching PID; the original saved A directory remained intact.
Its retained directory is fault evidence, not a claim of automatic rollback or
automatic cleanup of abandoned backups. ACL and HKCU cleanup passed.
Independent review verified the eight source snapshots, binary/installed/saved
hashes, process evidence, exact ACL restoration and HKCU removal. Both participants
explicitly accepted this final slice and selected the full-Setup Windows migration
direction described in [UPDATES.md](UPDATES.md).

The production comparison remains bounded: these runs support replacing custom
Windows extraction/publication/repair with a full Setup candidate. They do not
qualify full-NOH installed clients, remote feeds, production trust, prerequisites,
large-package performance, recovery quotas, Authenticode, other operating systems
or power-loss durability. Portable ZIP delivery keeps its separate release gates.

The sections below retain the legacy controlled portable prototype procedure for
regression work; they do not qualify the new Setup path or supersede that decision.

## Prerequisites

Use PowerShell 7 on Windows x64, native GNU Rust 1.95+, cached Cargo dependencies,
the qualified portable runtime, LLVM/MinGW, .NET 8 and the extracted official
Velopack 1.2.161 package. No old test key, media fixture or qualification case is
required. The runner downloads nothing.

```powershell
pwsh -NoProfile -File tools/update-qualification.ps1 -Stage Preflight
pwsh -NoProfile -File tools/update-qualification.ps1 -Stage All
```

Preflight is read-only. It checks tool/runtime paths, the independently pinned
helper hash, speech/preview hashes, active owners, version ordering and free
space. Missing offline Cargo dependencies fail at compilation; preflight does
not claim to have resolved them.

Default inputs use `dist/noh` plus cached LLVM/MinGW, `dotnet8` and `vpk` under
`.mcp-dev/update-feasibility`. Override them with `-RuntimeDirectory`,
`-CompilerDirectory` (containing `clang.exe` and `llvm-ar.exe`), `-DotnetPath` and
`-PackagerDirectory` (the extracted NuGet root, including `vpk.nuspec` and vendor).
Supply overrides to both Preflight and Prepare/All.

## Stages and evidence

All creates a unique case under `.mcp-dev/update-feasibility`. An explicit
`-Case` must name a new child. Versions default to 0.6.1 and 0.6.2; choose another
increasing pair with `-VersionA` and `-VersionB` during preparation.

1. **Prepare:** freeze source inventories, record revision/working state, create
   A/B versions, compile the QA verifier, generate a protected local signing key
   and tiny synthetic media. Private keys stay in the ignored case.
2. **Build:** build release A then B with the optimized helper and one shared
   target directory. The ordinary portable remains an input, not a destination.
3. **Package:** produce full official packages without deltas, sign and retain
   A/B, then record the completed stage and input hashes.
4. **Accept:** run the isolated real GUI update with automated consent, observe
   the actual guardian exit, verify B and its project, then execute the delivered
   external repair with invalid-input and simulated absent-current checks.

Stages can run individually using the same `-Case`. Build/Package/Accept consume
the saved plan and require unchanged frozen sources and repository harness.
They never rebuild earlier stages automatically. A failed build/package attempt
or changed harness requires a new case; preserve its diagnostic evidence.
An unchanged complete pair can run Accept again in a fresh acceptance directory
once earlier owners have terminated. Stage scripts are internal entry points.

Each attempt records passed/failed/skipped stages, duration and exact available
child exits in `attempt-*-result.json`. Process records and stdout/stderr logs
retain partial output on timeout. Failure stops dependent work. Preflight
failures before case creation leave a terminal report in the parent directory;
invalid arguments and lock refusals occur before an attempt starts.

Prepare, Build, Package and Accept have outer limits of 30, 120, 30 and 20 minutes.
The first three stop their owned process trees on timeout. Acceptance preserves
native actors for inspection; the recorded live owner and remaining qualification
executables block continuation. Reserve the checkout/target while this runs.
The lock does not prevent an unrelated developer from launching Cargo later.

Await the existing process and inspect its terminal result. A log line or
completion marker alone is not a successful native qualification. Automated
consent is not a human-click test; a local import is not GitHub delivery.

Cheap runner regressions, without application builds or installation:

```powershell
pwsh -NoProfile -File tools/test-update-qualification.ps1
```

## External manual repair contract

Before updating, keep the signed previous full package and its original envelope,
trusted public-key configuration, data baseline and the actual delivered repair
executable outside the replaceable installation. The runtime archive selector
independently authenticates identity, target, channel, exact version and bytes.
It retains incoming full packages and does not prune earlier versions.

The standalone `noh-update-repair` uses compiled qualification trust/root and
refuses ordinary installations. It restores an explicitly selected retained
signed full version with no network discovery, elevation or automatic rollback:

```powershell
& $externalRepair --root $installation --retained-archive $retainedA `
  --version $versionA --controlled-manual-restore
```

The caller must observe termination of its prior guardian/helper handles and
prevent new updater actors throughout repair. Process-name scans supplement
that ownership contract; they do not establish atomic exclusion on their own.
The exclusive runtime lease, protected inputs, confined extraction and final
file checks are required. Interrupted trees and engine files are preserved.

A successful repair exit proves file restoration, not runtime readiness.
Qualification must start the restored CLI, reopen the synthetic project in the
real GUI and verify project/media/preferences after closure. A simulated absent
`current` does not establish crash or power-loss recovery. Velopack's temporary
backup is not a guaranteed rollback mechanism.

[UPDATES.md](UPDATES.md) describes ownership boundaries;
[UPDATE_RELEASE.md](UPDATE_RELEASE.md) describes the separate publication gate.
