## Purpose

List active browser and application connections feeding observations into the daemon.

## When

Confirm the daemon receives telemetry from the expected source, or diagnose missing observations -- unlisted sources are not connected.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args

none.

## Returns
Common envelope with `data.browser` and optional `data.applications`.
  State values: connected | connecting | reconnecting | disconnected.

## Errors

none expected (read-only).

## Next

Call `connect_browser` if the browser endpoint shows disconnected. Call `read_live_feed(origins=["app:<name>"])` once a source is confirmed.
