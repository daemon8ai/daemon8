## Purpose

List active browser and application connections feeding observations into the daemon.

## When

Confirming the daemon is receiving telemetry from the expected source; diagnosing missing observations (a source not listed is not connected).

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

connect_browser if the browser endpoint shows disconnected; read_live_feed(origins=["app:<name>"]) once a source is confirmed.
