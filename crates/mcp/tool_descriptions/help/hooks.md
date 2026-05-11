# Hook management

CLI provider hooks let daemon8 capture every tool call (Bash, Edit, Write, Read, etc.) the agent makes as a structured `tool_call` observation. PreToolUse and PostToolUse events are paired via `correlation_id = tool_use_id` so the input/output round-trip is queryable.

## Tools

- `hooks_list` — enumerate every daemon8 hook installed across providers and scopes. Returns `[{provider, scope, settings_path, entries: [{event, command}]}]`.
- `hooks_remove(provider, scope?)` — uninstall daemon8 entries from `claude` (per scope or all) or `codex`. Other (non-daemon8) hooks in the same file are left untouched.
- `hooks_update(provider, scope?)` — reinstall (force=true). Useful when the daemon binary has moved or the canonical hook spec changed.
- `hooks_repair` — scan all installed hook entries; reinstall only those whose command path no longer matches the running daemon binary.

## Providers / scopes

| Provider | Scopes available |
|----------|------------------|
| claude   | local, shared, global |
| codex    | global only      |
| gemini   | global only      |

OpenCode uses a plugin system instead of CLI hooks; its MCP server registration happens via `setup_apply`.
