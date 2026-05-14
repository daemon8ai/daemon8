## Purpose

Register daemon8 MCP server with detected AI coding tools. Auto-detects installed providers (Claude Code, Codex, Gemini CLI, OpenCode) and writes their config files. Idempotent.

## When

First-time setup, or re-applying after a provider install.

## Prereq

None.

## Args
  - yes: required boolean. MUST be true to confirm the mutation.
  - cwd: optional string. Project working directory.
  - providers: optional string. Comma-separated providers to configure (e.g. "claude-code,gemini,codex"). Omit for auto-detection.

## Returns
  result: {providers: [{name, config_path, was_configured, action}], daemon_running, issues}.

## Errors
  - missing_yes: yes was absent or false. hint: pass yes=true to confirm.

## Next

setup_status to confirm; use `daemon8 setup init` for explicit project source overrides.
