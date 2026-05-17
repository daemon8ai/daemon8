# daemon8

Runtime observation for AI coding agents.

daemon8 runs locally and gives an agent structured access to the live development surface: browser console/network/DOM state, app telemetry, device logs, checkpoints, lenses, and debug-session summaries.

Everything is local by default. The daemon-owned database lives under the platform data directory. Project-specific configuration lives in one project file: `.daemon8/config.md`.

## Install

```bash
cargo install --path crates/daemon
```

For development:

```bash
cargo check --workspace
cargo test --workspace
```

## Start

```bash
daemon8 serve
```

Useful CLI checks:

```bash
daemon8 status
daemon8 connections
daemon8 logs --follow
```

## Project Config

Create the project-local alpha config:

```bash
daemon8 init
```

This writes `.daemon8/config.md`. The file is Markdown with YAML frontmatter so daemon8 can parse it and the LLM can read it directly.

Source entries use `kind`, not `type`. The alpha source kinds are strict:

```yaml
sources:
  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/logs/app.log"
    parser: line
    tags: ["app"]

  - id: claude.conversations
    service: claude
    kind: conversation
    provider: claude
    path: "/absolute/path/to/claude/sessions"
    tags: ["conversation"]
```

`vars.PRJ_ROOT` is declared in the config frontmatter and points at the absolute project root.

## MCP Surface

Except for `daemon8_connect`, `daemon8_init`, and `daemon8_status`, MCP tools require an established `daemon8_connect` session. Debug lifecycle tools require project mode.

Core runtime tools:

| Tool | Purpose |
|------|---------|
| `daemon8_connect` | Bind this MCP session to an explicit project/general scope. |
| `daemon8_init` | Write `.daemon8/config.md` for an explicit project path. |
| `daemon8_status` | Snapshot daemon health, connected sources, and current MCP scope. |
| `read_live_feed` | Query observations with filters and checkpoint windows. |
| `write_to_live_feed` | Ingest an agent/app note, metric, exception, or custom event. |
| `watch_live_feed` | Subscribe this MCP session to live matching observations. |
| `create_checkpoint` | Bookmark the current observation sequence. |
| `connect_browser` | Override the browser DevTools endpoint. |
| `issue_command` | Browser/device action surface: eval JS, screenshot, navigate, storage, viewport, network. |
| `list_connections` | Inspect browser/app/device connections feeding the stream. |
| `set_lens` | Buffer matching observations between reads. |
| `lens_status` | Inspect the active lens. |
| `clear_lens` | Clear the active lens. |
| `daemon8_help` | Load focused MCP help topics. |

Debug-session tools are available when debug-session storage is enabled:

| Tool | Purpose |
|------|---------|
| `start_debug_session` | Open a scoped investigation. |
| `list_debug_sessions` | Review active or past investigations. |
| `resolve_debug_session` | Close with a rich SessionSummary. |
| `end_debug_session` | Close without a fix. |

The standard debugging loop is:

```text
daemon8_connect -> start_debug_session -> create_checkpoint -> change/repro/test -> read_live_feed(since_checkpoint=...) -> resolve_debug_session
```

## Response Shape

MCP responses use a common envelope:

```json
{
  "status": "success",
  "code": "connected",
  "message": "connected to project",
  "data": {},
  "hints": [],
  "next_actions": []
}
```

The envelope is part of the control flow. `status`, `code`, and structured `next_actions` guide the model at decision points. Connection context may appear in `data.connection`; clients should branch on `status`, `code`, and `next_actions`.

## Reset Safety

```bash
daemon8 reset --yes
```

Reset only clears daemon-owned state: observations, memories, debug sessions/checkpoints, session/scope bookkeeping, cursors, and schema metadata. It must not touch project files, `.daemon8/config.md`, transcripts, source files, plans, or docs.

## Workspace

| Crate | Purpose |
|-------|---------|
| `daemon` | CLI binary, command dispatch, runtime wiring. |
| `types` | Shared observation, filter, source, and debug-session types. |
| `store` | SurrealDB-backed observation, memory, debug-session, lens, cursor, and schema state. |
| `api` | Axum HTTP routes for observation ingest, SSE streaming, and health. |
| `mcp` | MCP tool surface and control-flow envelopes. |
| `providers` | Standalone AI provider detection/config utilities. |
| `ingest` | HTTP, UDP, and Unix socket ingestion endpoints. |
| `chrome` | Chrome DevTools Protocol bridge. |
| `adb` | Android Debug Bridge transport. |
| `parse` | Log/conversation parser trait and built-in parsers. |
