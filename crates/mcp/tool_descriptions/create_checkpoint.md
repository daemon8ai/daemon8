## Purpose

Bookmark a moment in the observation stream. Used to ask "what happened since this point" — typically right before applying a change you might want to roll back through, or before a verification step.

Checkpoint ids and returned observations are runtime signals. They explain what changed, but the agent still has to interpret the signal before recording a durable conclusion.

## When

Inside an active debug session, immediately before any action whose effects you'll want to compare against ("before applying patch X", "before re-running test"). Pair with read_live_feed(since_checkpoint=...) afterward to see only what changed.

## Prereq

A connected project-scope MCP session (`data.connection.mode == "project"`) and an active debug session. Call `daemon8_connect` with a project path first, then start_debug_session. Checkpoints cannot exist outside a session.

## Args
  - description: optional one-line note. Recommended — future you will want to know why this checkpoint mattered.

## Returns
Common envelope with `code="checkpoint_created"`, `data.checkpoint_id`, `data.debug_session_id`, `data.seq_at_creation`, and `data.created_at`.

## Errors
  - no_active_debug_session: no session is open; `next_actions[].tool` points to start_debug_session.
  - create_checkpoint_failed: db write failed. hint: check daemon logs.

## Next

Do the thing you were about to do (apply patch, run test, ask the user to reproduce), then call read_live_feed(since_checkpoint=<seq_at_creation>) to see only what changed. Record durable conclusions through `resolve_debug_session` after interpreting observations; checkpoint/feed rows are signals only.
