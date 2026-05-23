# Response envelope

All MCP tools return one common shape:

```json
{
  "status": "success|error|connect_required|setup_required|blocked",
  "code": "machine_code",
  "message": "short user-facing summary",
  "why": "reason, present for non-success responses",
  "data": {},
  "hints": [],
  "next_actions": [{"tool": "daemon8_connect", "reason": "...", "params": {}}]
}
```

## Status first

Branch on `status`, then `code`.

- `success`: continue.
- `connect_required`: call `daemon8_connect`.
- `setup_required`: call `daemon8_init`, then retry `daemon8_connect`.
- `blocked`: follow `next_actions` when present; otherwise adjust the request according to `why`/`message`. Ask the user when `why`/`message` indicates a decision only the user can make (e.g., which file to target, which transcript to prefer).
- `error`: surface `message` and `why`.

## Connect-first flow

`daemon8_connect` binds the MCP session to project or general mode. `daemon8_status` and `daemon8_help` are diagnostic pre-connect exceptions. Call `daemon8_init` before connect when `daemon8_connect` returns `setup_required` or the user explicitly asks to initialize a path. All other tools start with `daemon8_connect`.

Connect returns flattened scope fields (`data.session_id`, `data.mode`, `data.requested_path`, `data.scope_root`). Project connect may bind the active provider transcript -- ambiguous transcripts return `blocked/transcript_ambiguous` with candidates.
