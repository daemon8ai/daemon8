## Purpose

Bookmark a moment in the observation stream. Used to ask "what happened since this point" — typically right before applying a change you might want to roll back through, or before a verification step.

## When

Inside an active debug session, immediately before any action whose effects you'll want to compare against ("before applying patch X", "before re-running test"). Pair with query_observations(since_checkpoint=...) afterward to see only what changed.

## Prereq

An active debug session (call start_debug_session first). Checkpoints cannot exist outside a session.

## Args
  - description: optional one-line note. Recommended — future you will want to know why this checkpoint mattered.

## Returns
  result.checkpoint_id: opaque id; pass to query_observations(since_checkpoint=...).
  result.debug_session_id: parent session id.
  result.seq_at_creation: integer observation seq at the moment of creation.
  result.created_at: integer ns since epoch.

## Errors
  - no_active_debug_session: no session is open. hint: call start_debug_session first; fix.tool: start_debug_session.
  - create_checkpoint failed: db write failed. hint: check daemon logs.

## Next

do the thing you were about to do (apply patch, run test, etc.), then query_observations(since_checkpoint=<this id>) to see only what came after.
