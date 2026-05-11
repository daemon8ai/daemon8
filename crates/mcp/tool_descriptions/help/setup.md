# Setup

daemon8 needs three things to be useful in a project:
1. A `.daemon8.toml` at the project root — registers the project slug and any custom sources.
2. CLI hooks installed on the AI tool you use (Claude Code, Codex, Gemini CLI) — the daemon receives PreToolUse/PostToolUse events as `tool_call` observations.
3. The daemon8 service running — `daemon8 install` registers it as a system service.

## Tools

- `setup_status(cwd)` — read-only state report. Returns `{config_present, providers, runtime_sources, issues}`.
- `setup_plan(cwd)` — preview what `setup_apply` would write. Diff against current state.
- `setup_apply(cwd, yes=true, providers?, install_hooks?, force_hooks?)` — write `.daemon8.toml`, register MCP server entries with detected providers, install hooks at the requested scope. Idempotent.

## Hook scopes (Claude Code)

- `local` — `<cwd>/.claude/settings.local.json` (gitignored, per-machine)
- `shared` — `<cwd>/.claude/settings.json` (committed, team-wide)
- `global` — `~/.claude/settings.json` (cross-project, user-wide)

For codex, hooks are global only (`~/.codex/hooks.json`).

## After setup

The agent should call `start_debug_session` when investigating something specific — that scopes observations and tool calls into a retrievable artifact.
