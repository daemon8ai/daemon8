## Purpose

Override the Chrome DevTools endpoint the daemon connects to. Automatic browser connect handles localhost:9222 by default; this tool is for non-standard ports or remote endpoints.

## When

Non-standard DevTools port, remote endpoint over a tunnel, or explicit endpoint override. **Do not** call as a debugging shortcut -- the daemon reconnects automatically.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args
  - endpoint: required string. Full URL including scheme (e.g. "http://localhost:9222").

## Returns
Common envelope with `data.status="connecting"` and `data.endpoint`.

## Errors
  - daemon_shutting_down: connect command channel closed.

## Next

Call `list_connections` to confirm the new endpoint is reachable. Call `issue_command` for any browser action.
