## Purpose

One-shot alpha snapshot: daemon health, connected sources, daemon version, MCP session id, and current connect scope when present.

## When

Diagnostic exception to connect-first flow. Use when you need to inspect daemon/session state before deciding the next call.

## Prereq

None.

## Args

none.

## Returns

Common envelope with summary data in `data`.

## Next

If `data.connection` is absent, call `daemon8_connect` before project-aware work.
