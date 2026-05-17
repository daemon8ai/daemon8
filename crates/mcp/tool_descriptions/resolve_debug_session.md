## Purpose

Close the active debug session WITH a captured fix. Writes a typed SessionSummary for future error/session recall. THIS is the high-value operation — every field you fill in here makes the fix more retrievable later.

## When

You found the fix. The bug is understood, the change is applied, the test passes. Call this BEFORE moving on to the next task — context is hot now, and reconstructing it later is much harder.

## Prereq

A connected MCP session and an active debug session. Call `daemon8_connect` first; call start_debug_session if no debug session is active.

## Args
  - summary: REQUIRED. One paragraph in your own words: what was wrong, what you tried, what fixed it. The single field that future search will hit hardest. Be specific.
  - root_cause: optional one-sentence "the real reason this broke" — distinguishes the fix from the symptom. Worth filling in.
  - fix_diff: optional unified-diff or short patch string. Even partial is valuable.
  - commands_used: optional array of CLI commands that were part of the investigation or fix (e.g. ["pg_dump …", "rg 'TimeoutError'"]). Conversation/tool observation sources capture tool activity; this field is the curated subset that mattered.
  - related_errors: optional array of error_hash strings from read_live_feed that this fix resolves. Lets future occurrences of those errors surface this session.
  - tags: optional array of additional tags ("auth", "race-condition", "flaky-test"). These join automatic tags ("kind:debug_session_summary", project_slug) on the SessionSummary.

## Returns
Common envelope with `data.debug_session_id`, `data.summary_memory_id`, `data.project_slug`, `data.evidence_ref`, and `data.checkpoint_count`.

## Errors
  - no_active_debug_session: nothing to resolve; `next_actions[].tool` points to start_debug_session.
  - debug_session_unavailable: see start_debug_session.

## Next

Call `list_debug_sessions` if you need to confirm the session closed. Start a fresh debug session for unrelated follow-up work.
