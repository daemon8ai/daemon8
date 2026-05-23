## Purpose

Write an observation into the daemon's stream. Becomes immediately queryable and triggers any matching subscriptions.

## When

Logging a debugging note, sending an alert (severity=warn|error), recording a state snapshot, emitting a metric, or messaging another agent that listens on a known origin.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args
  - kind: optional string. One of log, query, http_exchange, exception, state_snapshot, metric, custom, js_exception, lifecycle, tool_call. Defaults to log.
  - severity: optional string. trace | debug | info | warn | error. Defaults to debug.
  - app: optional string. Origin tag (e.g. "my-api", "agent-name"). Defaults to MCP session app.
  - data: required object. Free-form payload. Conventional `data.message` for a clean log line.
  - tags: optional list. Retrieval tags.
  - correlation_id: optional string. Ties multiple observations to one logical operation.
  - service: optional string. Service provenance.
  - source: optional string. Logical source id.
  - source_instance: optional string. Concrete source instance.

## Returns
Common envelope with `data.ok=true`, `data.queued=true`, and session context. If a debug session is active, the observation is stamped with its `debug_session_id`.

## Errors
  - daemon_shutting_down: ingest channel closed. hint: not retryable in the same session.

## Next

Call `read_live_feed` to confirm the row landed. If severity=error, resolve or link it through the active debug session when the root cause is known.
