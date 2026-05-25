## Purpose

Link a conversation transcript from another AI provider to the current project session. Linked transcripts become available to `build_context_snapshot`.

## When

Use after `daemon8_connect` in project scope when daemon8 reports missing/unlinked transcript sources, or when the user points to a specific provider/session that should be included in a requested recall. Do not use this as an automatic recall step when daemon8 already discovers provider conversations for the project.

## Prereq

Project-scoped connection via `daemon8_connect`.

## Args
  - provider: required string. The AI provider id ("claude", "codex", "gemini") or alias ("claude-code", "codex-cli").
  - project_path: optional string. Absolute path to the project root for transcript discovery. Searches for the most recent transcript in the selected lookback window.
  - transcript_path: optional string. Absolute path to a specific transcript file to link directly.
  - conversation_lookback_hours: optional discovery lookback when using project_path. When omitted, daemon8 includes the current project workday starting at the earliest matching project conversation modified today, falling back to local midnight.

One of `project_path` or `transcript_path` required. Both given → `transcript_path` takes precedence. When linking a `daemon8_connect` candidate, pass its `transcript_path` directly; otherwise repeat the same `conversation_lookback_hours`.

## Returns
Common envelope with linked transcript metadata: provider, path, scope_root, linked_at. Idempotent -- relinking returns the existing link.

## Errors
  - invalid_provider: unrecognized provider id or alias.
  - no_scope_root: connection has no project scope root.
  - transcript_unreadable: transcript_path cannot be read, is not a JSONL file, or contains no recognizable provider events.
  - no_transcripts_found: project_path discovery found no recent transcripts.
  - missing_params: neither project_path nor transcript_path provided.

## Next

Call `build_context_snapshot` to decompose linked conversation history into faceted markdown. For real-time project observations, use `read_live_feed` with `service` or `source` filters against configured file sources and live ingested observations.
