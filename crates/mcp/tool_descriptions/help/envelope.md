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
- `blocked`: user intent or explicit overwrite/confirm input is needed.
- `error`: surface `message` and `why`.

## Connect-first flow

`daemon8_connect` binds the MCP session to project or general mode. `daemon8_status` is the diagnostic exception and may be called before connect.
