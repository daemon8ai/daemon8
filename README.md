<p align="center">
  <img src="logo.svg" alt="daemon8" width="240">
</p>

# daemon8

daemon8 gives AI coding agents runtime awareness _while the work is happening_.

Static context is not enough. daemon8 keeps the agent connected to what is actually happening.

1. Runtime signals stream into one queryable feed.
2. Lenses, checkpoints, and debug sessions create boundaries and controlled context while debugging.
3. An _awareness profile_ - managed without manual intervention - keeps the project's docs, sources, and metadata close at hand.
4. Provider-agnostic conversation sync helps agents pick up where you left off - _**across** the vendor divide_.

**Awareness profile:** A small project-local profile that points daemon8 at the debugging/logging context the agent needs:

```plaintext
your-project/
`-- .daemon8/
    |-- config.md
    |-- conversations/
    |-- knowledge/
    |   |-- DOCS.md
    |   |-- fixes/
    |   |-- patterns/
    |   |-- sessions/
    |   `-- services/
    `-- sources/
```

Daemon8 uses [SurrealDB](https://surrealdb.com/) for the high-demand persistence: live-feed history, cursor state, debug sessions, checkpoints, and the query layer behind the lenses. Your project's awareness profile stays small (by design); SurrealDB carries the context for agents to search, slice, and traverse as needed. It's the real workhorse behind daemon8's existing features - both now, and for the ambitious roadmap ahead.

---

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
## Daemon8 -- Runtime Observation Layer (ALWAYS ON)

Daemon8 is the runtime source of truth. Use it for ALL debugging, browser control, ADB/device logs, and application telemetry. Never guess console output, network activity, or runtime state -- query daemon8.

Call `daemon8_connect` once at session start. All tools and decisions depend on it.

**Debug loop:**
`start_debug_session`
  -> [review DOCS.md/review knowledge]
  -> [start loop]
    -> `create_checkpoint`
    -> [make changes]
    -> [run/test]
    -> `read_live_feed` (with `since_checkpoint`)
    -> [review results / update knowledge]
    -> [fixed? end loop]
  -> `resolve_debug_session`
  -> [sync knowledge/sync DOCS.md]

**Primary tools:**
- `read_live_feed` -- console, network, errors, app telemetry (use `since_checkpoint` for incremental reads)
- `list_connections` -- see active input sources (browsers, devices, apps)
- `issue_command` -- browser control (eval_js, screenshot, navigate, viewport, storage, network throttle)
- `write_to_live_feed` -- emit notes, metrics, or agent-to-agent messages
- `set_lens` / `clear_lens` -- persistent filters that surface matching observations automatically
- `daemon8_connect` -- bind the session to a project scope and provider transcript (call once at start)
- `daemon8_help` -- guidance on any daemon8 topic
```

That note is the whole trick. It turns daemon8 from "a tool the model may discover" into a standing operating rule: connect first, debug with checkpoints, resolve the session when the fix is real.

If your agent is not calling daemon8 tools, run `daemon8 status`. If daemon8 is healthy and the agent still ignores the MCP guidance, use a stronger coding model. Good current picks: Claude Sonnet 4.5, Gemini 3 Flash Preview, Gemini 2.5 Flash, or GPT-5.2-Codex.

## Project Config

Create the project-local config only when daemon8 asks for it:

```bash
daemon8 init
```

This writes `.daemon8/config.md`: Markdown for the agent, YAML frontmatter for daemon8. Project config is explicit on purpose. No hidden project scan, no mystery registry.

To remove daemon8 from a project:

```bash
daemon8 init --remove
```

This deletes `.daemon8/` and cleans up scope ledger records. It does not affect daemon-owned state (observations, memories, debug sessions) -- use `daemon8 reset` for that.

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
