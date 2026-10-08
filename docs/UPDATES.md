# Update architecture

The optional `updates` feature implements authenticated release metadata,
download/verification, desktop controls and a controlled Windows guardian/repair
prototype. **Default builds have no installation configuration.** Windows Setup
builds explicitly embed `NOH_UPDATE_FORMAT=windows-setup`, independent public
keys, package identity and feed URL. Their managed installation uses a stable
external anchor; it is no longer tied to the build machine's qualification path.
Hosted Setup delivery and the full real-client path are awaiting validation.
Public distribution remains unqualified.

## Windows migration direction — 2026-10-05

Both participants agreed to use an authenticated **complete Velopack Setup** as
the next Windows implementation direction for both version replacement and
external repair. It preserves a bundled model, coordinates clients through a
stable anchor outside the replaced root, and keeps protected input/process
ownership. Cases cover absent `current`, missing helper/stub, guardian loss,
permission-change refusal, real A-to-B client restart, and recovery after an
observed root rename. See [UPDATE_RECOVERY.md](UPDATE_RECOVERY.md) for the exact
outcomes, rejected preliminary evidence and limits. No extraction or installed-file
publication was implemented by NOH in this candidate.

This selects the direction after native synthetic qualification and independent
review of each slice; it does not complete migration or qualify ordinary NOH builds.
No further Inno Setup/WinSparkle comparison is justified in this completed spike
without a new necessary criterion failure. Reassess if real integration requires
an engine fork, custom extraction/publication or a weakened outcome requirement.

The normal `Update.exe apply` variant is not selected for further qualification
under the current nonprivileged contract: its pinned implementation can invoke
`runas` after a permission change, with no identified public switch to disable
that path. This source-level limitation also concerns the current prototype;
its earlier native successes do not qualify the privileged fallback. Full Setup
with no runtime prerequisites is a distinct candidate, not a successful helper
update or a guarantee about every Velopack package.

