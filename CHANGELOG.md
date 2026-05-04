# Changelog

This changelog summarizes major repository changes by release phase.
For full commit-by-commit detail, use `git log` and
[daemon8ai/daemon8/releases](https://github.com/daemon8ai/daemon8/releases).

## Unreleased

### Runtime, memory, and embeddings

- Added tiered memory tables (`short`, `reference`, `long`) with
  bookkeeper sweep and dedupe operations.
- Added embedding profile registry and wired embeddings through memory-save
  paths.
- Added tier query, sweep, dedupe, and embedding-profile MCP tools plus
  matching HTTP routes.

### Deliber8 and operations

- Replaced the legacy agent namespace with deliber8 CLI/runtime surfaces.
- Added deliber8 roster and inbox MCP tools and doctor backlog checks.
- Hardened envelope/card storage behavior and router gating behavior.

### Setup, diagnostics, and reliability

- Added guided setup flows (`setup status`, `setup plan`, `setup apply`) with
  post-apply status reporting.
- Expanded `daemon8 doctor` checks and improved `--config` handling.
- Improved shutdown/browser lifecycle and tightened logging tests and
  operational logging.

### Documentation

- Rewrote README and MCP tool documentation to match current source.
- Simplified root meta docs and contribution routing.

## v0.2.4 - 2026-04-27

- Wired embeddings through `save_memory` and switched CLI wording to
  provider-agnostic terminology.
- Adjusted release build matrix based on cross-platform packaging limits.
- Updated release workflow gating to handle partial matrix success during
  target stabilization.

## v0.2 (Summary)

- Migrated to the v0.2 workspace after merging v0.1 history.
- Adopted SurrealDB-backed storage, port `8888` defaults, and a broader CLI.
- Added lens management, checkpoint API, richer observation filtering, and
  `_system` tag handling for hook-origin events.
- Expanded parser/input-source pipelines, embedding integration, install flow,
  and project docs.

## v0.1.0 - 2026-04-25

- Initial open-source daemon8 release.
- Flattened the early repository structure and corrected release/doc setup
  details.
