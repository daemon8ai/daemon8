# Checkpoints

A checkpoint is a bookmark within an active debug session. It records the observation `seq` at the moment of creation so subsequent queries can ask "what happened since this point" without juggling timestamps.

## Why use them

Right before any change you want to compare against:

```
daemon8_connect(provider="codex", project_path="/path/to/project")
start_debug_session(agent_id=":host/codex+agent>")
create_checkpoint(description="before applying retry patch")
# ... apply patch, run test ...
read_live_feed(since_checkpoint=<seq_at_creation from above>)
```

Returns only what arrived after the checkpoint -- the error/log delta.

## Constraints

- Requires project-mode connection + active debug session. Without project: `project_required`. Without session: `no_active_debug_session` with next_action to `start_debug_session`.
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
