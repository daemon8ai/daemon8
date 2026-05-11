## Purpose

Remove the active observation lens. Stops buffering and discards the ring buffer contents. No observations are deleted from the underlying store.

## When

Done with the focused-watch session for which the lens was set, or you want to switch to a different filter.

## Prereq

None. No-op if no lens is active.

## Args

none.

## Returns
  result: {cleared: true}.

## Errors

none expected.

## Next

set_lens with a new filter, or query_observations directly without the lens.
