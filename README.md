<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-light.svg">
    <img src="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg" alt="Daemon8" width="240">
  </picture>
</p>

<p align="center">
  <em>A runtime layer made for AI agents.</em>
</p>

---

# What is Daemon8?

Daemon8 is a local runtime layer for AI agents.

Today, most agents can read and write code quickly, but they still do not have one reliable source of runtime truth while debugging. At its core, that is what Daemon8 provides: one place where logs and context converge.

When agents have one place to look for errors, they can move from "I changed code" to "I can see what happened" in one loop. They query observations, filter what matters, and act on the result without guessing.

Picture application logs, browser console output, network failures, traces, and device logs funneled into one stream. With lens-based filtering, the agent can ignore noise and focus directly on the signal it needs.

> Note: daemon8 is in active development right now. Show support by starring, trying it out, and/or submitting issues - it's greatly appreciated!

### Daily Workflow Scenarios

**Working in Chrome / frontend**

- While editing frontend or backend code, your agent can use `query_observations` to immediately inspect browser console and network failures.
- It can use `issue_command` to run JS, capture screenshots, inspect DOM, and validate fixes without leaving the workflow.

**Working in backend / API debugging**

- App logs and ingestion events land in the same stream as browser signals, so the agent can inspect frontend and backend state in one query path.
- `create_checkpoint` and `query_observations` make before/after verification explicit after each change.

**Working with multiple agents**

- Agents can emit observations with `ingest_observation` so handoffs, findings, and status notes stay in the same stream as runtime facts.
- Other sessions can filter by origin, tag, correlation id, or checkpoint without depending on a separate coordination system.

Daemon8 runs locally and provides one stream for runtime awareness plus one MCP surface to query, act, and record useful findings. Runtime data and curated memory stay on your machine.

## Features

- **Observation bus** — one stream for browser console, network, JS exceptions, lifecycle events, device logs, and app telemetry.
- **Lens** — per-session filter with buffered matches for quick follow-up queries.
- **Browser actions** — eval JS, screenshot, inject CSS, navigate, set viewport, throttle network, inspect/set storage, and tab controls via Chrome DevTools Protocol.
- **Curated memory** — save and query verified lessons, decisions, and error signatures through one local memory table.
- **Doctor** — checks config, storage, setup state, sources, and service health; `--fix` repairs what it can.
- **System service** — `daemon8 install` registers launchd (macOS), systemd user service (Linux), or Task Scheduler (Windows).

## Install

```bash
cargo install daemon8
```

macOS — sign the binary so Gatekeeper and launchd accept it:

```bash
codesign --force --sign - ~/.cargo/bin/daemon8
```

Register as a system service and configure your AI clients:

```bash
daemon8 install
daemon8 init
daemon8 setup plan
daemon8 setup apply --yes
```

`daemon8 install` registers a user-level service. `daemon8 init` creates the project `.daemon8.toml`. `daemon8 setup plan` previews configuration changes. `daemon8 setup apply --yes` registers runtime sources and configures supported AI clients.

> [!WARNING]
> **macOS:** first launch triggers an "unidentified developer" Gatekeeper prompt and may request Background Items and App Management permissions. Both are expected until signed release binaries ship.

## Verify

```bash
daemon8 status
daemon8 doctor
```

## How it works

```
Sources (inputs)                          Agents (outputs)
-----------------                         ----------------
Browser (CDP)   ---\                       /--  Codex
Applications    ---->  daemon8            ----> Claude Code
Devices (ADB)   ---/   [localhost:8888]    \--> Gemini CLI
CLI hooks       --/                    
```

Sources push observations into the daemon loop. Agents then query, subscribe, and run actions through MCP (`http://localhost:8888/mcp`) or the parallel HTTP API.

_some sources are configured by you, some are integrated directly into the daemon_

## MCP tools

There are 17 MCP tools grouped by capability. Every tool returns JSON. Mutating operations require explicit confirmation flags.

### Observation

| Tool | Purpose |
|------|---------|
| `query_observations` | Filter the observation stream by kind, severity, origin, text, tags, correlation id, or checkpoint. |
| `status` | Health snapshot: error rate, source counts, observation count, version. |
| `create_checkpoint` | Mark a stream position; subsequent queries can resume from it. |
| `list_connections` | Active sources plus browser connection state. |
| `subscribe_observations` | Register a real-time alert filter. Matching observations are buffered into the next `query_observations` response. |
| `ingest_observation` | Record an observation from inside an agent loop. |

### Action

| Tool | Purpose |
|------|---------|
| `connect_browser` | Attach the daemon to a Chrome DevTools Protocol endpoint. |
| `issue_command` | Eval JS, screenshot, CSS inject, storage inspect/set/clear, navigate, set viewport, network throttle, list/new/close tabs. |

### Lens

| Tool | Purpose |
|------|---------|
| `set_lens` | Create a focused filter that buffers matches into a ring buffer (max 1000). |
| `lens_status` | Inspect the active lens filter and buffer depth. |
| `clear_lens` | Remove the active lens. |

