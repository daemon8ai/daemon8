## Purpose

Snapshot of daemon health, connected sources, daemon version, MCP session id, and current connect scope when present.

## When

Diagnostic exception to connect-first flow. Use when you need to inspect daemon/session state before deciding the next call.

## Prereq

None.

## Args

none.

## Returns

Common envelope with daemon health in `data`, including `connection` when this MCP session is connected. Project connections include `connection.transcript_path` only when a transcript is bound. `scope_ledger.recent_scopes` and `scope_ledger.recent_failures` contain scope history.

- `data.connection` null → call `daemon8_connect` before project work.
- `data.connection.mode == "general"` → reconnect with a project path for project-scoped tools.
- Sources show zero entries → project config needs source population.

## Errors

none expected (read-only).

## Next

If `data.connection` is null or absent, call `daemon8_connect` before project-aware work.
