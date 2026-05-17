## Purpose

One-shot alpha snapshot: daemon health, connected sources, daemon version, MCP session id, and current connect scope when present.

## When

Diagnostic exception to connect-first flow. Use when you need to inspect daemon/session state before deciding the next call.

## Prereq

None.

## Args

none.

## Returns

Common envelope with summary data in `data`, including `connection` when this MCP session is connected. A project connection includes `connection.transcript_path` when an active transcript is bound. Scope history is reported under `scope_ledger.recent_scopes` / `scope_ledger.recent_failures`.

## Next

If `data.connection` is null or absent, call `daemon8_connect` before project-aware work.
