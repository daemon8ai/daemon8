## Purpose

Link an additional conversation transcript from another AI provider to the current project session. Linked transcripts are ingested alongside the primary transcript on each source trigger.

## When

You know another AI agent (Claude, Codex, Gemini) has worked on this project and you want its conversation history available as observations. Use after `daemon8_connect` in project scope.

## Prereq

Project-scoped connection via `daemon8_connect`.

## Args
  - provider: required string. The AI provider id ("claude", "codex", "gemini") or alias ("claude-code", "codex-cli").
  - project_path: optional string. Absolute path to the project root for transcript discovery. Uses `project_conversation_files()` to find the most recent transcript.
  - transcript_path: optional string. Absolute path to a specific transcript file to link directly.

At least one of `project_path` or `transcript_path` must be provided. If both are given, `transcript_path` takes precedence for the file, and `project_path` is recorded as the scope root.

## Returns
Common envelope with `data` containing the linked transcript metadata: provider, path, scope_root, linked_at.

## Errors
  - invalid_provider: unrecognized provider id or alias.
  - no_scope_root: connection has no project scope root.
  - transcript_unreadable: transcript_path cannot be read.
  - transcript_not_file: transcript_path exists but is not a file.
  - no_transcripts_found: project_path discovery found no recent transcripts.
  - missing_params: neither project_path nor transcript_path provided.

## Notes

Linking is idempotent: linking the same transcript path again returns the existing link without duplicating. Linked transcripts use `linked.transcript.{provider}.{hash}` source IDs to avoid cursor collisions with the primary transcript.

## Next

`read_live_feed` to see ingested observations from linked transcripts.
