## Purpose

Remove the active observation lens. Stops buffering and discards the ring buffer contents. No observations are deleted from the underlying store.

## When

The focused-watch session is complete, or you need a different filter pattern.

## Prereq

A connected MCP session. Call `daemon8_connect` first. No-op if no lens is active.

## Args

none.

## Returns
Common envelope with `data.cleared=true`.

## Errors

none expected.

## Next

Call `set_lens` with a new filter, or call `read_live_feed` directly without the lens.
