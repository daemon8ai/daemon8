# Checkpoints

A checkpoint is a bookmark within an active debug session. It records the observation `seq` at the moment of creation so subsequent queries can ask "what happened since this point" without juggling timestamps.

## Why use them

Right before any change you might want to compare against:

```
create_checkpoint(description="before applying retry patch")
# ... apply patch, run test ...
read_live_feed(since_checkpoint=<id from above>)
```

Returns only what arrived after the checkpoint — typically the relevant error/log delta.

## Constraints

- Requires an active debug session. Without one, returns a structured `no_active_debug_session` error with `fix.tool: "start_debug_session"`.
- Persisted as a row in the `checkpoint` table; survives daemon restart.
- Linked to its parent session via `debug_session_id`.

## What you get back

```json
{
  "checkpoint_id": "abc123",
  "debug_session_id": "ds_xyz",
  "seq_at_creation": 4271,
  "created_at": 1747000000000000000
}
```

The envelope's `daemon8.next_actions` will hint `read_live_feed` as the natural follow-up.
