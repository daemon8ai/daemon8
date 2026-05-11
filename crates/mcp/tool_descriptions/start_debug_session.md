## Purpose

Open a daemon8 debug investigation. Required before create_checkpoint or resolve_debug_session can be used.

## When

At the start of any non-trivial debugging task. Errors observed, tests failing, "this isn't behaving" -- open a session, then work inside it. The session is the persistent artifact future-you (or another agent) will retrieve when a similar issue resurfaces.

## Prereq

No other debug session is currently active IN THIS MCP SESSION. Other agents/connections can each have their own active session. If one is active here, end_debug_session or resolve_debug_session must close it first.

## Args
  - agent_id: REQUIRED string. Agent identity in format :host/tool+role> (e.g. :mbp/claude+plan-agent>). Identifies who is running this investigation.
  - project: optional string. Defaults to "unknown" if not provided. Use the project slug (e.g. "daemon8", "rcn-scheduler") so future search can scope.
  - description: optional one-line summary of what you're investigating ("login form 500 after password reset").
  - feature: optional string. The feature being investigated (e.g. "auth", "search"). Other agents can discover overlapping work via list_debug_sessions(feature="auth").

## Returns
  result.debug_session_id: opaque id; pass to create_checkpoint, resolve, end.
  result.started_at: integer ns since epoch.
  daemon8.active_debug_session: the session you just opened.

## Errors
  - invalid_agent_id: agent_id does not match format :host/tool+role>. hint: use the convention.
  - already_active_debug_session: another session is open IN THIS MCP SESSION. hint: call end_debug_session(outcome="abandoned") or resolve_debug_session first; fix.tool: end_debug_session.
  - debug_session_unavailable: daemon was started without a debug-session store. hint: ensure setup_apply has run; fix.tool: setup_apply.

## Next

create_checkpoint right before any action you may want to roll back through, then query_observations(since_checkpoint=...) after.
