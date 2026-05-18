## Purpose

Override the Chrome DevTools endpoint the daemon connects to. Automatic browser connect handles localhost:9222 by default; this tool is for non-standard ports or remote endpoints.

## When

Browser is on a non-standard port, you're targeting a remote DevTools endpoint over a tunnel, or you want to fail fast on a config mismatch instead of waiting for automatic reconnect. Do NOT call as a debugging shortcut — reconnection is automatic.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args
  - endpoint: required string. Full URL including scheme (e.g. "http://localhost:9222").

## Returns
Common envelope with `data.status="connecting"` and `data.endpoint`.

## Errors
  - daemon_shutting_down: connect command channel closed.

## Next

list_connections to confirm the new endpoint is reachable; issue_command for any browser action.
