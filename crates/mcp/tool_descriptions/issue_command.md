## Purpose

Issue a browser DevTools command (eval_js, screenshot, inject_css, navigate, storage, viewport, network conditions, tab management). Screenshot also works on ADB devices via `device_serial`.

## When

Read DOM state, capture visual proof, prototype CSS, simulate conditions, or drive any non-observation browser action.

## Prereq

Connected MCP session + connected browser. Browser auto-connects on localhost:9222; use `connect_browser` for non-standard endpoints.

## Args
  - action: required string. One of: eval_js, screenshot, inject_css, revert_css, list_tabs, get_perf_metrics, get_dom, set_viewport, clear_viewport, network_conditions, navigate, storage_clear, storage_inspect, storage_set, element_at_point, new_tab, close_tab.
  - tab_id: optional string. Target a specific tab (use list_tabs to discover); omit for default tab.
  - device_serial: optional string. Target an ADB/Vega device for screenshots.
  - Per-action: see action-specific keys (expression, selector, url, viewport_width/height/scale/mobile, network_preset, storage_types, storage_key, storage_value, x, y, css).

## Returns
Common envelope with action-specific `data`. Common payload fields include `data.result`, `data.screenshot`, `data.size_bytes`, and `data.tabs`.

## Errors
  - `missing_param`: required action-specific param absent; `why` names the missing field.
  - `browser_not_connected`: see list_connections / connect_browser.

## Next

Call `read_live_feed(origins=["browser"])` to see browser console activity your action triggered.
