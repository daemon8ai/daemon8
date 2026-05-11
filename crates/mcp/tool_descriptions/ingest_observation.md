## Purpose

Write an observation into the daemon's stream. Becomes immediately queryable and triggers any matching subscriptions.

## When

Logging a debugging note, sending an alert (severity=warn|error), recording a state snapshot, emitting a metric, or messaging another agent that listens on a known origin.

## Prereq

None.

## Args
  - kind: required string. One of log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom, tool_call.
  - severity: required string. trace | debug | info | warn | error.
  - app: optional string. Origin tag (e.g. "my-api", "agent-name"). Defaults to MCP session app.
  - data: optional object. Free-form payload. Conventional `data.message` for a clean log line.
  - tags: optional list. Retrieval tags.
  - correlation_id: optional string. Ties multiple observations to one logical operation.

## Returns
  result: {"ok": true}.
  daemon8.active_debug_session: present if a session is active (which case the obs is auto-stamped with debug_session_id).

## Errors
  - daemon_shutting_down: ingest channel closed. hint: not retryable in the same session.

## Next

query_observations to confirm the row landed; if severity=error, check query_memory(tags=["hash:..."]) for prior fixes.
