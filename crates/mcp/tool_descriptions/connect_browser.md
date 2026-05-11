## Purpose

Override the Chrome DevTools endpoint the daemon connects to. Auto-discovery handles localhost:9222 by default; this tool is for non-standard ports or remote endpoints.

## When

Browser is on a non-standard port, you're targeting a remote DevTools endpoint over a tunnel, or you want to fail fast on a config mismatch instead of waiting for auto-discovery. Do NOT call as a debugging shortcut — reconnection is automatic.

## Prereq

None.

## Args
  - endpoint: required string. Full URL including scheme (e.g. "http://localhost:9222").

## Returns
  result: {status: "connecting", endpoint}.

## Errors
  - daemon_shutting_down: connect command channel closed.

## Next

list_connections to confirm the new endpoint is reachable; issue_command for any browser action.
