# daemon8 help index

Available topics (call `daemon8_help(topic="<name>")`):

- `debug_session` — protocol for opening/closing debugging investigations
- `checkpoint` — bookmarking moments in the observation stream
- `setup` — first-time configuration and provider enrollment
- `hooks` — managing CLI provider hooks (Claude Code, Codex)
- `lens` — buffering matching observations between queries
- `memory` — persisting long-lived insights across sessions
- `observations` — querying, filtering, subscribing to runtime telemetry
- `envelope` — the standard `{result, daemon8, error}` response shape

Call any topic name above. Unknown topics fall back to this index.
