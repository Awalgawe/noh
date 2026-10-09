# Code signing for GitHub releases

## Current state

The 0.1.1 Windows application and installers are unsigned. A real browser download
of the Minimal installer displayed SmartScreen's unrecognized-application warning.
The downloaded Web and Minimal executables match the CI-produced release hashes.
Earlier Defender/Attachment Services checks did not establish SmartScreen reputation.
Do not replace these published files with signed variants under the same names.

The repository currently has no configured Windows or Apple signing credentials.
The existing [Mac helper](MACOS_SIGNING.md) implements Developer ID signing,
notarization and stapling, but has only simulated validation. No real signature
or Apple acceptance has been obtained. Windows provider integration is pending
provider selection and enrollment; the verification helper below does not sign.

## Provider decision, checked 2026-10-09

| Route | Fit for NOH | Prerequisite |
| --- | --- | --- |
| SignPath Foundation | Free service for accepted open-source projects, with GitHub-hosted build provenance. Eligibility of NOH's current Complete bundle is unresolved because it embeds proprietary NVIDIA CUDA libraries. Do not assume acceptance or call those SDK libraries operating-system components. | Foundation review, an approved artifact scope, named project roles, MFA and manual release-signing approval. |
| Azure Artifact Signing | Suitable for hosted CI and EU organizations. Public-trust individual enrollment is currently limited to the United States and Canada. | Confirm the owner's country and individual/organization status before choosing this route; paid Azure account and identity validation. |
| SSL.com eSigner | A documented cloud/CI option supporting individual and organization certificates. Potential fallback if the free service is unavailable. | Issuer eligibility, certificate and cloud-service costs, owner approval and identity verification. No subscription has been purchased. |
| Certum SimplySign | Offers an open-source individual certificate stored in the cloud. Cloud key storage alone does not prove an unattended GitHub runner integration. | Verify the supported CI authentication method and total cost before selecting it. No desktop-dependent workaround is approved. |
| Apple Developer ID | Appropriate for a Mac app distributed on GitHub; no App Store submission is required. | Apple Developer Program membership, Developer ID Application certificate/private key, team ID and notarization credentials. Apple's listed fee is USD 99/year, with local pricing shown at enrollment. |

Preserve CUDA functionality while evaluating providers. Changing the speech
backend or removing CUDA to obtain a free certificate is a separate product
decision, not an implicit consequence of this investigation. The current Complete
bundle's NVIDIA files are documented in [redistribution](WINDOWS_REDISTRIBUTION.md).

Sources: [Foundation conditions](https://signpath.org/terms),
[SignPath GitHub integration](https://docs.signpath.io/trusted-build-systems/github),
[Azure eligibility](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart),
[SSL.com cloud signing](https://www.ssl.com/products/software-integrity/signing-service/),
[SSL.com pricing](https://www.ssl.com/guide/esigner-pricing-for-code-signing/),
[Certum open-source offer](https://shop.certum.eu/open-source-code-signing-on-simplysign.html),
[Apple enrollment](https://developer.apple.com/programs/enroll/).

## Required Windows signing order

Signing changes file hashes. A final signed release needs the following order on
GitHub-hosted runners, using one application compilation and a separate signing
step. Never expose signing credentials to pull requests or arbitrary branches.

1. Build the NOH GUI, CLI and MCP workers from the reviewed source. Sign those
   NOH executables, verify their exact publisher and timestamp, then regenerate
   the application manifest and seal the portable. Preserve upstream signatures
   and do not sign third-party libraries with NOH's identity.
2. Configure Inno's signed-uninstaller facility before compiling the maintenance
   helper or any of the three profile installers. Each can install or replace
   `unins000.exe`; sign that inner uninstaller before its enclosing executable.
   Then build and sign the maintenance helper before embedding it in the profiles.
   Generate profile manifests from the final signed application/helper bytes.
3. Compile Minimal/Standard/Complete with signed uninstallers, sign each outer
   setup executable and finalize its `INSTALLER.json` only afterwards.
4. Compile the Web selector against the signed profile installer hashes. Sign the
   Web executable, then produce final provenance and checksums.
5. Verify the exact final signatures and run installation/repair/removal tests on
   those files. Verify the installed `unins000.exe` after initial installation,
   repair and maintenance so a component addition cannot restore an unsigned
   uninstaller. Test the actual browser-download launch path separately; a valid
   signature does not guarantee immediate SmartScreen reputation.

This ordering is an implementation requirement, not a claim that the existing
Windows workflow signs anything. Complete the selected provider adapter and
manifest integration before enabling a signed release. Publish a new release
version after qualification; keep 0.1.1 assets immutable.

`tools/verify-windows-signatures.ps1` provides the provider-independent Windows
verification step. Supply explicit file paths, the expected signing certificate's
SHA-256 fingerprint and a new evidence path. It rejects unsigned/untrusted files,
catalog-only signatures, a different publisher certificate, missing timestamps
and changes during verification. It neither executes nor modifies the inputs,
installs a certificate, changes trust settings, nor signs a file. Certificate
rotation requires an explicitly reviewed new fingerprint.

```powershell
./tools/verify-windows-signatures.ps1 -File $signedNohFiles `
  -CertificateSha256 $expectedCertificateFingerprint `
  -Output "$env:RUNNER_TEMP/noh-signatures.json"
```

## Mac activation

Follow [MACOS_SIGNING.md](MACOS_SIGNING.md) to configure the existing protected
environment. The owner supplies credentials directly to GitHub environment
secrets, not chat messages or committed files. Account enrollment, identity
verification and payment cannot be fabricated by CI. Once configured, validate
identity import, cleanup, nested signatures, notarization, stapling and the exact
downloaded archive on a native runner. Existing package/source qualification
requirements still apply independently of the certificate.

The repository inspection found only `noh-candidate-build`; `noh-macos-signing`
and Windows signing configuration are not yet provisioned. No enrollment,
provider contact, purchase or secret-setting action has been performed.

## Prepared Foundation eligibility request

The following text is a draft for the maintainer to approve and send. It is not a
declaration that NOH qualifies and must not be submitted with the CUDA detail omitted.

> NOH is a GPL-3.0-only desktop media application maintained at
> https://github.com/Awalgawe/noh, with Windows releases built on GitHub-hosted
> Actions runners. We want to sign only our own GUI/CLI/MCP executables and Inno
> installers, keeping upstream libraries' existing identities. The Complete
> installer and portable currently include unmodified proprietary NVIDIA CUDA
> runtime libraries used by the separate whisper.cpp process. Minimal and
> Standard do not embed those libraries, but the application and helper can
> download the Complete components from our release. Could this project qualify
> for Foundation signing under an explicitly approved scope, or does this
> optional component make it ineligible? We will preserve this functionality
> unless an explicit product decision changes it. Please identify any other
> prerequisites for this recently public project before we prepare enrollment.

## What signing does and does not establish

An OS-recognized signature identifies a publisher and protects the signed bytes.
The [Microsoft reputation model](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)
also evaluates download history; neither OV nor EV guarantees an immediate
warning-free first download. Apple's notarization is an additional check and
ticket for distribution outside its Store. Neither process replaces NOH's
functional tests or source/license review.
