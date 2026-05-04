<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-light.svg">
    <img src="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg" alt="Daemon8" width="240">
  </picture>
</p>

<p align="center">
  <em>Local runtime observation bus and memory substrate for AI coding agents.</em>
</p>

<p align="center">
  <a href="https://daemon8.ai">Website</a> ·
  <a href="https://github.com/daemon8ai/daemon8/discussions">Discussions</a> ·
  <a href="https://github.com/daemon8ai/daemon8/issues">Issues</a>
</p>

---

Daemon8 is a single local service that runs alongside your AI coding tools. It captures runtime observations from the browser, devices, and applications, and exposes them — together with a tiered memory substrate, lens-based filters, and browser actions — over MCP and HTTP. No cloud. No account. No telemetry. Everything stays on your machine.

## Features

- **Observation bus** — unified stream of browser console, network, JS exceptions, lifecycle, device logs, and application telemetry (logs, queries, custom events, exceptions).
- **Memory tiers** — three durable tables (`memory_short` TTL working memory, `memory_reference` external source mirrors, `memory_long` distilled knowledge) with a bookkeeper for sweep and dedup.
- **Embedding profile registry** — per-generator metadata so vectors stored on memory rows can be safely filtered without mixing models.
- **Lens** — per-session reactive filter with a 1000-entry ring buffer that surfaces matching observations to the next query.
- **Browser actions** — eval JS, screenshot, inject CSS, navigate, set viewport, throttle network, manipulate storage, list/close tabs, all via Chrome DevTools Protocol.
- **Deliber8 agents** — register specialist/steward/bookkeeper cards, send envelopes through inboxes, audit roster and backlog through `daemon8 doctor`.
- **Doctor** — diagnose configuration, environment, store health, embedding provider, stuck agents, and inbox backlog. `--fix` repairs what it can.
- **System service** — `daemon8 install` registers the daemon as a launchd agent (macOS), systemd unit (Linux), or Task Scheduler entry (Windows).

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
daemon8 setup
```

`daemon8 install` registers a user-level service. `daemon8 setup` detects your browser and auto-configures Claude Code, Cursor, Windsurf, Gemini CLI, and Codex.

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
Browser (CDP)    --\                  /--  Claude Code
Applications     ----> daemon8 loop ----> Cursor
Devices (ADB)   --/    localhost:8888 \--> Windsurf
CLI hooks        --/                  \--> Gemini CLI, Codex
```

Sources push observations into the loop. Agents query, subscribe, and act through MCP tools on `http://localhost:8888/mcp` or the parallel HTTP API on the same port.

## MCP tools

Twenty-four tools, organized by capability. Every tool returns JSON; mutations require explicit confirmation flags.

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

### Memory (legacy table)

| Tool | Purpose |
|------|---------|
| `save_memory` | Store a `pattern`, `insight`, `decision`, or other typed memory. |
| `query_memory` | Search by kind, tags, project, or text. |
| `forget_memory` | Delete by id; requires `confirm=true`. |

### Memory tiers

| Tool | Purpose |
|------|---------|
| `query_memory_tier` | Read rows from `short`, `reference`, or `long` with scope/agent/tag/profile filters. |
| `memory_sweep_short` | Reap expired rows from the short tier. Defaults to dry-run (`apply=false`). |
| `memory_dedupe_long` | Collapse exact `content_hash` collisions on the long tier, keeping the highest-confidence row. Defaults to dry-run. |

### Embedding profiles

| Tool | Purpose |
|------|---------|
| `list_embedding_profiles` | List registered embedding profiles (provider/model/dimensions). |
| `register_embedding_profile` | Register a profile. Idempotent on `(provider, model)`. |

### Setup

| Tool | Purpose |
|------|---------|
| `setup_status` | Report current setup state for the project at `cwd`. |
| `setup_plan` | Compute the action plan that `setup_apply` would run. |
| `setup_apply` | Apply the plan to the project; requires `yes=true`. |

### Deliber8

| Tool | Purpose |
|------|---------|
| `deliber8_roster` | List registered agent cards (specialist/steward/bookkeeper) with status/kind/team/project filters. |
| `deliber8_inbox` | Read envelopes for one address (`agent:slug`) by status, with counts per status. |

## HTTP API

Same data surface as MCP, accessible to non-MCP clients. All routes return JSON.

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
| GET | `/api/memory/short` | Read short-tier rows. |
| GET | `/api/memory/reference` | Read reference-tier rows. |
| GET | `/api/memory/long` | Read long-tier rows. |
| POST | `/api/bookkeeper/sweep` | Sweep expired short-tier rows. Body: `{ agent_id?, apply? }`. |
| POST | `/api/bookkeeper/dedupe` | Dedupe long-tier rows. Body: `{ scope?, apply? }`. |
| GET | `/api/embedding/profiles` | List embedding profiles. |
| POST | `/api/embedding/profiles` | Register a profile. Body: `{ provider, model, dimensions, id? }`. |

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
| `daemon8 deliber8` | Manage deliber8 background agents and inboxes. |
| `daemon8 doctor` | Diagnose configuration and environment (`--fix` to auto-repair). |
| `daemon8 init` | Scaffold a `.daemon8.toml` in the current project. |

## Ingestion examples

```bash
curl -X POST http://localhost:8888/ingest -H 'Content-Type: application/json' -d '{"kind":"query","severity":"info","app":"my-api","data":{"sql":"SELECT * FROM users","duration_ms":3.2}}'
```

Batch endpoint: `POST /ingest/batch` with a JSON array. UDP fire-and-forget on port 8889.

## Architecture

Ten crates in a Cargo workspace:

| Crate | Purpose |
|-------|---------|
| `daemon` | CLI binary, command dispatch, runtime wiring. |
| `types` | Shared types: `Observation`, `Filter`, memory tier records, agent cards, envelopes, embedding profile. |
| `store` | SurrealDB backend, tier stores, bookkeeper, embedding profile registry, `LensManager` ring buffer. |
| `api` | Axum HTTP routes: observe, stream, lens, memory, bookkeeper, embedding, browser. |
| `mcp` | MCP server: 24 tools across observe, action, lens, memory, tier, embedding, setup, and deliber8 routers. |
| `ingest` | HTTP, UDP, and Unix socket ingest endpoints. |
| `chrome` | Chrome DevTools Protocol bridge over raw WebSocket. |
| `adb` | Android Debug Bridge transport for device logcat. |
| `embed` | Embedding provider abstraction (fastembed, ollama, openai). |
| `parse` | Observation parsing and extraction for log sources. |

## Contributing

Pull requests welcome. See [`CONTRIBUTING.md`](./CONTRIBUTING.md) and the testing gauntlet in [`TESTING.md`](./TESTING.md). Code of conduct: [`CODE_OF_CONDUCT.md`](./CODE_OF_CONDUCT.md).

## License

[Fair Core License 1.0 with Apache 2.0 Fallback](LICENSES/FCL-1.0-ALv2.txt) (FCL-1.0-ALv2). Full rights for internal use, education, research, and professional services. Restricts competing use. Each release relicenses under Apache 2.0 two years after publication.

DAEMON8 is a trademark of Havy.tech, LLC.

## Contact

- **General / security:** mail@daemon8.ai
- **Discussion:** [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
- **Bugs / features:** [Issues](https://github.com/daemon8ai/daemon8/issues)

Copyright 2026 Havy.tech, LLC.
