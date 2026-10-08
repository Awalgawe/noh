# Release workflow contract

## Double-click installer and content profiles

The candidate runner now builds A (0.1.0) and B (0.1.1), deriving Minimal and
Standard payloads from each common Complete build. It packs and signs an exact
Setup inventory for every profile; it does not compile three different apps.
Run native acceptance sequentially with `-Profile complete`, `-Profile standard`
and `-Profile minimal`. The same runner covers install, GUI update/restart,
external repair and profile preservation. A rebuilt A is not proof of compatibility
with the previously delivered guardian; retain a separate historical A-to-B check.
For that check, add `-PreviousCandidateDirectory`, `-ExpectedPreviousCommit` and
`-ExpectedPreviousCandidateSha256` to Complete acceptance. The historical
candidate's B becomes A, including its original bootstrap, GUI and guardian;
the report records both candidate identities and the signed guardian digest.

The Complete release keeps schema 2 with no serialized profile field so the
previous reader can consume a bridge release. Minimal and Standard use schema 3.
The profile is signed, persisted in the stable anchor, and checked in selection,
retention, handoff, recovery-tool lookup and the installed manifest. Missing profile
means Complete only in the historical schema; unknown values fail. Profiles do not
replace release channels. Changing an installed profile is currently refused.

After the native results are reviewed, `prepare-setup-draft.ps1` takes the Complete
report as `-AcceptanceReport` and the other two as `-AdditionalAcceptanceReports`.
For a profile candidate it uploads only signed payloads/envelopes and marks wizard
qualification pending. Its `installer-download-sources.json` contains stable asset
API URLs, never credentials. The draft remains private and unpublished.

Build the wizard from those authenticated CI inputs with `build-installer.ps1`:

```powershell
pwsh -NoProfile -File tools/build-installer.ps1 `
  -CandidateDirectory '<verified-candidate>' `
  -ExpectedCandidateSha256 '<independently-verified-catalog-sha256>' `
  -ExpectedCommit '<application-CI-commit>' -PublicKeys '<independent-keys.json>' `
  -CompilerDirectory '<verified-Inno-7.1.0>' -OutputDirectory '<new-output>' `
  -Mode web -DownloadSources '<candidate>/installer-download-sources.json'
```

For an offline executable use `-Mode offline -Profile minimal|standard|complete`
and omit `-DownloadSources`. Use the official Inno Setup 7.1.0 x64 installer,
SHA-256 `0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f`,
with `/PORTABLE=1` for an isolated compiler. The builder pins ISCC/ISPP and leaves
Inno's precompiled-file verification enabled. Embedded files have consumption-time
hash checks. Reports distinguish application and wrapper source identity and retain
the exact wrapper sources/parameters and final executable digest.

The wizard uses the existing NOH controller for exact-version authentication and
Velopack application installation. Network staging uses the existing bounded,
cancellable transport; private authentication comes only from runtime
`NOH_UPDATE_TOKEN`, injected into the trial process by the authorized harness, never
the command line, compiler, logs or executable. A mirror is restricted to the signed
release's GitHub repository and bytes remain bound to the signed size/digest. The
web candidate is not evidence of an anonymous public feed.

Qualify the exact final EXE: actual UI install/launch, download cancellation,
network/authentication failure without success, active-client refusal, offline
repair with the application root missing, and shortcut removal by the single
Velopack uninstaller. Preserve language/settings and the anchor. Only after peer
review of that evidence should the EXEs be added to the same draft and its pending
qualification note replaced. Do not publish the draft.

## Windows Setup candidate workflow

`.github/workflows/windows-setup.yml` builds a private candidate from reviewed
sources on `codex/setup-release`, or by explicit dispatch. It preserves GNU/LLD,
tests matching QA workers with `gui,mcp,updates` and media enabled, and produces
full Setup versions A and B once. `assets/setup-tools.lock.json` pins the compiler,
official Velopack packager and its .NET runtime. Runtime acquisition reuses the
existing locked delivery inputs. No local intermediate build is required.

Configure independent public trust in repository variable `NOH_SETUP_PUBLIC_KEYS`
and the corresponding PKCS#8 Ed25519 key, base64 encoded, in repository secret
`NOH_SETUP_SIGNING_KEY_BASE64`. Key ID is `noh-release`; identity is `NOH`.
Reuse this trust configuration for subsequent versions. Keys are checked against
each other before the two application builds. No private key enters artifacts,
source snapshots or the application. A build without external signing inputs
uses a disposable identity only for local development experiments.

The successful run retains one explicitly allowlisted candidate artifact plus
diagnostics. Check its source commit, private repository, workflow path, success
and GitHub artifact SHA-256 before executing downloaded files. Preserve the
original archive. Extract into a fresh directory, then run **without elevation**:

```powershell
pwsh -NoProfile -File tools/update-setup-clients.ps1 -Stage Accept `
  -CandidateDirectory '<downloaded-candidate>' -ExpectedCommit '<verified-CI-sha>'
```

