## Purpose

Close the active debug session WITHOUT a fix. Use when investigation stalls, scope changes, or the issue self-resolves without a learnable fix.

## When

You're stopping investigation but you don't have a captured fix worth indexing for future retrieval. If you DO have a fix (root cause + diff + commands), call resolve_debug_session instead — it produces a richer, retrievable SessionSummary memory.

## Prereq

A debug session must be active. Call start_debug_session first if not.

## Args
  - outcome: optional string. Defaults to "abandoned". Allowed: "abandoned", "in_progress" (if you intend to resume later in a new session). Do NOT use "resolved" here — use resolve_debug_session for that.

## Returns
  result.debug_session_id: id of the session that was just closed.
  result.summary_memory_id: id of the thin SessionSummary memory written for this session (always written, even on abandon, so the session never silently disappears).
  daemon8.active_debug_session: null after this call.

## Errors
  - no_active_debug_session: nothing to end. hint: this is usually safe to ignore; fix.tool: null.
  - debug_session_unavailable: see start_debug_session.

## Next

start_debug_session for a fresh investigation, or query_memory(kinds=["session_summary"]) to review past sessions.
