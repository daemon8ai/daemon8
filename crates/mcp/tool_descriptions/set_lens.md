## Purpose

Install a per-session observation filter. Matching observations accumulate; subsequent `read_live_feed` calls auto-include buffered matches.

## When

Call when watching for a specific event over time without polling, focusing on one origin/severity band, or building a working set during a debugging run.

## Prereq

A connected MCP session. Call `daemon8_connect` first. Setting a lens replaces any existing lens; `clear_lens` to remove.

## Args
  - kinds: optional list. (Same vocabulary as read_live_feed.)
  - severity_min: optional string.
  - origins: optional list.
  - text_match: optional string.
  - correlation_id: optional string.
  - tags: optional list.
  - service: optional list.
  - source: optional list.
  - source_instance: optional list.
  - include_system: optional bool.
  - capacity: optional integer. Ring buffer capacity, default 200, max 1000.

## Returns
Common envelope with `data.active`, `data.filter`, `data.capacity`, `data.buffered`, and `data.cursor`.

## Errors

none expected.

## Next

- Call `lens_status` to verify the filter is matching what you expect.
- Call `read_live_feed` to drain buffered matches -- the lens results appear automatically alongside live observations.
- Call `clear_lens` when the focused-watch session is complete or you need a different filter pattern.
