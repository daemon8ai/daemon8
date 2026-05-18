# Changelog

This changelog summarizes major repository changes by release phase.
For full commit-by-commit detail, use `git log` and
[daemon8ai/daemon8/releases](https://github.com/daemon8ai/daemon8/releases).

## v0.4.0 - 2026-05-18

- Replaced the pre-alpha setup flow with the alpha control surface:
  `daemon8 init`, `daemon8 connect`, `daemon8 status`, and the MCP
  `daemon8_init` / `daemon8_connect` / `daemon8_status` tools.
- Project config is now `.daemon8/config.md` with YAML frontmatter.
  Legacy TOML project config is not an alpha input.
- Runtime state, cursors, debug sessions, and the session/scope ledger are
  daemon-owned SurrealDB state. `daemon8 reset --yes` clears daemon-owned
  state only and leaves project files untouched.
- No public memory CLI or MCP tools ship; daemon-owned memory remains for
  debug-session summaries and error-signature dedupe.
- Service removal is `daemon8 service uninstall`.

## v0.3.0 - 2026-05-08

### Upgrading from v0.2.x

Historical notes below describe pre-alpha releases. Use the
`v0.4.0` section above for current alpha commands and config.

The lean MVP cull tightened TOML config parsing to reject unknown keys via `deny_unknown_fields`. Existing global and project configs that contain removed sections will fail to parse and the daemon will refuse to start until the stale keys are removed.

If the daemon reports `[ERR] config load (unknown field: found '<X>')` after upgrade, edit the offending file and remove the listed key. Affected sections to delete from pre-alpha TOML config:

- `[embeddings]` (entire section — embedding runtime is gone)
- `[mcp]` `http = ...` field (HTTP MCP key removed; HTTP transport is now controlled at the server layer)
- `[storage]` `embedding_path = ...` field
- Any `role_default = ...` field on `[browser]` or `[sources.*]` (legacy agent-role concept)
- `desired_scope` entries beyond `["file-sources"]` (now the only valid scope)
- `hook_policy` values other than `"install"` or `"manual"`
- Any deliber8/inbox/envelope/specialist/bookkeeper/agent keys

Memory data: rows in the `memory` SurrealDB table that contain `embedding` columns are not migrated. The export pipeline strips them; live queries and saves no longer touch the column. If a fresh start is preferred, delete the local data directory before first `0.3.0` run (`daemon8 uninstall` removes it).

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
  when the broadcast channel has shut down instead of silently returning
  `202 Accepted`.
- MCP subscription filters are now per-session. The push task observes the
  daemon-wide cancellation token so shutdown propagates cleanly.
- `validated_memory_export_query` now returns a typed `MemoryExportError`
  enum instead of stringly-typed errors.
- `ProjectSetupState.hook_policy` is now a typed `HookPolicy` enum
  (`install` / `manual`); arbitrary strings are rejected by the parser.
- Dead `restart_service` removed.
- Memory tool descriptions enforce the curated-lessons rule: only stable,
  reusable, verified conclusions — no raw logs, transcripts, or guesses.
- README CLI table now lists `daemon8 memory`.

### Setup, diagnostics, and reliability

- Added guided setup flow work for the pre-alpha surface.
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
