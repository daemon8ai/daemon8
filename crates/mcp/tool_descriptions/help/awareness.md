# Awareness

Use awareness tools to keep the agent oriented without creating a generic memory bank.

`awareness_status` is the read tool. It returns source posture plus a compact project-state manifest. Focused traversal is opt-in with `focus_path`; notes and evidence are omitted unless requested.

`awareness_sync` is the write tool. It captures durable semantic state changes: objectives, questions, hypotheses, facts, decisions, constraints, risks, and blockers.

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
7. `awareness_sync` if evidence verifies, refines, answers, or retires a node.
8. `resolve_debug_session` once the root cause is verified.

Do not sync every tool call. Sync only when the state of the work changes.
