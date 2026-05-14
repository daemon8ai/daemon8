## Purpose

Close the active debug session WITH a captured fix. Writes a rich SessionSummary memory that future retrieval can match against the next time a similar error appears. THIS is the high-value operation — every field you fill in here makes the fix more retrievable later.

## When

You found the fix. The bug is understood, the change is applied, the test passes. Call this BEFORE moving on to the next task — context is hot now, and reconstructing it later is much harder.

## Prereq

A debug session must be active. Call start_debug_session first if not.

## Args
  - summary: REQUIRED. One paragraph in your own words: what was wrong, what you tried, what fixed it. The single field that future search will hit hardest. Be specific.
  - root_cause: optional one-sentence "the real reason this broke" — distinguishes the fix from the symptom. Worth filling in.
  - fix_diff: optional unified-diff or short patch string. Even partial is valuable.
  - commands_used: optional array of CLI commands that were part of the investigation or fix (e.g. ["pg_dump …", "rg 'TimeoutError'"]). The hook handler captured these as ToolCall observations; this field is the curated subset that mattered.
  - related_errors: optional array of error_hash strings from query_observations that this fix resolves. Lets future occurrences of those errors surface this session.
  - tags: optional array of additional tags ("auth", "race-condition", "flaky-test"). These join automatic tags ("kind:debug_session_summary", project_slug) on the memory row.

## Returns
  result.debug_session_id: id of the session that was just resolved.
  result.summary_memory_id: id of the internal typed SessionSummary record written for future error/session recall.
  result.checkpoint_count: number of checkpoints rolled into the summary's source_observations.
  daemon8.active_debug_session: null after this call.

## Errors
  - no_active_debug_session: nothing to resolve. hint: call start_debug_session first; fix.tool: start_debug_session.
  - debug_session_unavailable: see start_debug_session.

## Next

list_debug_sessions confirms the session closed. Then start_debug_session for the next investigation.
