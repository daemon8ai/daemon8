## Purpose

Bind this MCP session to an explicit scope and, in project mode, bind the active provider transcript when daemon8 can identify it.

## When

Call once at the start of an LLM session before any MCP tool except `daemon8_init`, `daemon8_status`, and `daemon8_help`. Retry after `daemon8_init` if the first response returns `setup_required` -- daemon8 has no project config yet and cannot observe without one. If the response returns `blocked/transcript_ambiguous`, retry with `transcript_path` set to one returned candidate -- the daemon found multiple plausible transcripts and needs explicit disambiguation.

## Prereq

None.

## Args

- `provider`: required calling provider name, e.g. `codex`, `claude`, `gemini`.
- `project_path`: required directory path to classify.
- `agent_name`: optional human-readable agent name.
- `transcript_path`: optional provider transcript path. Use this to resolve ambiguous active transcripts or bind an explicit transcript file.
- `conversation_lookback_hours`: optional conversation discovery lookback. When omitted, daemon8 includes the current project workday starting at the earliest matching project conversation modified today, falling back to local midnight.

## Returns

Common envelope with `status`, `code`, `message`, `data`, and structured `next_actions`. Project success includes `data.project_id` (the deterministic project slug from config), `data.source_count`, `data.transcript.status` as `bound` or `not_found`, and `data.related_projects` (list of sibling project IDs, if any are declared in config). Structured tags (`project:{id}`, `lang:`, `framework:`, `tool:`) are automatically applied to all observations from this project -- use `tags: ["project:{id}"]` in `read_live_feed` to scope queries.

Project connect responses include `data.conversations`:
  - `primary`: the bound transcript metadata (provider, path, session_id, status), or null if not found.
  - `available`: other provider transcripts discovered for this project scope and lookback window, sorted by recency. Use `link_conversation` to link any of these.
  - `linked`: transcripts linked via `link_conversation` (empty on fresh connect).

daemon8 detects workspace roots by scanning immediate child directories for ecosystem markers. A directory without root-level markers (no `.git`, `Cargo.toml`, etc.) can still be classified as a project if its children contain recognizable ecosystems. This means `setup_required` fires for workspaces too -- the init flow handles them the same as single projects.

When `data.source_count` is 0, the response includes a `requirements` field. The `requirements` field lists MANDATORY actions -- complete every listed action before proceeding with any other daemon8 tools. daemon8 cannot observe the project without sources. Open `.daemon8/config.md` and complete ALL steps in the markdown body to populate the config.

## Errors
  - invalid_provider: provider string is empty or not a recognized provider. Pass the calling agent's provider name (claude, codex, gemini, or opencode).
  - invalid_scope: the supplied project path does not exist, is not a directory, or cannot be resolved.

## Next

If `status=success` and no `requirements` field is present, continue normally. If `status=success` but `requirements` is present, you MUST complete every listed requirement FIRST. Do NOT proceed to debugging, reading live feed, writing observations, or any other tool until sources are populated and the user has confirmed the config. This is the enforcement gate -- it fires every session until the config is complete.

If `status=setup_required`, call `daemon8_init` with the supplied project path, then retry `daemon8_connect`. Setup is mandatory -- daemon8 has no visibility into the project without config. If `code=transcript_ambiguous`, retry with `transcript_path` set to one returned candidate. If `code=transcript_provider_mismatch`, retry with a transcript from the requested provider. If `code=transcript_scope_mismatch`, retry with a transcript for the connected project.

When `status=success` and `data.conversations.available` is non-empty, tell the user what daemon8 found -- this is a statement, not a question. Use `modified_at_ms` to compute recency: "daemon8 found 2 Claude sessions and 1 Codex session for this project. The most recent Claude session was active 45 minutes ago." Link relevant transcripts with `link_conversation` (use the candidate `transcript_path`; if rediscovering by `provider` + `project_path`, repeat any `conversation_lookback_hours` used during connect). Then present the recall path: "Call `build_context_snapshot` with `facets: ["summary"]` for a quick overview of what happened, or omit facets for the full decomposition."

After first setup or any fresh config confirmation, orient the user with a cohesive handoff -- do not skip this step. State how many sources daemon8 is watching and name them. Mention available conversations by provider. If `data.related_projects` is empty, ask whether sibling projects exist (frontend, backend, API, mobile app) because daemon8 links observations across them. Summarize what daemon8 can do from here: real-time log monitoring via `read_live_feed`, before/after comparisons via `create_checkpoint`, cross-session awareness via `build_context_snapshot`, and structured debugging via `start_debug_session`.
