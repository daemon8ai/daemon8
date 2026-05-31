## Purpose

Bind this MCP session to a project or general scope. In project mode, also bind the active provider transcript.

## When

Call **exactly once** at the start of an LLM session before any MCP tool except `daemon8_init`, `daemon8_status`, and `daemon8_help`. If the response returns `status=success`, do not call again. Retry **only** after `daemon8_init` if the first response returns `setup_required` -- daemon8 has no project config yet and cannot observe without one. If the response returns `blocked/transcript_ambiguous`, retry with `transcript_path` set to one returned candidate -- the daemon found multiple plausible transcripts and needs explicit disambiguation.

## Prereq

None.

## Args

- `provider`: required calling provider name, e.g. `codex`, `claude`, `gemini`.
- `project_path`: required directory path to classify.
- `agent_name`: optional human-readable agent name.
- `transcript_path`: optional provider transcript path. Use this to resolve ambiguous active transcripts or bind an explicit transcript file.
- `conversation_lookback_hours`: optional discovery lookback. Default: last 24 hours.

## Returns

Common envelope with `status`, `code`, `message`, `data`, and structured `next_actions`. Project success includes `data.project_id` (the deterministic project slug from config), `data.source_count`, `data.transcript.status` as `bound` or `not_found`, and `data.related_projects` (list of sibling project IDs, if any are declared in config). Project and stack tags are derived from config for file-source observations; MCP-written observations get project session provenance. Use `tags: ["project:{id}"]` in `read_live_feed` to scope project-tagged observations.

Project connect responses include `data.conversations`:
  - `primary`: the bound transcript metadata (provider, path, session_id, status), or null if not found.
  - `available`: other provider transcripts discovered for this project scope and lookback window, sorted by recency. Use `link_conversation` to link any of these.
  - `linked`: transcripts linked via `link_conversation` (empty on fresh connect).

The `requirements` field lists **mandatory** actions when present (empty sources, generated config body still in place, etc.). Complete every requirement before using other tools.

Project connect includes `data.config_body_status` (`"project_notes"` or `"generated_setup_instructions_present"`) and `data.config_body_action` (present only when body needs replacement). When `config_body_action` is `"replace_with_project_notes"`, replace the markdown body after frontmatter with concise project-specific notes (dev commands, service startup, build outputs, environment assumptions, gotchas). Do not repeat log paths or sources already in frontmatter.

## Errors
  - invalid_provider: provider string is empty or not a recognized provider. Pass the calling agent's provider name (claude, codex, gemini, or opencode).
  - invalid_scope: the supplied project path does not exist, is not a directory, or cannot be resolved.

## Next

After `status=success` with no `requirements`: **stop calling daemon8_connect.** Connection is complete. Proceed with `read_live_feed`, `create_checkpoint`, or `start_debug_session`. Do not call daemon8_connect again in this session.

After `status=success` with `requirements`: complete **every** listed requirement before any other tool. Do not retry daemon8_connect -- the requirements are the next step.

Retry daemon8_connect **only** when the response status is not `success`:
- `status=setup_required` → call `daemon8_init(project_path)`, then retry `daemon8_connect`.
- `code=transcript_ambiguous` → retry with `transcript_path` set to one returned candidate.
- `code=transcript_provider_mismatch` → retry with a transcript from the requested provider.
- `code=transcript_scope_mismatch` → retry with a transcript for the connected project.

**Conversation discovery**: when `data.conversations.available` is non-empty, state what daemon8 found -- provider count, most recent activity (compute from `modified_at_ms`). Link relevant transcripts with `link_conversation`. Present: "Call `build_context_snapshot` with `facets: ["summary"]` for a quick recent overview, or omit facets for full recent decomposition. Use `since: "conversation_start"` only for full history."

**Post-setup orientation**: after first setup or fresh config confirmation, state how many sources daemon8 watches and name them. Mention available conversations by provider. If `data.related_projects` is empty, ask about sibling projects. Summarize capabilities: `read_live_feed`, `create_checkpoint`, `build_context_snapshot`, `start_debug_session`.
