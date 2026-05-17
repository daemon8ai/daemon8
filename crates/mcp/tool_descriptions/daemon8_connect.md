## Purpose

Bind this MCP session to an explicit alpha scope: project, general, or invalid.

## When

Call once at the start of an LLM session before normal project-aware tools. Retry after `daemon8_init` if the first response returns `setup_required`.

## Prereq

None.

## Args

- `provider`: required calling provider name, e.g. `codex`, `claude`, `gemini`.
- `project_path`: required directory path to classify.
- `agent_name`: optional human-readable agent name.
- `transcript_path`: optional provider transcript path.

## Returns

Common envelope with `status`, `code`, `message`, `data`, and structured `next_actions`.

## Next

If `status=success`, continue. If `status=setup_required`, call `daemon8_init` with the supplied project path, then retry `daemon8_connect`.