Acceptance performs no compilation: initial A installation, all three clients'
identity/exclusion checks, signed B import through the real GUI, supervised
replacement and first-frame restart acknowledgement, whole-root removal into a
preserved backup, and exact B repair through the external tool. The signed file
inventory and external user settings are checked afterwards. Its JSON report
binds the accepted candidate manifest and source commit. Existing NOH registration
causes refusal; the test never replaces an unrelated installation.

After independent review agrees on those results, prepare the draft plan:

```powershell
pwsh -NoProfile -File tools/prepare-setup-draft.ps1 `
  -CandidateDirectory '<downloaded-candidate>' -CandidateArchive '<original.zip>' `
  -AcceptanceReport '<acceptance-result.json>' -PublicKeys '<independent-keys.json>' `
  -RunId <successful-run-id> -ExpectedCommit '<verified-CI-sha>'
```

Add `-CreateDraft` to create the private draft and upload the accepted B Setup,
its signed `envelope.json`, and the matching controller as `NOH-Install.exe`.
The script checks authoritative run provenance and archive digest, compares every
extracted byte to that archive, independently authenticates the Setup, and holds
input files during upload. It refuses public repositories and existing release
tags. It records the new draft ID before uploading; after interruption, inspect
that draft rather than creating another. It never publishes or clears public
redistribution/signing requirements. A draft feed is not a live public feed.

For a manual initial install of the downloaded B assets, use an ordinary Windows
PowerShell terminal in their directory (example for version 0.1.0):

```powershell
$base = Join-Path $env:LOCALAPPDATA 'NOH'
New-Item -ItemType Directory -Force $base | Out-Null
.\NOH-Install.exe setup --base $base --package "$PWD/NOH-0.1.0-win-x64-Setup.exe" `
  --envelope "$PWD/envelope.json" --version 0.1.0 --confirm-install --initialize
& "$base/application/current/noh.exe"
```

Later exact-version repair uses the same authenticated controller command without
`--initialize`. In-app update checks use the embedded feed URL; downloads and
installation are separate user actions. The native acceptance imports the signed
CI bytes locally; network selection/download is covered separately by the CI
transport/controller tests. This is not a claim of a live published feed test.

## Legacy nupkg trial workflow

The workflow and offline verifier are implemented; remote execution and public
delivery remain unqualified. Preparing this workflow does not authorize a release.

## Inputs and independent policy

The prepared workflow is `.github/workflows/update-release.yml`; its consumer
script is `tools/publish-update-release.ps1`. Dispatch defaults to verification
only. It currently permits controlled private Windows trials, never production
or public publication. Before an authorized remote run, configure the independently
reviewed `NOH_UPDATE_ARTIFACT_WORKFLOW_ID` and `NOH_UPDATE_PUBLIC_KEYS` repository
variables, and required reviewers with self-review prevented for the
`noh-private-update-trial` environment. An environment name alone is insufficient;
the publication job inspects its protection rules and refuses missing protection.

