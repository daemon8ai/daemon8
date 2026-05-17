## Purpose

Close the active debug session WITHOUT a fix. Use when investigation stalls, scope changes, or the issue self-resolves without a learnable fix.

## When

You're stopping investigation but you don't have a captured fix worth indexing for future retrieval. If you DO have a fix (root cause + diff + commands), call resolve_debug_session instead — it produces a richer, retrievable SessionSummary.

## Prereq

A connected project-scope MCP session (`data.connection.mode == "project"`) and an active debug session. Call `daemon8_connect` with a project path first; call start_debug_session if no debug session is active.

## Args
  - outcome: optional string. Defaults to "abandoned". Allowed: "abandoned", "in_progress" (if you intend to resume later in a new session). Do NOT use "resolved" here — use resolve_debug_session for that.

## Returns
Common envelope with `code="debug_session_ended"`, `data.debug_session_id`, and `data.summary_memory_id`.

## Errors
  - no_active_debug_session: nothing to end. This is usually safe to ignore.
  - debug_session_unavailable: see start_debug_session.

## Next

Start a fresh debug session for the next investigation, or call list_debug_sessions to review recent sessions.
