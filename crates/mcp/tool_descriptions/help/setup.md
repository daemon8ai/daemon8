# Setup

daemon8 setup registers the MCP server with detected AI coding tools. It auto-detects installed providers and writes their config files.

Supported providers: Claude Code, Codex, Gemini CLI, OpenCode.

## Tools

- `setup_status(cwd?)` — read-only state report. Returns `{providers, daemon_running, issues}`.
- `setup_plan(cwd)` — alias for setup_status (backward compatibility).
- `setup_apply(cwd, yes=true, providers?)` — write MCP server config to detected providers. Idempotent.

## After setup

For project-local source coverage, call `discover_project(project_root=...)` and persist reusable source knowledge with `librarian_index`. Use `daemon8 setup init` only for explicit source overrides.

The agent should call `start_debug_session` when investigating something specific — that scopes observations and tool calls into a retrievable artifact.
