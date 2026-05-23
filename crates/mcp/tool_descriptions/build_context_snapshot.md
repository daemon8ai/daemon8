## Purpose

Build a faceted snapshot of provider conversation history for this project. Decomposes Claude, Codex, and Gemini transcripts into independently readable markdown files: user messages, assistant messages, tool activity (condensed), file changes, log activity, and a per-turn summary.

## When

When catching up on other sessions ("what happened while I was away?"), reviewing what another agent did, or building context before a complex task. Call once to build the snapshot, then read whichever facet files are relevant.

When the user says "catch me up", "what happened while I was away?", or asks about another agent's work, this is the tool. Build the snapshot, read `summary.md` first, then present a conversational overview. The user can then request specific facets for detail.

## Prereq

A connected project-scope MCP session (`data.connection.mode == "project"`). At least one provider transcript must be discoverable or linked. Call `daemon8_connect` with a project path first. If no transcripts are found, use `link_conversation` to bind one.

## Args
  - since: optional string. Time scope: `"conversation_start"` (default, full transcript), `"checkpoint"` (since active debug checkpoint), or `"duration:N"` (last N minutes). Use `"duration:30"` for a quick recent-activity check.
  - facets: optional string array. Which facets to build. Valid values: `user_messages`, `assistant_messages`, `tool_activity`, `file_changes`, `log_activity`, `summary`. Omit for all facets. Use `["summary"]` for a quick orientation.
  - providers: optional string array. Filter to specific providers (`claude`, `codex`, `gemini`). Omit for all discovered.

## Returns
Common envelope with `code="snapshot_built"`, `data.snapshot_path` (absolute path to snapshot directory), `data.facets` (map of facet name to path/bytes/entry_count), `data.sources_read`, `data.time_range`.

## Errors
  - no_scope_root: connected but no project scope root available. Reconnect with a project path.
  - no_transcript_sources: no provider transcripts found or linked. `next_actions` points to `link_conversation`.
  - invalid_since_param: `since` value is not one of the three valid forms.
  - invalid_facet: a `facets` entry is not one of the six valid facet names.
  - no_active_checkpoint: `since="checkpoint"` used without an active debug session or without a checkpoint in the session.
  - snapshot_build_failed: output directory could not be created or a facet file could not be written.

## Next

Read the facet files with your file-reading tool. Start with `summary.md` for orientation, drill into `user-messages.md` or `tool-activity.md` for detail.

For a recall flow: read `summary.md` and present a conversational overview to the user. If they want detail on what changed, read `file-changes.md`. For what tools were used, read `tool-activity.md`. For the user's original requests, read `user-messages.md`. Present each facet conversationally -- do not dump raw markdown.

If no sources were found: call `link_conversation` to bind a transcript manually.
If facets are empty: verify the time scope covers the period of interest -- use `"conversation_start"` to see everything.
For runtime state alongside conversation history: call `read_live_feed`.
