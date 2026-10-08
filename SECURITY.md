# Security policy

NOH is in development. There is no supported public binary release yet; fixes
are made on the default branch. Development builds do not carry a security
support or response-time guarantee.

## Report a vulnerability privately

In this repository's **Security > Advisories** tab, select **Report a
vulnerability**. The maintainer must enable GitHub private vulnerability
reporting before opening the repository to outside contributors. If that option
is unavailable, open an issue requesting a private contact channel without
including exploit details, secrets or personal data.

Include the affected commit/version, operating system, expected and observed
behavior, impact, and a minimal synthetic reproduction. Do not attach private
media, access tokens, signing keys or unredacted diagnostics. Keep disclosure
private while the maintainer investigates.

## Security boundaries

Media parsing uses native tools such as FFmpeg and libmpv; local processing is
not a sandbox for hostile inputs. Use trusted executables and dependencies.
Cancellation, exclusive output creation, worker isolation and updater signature
verification are security-relevant behavior. Please report violations of these
boundaries through the private channel above.

Publishing this source does not qualify installers, native runtime bundles,
automatic updates or signing infrastructure for public distribution. Their
independent release requirements remain in [the release guide](docs/RELEASING.md).
