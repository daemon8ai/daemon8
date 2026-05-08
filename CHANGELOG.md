# Changelog

This changelog summarizes major repository changes by release phase.
For full commit-by-commit detail, use `git log` and
[daemon8ai/daemon8/releases](https://github.com/daemon8ai/daemon8/releases).

## Unreleased

### Lean MVP cull

- Removed experimental Deliber8 runtime, CLI, roster, inbox, and storage
  surfaces from the shipped workspace.
- Removed memory tiers, bookkeeper operations, embedding profiles, and
  embedding runtime/config/dependencies.
- Kept daemon8 focused on runtime observations, checkpoints, lenses, browser
  and device actions, hooks as telemetry, setup, and curated non-embedded
  memory.
- Memory export retains a sanctioned 2-layer defense against legacy embedding
  data: validate-time rejection of `embedding` projections plus result-time
  stripping of `embedding` fields from result rows. This is the one ADR-004
  backward-compatibility exception, kept because no migration command ships
  and operators should not see stale embedding bytes leak into curated memory
  exports.

### Audit closeout

- HTTP `/ingest` and `/ingest/batch` now return `503 Service Unavailable`
  when the broadcast channel has shut down, matching the MCP
  `ingest_observation` tool contract instead of silently returning
  `202 Accepted`.
- MCP subscription filters are now per-session: concurrent agents calling
  `subscribe_observations` no longer overwrite each other. The push task
  observes the daemon-wide cancellation token so shutdown propagates
  cleanly.
- `validated_memory_export_query` now returns a typed `MemoryExportError`
  enum instead of stringly-typed errors.
- `ProjectSetupState.hook_policy` is now a typed `HookPolicy` enum
  (`install` / `manual`); arbitrary strings are rejected by the parser.
- Dead `restart_service` removed.
- Memory tool descriptions enforce the curated-lessons rule: only stable,
  reusable, verified conclusions — no raw logs, transcripts, or guesses.
- README CLI table now lists `daemon8 memory`.

### Setup, diagnostics, and reliability

- Added guided setup flows (`setup status`, `setup plan`, `setup apply`) with
  post-apply status reporting.
- Improved `--config` handling.
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
