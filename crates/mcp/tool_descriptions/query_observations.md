## Purpose

Query runtime observations from every connected source — browser console, network, exceptions, SQL, app logs, device telemetry, agent tool calls.

## When

Diagnosing a runtime symptom, scanning for a specific error, monitoring a service after a change. Pair with `create_checkpoint` + `since_checkpoint` for incremental "what just changed".

## Prereq

None. Observations are live; re-call to refresh.

## Args
  - kinds: optional list. Filter by kind (log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom, tool_call).
  - severity_min: optional string. Minimum severity (trace, debug, info, warn, error).
  - origins: optional list. Patterns: "app", "app:name", "browser", "browser:tab_id", "device", "device:serial".
  - text_match: optional string. Substring across materialized search_text.
  - since_checkpoint: optional integer. Only obs after this seq (use `create_checkpoint`).
  - limit: optional integer. Default 50.
  - correlation_id: optional string. Exact match (Pre/PostToolUse pair on tool_use_id).
  - tags: optional list. All listed tags must be present.
  - include_system: optional bool. Default excludes "_system"-tagged rows.

## Returns
  result: {observations: [...], total, lens_observations?, lens_count?, browser_state?}.
  daemon8.active_debug_session: present if a debug session is active (and stamping new obs).

## Errors
  - query_failed: db query error. hint: check daemon logs.

## Next

create_checkpoint before testing a fix; query_memory(tags=["hash:<error_hash>"]) on any error obs to find prior fixes.

## Discovery hints

Observations with `kind=custom` and `channel=discovery_hint` are project-onboarding hints emitted when daemon8 classifies a project but finds no `source_template` entries covering its tags. The `data` field carries a `DiscoveryHintPayload` (project_root, classification_tags, framework_versions, platform, known_project_type_tags_ref, instruction_text, ...). When you see one, follow the `instruction_text` and call `librarian_index` with one or more `source_template` nodes for the locations you discover.

