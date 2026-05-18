# Checkpoints

A checkpoint is a bookmark within an active debug session. It records the observation `seq` at the moment of creation so subsequent queries can ask "what happened since this point" without juggling timestamps.

## Why use them

Right before any change you might want to compare against:

```
daemon8_connect(provider="codex", project_path="/path/to/project")
start_debug_session(agent_id=":host/codex+agent>")
create_checkpoint(description="before applying retry patch")
# ... apply patch, run test ...
read_live_feed(since_checkpoint=<seq_at_creation from above>)
```

Returns only what arrived after the checkpoint — typically the relevant error/log delta.

## Constraints

- Requires a connected project-mode MCP session and an active debug session. Without a project connection, returns `project_required`; without an active debug session, returns `no_active_debug_session` with `next_actions[].tool="start_debug_session"`.
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

The envelope's `next_actions` will pass `data.seq_at_creation` as the `read_live_feed` sequence cursor.
