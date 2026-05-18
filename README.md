# daemon8

Runtime observation for AI coding agents.

daemon8 runs locally and gives an agent structured access to the live development surface: browser console/network/DOM state, app telemetry, device logs, checkpoints, lenses, and debug-session summaries.

Everything is local by default. The daemon-owned database lives under the platform data directory. Project-specific configuration lives in one project file: `.daemon8/config.md`.

## Install

From a release:

```bash
curl -fsSL https://raw.githubusercontent.com/daemon8ai/daemon8/main/install.sh | bash
```

Windows PowerShell:

```powershell
iwr https://raw.githubusercontent.com/daemon8ai/daemon8/main/install.ps1 -UseB | iex
```

Pin a release with `DAEMON8_VERSION=v0.4.0-alpha.1`. Override the destination with `DAEMON8_INSTALL_DIR`.

From a checkout:

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

Install the login service when you want daemon8 to start automatically:

```bash
daemon8 service install
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

Observation records include optional `service`, `source`, and `source_instance` provenance for live-feed filtering. Cursor state uses the same source identity.

## MCP Surface

Except for `daemon8_connect`, `daemon8_init`, `daemon8_status`, and `daemon8_help`, MCP tools require an established `daemon8_connect` session. Debug lifecycle mutation and checkpoint tools require project mode; `list_debug_sessions` is available after connect in project or general mode.

Core runtime tools:

| Tool | Purpose |
|------|---------|
| `daemon8_connect` | Bind this MCP session to an explicit project/general scope and active provider transcript when available. |
| `daemon8_init` | Write `.daemon8/config.md` for an explicit project path. |
| `daemon8_status` | Snapshot daemon health, connected sources, and current MCP scope. |
| `read_live_feed` | Query observations with filters and checkpoint windows. |
| `write_to_live_feed` | Ingest an agent/app note, metric, exception, or custom event. |
| `watch_live_feed` | Subscribe this MCP session to live matching observations. |
| `connect_browser` | Override the browser DevTools endpoint. |
| `issue_command` | Browser/device action surface: eval JS, screenshot, navigate, storage, viewport, network. |
| `list_connections` | Inspect browser/app/device connections feeding the stream. |
| `set_lens` | Buffer matching observations between reads. |
| `lens_status` | Inspect the active lens. |
| `clear_lens` | Clear the active lens. |
| `daemon8_help` | Load focused MCP help topics. |

Debug-session tools are available when debug lifecycle storage is enabled:

| Tool | Purpose |
|------|---------|
| `start_debug_session` | Open a scoped investigation. |
| `create_checkpoint` | Bookmark the current observation sequence. |
| `list_debug_sessions` | Review active or past investigations. |
| `resolve_debug_session` | Close with a rich SessionSummary. |
| `end_debug_session` | Close without a fix. |

The standard debugging loop is:

```text
daemon8_connect -> start_debug_session -> create_checkpoint -> change/repro/test -> read_live_feed(since_checkpoint=...) -> resolve_debug_session
```

## Response Shape

MCP responses and CLI `connect/init/status --json` output use the common alpha envelope:

```json
{
  "status": "success",
  "code": "connected",
  "message": "connected to project",
  "why": null,
  "data": {},
  "hints": [],
  "next_actions": []
}
```

The envelope is part of the control flow. `status`, `code`, and structured `next_actions` guide the model at decision points. `daemon8_connect` returns flattened scope fields such as `data.session_id`, `data.mode`, `data.requested_path`, and `data.scope_root`. MCP connect/status responses and guarded MCP tool responses also include `data.connection`; CLI connect JSON uses the shared flattened core fields. Clients should branch on `status`, `code`, and `next_actions`.

## HTTP API

The daemon serves local HTTP endpoints on the configured server port:

| Route | Method | Purpose |
|-------|--------|---------|
| `/ingest` | `POST` | Write one observation. |
| `/ingest/batch` | `POST` | Write multiple observations. |
| `/api/observe` | `GET` | Query observations with filters. |
| `/api/checkpoint` | `GET` | Read the current observation sequence. |
| `/api/summary` | `GET` | Snapshot daemon health and source summary. |
| `/api/connections` | `GET` | Inspect browser/app connection state. |
| `/api/connect` | `POST` | Connect the browser/CDP endpoint. |
| `/api/stream` | `GET` | Stream observations over SSE; `project_path` refreshes configured project sources before replay. |
| `/api/lens` | `GET`/`PUT`/`DELETE` | Inspect, set, or clear the observation lens. |
| `/api/browser/act` | `POST` | Run a browser/device action. |

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
| `api` | Axum HTTP routes for observe, checkpoint, summary, connections, lens, browser action, and SSE streaming. |
| `mcp` | MCP tool surface and control-flow envelopes. |
| `providers` | Standalone AI provider detection/config utilities. |
| `ingest` | HTTP `/ingest`, `/ingest/batch`, `/health`, UDP, and Unix socket ingestion endpoints. |
| `chrome` | Chrome DevTools Protocol bridge. |
| `adb` | Android Debug Bridge transport. |
| `parse` | Log/conversation parser trait and built-in parsers. |
