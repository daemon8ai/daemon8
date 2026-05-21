## Purpose

Bind this MCP session to an explicit alpha scope and, in project mode, bind the active provider transcript when daemon8 can identify it.

## When

Call once at the start of an LLM session before any MCP tool except `daemon8_init`, `daemon8_status`, and `daemon8_help`. Retry after `daemon8_init` if the first response returns `setup_required`. If the response returns `blocked/transcript_ambiguous`, retry with `transcript_path` set to one returned candidate.

## Prereq

None.

## Args

- `provider`: required calling provider name, e.g. `codex`, `claude`, `gemini`.
- `project_path`: required directory path to classify.
- `agent_name`: optional human-readable agent name.
- `transcript_path`: optional provider transcript path. Use this to resolve ambiguous active transcripts or bind an explicit transcript file.

## Returns

Common envelope with `status`, `code`, `message`, `data`, and structured `next_actions`. Project success includes `data.project_id` (the deterministic project slug from config), `data.source_count`, `data.transcript.status` as `bound` or `not_found`, and `data.related_projects` (list of sibling project IDs, if any are declared in config). Structured tags (`project:{id}`, `lang:`, `framework:`, `tool:`) are automatically applied to all observations from this project -- use `tags: ["project:{id}"]` in `read_live_feed` to scope queries.

Project connect responses include `data.conversations`:
  - `primary`: the bound transcript metadata (provider, path, session_id, status), or null if not found.
  - `available`: other provider transcripts discovered for this project scope, sorted by recency. Use `link_conversation` to link any of these.
  - `linked`: transcripts linked via `link_conversation` (empty on fresh connect).

daemon8 detects workspace roots by scanning immediate child directories for ecosystem markers. A directory without root-level markers (no `.git`, `Cargo.toml`, etc.) can still be classified as a project if its children contain recognizable ecosystems. This means `setup_required` fires for workspaces too -- the init flow handles them the same as single projects.

When `data.source_count` is 0, the response includes a `requirements` field. The `requirements` field lists MANDATORY actions -- every listed action MUST be completed before proceeding with any other daemon8 tools. daemon8 cannot observe the project without sources. Open `.daemon8/config.md` and complete ALL steps in the markdown body to populate the config.

## Next

If `status=success` and no `requirements` field is present, continue normally. If `status=success` but `requirements` is present, you MUST complete every listed requirement FIRST. Do NOT proceed to debugging, reading live feed, writing observations, or any other tool until sources are populated and the user has confirmed the config. This is the enforcement gate -- it fires every session until the config is complete.

If `status=setup_required`, call `daemon8_init` with the supplied project path, then retry `daemon8_connect`. If the user declines setup, call `daemon8_init` with `ignore=true` to suppress future prompts. If `status=blocked` and `code=project_ignored`, this project was explicitly ignored -- stop using daemon8 tools for this session, no connection was established. If `code=transcript_provider_mismatch`, retry with a transcript from the requested provider. If `code=transcript_scope_mismatch`, retry with a transcript for the connected project.
