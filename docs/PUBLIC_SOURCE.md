# Publishing the source repository

The prepared source tree is intended for a **new initial commit without parents**.
The owner may delete and recreate the GitHub repository under the same name.
Keep a verified local Git bundle before that deletion. A Git bundle does not
include release assets, Actions artifacts or logs; retain any needed files
separately before deleting the remote. Never publish the backup bundle or import
its old branches into the recreated repository.
The source snapshot includes GPL-3.0-only code, third-party source notices,
embedded fonts and a synthetic screenshot. It contains no native runtimes,
installers, speech models or personal media.

## Publication sequence

1. Prepare and review the exact source tree and a parentless initial commit
   locally. Verify that its reachable history contains one commit and that author
   and committer use the intended public identity. Preserve LICENSE and all
   third-party notices. A new `.gitignore` alone does not remove old tracked files
   or historical commits.
2. Wait for the owner to recreate an empty repository and give the go-ahead.
   Before pushing, verify the destination owner/name, its new repository ID and
   absence of branches/tags. If it is not empty, stop and inspect the unexpected
   content rather than overwriting it. Push only the reviewed parentless commit
   as `main`, without force, tags, mirror or other development branches. Do not
   copy old secrets, environments, caches or releases. The retained development
   checkout has old history; never use a default or mirror push from it.
3. Confirm public visibility under the owner's publication instruction before
   running hosted CI; private execution still uses the account allowance. Enable
   private vulnerability reporting, secret scanning/push protection where
   available, and dependency alerts. Confirm these settings through GitHub;
   adding SECURITY.md alone does not enable private reporting.
4. Keep workflow permissions read-only and Actions pull-request approval disabled.
   Require approval for all outside collaborators' workflow runs. Use only
   standard GitHub-hosted runners, without self-hosted machines or secrets in PR
   workflows. Do not grant write tokens to fork pull requests.
5. Run PR validation on the imported `main`. Its initial push has no comparison
   base and therefore checks every platform. Confirm all selected tests, captures,
   the RustSec audit and the final `PR checks` result. Existing private CI results
   are useful evidence but do not prove this recreated repository's CI setup works.
6. Require `PR checks` on `main`, require branches to be up to date, block force
   pushes/deletions and verify the rule is enforced. Use required review for
   outside contributions, particularly workflow and updater changes. Only then
   accept normal contributor pull requests.

Standard GitHub-hosted runner execution is free for public repositories. This
is distinct from Actions artifact/cache storage and from larger paid runners.
Do not turn on paid capacity as part of source publication. Optional PR/native
and public audit uploads cannot override a failed test or audit, and do not make
validation depend on exhausted artifact storage.

## Delivery remains separate

Historical CI reports retain old evidence links; deleting the original repository
can make those links unavailable. They are not public download links. Private
delivery manifests retain their original repository ID and are not public release
inputs, even if the replacement has the same name.

The ordinary `PR validation` workflow selects the affected tests across Windows,
Linux, macOS ARM64 and Intel. Shared or unknown changes select every platform;
documentation-only changes avoid native compilation. It never builds distributions.

The explicit main-branch **Release** workflow is the public delivery entrypoint:

- `verify` builds and tests portable candidates on disposable runners without
  uploading application or source-material archives.
- `build` first requires reviewed redistribution evidence, then builds, tests and
  uploads candidate archives for later installation qualification.
- `draft` verifies an exact successful build run, attempt, commit and artifact
  hashes against independent qualification records, then prepares a release draft.

Packaging is implemented for Windows x64 and macOS ARM64. Linux and macOS Intel
have native CI checks but no delivery packager. They are not release options.
No platform has a qualified public binary yet. See [RELEASING.md](RELEASING.md)
for signing, installation checks and the required environment configuration.

The original direct candidate/draft entrypoints remain private-only; the public
repository can use them only through **Release** on main. The private updater
trial remains disabled publicly. Nothing publishes a release on push, a tag or a
PR, and the release workflow never publishes its draft automatically.

## References

- [GitHub repository visibility and historical log exposure](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/setting-repository-visibility)
- [Standard hosted runners and public-repository execution](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [Enable private vulnerability reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository)
