# Observations

Observations are the raw runtime telemetry stream — logs, queries, exceptions, HTTP exchanges, JS exceptions, lifecycle events, tool calls, custom events. Stored in the `observation` table; the 24h reaper drops untied rows.

## Kinds

- `log` — generic message
- `query` — SQL with `sql`, `duration_ms`
- `http_exchange` — `method`, `url`, `status?`, `duration_ms?`
- `exception` — `message`, `trace?`
- `js_exception` — browser JS exception with `message`, `line?`, `column?`
- `state_snapshot` — labeled snapshot for diffing
- `metric` — `name`, `value`
- `lifecycle` — `event_name`, `frame_id` (browser navigation, etc.)
- `tool_call` — agent CLI tool invocation: `tool`, `input`, `output?`, `exit_code?`, `duration_ms?` (paired Pre/PostUse via `correlation_id`)
- `custom` — fallback with `channel`

## Querying

`read_live_feed(kinds?, severity_min?, origins?, service?, source?, source_instance?, text_match?, since_checkpoint?, limit?, correlation_id?, tags?, include_system?)`. Most filters are AND-ed.

## Subscribing

`watch_live_feed(filter)` for live push (one filter per session). Default subscribes to `severity >= warn`.

## Per-observation linkage

MCP-written observations from `write_to_live_feed` get the active `debug_session_id`, and get `checkpoint_id` when a checkpoint exists. Error observations written through that path also get `error_hash` and first-sight error signature promotion. File read-through and external runtime sources keep their own provenance.

## Search

`text_match` searches a denormalized `search_text` field that includes severity, kind, origin, service/source provenance, tags, source location, correlation/session ids, and the data blob.
