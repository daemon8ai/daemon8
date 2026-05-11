# Memory

Memories are long-lived insights persisted in the `memory` table — they survive observation cleanup. Use them to preserve fixes, decisions, error signatures, session summaries, and user-flagged notes.

## Kinds

- `pattern` — recurring code/architecture pattern worth recording
- `decision` — an architectural decision and its rationale
- `error_signature` — auto-promoted on first sight of any error observation; carries a `hash:<x>` tag matching `observation.error_hash`
- `session_summary` — written by `resolve_debug_session` (rich) or `end_debug_session` (thin)
- `user_flagged` — generic "remember this" from the user

## Tools

- `save_memory(content, kind?, tags?, source_observations?, project_slug?, session_id?, confidence?)` — persist
- `query_memory(text?, kinds?, tags?, project_slug?, limit?)` — search
- `forget_memory(id, confirm=true)` — delete (confirm gate is required)

## Auto-promotion: error signatures

Every error observation gets a normalized `error_hash`. First time we see a hash in a project, daemon8 writes a `memory` row of kind `error_signature` tagged `hash:<x>` and `seen:1`. Each subsequent occurrence updates the existing row, bumping `seen:N` and appending to `source_observations`.

To find what fixed a recurring error: `query_memory(tags=["hash:<x>"])` returns both the ErrorSignature memory and any SessionSummary memories whose `resolve_debug_session` listed this hash in `related_errors`.
