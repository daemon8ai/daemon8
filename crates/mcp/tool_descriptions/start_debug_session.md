## Purpose

Open a daemon8 debug investigation. Required before create_checkpoint or resolve_debug_session can be used.

Opening a session alone does nothing. Follow immediately with `create_checkpoint`, then the change/repro, then `read_live_feed(since_checkpoint=...)`.

## When

At the start of any non-trivial debugging task. Errors observed, tests failing, "this isn't behaving" -- open a session, then work inside it. The session is the persistent artifact future-you (or another agent) will retrieve when a similar issue resurfaces.

Before opening, check for overlapping debug investigations: `list_debug_sessions(feature="...")`. Do not run conversation recall automatically. If the user asks for prior context, another provider's work, or a catch-up before debugging, call `build_context_snapshot` with no `facets` filter for the recent default window. Use `since="conversation_start"` only for explicit full-history requests.

## Prereq

A connected project-scope MCP session (`data.connection.mode == "project"`) and no other debug session currently active IN THIS MCP SESSION. Call `daemon8_connect` with a project path first. Other agents/connections can each have their own active session. If one is active here, end_debug_session or resolve_debug_session must close it first.

## Args
  - agent_id: REQUIRED string. Agent identity in format :host/tool+role> (e.g. :mbp/claude+plan-agent>). Identifies who is running this investigation.
  - project: optional string. Defaults to "unknown" if not provided. Use the project slug (e.g. "daemon8", "rcn-scheduler") so future search can scope.
  - description: optional one-line summary of what you're investigating ("login form 500 after password reset").
  - feature: optional string. The feature being investigated (e.g. "auth", "search"). Other agents can discover overlapping work via list_debug_sessions(feature="auth").

## Returns
Common envelope with `code="debug_session_started"`, `data.debug_session_id`, `data.started_at`, and `data.active_debug_session`.

## Errors
  - invalid_agent_id: agent_id does not match format :host/tool+role>. hint: use the convention.
  - already_active_debug_session: another session is open IN THIS MCP SESSION. `next_actions[].tool` points to the close/resolve step.
  - debug_session_unavailable: daemon was started without debug-session storage. hint: restart daemon8 with debug-session storage enabled.

## Next

Call `create_checkpoint` right before the action you want to compare. Then use `read_live_feed(since_checkpoint=...)` and interpret the runtime signal before resolving the session.
