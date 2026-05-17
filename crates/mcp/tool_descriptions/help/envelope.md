# Response envelope

Alpha MCP tools return one common shape:

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
- `setup_required`: call the next action, usually `daemon8_init`, then retry `daemon8_connect`.
- `blocked`: follow `next_actions` when present; otherwise adjust the request according to `why`/`message` or ask for explicit user input when required.
- `error`: surface `message` and `why`.

## Connect-first flow

`daemon8_connect` binds the MCP session to project or general mode. `daemon8_status` is the diagnostic pre-connect exception. `daemon8_init` is also allowed before connect when setup is required or the user explicitly asks to initialize a path. All other tools start with `daemon8_connect`.

Project connect may also bind the active provider transcript. If daemon8 finds multiple plausible transcripts, the response is `blocked/transcript_ambiguous` with candidate paths and a `daemon8_connect` retry action. If the caller supplies a transcript from the wrong provider or project, the response is `error/transcript_provider_mismatch` or `error/transcript_scope_mismatch`.
