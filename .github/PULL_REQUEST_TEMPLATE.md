<!--
Thanks for sending a PR. A few things before you hit submit:

1. Read CONTRIBUTING.md if this is your first PR.
2. If the change is non-trivial, an issue should already exist for scope alignment.
3. CLA Assistant will prompt on first PR. Every contribution must be signed.
-->

## What changed

<!-- One or two sentences on the behavior change. Include user-visible impact. -->

## Why

<!-- The problem this solves. Link the issue if one exists (#123). -->

## Area affected

- [ ] Daemon core (`crates/`)
- [ ] MCP tools (`crates/mcp/`)
- [ ] HTTP API (`crates/api/`)
- [ ] Store / memory (`crates/store/`)
- [ ] Browser / ADB bridges (`crates/chrome/`, `crates/adb/`)
- [ ] CLI (`crates/daemon/`)
- [ ] Connect / init / service / release surface
- [ ] Docs / meta (root files, .github/, etc.)

## Tests

<!-- What you ran locally. Paste the last few lines of output or a summary. -->

- [ ] `cargo test --workspace -- --test-threads=1` green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `cargo deny check` clean

## Checklist

- [ ] CLA signed (CLA Assistant will prompt automatically)
- [ ] CHANGELOG updated if this is a user-visible change
- [ ] New behavior is covered by a test; bug fixes include a regression test
- [ ] Commit message is a single line with a scope prefix (`daemon:`, `repo:`, `ci:`)

## Notes for reviewers

<!-- Anything worth calling out — tricky edge cases, alternatives you considered, follow-up work you'd queue. -->
