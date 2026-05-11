## Purpose

Enumerate debug sessions, optionally filtered by status or feature. Used to resume an abandoned session, discover overlapping work across agents, audit recent investigations, or find a session whose summary you want to read.

## When

Looking for "what was I working on yesterday", "which sessions are still open", "is anyone else investigating the 'auth' feature", or "show me recent resolved fixes for project X".

## Prereq

None.

## Args
  - status: optional string. One of "active", "completed", "abandoned". Omit for all statuses.
  - feature: optional string. Filter to sessions investigating this feature (e.g. "auth", "search"). Used by agents to discover overlapping work.

## Returns
  result.sessions: array of {id, project_slug, agent_id, feature, description, status, outcome, started_at, ended_at, last_activity, summary_memory_id}, ordered most-recent-first.
  result.count: integer. Filtered count when feature or status is provided.

## Errors
  - debug_session_unavailable: see start_debug_session.

## Next

get_memory(id=<summary_memory_id>) to read the rich SessionSummary for any completed/abandoned session. start_debug_session if you find overlapping work and want to coordinate.
