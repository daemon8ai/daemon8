## Purpose

One-shot health snapshot of the daemon: total observation count, errors observed in the last 60 seconds, connected sources, daemon version.

## When

First tool call in a session, or any time you want to confirm the daemon is alive and how loud the stream currently is.

## Prereq

None.

## Args

none.

## Returns
  result: {observation_count, error_count_last_60s, active_channels, connections, health, daemon_version}.
  daemon8.active_debug_session: present if a debug session is active.

## Errors
  - summary_failed: db query failed. hint: check daemon logs.

## Next

query_observations to see the current stream; start_debug_session if you're about to investigate something specific.
