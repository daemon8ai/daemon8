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

Common envelope with `status`, `code`, `message`, `data`, and structured `next_actions`. Project success includes `data.transcript.status` as `bound` or `not_found`.

## Next

If `status=success`, continue. If `status=setup_required`, call `daemon8_init` with the supplied project path, then retry `daemon8_connect`. If `code=transcript_provider_mismatch`, retry with a transcript from the requested provider. If `code=transcript_scope_mismatch`, retry with a transcript for the connected project.
