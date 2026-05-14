# Awareness Posture

Before a debug session relies on checkpoint deltas, check `awareness_status`.

Levels:

- `unknown` -- daemon8 has not checked project/source coverage for this session.
- `limited` -- daemon8 has only generic/global streams or no librarian topology for this project.
- `partial` -- daemon8 knows relevant sources exist, but at least one source is unavailable, stale, drifted, inaccessible, or missing a usable reference.
- `optimal` -- librarian-known sources for the project are accessible and daemon8 has usable references for the debug stream.

Debug flow:

1. `start_debug_session`
2. `awareness_status`
3. Fix source coverage if status is not `optimal`, or explicitly accept the gap.
4. `create_checkpoint`
5. Run the action/test.
6. `query_observations(since_checkpoint=...)`
7. `resolve_debug_session` once the root cause is verified.
