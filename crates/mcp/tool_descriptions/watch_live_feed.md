## Purpose

Configure which observations trigger live MCP alerts (notifications/message) in this session. One filter per session.

## When

Call to watch for a specific failure pattern, monitor a particular app, or listen for inter-agent messages on a known origin. Default subscribes to severity >= warn.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args
  - kinds: optional list. (See read_live_feed.)
  - severity_min: optional string.
  - origins: optional list.
  - text_match: optional string.
  - correlation_id: optional string.
  - tags: optional list.
  - service: optional list.
  - source: optional list.
  - source_instance: optional list.
  - include_system: optional bool.

## Returns
Common envelope with `data.subscribed=true` and `data.filter`.

## Errors

none expected.

## Next

`watch_live_feed` is push-side. Pair with `set_lens` (pull-side buffer) when you need both live alerts and a queryable backlog of matches.
