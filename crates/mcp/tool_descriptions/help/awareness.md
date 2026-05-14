# Awareness

Use awareness tools to keep the agent oriented through project-aware source,
context, and reasoning state.

`awareness_status` is the read tool. It returns three layers: source awareness, context awareness, and reasoning awareness. Focused traversal is opt-in with `focus_path`; notes and refs are omitted unless requested.

`awareness_sync` is the write tool when the awareness store is enabled. It captures durable semantic state changes: objectives, questions, hypotheses, facts, decisions, constraints, risks, and blockers.

Raw observations, checkpoints, log rows, browser/device events, and tool output are signals. Durable evidence comes from interpreted persistent records: resolved debug sessions, accepted research, plan changes, decisions, fixed bugs, lessons, business rules, source verifications, user confirmations, and librarian refs.

Source posture levels:

- `unknown` -- daemon8 has not checked project/source coverage for this session.
- `limited` -- daemon8 has only generic/global streams or no librarian topology for this project.
- `partial` -- daemon8 knows relevant sources exist, but at least one source is unavailable, stale, drifted, inaccessible, or missing a usable reference.
- `optimal` -- librarian-known sources for the project are accessible and daemon8 has usable references for the debug stream.

Cadence:

1. `start_debug_session`
2. `awareness_status`
3. `awareness_sync` if objective/questions/hypotheses need capture.
4. `create_checkpoint`
5. Run the action/test.
6. `query_observations(since_checkpoint=...)`
7. `awareness_sync` only if the interpreted conclusion verifies, refines, answers, or retires a node.
8. `resolve_debug_session` once the root cause is verified.

Do not sync every tool call. Sync only when the state of the work changes.