A migration must retain NOH's authentication, consent, GUI/CLI/MCP coordination,
protected supervised launch and final readiness checks while delegating extraction,
replacement and reinstallation to Setup. It must introduce a reviewed production
Setup artifact contract (the experiment's envelope is fixture-only), a durable
external controller/coordination identity, and real-client integration. Custom
repair extraction and engine-private member mappings can be removed only after
equivalent installed-NOH acceptance. Installer signing, remote delivery and final
bundle qualification remain separate gates. Complete offline ZIP distribution
with manual replacement remains independent; this experiment does not enable
automatic updates in a ZIP or change model storage.
The retained Setup, trusted keys and repair entry point must remain reachable
when the installed root is broken. Recovery also needs a policy for abandoned
backups and disk limits. No total maintenance saving or full-package timing was
measured by the synthetic experiment.

## Shared contract

Release metadata binds package identity, semantic version, channel, platform,
architecture and exact artifact hashes/sizes. The client verifies the signed
envelope with independently compiled public keys, selects only eligible targets
and bounds downloads and retries. Private signing keys do not enter application
builds, feeds or source control. Media stays usable while the update worker checks
and downloads; installation requires a separate safe-close decision.

A successful download is not permission to replace the application. Inputs are
authenticated again at the consumption boundary. Retained previous full packages
support the controlled manual repair path. Deltas are disabled; neither cache
presence nor a completion marker supplies installation authority.

## Windows prototype

The pinned engine is Velopack 1.2.161. NOH independently owns publisher trust,
protected source/ancestor handles, runtime leases, guardian lifetime, consent,
installed-file checks and verified restart. Signed Squirrel and execution-stub
members have explicit destination mappings; unsupported layouts are refused.
Ordinary builds have no compiled qualification root and cannot invoke this
installation/repair path.

The GUI blocks project/media commands after safe-close consent and exits only
after persistent ownership is committed. A guardian completion marker is useful
only with actual process termination and the flushed result. A launched PID does
not establish a ready, usable GUI. Session restoration, automatic rollback and
power-loss durability are not implemented guarantees.

The repository [qualification runner](UPDATE_RECOVERY.md) builds fresh A/B
packages, imports a local signed full update, checks the real restarted GUI and
then executes the external repair binary with retained inputs. Its automated
consent does not qualify human interaction or GitHub delivery.

## Maintenance ownership

| Component | Responsibility |
| --- | --- |
| Velopack, pinned | Official package format, execution stub and replacement helper |
| NOH shared modules | Signed metadata, identity/version/target/channel policy, bounded network work and retained full packages |
| NOH Windows adapter | Protected inputs, process/lease ownership, consent, final file verification, restart and manual repair |
| Native macOS/Linux adapters | Not implemented or qualified for authenticated installation |

An engine upgrade can invalidate helper identity, member mapping, restart and
repair assumptions. Review and requalify these together. Keep the Windows
prototype bounded; do not expand it into a general cross-platform installer.
Before adding another native adapter, compare the maintained engine's guarantees
and the permanent installation/recovery code NOH would still own. Reassess the
choice if it requires an engine fork or a second general installer.

## Remaining boundaries

[UPDATE_NATIVE_GATES.md](UPDATE_NATIVE_GATES.md) records native requirements.
[UPDATE_RELEASE.md](UPDATE_RELEASE.md) describes the guarded publication workflow.
Its qualification registry is empty, and preparing or publishing source does
not authorize an update release. Production key lifecycle, retention quotas,
recovery qualification and remote delivery remain separate release work.

## Setup integration contract (in progress, 2026-10-05)

Schema 2 explicitly authenticates `windows-setup` artifacts for Windows x64.
Schema 1 remains the legacy `.nupkg` contract with its original size limits.
`select_setup` is a separate opt-in; existing `.nupkg` consumers cannot receive
an executable through `select`. No publication policy or qualification gate is
opened by accepting this metadata.

The Setup contract signs the pinned engine identity, an empty prerequisite list,
build fingerprint, installed byte count and complete application payload inventory
(relative to `current/`). The signer attests that the producer used the official
engine without installable runtime dependencies. This is not executable inspection
or a sandbox guarantee. The engine owns extraction, publication and `sq.version`;
NOH will verify the payload after installation without extracting its own copy.
The schema 2 envelope is bounded to 1 MiB and its decoded payload to 768 KiB.

The stable layout is `<base>/application/current` with coordination in
`<base>/.noh-update/identity.json`. Its identity binds package and channel, never
supplies trusted keys. Clients pin that file and its ancestors before sharing the
external activity lock. Moving or removing `application` leaves the lock domain
intact; saved/obsolete application images are refused. This is cooperative
coordination, not protection against arbitrary replacement of the controller.
The compiled key set remains the independent trust source. External controller
provisioning, real Setup launch, retention and readiness are subsequent slices.

The Setup adapter now retains the authenticated executable before invoking it,
keeps the external anchor and exclusive activity lease through the supervised
process tree, requires successful exit and natural drain, and verifies the signed
application inventory before provisioning external recovery tools. It uses the
public `--silent --installto --log` Setup interface. It neither extracts nor
renames application files. Failure/cancellation terminates owned consumers before
exclusion is released; an abrupt guardian death retains consumer protections.

`noh-update-repair setup` accepts exact-version consent and optional first-time
initialization. This entry point requires explicit Setup build configuration and
compiled independent public keys. If a legacy qualification root is compiled,
its path restriction still applies. Recovery tools are copied from verified installed payload members
into immutable external generations; their future selection reauthenticates the
signed inventory and hashes the chosen executable. Old generations, retained
Setups and engine-saved roots are preserved. Free-space checks are conservative
estimates, not reservations or an automatic pruning policy.

The producer's `update-release describe-setup` inventories a complete update-enabled
Windows payload. `sign`, `verify` and local retention understand schema 2; the
publication policy still rejects it until its own qualification. The local
`tools/update-setup-integration.ps1` runner checks the pinned packager closure and
packs without prerequisites. Its successful synthetic native adapter assertions
are recorded in STATUS; real client startup and GUI readiness remain separate.

An installed Setup GUI also accepts the explicit offline consent command
`noh.exe --install-retained-update <archive>`. This imports a newer independently
signed retained Setup from inside the installation base and runs the same
controller, safe-close, guardian and restart path as the settings controls.
Ordinary builds never infer consent from qualification environment variables.
The first GUI-frame acknowledgement uses a local named pipe, the operating
system's writer PID and the signed version/build fingerprint. It proves GUI
initialization, not subsequent media operation health. Internal update arguments
are excluded from opening media files.

The reviewed retention API now requires an explicit format: legacy free functions
remain `.nupkg` only, while `retention::SETUP` opts into executable archives.
`update-release retain` and `verify` require `--format windows-setup` for schema 2.
Catalogs keep the formats separate even for one package/version. Manifest size is
bounded to the same 128 KiB by signed inventory validation, the producer,
post-installation verification and shared client startup. Member lookup follows
Windows ASCII case-insensitive semantics consistently. Waiting for installed
processes now observes cancellation before any Setup launch.
