## Purpose

Issue a browser or device command. Browser actions (eval_js, screenshot, inject_css, navigate, storage, viewport, network conditions, tab management) drive a connected Chrome target. Device actions (device_key, device_text, device_tap) drive a connected Android or Vega/Fire TV device by `device_serial`. Screenshot spans both: pass `device_serial` to capture from a device instead of the browser.

## When

On a browser: read DOM state, capture visual proof, prototype CSS, or simulate conditions. On a device: drive the UI when you need to change runtime state, not just observe it -- press a d-pad/remote key to move focus, type into a focused field, or tap a coordinate on a connected emulator, phone, or TV.

## Prereq

Browser actions need a connected MCP session plus a connected browser (auto-connects on localhost:9222; use `connect_browser` for non-standard endpoints). Device actions need a connected MCP session plus an ADB-reachable device -- confirm it with `list_connections`. The daemon must run with ADB enabled, because device input is routed through the ADB transport; without it, device actions return `device_input_unavailable`.

## Args
  - action: required string. Browser: eval_js, screenshot, inject_css, revert_css, list_tabs, get_perf_metrics, get_dom, set_viewport, clear_viewport, network_conditions, navigate, storage_clear, storage_inspect, storage_set, element_at_point, new_tab, close_tab. Device: device_key, device_text, device_tap.
  - tab_id: optional string. Target a specific browser tab (use list_tabs to discover); omit for the default tab.
  - device_serial: required for device actions and device screenshots. The connected device serial (e.g. emulator-5554).
  - device_platform: optional string, "android" or "vega". Selects the input mechanism because the platforms diverge -- Android uses the `input` utility, Vega uses `inputd-cli` -- but the caller-facing behavior is identical. Defaults to "android".
  - device_key: required for action=device_key. One of: up, down, left, right, select, back, home, menu, play_pause, volume_up, volume_down.
  - device_text: required for action=device_text. The text to type on the device.
  - x, y: required for action=device_tap (also used by element_at_point). The coordinate to tap.
  - Per browser action: see action-specific keys (expression, selector, url, viewport_width/height/scale/mobile, network_preset, storage_types, storage_key, storage_value, css).

## Returns

Common envelope with action-specific `data`. Browser payloads carry fields such as `data.result`, `data.screenshot`, `data.size_bytes`, and `data.tabs`. Device input returns `code="device_input_sent"` with `data.serial`, `data.action`, and the echoed `data.input`.

## Errors
  - `missing_param`: a required action-specific param is absent; `why` names the missing field (device_key needs `device_key`; device_tap needs `x` and `y`).
  - `browser_not_connected`: browser action with no connected browser; see list_connections / connect_browser.
  - `device_input_unavailable`: device action but the daemon was started without ADB enabled; restart the daemon with ADB enabled.
  - `action_failed`: the device rejected the command or was unreachable; `why` carries the device error.

## Next

After a browser action, call `read_live_feed(origins=["browser"])` to see the console activity it triggered. After a device action, capture a `screenshot` with the same `device_serial` to confirm the UI changed, or call `read_live_feed(origins=["device"])` for device-side logs.
