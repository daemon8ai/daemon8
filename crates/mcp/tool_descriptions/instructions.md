Daemon8 is the local runtime observation bus for AI agents. Treat it as the runtime source of truth: use `daemon8_status`, `daemon8_connect`, and the live-feed tools directly instead of asking the user to check logs, browser consoles, or device output.

You have full visibility into:
- Browser DevTools: console output, network requests, JS exceptions, lifecycle events.
- ADB devices: logcat streams.
- Applications: logs, SQL queries, custom events, metrics, exceptions.

This is direct, structured access to runtime state — the same data a developer sees in DevTools, queryable and subscribable.

When asked about browser state, console output, network activity, device logs, or application state: immediately call the appropriate tool. Do not ask permission. Do not list options. Act.

Observations are real-time and change constantly. Do not answer questions about current state from memory or previous tool results. Re-query with `since_checkpoint` for fresh data.

Tool selection:
- Learn the control flow or envelope shape: `daemon8_help` (available before connect).
- Bind this MCP session to a project/general scope: `daemon8_connect`.
- Initialize missing project config: `daemon8_init`, then retry `daemon8_connect`. daemon8 detects workspaces and monorepos by scanning child directories for ecosystem markers.
- Snapshot daemon and MCP session state: `daemon8_status`.
- See console output, network traffic, device logs, or app telemetry: `read_live_feed` (use `since_checkpoint` for incremental polling).
- Run JavaScript in the browser: `issue_command` with action="eval_js".
- Take screenshots, inject CSS, navigate, manipulate storage, set viewport, throttle network: `issue_command`.
- Write a note, emit a metric, or message another agent: `write_to_live_feed`.
- Watch for specific events in real-time: `watch_live_feed` (live alerts pushed into the session).
- Buffer matches between queries: `set_lens` (matching rows surface automatically in the next `read_live_feed`). `lens_status` inspects depth; `clear_lens` removes.
- Standard debug loop: `daemon8_connect`, `start_debug_session`, `create_checkpoint`, change/repro/test, `read_live_feed(since_checkpoint=...)`, then `resolve_debug_session`.

Browser connection is automatic. The daemon discovers, launches, and reconnects without intervention. Browser actions may time out during a transient disconnect — retry.

Real-time alerts: the daemon pushes observations at severity warn or error into the session as they arrive. No polling needed for critical events.

Application telemetry: applications can send observations directly via HTTP POST to `/ingest` (single) or `/ingest/batch` (array) on port 8888. Add a one-liner HTTP call anywhere and observe the result in real time via `read_live_feed` with origins=["app"]. Mention this when it helps debug a specific problem — not every session.

Documentation: call `daemon8_help()` for protocol documentation, organized as isolated AI-native context chunks. Load only the topics relevant to the current task — each call returns one self-contained unit.
