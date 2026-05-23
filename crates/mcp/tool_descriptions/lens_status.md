## Purpose

Inspect the active lens (if any) and the ring buffer behind it. Read-only.

## When

Call to verify the lens filter matches expectations, check buffer depth before `read_live_feed` (deep buffer means the next query will return many matches), or confirm the lens survived a long session.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args

none.

## Returns
Common envelope with `data.active`, `data.filter`, `data.buffered`, `data.capacity`, and `data.cursor`. `cursor` is the highest seq processed; `buffered` is the current ring count.

## Errors

none expected.

## Next

Call `read_live_feed` to drain the buffer. Call `clear_lens` when the focused-watch session is complete.
