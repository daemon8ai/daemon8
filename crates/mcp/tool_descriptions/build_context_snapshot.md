## Purpose

Build a faceted snapshot of provider conversation history. Decomposes Claude, Codex, and Gemini transcripts into markdown files: user messages, assistant messages, tool activity, file changes, log activity, and per-turn summary.

## When

When the user asks for or accepts conversation/history recovery: prior work, recent activity, a project review, catch-up, continuity, or another provider's activity. Do **not** call this automatically after connect. When recall is requested, omit `facets` for a full recent cross-provider review unless the user explicitly asks for a narrow summary. Use `since="conversation_start"` only when the user explicitly asks for full history.

## Prereq

Project-scoped connection (`data.connection.mode == "project"`) with at least one discoverable or linked transcript. If none found, use `link_conversation`.

## Args
  - since: optional string. Time scope: `"duration:1440"` (default, last 24 hours), `"conversation_start"` (full transcript), `"checkpoint"` (since active debug checkpoint), or `"duration:N"` (last N minutes). Use `"duration:30"` for a quick recent-activity check.
  - facets: optional string array. Which facets to build. Valid values: `user_messages`, `assistant_messages`, `tool_activity`, `file_changes`, `log_activity`, `summary`. Omit for all facets when the user asks for recall/review/catch-up. Use `["summary"]` only when the user explicitly asks for a brief or narrow summary.
  - providers: optional string array. Filter to specific providers (`claude`, `codex`, `gemini`). Omit for all discovered.

## Returns
Common envelope with `code="snapshot_built"`, `data.snapshot_path` (absolute path to the unique snapshot run directory), `data.facets` (map of facet name to path/bytes/entry_count), `data.sources_read`, `data.time_range`. Snapshot run directories are retained for roughly 24 hours and then purged by daemon cleanup.

Default recall is bounded and hygiene-filtered: instruction blocks (system prompts, CLAUDE.md content, `<system-reminder>` injections) are hidden from user-message facets; text entries are truncated at 8KB; each facet caps at 200 entries and 128KB total. File changes are derived from Edit/Write/apply_patch tool calls across all providers, so Codex and Gemini sessions produce useful file-change facets even without native FileChange events. `since="conversation_start"` provides full history but still hides instruction blocks.

## Errors
  - no_scope_root: connected but no project scope root available. Reconnect with a project path.
  - no_transcript_sources: no provider transcripts found or linked. `next_actions` points to `link_conversation`.
  - invalid_since_param: `since` value is not one of the three valid forms.
  - invalid_facet: a `facets` entry is not one of the six valid facet names.
  - no_active_checkpoint: `since="checkpoint"` used without an active debug session or without a checkpoint in the session.
  - snapshot_build_failed: output directory could not be created or a facet file could not be written.

## Next

Read facet files with your file-reading tool:
- Read across the generated facets and present a cohesive review -- **do not dump raw markdown**.
- `file-changes.md` for what changed, `tool-activity.md` for tools used, `user-messages.md` for original requests.
- No sources found → call `link_conversation`.
- Empty facets → verify time scope covers the period of interest (`"conversation_start"` for everything).
- For runtime state alongside history → call `read_live_feed`.