### Memory

| Tool | Purpose |
|------|---------|
| `save_memory` | Store a `pattern`, `decision`, `error_signature`, `session_summary`, or `user_flagged` memory. |
| `query_memory` | Search by kind, tags, project, or text. |
| `forget_memory` | Delete by id; requires `confirm=true`. |

### Setup

| Tool | Purpose |
|------|---------|
| `setup_status` | Report current setup state for the project at `cwd`. |
| `setup_plan` | Compute the action plan that `setup_apply` would run. |
| `setup_apply` | Apply the plan to the project; requires `yes=true`. |

## HTTP API

Observation, browser, lens, and memory routes for non-MCP clients. Most routes return JSON; `/api/stream` is SSE and `/health` returns plain text.

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/observe` | Query observations (same filters as `query_observations`). |
| GET | `/api/checkpoint` | Current checkpoint id. |
| GET | `/api/summary` | Runtime summary (errors, connections, counts). |
| GET | `/api/connections` | Active source and browser state. |
| POST | `/api/connect` | Set Chrome DevTools endpoint. |
| GET | `/api/stream` | Server-Sent Events stream with `Last-Event-ID` replay. |
| GET&nbsp;/&nbsp;PUT&nbsp;/&nbsp;DELETE | `/api/lens` | Inspect, set, or clear the active lens. |
| POST | `/api/browser/act` | Same action surface as `issue_command`. |
| GET&nbsp;/&nbsp;POST | `/api/memory` | Query or save curated memory rows. |
| POST | `/api/memory/export` | Stream ordered memory export rows as NDJSON. |

Ingestion endpoints live alongside the API on the same port:

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/ingest` | Single observation. JSON body. |
| POST | `/ingest/batch` | Array of observations. |
| GET | `/health` | Health probe (returns `200 ok`). |

A UDP listener accepts the same JSON shapes on port 8889 for fire-and-forget telemetry.

## CLI

| Command | Description |
|---------|-------------|
| `daemon8 serve` | Start the daemon (default when no subcommand given). |
| `daemon8 status` | Show daemon health and status. |
| `daemon8 tail` | Stream observations in real-time. |
| `daemon8 query` | Query stored observations. |
| `daemon8 connections` | List active data source connections. |
| `daemon8 browser` | Browser DevTools commands. |
| `daemon8 lens` | Manage per-session observation lens. |
| `daemon8 logs` | Show log file location or tail logs (`--follow`). |
| `daemon8 config` | Show or modify configuration. |
| `daemon8 completions` | Generate shell completions. |
| `daemon8 install` | Install as a system service. |
| `daemon8 uninstall` | Remove the system service. |
| `daemon8 setup` | Inspect, plan, or apply guided setup. |
| `daemon8 channel` | Real-time alert relay for MCP clients (experimental). |
| `daemon8 doctor` | Diagnose configuration and environment (`--fix` to auto-repair). |
| `daemon8 init` | Scaffold a `.daemon8.toml` in the current project. |

## Ingestion examples

```bash
curl -X POST http://localhost:8888/ingest -H 'Content-Type: application/json' -d '{"kind":"query","severity":"info","app":"my-api","data":{"sql":"SELECT * FROM users","duration_ms":3.2}}'
```

Batch endpoint: `POST /ingest/batch` with a JSON array. UDP fire-and-forget on port 8889.

## Architecture

Core crates in the Cargo workspace:

| Crate | Purpose |
|-------|---------|
| `daemon` | CLI binary, command dispatch, runtime wiring. |
| `types` | Shared types: `Observation`, `Filter`, severity, origin, and memory kind records. |
| `store` | SurrealDB backend for observations, curated memory, and the `LensManager` ring buffer. |
| `api` | Axum HTTP routes: observe, stream, lens, memory, and browser actions. |
| `mcp` | MCP server: 17 tools across observe, action, lens, memory, and setup routers. |
| `ingest` | HTTP, UDP, and Unix socket ingest endpoints. |
| `chrome` | Chrome DevTools Protocol bridge over raw WebSocket. |
| `adb` | Android Debug Bridge transport for device logcat. |
| `parse` | Observation parsing and extraction for log sources. |

## Contributing

Pull requests welcome. See [`CONTRIBUTING.md`](./CONTRIBUTING.md). Code of conduct: [`CODE_OF_CONDUCT.md`](./CODE_OF_CONDUCT.md).

## License

[Fair Core License 1.0 with Apache 2.0 Fallback](LICENSES/FCL-1.0-ALv2.txt) (FCL-1.0-ALv2). Full rights for internal use, education, research, and professional services. Restricts competing use. Each release relicenses under Apache 2.0 two years after publication.

DAEMON8 is a trademark of Havy.tech, LLC.

## Contact

- **General / security:** mail@daemon8.ai
- **Discussion:** [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
- **Bugs / features:** [Issues](https://github.com/daemon8ai/daemon8/issues)

Copyright 2026 Havy.tech, LLC.
