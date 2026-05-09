# Debug session protocol

A debug session is the lifecycle that bookends a debugging investigation. Every observation captured during the session is linked to it; on resolution, a rich SessionSummary memory is written so future-you (or another agent) can find this fix when the same error appears again.

## Lifecycle

1. `start_debug_session(project, description)` — opens a session. Errors if one is already active. Returns `debug_session_id`.
2. `create_checkpoint(description)` — bookmark a moment. Required prereq: an active session. Returns `checkpoint_id`. Pair with `query_observations(since_checkpoint=<id>)` to see only what came after.
3. `resolve_debug_session(summary, root_cause?, fix_diff?, commands_used?, related_errors?, tags?)` — close on success with rich capture. Writes a SessionSummary memory of kind `session_summary`. Every optional field you fill in increases retrievability when a similar error recurs.
4. `end_debug_session(outcome)` — close without a fix (abandoned). Writes a thin SessionSummary so the row never silently disappears.

## Auto-end safety net

If a session has no activity for 4 hours, daemon8 marks it `abandoned` and writes a thin SessionSummary automatically. Configurable via `[debug_session].inactivity_auto_end_secs` in `.daemon8.toml`. Observations from auto-ended sessions then become eligible for the 24-hour reaper.

## Source observation linkage

While a session is active, every ingested observation gets `debug_session_id` and (if checkpointed) `checkpoint_id` stamped on it. The cleanup task respects this: observations linked to active sessions are NEVER reaped, no matter how old.

## Retrieval

Find past sessions:
- `list_debug_sessions(status=?)` — by lifecycle status
- `query_memory(kinds=["session_summary"], tags=["project:<slug>"])` — by project
- `query_memory(tags=["hash:<error_hash>"])` — find which fix resolved a given error signature
