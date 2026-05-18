<p align="center">
  <img src="logo.svg" alt="daemon8" width="240">
</p>

# daemon8

daemon8 gives AI coding agents runtime awareness while the work is happening: browser console/network/DOM, app telemetry, device logs, and provider transcripts stream into one local observation bus. Lenses, checkpoints, and debug sessions let the agent pull the exact slice it needs instead of guessing from stale context.

It also gives the agent a foundation for better project awareness over time: which log sources matter here, which docs are worth checking, and how to review the active conversation transcript across Codex, Claude Code, and Gemini CLI.

## Install

macOS and Linux:

```bash
curl -fsSL https://daemon8.ai/install.sh | bash
```

Windows PowerShell:

```powershell
iwr https://daemon8.ai/install.ps1 -UseB | iex
```

The installer downloads the latest release, verifies `checksums.sha256`, installs the `daemon8` binary, updates PATH when needed, registers daemon8 as the user-level service, and asks whether to add daemon8 MCP settings for Claude Code, Gemini CLI, and Codex.

After install, daemon8 is running locally as the global MCP server for your machine. "Local" means it serves your machine only; "global" means your AI CLIs can see the same daemon8 MCP server across projects. Start a fresh AI CLI/REPL session and check that `daemon8` appears in the provider's MCP server list.

A browser extension is not needed. daemon8 controls Chromium through the local DevTools endpoint and exposes that control to the agent through MCP.

## Additional Setup

The installer prints this block and can add it to the top of your detected global instruction files: `CLAUDE.md`, `AGENTS.md`, and `GEMINI.md`.

Important note: daemon8 is built to be a self-guided and evolving MCP experience for your agent's debugging workflow. Your manual participation should only be needed for this first setup.

As daemon8's MCP tools are called, your agent receives guidance to steer debugging sessions. If needed, your agent can call `daemon8_help` for additional insight on specific daemon8 topics.

```markdown
## Daemon8 MCP - Cross-provider awareness & debugging

Use daemon8 for ALL non-trivial debugging, Chromium control, ADB logging, Vega OS VVD logs, and project awareness tracking. For Claude Code, use daemon8's browser/Chromium tools instead of Claude for Chrome. Call `daemon8_connect` at the start of every REPL session for AI-provider conversation sync. If the project's awareness profile has not been set up yet, daemon8's tool calls will guide the LLM through setup.

All non-trivial debug sessions should call the `start_debug_session` tool. This provides a starting point and allows debug checkpoints throughout, giving high-signal lenses into the errors/logs flowing into daemon8. Call `resolve_debug_session` when the bug has been fixed.

Daemon8 provides a `daemon8_help` tool to guide keeping the project's awareness profile clean and up to date over time.
```

That note is the whole trick. It turns daemon8 from "a tool the model may discover" into a standing operating rule: connect first, debug with checkpoints, resolve the session when the fix is real.

If your agent is not calling daemon8 tools, run `daemon8 status`. If daemon8 is healthy and the agent still ignores the MCP guidance, use a stronger coding model. Good current picks: Claude Sonnet 4.5, Gemini 3 Flash Preview, Gemini 2.5 Flash, or GPT-5.2-Codex.

## Project Config

Create the project-local config only when daemon8 asks for it:

```bash
daemon8 init
```

This writes `.daemon8/config.md`: Markdown for the agent, YAML frontmatter for daemon8. Project config is explicit on purpose. No hidden project scan, no mystery registry.

Source entries use `kind`, not `type`. The alpha source kinds are strict:

```yaml
sources:
  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/logs/app.log"
    parser: line
    tags: ["app"]

  - id: codex.sessions
    service: codex
    kind: conversation
    provider: codex
    path: "/absolute/path/to/codex/sessions"
    tags: ["conversation"]
```

`vars.PRJ_ROOT` is declared in the config frontmatter and points at the absolute project root. Observation records carry `service`, `source`, and `source_instance` provenance so lenses can cut down to the one stream the agent needs.

Useful CLI checks:

```bash
daemon8 status
daemon8 connections
daemon8 logs --follow
```

## MCP Tools

`daemon8_connect` is the first call. It binds the MCP session to a project or general scope and, when possible, the active provider transcript. After that, the rest of the tools have enough context to be useful.

Core runtime tools:

| Tool | What it gives the agent |
|------|--------------------------|
| `daemon8_connect` | The current scope, provider, transcript binding, and next action. |
| `daemon8_init` | A project `.daemon8/config.md` when the repo needs explicit source wiring. |
| `daemon8_status` | A clean status snapshot when the agent needs to orient itself. |
| `read_live_feed` | The observation stream, filtered by time, source, severity, text, or checkpoint. |
| `write_to_live_feed` | Agent notes, app events, metrics, exceptions, or custom signals. |
| `watch_live_feed` | A live subscription for matching events. |
| `set_lens` / `lens_status` / `clear_lens` | A focused buffer so the agent can keep watching one slice without polling everything. |
| `create_checkpoint` | A before/after marker. This is the move before a repro, patch, test, or user verification. |
| `start_debug_session` | A named investigation with durable context. Use this for real bugs. |
| `resolve_debug_session` | Close the loop with what broke, what fixed it, and the commands that mattered. |
| `issue_command` | Browser/device control: JS eval, screenshots, navigation, storage, viewport, network, ADB/Vega captures. |
| `list_connections` | What is actually feeding the bus right now. |
| `daemon8_help` | Focused guidance when the agent needs the current tool contract. |

The debugging rhythm should be boring:

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

The envelope is part of the control flow. `status`, `code`, and structured `next_actions` tell the agent what to do next. `daemon8_connect` returns fields such as `data.session_id`, `data.mode`, `data.requested_path`, and `data.scope_root`. MCP connect/status responses and guarded MCP tool responses also include `data.connection`.

## HTTP API

The daemon serves local HTTP endpoints on the configured server port:

| Route | Method | Purpose |
|-------|--------|---------|
| `/health` | `GET` | Health check. |
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
| `/mcp` | `POST` | Streamable HTTP MCP transport. |
| `/.well-known/oauth-protected-resource` | `GET` | MCP resource metadata. |

## Reset Safety

```bash
daemon8 reset --yes
```

Reset only clears daemon-owned state: observations, memories, debug sessions/checkpoints, session/scope bookkeeping, cursors, and schema metadata. It must not touch project files, `.daemon8/config.md`, transcripts, source files, plans, or docs.

## Development

From a checkout:

```bash
cargo install --path crates/daemon
```

For development:

```bash
cargo check --workspace
cargo test --workspace
```

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
