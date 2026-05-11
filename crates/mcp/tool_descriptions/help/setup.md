# Setup

daemon8 setup registers the MCP server with detected AI coding tools. It auto-detects installed providers and writes their config files.

Supported providers: Claude Code, Codex, Gemini CLI, OpenCode.

## Tools

- `setup_status(cwd)` — read-only state report. Returns `{providers, daemon_running, issues}`.
- `setup_plan(cwd)` — alias for setup_status (backward compatibility).
- `setup_apply(cwd, yes=true, providers?)` — write MCP server config to detected providers. Idempotent.

## After setup

For additional features (CLI hooks, project init), use `daemon8 features` interactively or the `hooks_*` MCP tools.

The agent should call `start_debug_session` when investigating something specific — that scopes observations and tool calls into a retrievable artifact.
