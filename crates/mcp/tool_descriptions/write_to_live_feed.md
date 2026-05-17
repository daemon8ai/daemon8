## Purpose

Write an observation into the daemon's stream. Becomes immediately queryable and triggers any matching subscriptions.

## When

Logging a debugging note, sending an alert (severity=warn|error), recording a state snapshot, emitting a metric, or messaging another agent that listens on a known origin.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args
  - kind: required string. One of log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom, tool_call.
  - severity: required string. trace | debug | info | warn | error.
  - app: optional string. Origin tag (e.g. "my-api", "agent-name"). Defaults to MCP session app.
  - data: optional object. Free-form payload. Conventional `data.message` for a clean log line.
  - tags: optional list. Retrieval tags.
  - correlation_id: optional string. Ties multiple observations to one logical operation.

## Returns
Common envelope with `data.ok=true` and session context. If a debug session is active, the observation is stamped with its `debug_session_id`.

## Errors
  - daemon_shutting_down: ingest channel closed. hint: not retryable in the same session.

## Next

read_live_feed to confirm the row landed; if severity=error, resolve or link it through the active debug session when the root cause is known.
