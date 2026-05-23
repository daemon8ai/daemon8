## Purpose

Build a faceted snapshot of provider conversation history. Decomposes Claude, Codex, and Gemini transcripts into markdown files: user messages, assistant messages, tool activity, file changes, log activity, and per-turn summary.

## When

When catching up on other sessions, reviewing another agent's work, or building context before a complex task. **Trigger phrases**: "catch me up", "what happened while I was away?", "what did the other agent do?" -- build the snapshot, read `summary.md` first, present a conversational overview.

## Prereq

Project-scoped connection (`data.connection.mode == "project"`) with at least one discoverable or linked transcript. If none found, use `link_conversation`.

## Args
  - since: optional string. Time scope: `"conversation_start"` (default, full transcript), `"checkpoint"` (since active debug checkpoint), or `"duration:N"` (last N minutes). Use `"duration:30"` for a quick recent-activity check.
  - facets: optional string array. Which facets to build. Valid values: `user_messages`, `assistant_messages`, `tool_activity`, `file_changes`, `log_activity`, `summary`. Omit for all facets. Use `["summary"]` for a quick orientation.
  - providers: optional string array. Filter to specific providers (`claude`, `codex`, `gemini`). Omit for all discovered.

## Returns
Common envelope with `code="snapshot_built"`, `data.snapshot_path` (absolute path to the unique snapshot run directory), `data.facets` (map of facet name to path/bytes/entry_count), `data.sources_read`, `data.time_range`. Snapshot run directories are retained for roughly 24 hours and then purged by daemon cleanup.

## Errors
  - no_scope_root: connected but no project scope root available. Reconnect with a project path.
  - no_transcript_sources: no provider transcripts found or linked. `next_actions` points to `link_conversation`.
  - invalid_since_param: `since` value is not one of the three valid forms.
  - invalid_facet: a `facets` entry is not one of the six valid facet names.
  - no_active_checkpoint: `since="checkpoint"` used without an active debug session or without a checkpoint in the session.
  - snapshot_build_failed: output directory could not be created or a facet file could not be written.

## Next

Read facet files with your file-reading tool:
- Start with `summary.md` for orientation, present conversationally -- **do not dump raw markdown**.
- `file-changes.md` for what changed, `tool-activity.md` for tools used, `user-messages.md` for original requests.
- No sources found → call `link_conversation`.
- Empty facets → verify time scope covers the period of interest (`"conversation_start"` for everything).
- For runtime state alongside history → call `read_live_feed`.
