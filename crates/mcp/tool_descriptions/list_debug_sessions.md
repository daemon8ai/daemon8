## Purpose

Enumerate debug sessions, optionally filtered by status or feature. Used to resume an abandoned session, discover overlapping work across agents, audit recent investigations, or find a session whose summary you want to read.

## When

Looking for "what was I working on yesterday", "which sessions are still open", "is anyone else investigating the 'auth' feature", or "show me recent resolved fixes for project X".

## Prereq

A connected MCP session in project or general mode. Call `daemon8_connect` first.

## Args
  - status: optional string. One of "active", "completed", "abandoned". Omit for all statuses.
  - feature: optional string. Filter to sessions investigating this feature (e.g. "auth", "search"). Used by agents to discover overlapping work.

## Returns
Common envelope with `code="debug_sessions_listed"`, `data.sessions` ordered most-recent-first, and `data.count` for the filtered count.

## Errors
  - debug_session_unavailable: see start_debug_session.

## Next

Use the returned session metadata to decide whether to resume context, open a new debug session, or inspect related observations. start_debug_session if you find overlapping work and want to coordinate.
