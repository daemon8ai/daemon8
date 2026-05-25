# Contributing

This repository contains the daemon8 Rust workspace and release metadata.

## Issues

Use the issue templates for bug reports and feature requests.

- Bugs need a daemon8 version, OS, reproduction steps, expected behavior, and actual behavior.
- Feature requests need a problem statement and proposed behavior.
- Security reports do not belong in public issues. Use `mail@daemon8.ai`; see [SECURITY.md](./SECURITY.md).
- Questions and design discussion belong in [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions).

## Pull Requests

Open an issue first for non-trivial behavior changes. Small fixes, doc updates, and test corrections can go straight to a PR.

Keep PRs scoped to one change. Include:

- What changed.
- Why it changed.
- What you tested.
- Any user-visible behavior change.

## Local Checks

Run the relevant checks before opening a PR:

```bash
cargo fmt --check
cargo test --workspace -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs the same core gates plus cross-platform `cargo check` jobs.

## Test Expectations

- Bug fixes should include a regression test when practical.
- New behavior should include focused coverage near the changed code.
- CLI, service, MCP, browser, and device behavior should include integration coverage when unit tests would not prove the behavior.

## Style

- Use `rustfmt` defaults.
- Keep public errors typed at crate boundaries.
- Use `anyhow` in the binary crate for application-level propagation, not public library APIs.
- Comments should explain why the code exists or why an unusual path is necessary.
- Avoid broad refactors in PRs that are meant to fix one behavior.

## Release Notes

Update [CHANGELOG.md](./CHANGELOG.md) when the change affects installation, commands, MCP behavior, release artifacts, public configuration, or user-visible runtime behavior.

## Contact

- Security: `mail@daemon8.ai`
- Code of Conduct reports: `mail@daemon8.ai`
- General discussion: [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
