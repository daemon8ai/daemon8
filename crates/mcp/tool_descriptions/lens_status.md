## Purpose

Inspect the active lens (if any) and the ring buffer behind it. Read-only.

## When

Verifying the lens filter matches expectations, checking buffer depth before `read_live_feed` (deep buffer means the next query will return many matches), or confirming the lens survived a long session.

## Prereq

A connected MCP session. Call `daemon8_connect` first.

## Args

none.

## Returns
Common envelope with `data.active`, `data.filter`, `data.buffered`, `data.capacity`, and `data.cursor`. `cursor` is the highest seq processed; `buffered` is the current ring count.

## Errors

none expected.

## Next

read_live_feed to drain the buffer; clear_lens when done.
