## Purpose

Enumerate every daemon8 CLI hook installed across providers and scopes (Claude Code: local/shared/global, Codex: global). Use to audit drift or confirm an install.

## When

Diagnosing "is daemon8 actually getting hook events?", or before running hooks_remove/hooks_update to see what's there.

## Prereq

None.

## Args

none.

## Returns
  result: array of {provider, scope, settings_path, entries: [{event, command}]}.

## Errors

filesystem read failures bubble up as {"error": "..."}.

## Next

hooks_repair if the listed `command` paths don't match the running daemon binary; hooks_remove to uninstall a specific provider/scope.
