## Purpose

Link an additional conversation transcript from another AI provider to the current project session. Linked transcripts become available through `read_live_feed` queries and `build_context_snapshot` faceted decomposition.

## When

You know another AI agent (Claude, Codex, Gemini) has worked on this project and you want its conversation history available as observations. Use after `daemon8_connect` in project scope.

## Prereq

Project-scoped connection via `daemon8_connect`.

## Args
  - provider: required string. The AI provider id ("claude", "codex", "gemini") or alias ("claude-code", "codex-cli").
  - project_path: optional string. Absolute path to the project root for transcript discovery. Searches for the most recent transcript in the selected lookback window.
  - transcript_path: optional string. Absolute path to a specific transcript file to link directly.
  - conversation_lookback_hours: optional discovery lookback when using project_path. When omitted, daemon8 includes the current project workday starting at the earliest matching project conversation modified today, falling back to local midnight.

At least one of `project_path` or `transcript_path` must be provided. If both are given, `transcript_path` takes precedence for the file, and `project_path` is recorded as the scope root. When linking a candidate returned by `daemon8_connect`, prefer passing that candidate's `transcript_path`; otherwise repeat the same `conversation_lookback_hours` used during connect.

## Returns
Common envelope with `data` containing the linked transcript metadata: provider, path, scope_root, linked_at. Linking is idempotent: linking the same transcript path again returns the existing link without duplicating. Linked transcripts use `linked.transcript.{provider}.{hash}` source IDs to avoid cursor collisions with the primary transcript.

## Errors
  - invalid_provider: unrecognized provider id or alias.
  - no_scope_root: connection has no project scope root.
  - transcript_unreadable: transcript_path cannot be read, is not a JSONL file, or contains no recognizable provider events.
  - no_transcripts_found: project_path discovery found no recent transcripts.
  - missing_params: neither project_path nor transcript_path provided.

## Next

Call `build_context_snapshot` to decompose linked conversation history into faceted markdown. For real-time project observations, use `read_live_feed` with `service` or `source` filters.
