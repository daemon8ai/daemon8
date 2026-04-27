# Contributing to Daemon8

Thanks for showing up. Daemon8 is built in the open and every serious
contribution is welcome. This doc covers how to file issues, send pull
requests, and where contributions land across the daemon8ai org.

Before anything below: by participating, you agree to the
[Code of Conduct](./CODE_OF_CONDUCT.md). Enforcement: `mail@daemon8.ai`.

## Where contributions are accepted

This repo (`daemon8ai/daemon8`) holds the daemon binary and its workspace
crates. The marketing site lives separately at
[`daemon8ai/daemon8-site`](https://github.com/daemon8ai/daemon8-site).

| Repository                                                                    | Scope                                                        | License                                    | Accepts PRs?                       |
|-------------------------------------------------------------------------------|--------------------------------------------------------------|--------------------------------------------|------------------------------------|
| **`daemon8ai/daemon8`** (this repo)                                           | Rust daemon binary and workspace crates                      | [FCL-1.0-ALv2](./LICENSES/FCL-1.0-ALv2.txt) | **Yes**                            |
| [`daemon8ai/daemon8-site`](https://github.com/daemon8ai/daemon8-site)         | daemon8.ai marketing & docs site (TanStack Start + React 19) | Public-visibility                          | Issues welcome; code PRs case-by-case |

This file covers `daemon8ai/daemon8` only.

## Filing issues

Use the templates in [`.github/ISSUE_TEMPLATE/`](./.github/ISSUE_TEMPLATE/).

- **Bug reports** — repro steps, expected vs actual, daemon version from
  `daemon8 --version`, OS, and (for browser bugs) your Chrome version.
- **Feature requests** — what problem it solves, who it serves, what you
  considered instead.
- **Security vulnerabilities** — **do not** open a public issue. Email
  `mail@daemon8.ai` per [`SECURITY.md`](./SECURITY.md).

Questions and design discussions go to
[GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
rather than Issues.

## Sending a pull request

### Before opening

1. **Sign the CLA** — first-time contributors will be prompted automatically
   by CLA Assistant on their first PR. Havy.tech, LLC is the beneficiary.
   The text is derived from the Apache ICLA.
2. **Open an issue first** for anything beyond a trivial fix or copy edit.
   Align on scope before writing the patch; avoids wasted effort on both
   sides.
3. **One logical change per PR.** Commits within a PR can be as granular as
   you want, but the PR itself should be one thing.

### Branch and commit discipline

- Branch off `main`. Branch names are your business — nothing enforced.
- **Commit messages are one-liners.** No multi-paragraph bodies, no emoji,
  no conventional-commit prefixes beyond the natural scope prefix
  (`daemon:`, `repo:`, `ci:`). Example:
  ```
  daemon: reduce cold-start allocations in MCP stdio handler
  ```

### Build and test locally

```bash
cargo test --workspace -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo deny check
```

All four gates must pass. CI runs the same four.

The `rust-toolchain.toml` pins the stable channel plus
rustfmt + clippy components — `cargo` auto-fetches what it needs.

### Tests

New features need new tests. Bug fixes need a regression test that fails
without the fix.

- Unit tests live alongside the code they cover: `#[cfg(test)] mod tests`
  inside each source file, or as `crates/<name>/tests/` for
  cross-module tests.
- Integration tests that exercise the full HTTP / MCP / SSE stack live
  in [`crates/daemon/tests/integration.rs`](./crates/daemon/tests/integration.rs).
- For **larger user-perspective coverage** (real Chrome, real service
  install, real MCP clients) we run a **Testing Gauntlet** —
  see [`TESTING.md`](./TESTING.md). Gauntlet phases are explicitly
  contributor-facing and labeled `help wanted`.

### What gets merged

- Code compiles, tests pass, clippy clean, fmt clean, `cargo deny check` passes.
- Behavior change is described in the PR body.
- Public API changes include a CHANGELOG entry under `[Unreleased]`.
- CLA is signed.

### What gets rejected (fast)

- Unsigned CLA.
- "While I was in there" scope creep — cosmetic refactors bundled with a
  behavior fix.
- Premature abstraction — three concrete call sites before a trait or
  helper is worth it.
- Fighting the borrow checker with `.clone()` in hot paths without a note
  explaining why the alternative is worse.
- Catching errors silently (no `let _ =` on a `Result` that could fail in
  production).
- Reintroducing license-key / tier / capability-gate machinery. Daemon8
  ships OSS, period.

## Style

- Rust: `rustfmt` defaults. No custom settings. Clippy with `-D warnings`.
- Comments explain **why**, never **what**. If a function needs a comment
  explaining what it does, rename the function.
- Errors are typed enums at public API boundaries (`ChromeError`,
  `IngestError`, `AdbError`, `StoreError`). `anyhow` is for the binary
  crate's application-level propagation only, never at a library
  boundary.
- Hot-path allocations need a reason. `Arc<str>` over `String` for IDs
  that cross task boundaries. Mutex poisoning propagates (never silently
  swallow it).

## What's out of scope

- **A paid tier, license keys, or capability gates.** Daemon8 ships OSS,
  period. Proposals for "could you add a premium feature" land as
  feature requests judged on merit, not as gated features.
- **Telemetry back to daemon8.ai.** The daemon is local; it does not
  phone home. Any feature that introduces outbound telemetry is a hard
  no.

## Getting help

- [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
  — questions, design, show-and-tell.
- `mail@daemon8.ai` — vulnerabilities, Code of Conduct enforcement,
  DAEMON8™ use inquiries.

Thanks again. First-time PRs get a prompt review.
