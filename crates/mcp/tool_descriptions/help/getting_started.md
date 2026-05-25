# Getting started with daemon8

daemon8 is the runtime observation bus for AI agents. It runs as one global background daemon on the developer's machine. Every MCP session talks to that single daemon -- it is already running, not something you start per project.

## First session checklist

1. Call `daemon8_connect(provider, project_path)` to bind this MCP session.
2. If the response is `setup_required`, call `daemon8_init(project_path)` to write `.daemon8/config.md`. Complete ALL steps in the generated config body (stack audit, source population, user confirmation), then retry `daemon8_connect`.
3. After a successful connect with sources populated, you have full access to runtime observation tools.
4. Offer once to recover recent conversation history across AI providers. Run recall only if the user asks for or accepts it.

Setup is mandatory. daemon8 cannot observe a project without `.daemon8/config.md` and populated sources.

## What you gain after connect

- `read_live_feed` -- query logs, browser console, network, exceptions, SQL, device telemetry, and agent tool calls across every connected source.
- `set_lens` / `watch_live_feed` -- focused watch patterns and live push alerts.
- `start_debug_session` + `create_checkpoint` -- before/after investigation with retrievable fix summaries.
- `issue_command` -- browser DevTools actions (eval_js, screenshot, navigate, inject_css, storage, viewport, network conditions).
- `write_to_live_feed` -- emit notes, metrics, or agent-to-agent messages into the stream.
- `build_context_snapshot` -- full cross-provider recall when the user asks for or accepts history/review/catch-up/continuity.
- `link_conversation` -- link missing or explicit provider transcripts before a requested recall.

## Deeper topics

Call `daemon8_help(topic="<name>")` for focused protocol documentation: observations, envelope, checkpoint, lens, debug_session, sources, setup.
