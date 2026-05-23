## Purpose

Query runtime observations from every connected source — browser console, network, exceptions, SQL, app logs, device telemetry, agent tool calls.

## When

Diagnosing a runtime symptom, scanning for a specific error, monitoring a service after a change. Pair with `create_checkpoint` + `since_checkpoint` for incremental "what just changed".

## Prereq

A connected MCP session. Call `daemon8_connect` first. Observations are live; re-call to refresh.

## Args
  - kinds: optional list. Filter by kind (log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom, tool_call).
  - severity_min: optional string. Minimum severity (trace, debug, info, warn, error).
  - origins: optional list. Patterns: "app", "app:name", "browser", "browser:tab_id", "device", "device:serial".
  - text_match: optional string. Substring across materialized search_text.
  - since_checkpoint: optional integer. Only obs after this seq (use `create_checkpoint`).
  - limit: optional integer. Default 50.
  - correlation_id: optional string. Exact match (Pre/PostToolUse pair on tool_use_id).
  - tags: optional list. All listed tags must be present.
  - service: optional list. Filter by service provenance.
  - source: optional list. Filter by logical source id from `.daemon8/config.md`.
  - source_instance: optional list. Filter by concrete file/transcript/source instance.
  - include_system: optional bool. Default excludes "_system"-tagged rows.

## Returns
Common envelope with `data.observations`, `data.summary.total`, optional `data.lens_observations`, `data.lens_count`, `data.browser_state`, and session context.

## Errors
  - narrow_filter_required: in general mode, add `kinds`, `severity_min`, `origins`, `service`, `source`, `source_instance`, `text_match`, `since_checkpoint`, `correlation_id`, or `tags`.
  - query_failed: db query error. hint: check daemon logs.

## Next

Call `create_checkpoint` before testing a fix. Observations are runtime signals -- record durable conclusions via `resolve_debug_session`.
