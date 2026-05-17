# Response envelope

Every daemon8 MCP tool response has the same shape:

```json
{
  "result": <tool-specific payload>,
  "daemon8": {
    "active_debug_session": {"id": "...", "project_slug": "...", "started_at_ns": ...} | absent,
    "next_actions": ["create_checkpoint", "read_live_feed"] | absent,
    "hint": "free-form one-liner about what to do next" | absent
  },
  "error": {"code": "...", "message": "...", "hint": "..." | absent, "fix": {"tool": "..."} | absent} | absent
}
```

## Error first

If `error` is present, `result` will be `null`. The LLM should:
1. Surface `error.message` (and `error.hint` if present) to the user.
2. If `error.fix.tool` is present, that's the tool the LLM should call to remediate.

## Active session echo

`daemon8.active_debug_session` is present on every response when a session is open. This is a visual reminder that work is being captured against a session — useful for the LLM to see "yes, we're still in the cookie-mismatch investigation" before deciding what to do next.

## Steering hints

`daemon8.next_actions` is an opinionated suggestion — not all responses include one. When it's there, the listed tool name is the natural next call given current state. The LLM may always override based on user intent.

## Connect-first flow

Alpha tools use the envelope to steer the next step. When a session is not connected to a project or general scope, tools return a connect/setup response with a short reason and the next tool to call. Do not keep repeating the same hint after connection succeeds.
