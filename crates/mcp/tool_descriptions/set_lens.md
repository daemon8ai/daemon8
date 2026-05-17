## Purpose

Install a per-session observation filter (lens) plus a ring buffer. Matching observations accumulate behind the scenes; subsequent `read_live_feed` calls auto-include the buffered matches.

## When

Watching for a specific event over time without polling, focusing on one origin/severity band, or building a working set during a debugging run.

## Prereq

A connected MCP session. Call `daemon8_connect` first. Setting a lens replaces any existing lens; `clear_lens` to remove.

## Args
  - kinds: optional list. (Same vocabulary as read_live_feed.)
  - severity_min: optional string.
  - origins: optional list.
  - text_match: optional string.
  - correlation_id: optional string.
  - tags: optional list.
  - include_system: optional bool.
  - capacity: optional integer. Ring buffer capacity, default 200, max 1000.

## Returns
Common envelope with `data.active`, `data.filter`, `data.capacity`, `data.buffered`, and `data.cursor`.

## Errors

none expected.

## Next

lens_status to inspect; read_live_feed to drain the buffer; clear_lens when done.
