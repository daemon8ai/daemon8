## Purpose

Configure which observations trigger live MCP alerts (notifications/message) in this session. One filter per session.

## When

Watching for a specific failure pattern, monitoring a particular app, listening for inter-agent messages on a known origin. Default subscribes to severity >= warn.

## Prereq

None.

## Args
  - kinds: optional list. (See query_observations.)
  - severity_min: optional string.
  - origins: optional list.
  - text_match: optional string.
  - correlation_id: optional string.
  - tags: optional list.
  - include_system: optional bool.

## Returns
  result: {subscribed: true, filter: "default (severity >= warn)" | "custom"}.

## Errors

none expected.

## Next

subscribe is push; pair with set_lens (pull-side buffer) when you want both live alerts and a queryable backlog of matches.
