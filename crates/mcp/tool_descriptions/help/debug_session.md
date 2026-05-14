# Debug session protocol

A debug session is the lifecycle that bookends a debugging investigation. Every observation captured during the session is linked to it; on resolution, a rich SessionSummary memory is written so future-you (or another agent) can find this fix when the same error appears again.

## Lifecycle

1. `start_debug_session(agent_id, project?, description?, feature?)` — opens a session. `agent_id` is required in format `:host/tool+role>` (e.g. `:mbp/claude+plan-agent>`). Errors if one is already active in THIS MCP session. Other agents can each have their own active session simultaneously. Returns `debug_session_id`.
2. `create_checkpoint(description)` — bookmark a moment. Required prereq: an active session. Returns `checkpoint_id`. Pair with `query_observations(since_checkpoint=<id>)` to see only what came after.
3. `resolve_debug_session(summary, root_cause?, fix_diff?, commands_used?, related_errors?, tags?)` — close on success with rich capture. Writes a SessionSummary memory of kind `session_summary`. Every optional field you fill in increases retrievability when a similar error recurs.
4. `end_debug_session(outcome)` — close without a fix (abandoned). Writes a thin SessionSummary so the row never silently disappears.

## Multi-agent coordination

Multiple agents (Claude Code, Codex, Gemini — in separate MCP sessions) can each open their own debug session on the same project concurrently. No global single-active constraint.

- Declare a feature: `start_debug_session(agent_id=":mbp/codex+build-agent>", feature="auth")`
- Discover overlapping work: `list_debug_sessions(status="active", feature="auth")` — find other agents investigating the same feature
- Each agent's observations are stamped with their own session ID. Observations from different agents never collide.

## Auto-end safety net

If a session has no activity for 4 hours, daemon8 marks it `abandoned` and writes a thin SessionSummary automatically. Configurable via `[debug_session].inactivity_auto_end_secs` in `.daemon8.toml`. Observations from auto-ended sessions then become eligible for the 24-hour reaper.

## Source observation linkage

While a session is active, observations ingested through that session's MCP connection get `debug_session_id` and (if checkpointed) `checkpoint_id` stamped by the MCP tool before they reach the store. The cleanup task respects this: observations linked to active sessions are NEVER reaped, no matter how old.

## Agent identity

Every session records an `agent_id` in format `:host/tool+role>`. This convention enables:
- Attribution: which agent and machine did the investigation
- `list_debug_sessions` surfaces agent identity for coordination
- Future retrieval: find sessions by agent or role

## Retrieval

Find past sessions:
- `list_debug_sessions(status=?)` — by lifecycle status
- `list_debug_sessions(feature="auth")` — discover overlapping work
- `list_debug_sessions(project=<slug>)` — review recent sessions by project
- `resolve_debug_session(related_errors=[...])` — link a verified fix to error signatures
