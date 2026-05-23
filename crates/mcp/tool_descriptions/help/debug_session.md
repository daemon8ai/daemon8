# Debug session protocol

A debug session is the lifecycle that bookends a debugging investigation. Every observation captured during the session is linked to it; on resolution, a rich SessionSummary is written so future-you (or another agent) can find this fix when the same error appears again.

## Lifecycle

0. `daemon8_connect(provider, project_path)` — bind this MCP session to project mode before starting a debug session.
1. `start_debug_session(agent_id, project?, description?, feature?)` — opens a session. Errors if one is already active in this MCP session. Other agents can each have their own active session simultaneously. Returns `debug_session_id`. Follow with `create_checkpoint`.
2. `create_checkpoint(description)` — bookmark a moment. Required prereq: a connected project-mode session and an active debug session. Returns `checkpoint_id` plus `seq_at_creation`. Pair with `read_live_feed(since_checkpoint=<seq_at_creation>)` to see only what came after.
3. `resolve_debug_session(summary, root_cause?, fix_diff?, commands_used?, related_errors?, tags?)` — close on success with rich capture. Writes a SessionSummary of kind `session_summary`. Each optional field increases future retrievability.
4. `end_debug_session(outcome)` — close without a fix (abandoned). Writes a thin SessionSummary so the row never silently disappears.

## Multi-agent coordination

Multiple agents (Claude Code, Codex, Gemini — in separate MCP sessions) can each open their own debug session on the same project concurrently. No global single-active constraint.

- Declare a feature: `start_debug_session(agent_id=":mbp/codex+build-agent>", feature="auth")`
- Discover overlapping work: `list_debug_sessions(status="active", feature="auth")` — find other agents investigating the same feature
- Each agent's observations are stamped with their own session ID. Observations from different agents never collide.

## Auto-end safety net

No activity for 4 hours → daemon8 marks the session `abandoned` with a thin SessionSummary. Auto-ended observations become eligible for the 24-hour reaper.

## Source observation linkage

While a session is active, observations ingested through that session's MCP connection get `debug_session_id` and (if checkpointed) `checkpoint_id` stamped by the MCP tool before they reach the store. The cleanup task respects this: observations linked to active sessions are NEVER reaped, no matter how old.

## Agent identity

Every session records an `agent_id` in format `:host/tool+role>`. This convention enables:
- Attribution: which agent and machine did the investigation
- `list_debug_sessions` surfaces agent identity for coordination
- Future retrieval: find sessions by agent or role

## Retrieval

Standard loop: `daemon8_connect -> start_debug_session -> create_checkpoint -> change/repro/test -> read_live_feed(since_checkpoint=...) -> resolve_debug_session`.

Find past sessions:
- `list_debug_sessions(status=?)` — by lifecycle status
- `list_debug_sessions(feature="auth")` — discover overlapping work
- `resolve_debug_session(related_errors=[...])` — link a verified fix to error signatures