The approved manually dispatched main-branch producer must upload exactly one
artifact named `noh-update-staged`, containing `envelope.json` and a `packages/`
directory with only the signed full package files. Its repository, workflow ID,
source commit/ref, event and successful conclusion are checked before collection.
No artifact-producing run, private access or remote configuration is claimed here.

The manually dispatched preparation job takes an exact artifact run ID,
expected source commit, version and release tag; the channel comes from the
independent reviewed policy. It checks the source
run's repository, allowed workflow, expected commit/ref and successful conclusion
before collecting named artifacts. Missing, additional or ambiguous artifacts
fail the job. Public verification keys and the allowed target matrix come from
the reviewed release configuration, never from downloaded artifacts.

All production installation targets currently remain disabled in
`Target::installation_qualified`. A controlled Windows trial uses a distinct
package identity and release tag, and requires its actual GUI/recovery evidence.
macOS and Linux artifacts cannot enter production feeds without the native gates
in [UPDATE_NATIVE_GATES.md](UPDATE_NATIVE_GATES.md).

## Exhaustive offline verification

The release verifier authenticates the original signed envelope against the
independent public keys, checks exact identity/version/channel, and requires the
complete expected target/type matrix. It rejects duplicate targets/types or file
names, unlisted packages, extra feed entries, absent packages, wrong repository
or tag asset URLs, and size/hash mismatches. Verification never implicitly
downloads a missing package. An artifact success flag is not qualification.

The existing `update-release verify` checks one retained native stable full
package. `update-release verify-release` now checks the exhaustive offline
contract against a separate reviewed policy file. Five signed-fixture tests pass,
covering complete trial input and rejection of changed/missing/extra packages,
wrong repository/tag/channel/version/key, duplicate or unqualified targets and
production execution. This does not itself freeze upload inputs or qualify a
trial; those gates belong to the publication workflow and native acceptance.

```powershell
update-release.exe verify-release --public-keys <independent-public-keys.json> `
  --policy <reviewed-policy.json> --envelope <original-envelope.json> `
  --packages <packages-only-directory> --expected-version <version> --tag <tag>
```

The policy declares repository (`owner/repo`), package_id, channel,
controlled_windows_trial and an exact targets array of os/arch objects. The
controlled trial is restricted to NohGuiUpdateQualification / Windows x86_64.
Production policies consult the compiled installation qualification gate, which
currently refuses every target. Neither keys nor policy come from the package
artifact directory. Full packages only are permitted by this publication policy;
delta publication would require a separately reviewed exhaustive contract.

## Publication boundary

Preparation has read-only repository permissions. Publication is a separate,
explicitly requested trial job with environment protection and its own write
credential. Client trial credentials are runtime-only, short-lived and restricted
to repository read access; neither credential enters application binaries,
tracked configuration, diagnostic logs or captures. Signing keys remain offline.

Stage the verified bytes without permitting mutation through upload. Refuse
existing release/tag/asset collisions rather than replacing an existing release.
Create a draft, upload every verified package and original envelope, then verify
the actual remote asset bytes before exposing the complete release. Never expose
a partial feed. Failure leaves the draft unexposed and records the failed step.
Drafts are preparation artifacts; the eventual authorized client trial requires
a discoverable private published release/prerelease.

## Required qualification

The independent `.github/update-qualification.json` registry is intentionally
empty. Publication requires one reviewed version/source-commit record with
`gui_and_external_repair_passed=true` and exact envelope/package SHA256 values.
Local packages with fixture URLs are not authorized GitHub assets; repository/tag
URLs and the remote source identity must also match. Review changes to keys,
policy and qualification records independently of the candidate artifacts.

Local verification must cover authentic complete input plus wrong keys,
extra/missing targets or files, URL mismatches and changed package bytes.
Workflow syntax checks are separate from execution on GitHub. Remote acceptance
requires actual controlled releases, feed retrieval and binary redirects,
authenticated installation, restart and data preservation. Local qualification
does not prove private GitHub access or the later anonymous public transition.
