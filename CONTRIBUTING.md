# Contributing to NOH

NOH is a desktop media application in active development. Start with the
[build guide](docs/BUILDING.md) and [architecture](docs/ARCHITECTURE.md).
Ready-to-use public installers are not available yet.

## Propose a change

Use an issue for reproducible bugs or to discuss a substantial change before
implementing it. For security issues, follow [SECURITY.md](SECURITY.md).
Keep each pull request focused and explain the user-visible behavior, relevant
validation, and any remaining limitations. Small corrections can go directly
through a pull request.

Use English for code, comments, documentation and command-line diagnostics.
Preserve all seven desktop translations and their interpolation placeholders.
Keep media behavior in the shared core used by the CLI, GUI and MCP server.
Preserve cancellation, background media work, no-overwrite behavior and the
Windows LLD configuration.

## Validate the affected behavior

- Documentation: inspect text, links and the diff; no application build is needed.
- Rust formatting: `cargo fmt --all --check`.
- Focused code: run the relevant tests first, with their real prerequisites.
- Media or shared integration: `cargo dev verify --media --mcp` with FFmpeg available.
- Update behavior: prepare matching workers and follow the update checks in
  [development](docs/DEVELOPMENT.md).

Tests use small synthetic fixtures. Missing FFmpeg or a skipped required test is
not evidence of success. Describe which checks ran and which did not. The PR
workflow selects affected platforms conservatively; shared inputs are checked
on Windows, Linux, macOS Intel and macOS ARM, without creating distributions.
Maintainers may need to approve a fork's Actions run before it starts.

## Protect contributors and users

Do not commit credentials, personal media, machine-specific paths, logs, caches,
executables, signing material or model weights. Before attaching logs, remove
tokens and personal paths. Share the smallest synthetic reproduction possible.
Do not add private tokens to make fork CI pass.

NOH's code is GPL-3.0-only. Contributions must be compatible with that license;
retain applicable third-party attribution and notices. See [LICENSE](LICENSE)
and [third-party components](docs/THIRD-PARTY.md).
