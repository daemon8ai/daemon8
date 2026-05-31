## Purpose

List active browser and application connections feeding observations into the daemon.

## When

Confirm the daemon receives telemetry from the expected source, or diagnose missing observations -- unlisted sources are not connected.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args

none.

## Returns
Common envelope with `data.browser`, `data.device_features`, optional `data.applications`, and optional `data.device_control`.
  State values: connected | connecting | reconnecting | disconnected.

`data.device_features` reports which device platforms are enabled in the running daemon: `adb_enabled` for generic Android/ADB and `vvd_enabled` for Vega Virtual Device support. Enable them from the CLI with `daemon8 feature adb enable` or `daemon8 feature vvd enable`, then restart daemon8.

`data.device_control` reports device action health from recent `issue_command` device actions. A device can still appear under `applications` because logs are flowing while `device_control[].state` is `degraded` because screenshot/input timed out or failed.

## Errors

none expected (read-only).

## Next

Call `connect_browser` if the browser endpoint shows disconnected. If a needed device platform is disabled, use the feature CLI first. If device logs are present but `device_control` is degraded, treat screenshot/input as unhealthy separately from log ingestion. Call `read_live_feed(origins=["app:<name>"])` once a source is confirmed.
