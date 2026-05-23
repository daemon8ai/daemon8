## Purpose

Snapshot of daemon health, connected sources, daemon version, MCP session id, and current connect scope when present.

## When

Diagnostic exception to connect-first flow. Use when you need to inspect daemon/session state before deciding the next call.

## Prereq

None.

## Args

none.

## Returns

Common envelope with summary data in `data`, including `connection` when this MCP session is connected. A project connection includes `connection.transcript_path` when an active transcript is bound. The `scope_ledger.recent_scopes` and `scope_ledger.recent_failures` fields contain scope history.

If `data.connection` is null, this MCP session has no bound scope -- call `daemon8_connect` before project work because project-scoped tools require a bound session. If `data.connection.mode` is "general", reconnect with a project path to unlock project-scoped tools. If sources show zero entries, the project config needs source population.

## Errors

none expected (read-only).

## Next

If `data.connection` is null or absent, call `daemon8_connect` before project-aware work.
