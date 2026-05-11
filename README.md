<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-light.svg">
    <img src="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg" alt="Daemon8" width="240">
  </picture>
</p>

<p align="center">
  <em>Situational awareness for AI agents.</em>
</p>

---

## Table of Contents

- [What is Daemon8?](#what-is-daemon8)
- [Install](#install)
- [Getting Started](#getting-started)
- [Features](#features)
  - [Observation Stream](#observation-stream)
  - [Browser Actions](#browser-actions)
  - [CLI Hooks](#cli-hooks)
  - [Lenses](#lenses)
  - [Checkpoints and Debug Sessions](#checkpoints-and-debug-sessions)
  - [Memory](#memory)
  - [File Sources](#file-sources)
  - [Device Monitoring (ADB)](#device-monitoring-adb)
- [MCP Tools](#mcp-tools)
- [HTTP API](#http-api)
- [CLI Reference](#cli-reference)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)
- [Contact](#contact)

## What is Daemon8?

Daemon8 is a local process that gives AI agents access to runtime data. Console errors, network failures, device logs, and application traces land in one observation stream. Agents query it through MCP or HTTP without leaving their workflow.

The daemon runs on your machine. Runtime data and curated memory stay local.

> daemon8 is in active development. Show support by starring, trying it out, and submitting issues.

## Install

```bash
curl -fsSL https://daemon8.ai/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://daemon8.ai/install.ps1 | iex
```

Or install from source:

```bash
cargo install daemon8
```

Pin a specific version with `DAEMON8_VERSION=v0.3.2` before the curl command.

The installer downloads the binary, verifies its SHA-256 checksum, installs it, and runs `daemon8 setup` to show current state.

> [!WARNING]
> **macOS:** first launch may trigger an "unidentified developer" Gatekeeper prompt and request Background Items and App Management permissions. Both are expected until signed release binaries ship.

## Getting Started

```bash
daemon8 install        # register as a system service
daemon8 init           # create .daemon8.toml in the current project
daemon8 setup apply    # register runtime sources and configure AI providers
```

`daemon8 features` opens an interactive menu for enabling hooks and project scaffolding.

For best results, add daemon8 instructions to your AI context file (`~/.claude/CLAUDE.md`, `~/.gemini/GEMINI.md`, or `~/.codex/AGENTS.md`) telling the agent to use daemon8 for debugging, note-taking, and agent communication.

Verify the daemon is running:

```bash
daemon8 status
daemon8 doctor
```

## Features

The observation stream is always on. Everything else is opt-in.

### Observation Stream

All runtime signals land in one stream: application logs, browser console output, network failures, device logs, CLI hook telemetry, and agent-emitted notes.

```bash
daemon8 tail                                # stream observations in real-time
daemon8 query --kinds log,exception --limit 20  # query stored observations
```

Ingestion endpoints accept observations over HTTP (`POST /ingest`), UDP (port 8889), and Unix socket.

```bash
curl -X POST http://localhost:8888/ingest -H 'Content-Type: application/json' -d '{"kind":"log","severity":"info","app":"my-api","data":{"message":"deploy complete"}}'
```

### Browser Actions

Connect the daemon to Chrome DevTools Protocol to observe console output, network requests, JS exceptions, and lifecycle events. Run JavaScript, capture screenshots, inject CSS, and inspect storage.

**Enable:** call the `connect_browser` MCP tool, or:

```bash
daemon8 browser connect
```

### CLI Hooks

Record what your agent does. Hooks capture tool calls, file edits, and command runs as observations in the stream.

**Enable:**

```bash
daemon8 setup apply --install-hooks local   # project-level hooks
daemon8 setup apply --install-hooks global  # machine-level hooks
```

Or manage hooks directly:

```bash
daemon8 hooks list
daemon8 hooks update
daemon8 hooks repair
```

### Lenses

A lens is a focused filter that buffers matching observations into a ring buffer (up to 1000 rows). Set a lens before making a change, then query to see only what matched.

Lenses are always available. No setup required.

```bash
daemon8 lens set --kinds exception --severity-min warn
daemon8 lens status
daemon8 lens clear
```

### Checkpoints and Debug Sessions

Bookmark the stream before a change, then query only what happened after.

```bash
# via MCP: create_checkpoint, then query_observations with since_checkpoint
```

Debug sessions group checkpoints, observations, and a resolution summary into one investigation. Sessions are always available through MCP tools (`start_debug_session`, `create_checkpoint`, `resolve_debug_session`).

### Memory

Save verified fixes, error signatures, patterns, and decisions to a local memory table. Query by kind, tags, project, or text. Tag `hash:<error_hash>` links error signatures with their fix summaries.

Memory is always available. No setup required.

```bash
daemon8 memory export --kind pattern --project my-app
```

### File Sources

Tail log files and parse them into structured observations. Supported formats: JSON, syslog, monolog, logfmt, CLF, and auto-detection. Custom patterns via grok syntax.

**Enable:** add `[sources.*]` entries to `.daemon8.toml`:

```toml
[sources.api-logs]
path = "/var/log/my-api/app.log"
parser = "auto"
tags = ["api"]
```

### Device Monitoring (ADB)

Stream logcat from Android devices connected via ADB.

**Enable:** set `[adb] enabled = true` in `~/.config/daemon8/config.toml`:

```toml
[adb]
enabled = true
```

## MCP Tools

26 MCP tools grouped by capability. Every tool returns the standard envelope (`{result, daemon8, error}`) -- see `daemon8_help(topic="envelope")`. Destructive and apply-style operations require explicit confirmation flags.

### Observation

| Tool | Purpose |
|------|---------|
| `query_observations` | Filter the observation stream by kind, severity, origin, text, tags, correlation id, or checkpoint. |
| `status` | Health snapshot: error rate, source counts, observation count, version. |
| `list_connections` | Active sources plus browser connection state. |
| `subscribe_observations` | Register a real-time alert filter. Matching observations push as MCP notifications. |
| `ingest_observation` | Record an observation from inside an agent loop. |

### Debug Session

| Tool | Purpose |
|------|---------|
| `start_debug_session` | Open an investigation. Required prereq for checkpoints and resolution. |
| `create_checkpoint` | Bookmark the stream inside an active session; pair with `since_checkpoint`. |
| `resolve_debug_session` | Close on success with a rich SessionSummary memory (root_cause, fix_diff, commands, related errors). |
| `end_debug_session` | Close without a fix; writes a thin SessionSummary so the row never silently disappears. |
| `list_debug_sessions` | Enumerate sessions, optionally filtered by status. |

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
| `query_memory` | Search by kind, tags, project, or text. Tag `hash:<x>` joins error signatures with their fix summaries. |
| `forget_memory` | Delete by id; requires `confirm=true`. |

### Setup

| Tool | Purpose |
|------|---------|
| `setup_status` | Report current setup state for the project at `cwd`. |
| `setup_plan` | Compute the action plan that `setup_apply` would run. |
| `setup_apply` | Apply the plan to the project; requires `yes=true`. |

### Hooks

| Tool | Purpose |
|------|---------|
| `hooks_list` | Enumerate installed daemon8 CLI hooks across providers and scopes. |
| `hooks_remove` | Uninstall daemon8 hook entries from a provider/scope. |
| `hooks_update` | Reinstall (force) -- fixes drift after binary moves or spec changes. |
| `hooks_repair` | Detect drift across all providers and reinstall only what's stale. |

### Help

| Tool | Purpose |
|------|---------|
| `daemon8_help` | Narrative protocol docs by topic (debug_session, checkpoint, setup, hooks, lens, memory, observations, envelope). |

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

## CLI Reference

| Command | Description |
|---------|-------------|
| `daemon8 serve` | Start the daemon (default when no subcommand given). |
| `daemon8 status` | Show daemon health and status. |
| `daemon8 tail` | Stream observations in real-time. |
| `daemon8 query` | Query stored observations. |
| `daemon8 connections` | List active data source connections. |
| `daemon8 browser` | Browser DevTools commands. |
| `daemon8 lens` | Manage per-session observation lens. |
| `daemon8 memory` | Export memory query results to per-row Markdown files. |
| `daemon8 logs` | Show log file location or tail logs (`--follow`). |
| `daemon8 config` | Show or modify configuration. |
| `daemon8 completions` | Generate shell completions. |
| `daemon8 install` | Install as a system service. |
| `daemon8 uninstall` | Remove the system service. |
| `daemon8 setup` | Inspect, plan, or apply guided setup. |
| `daemon8 channel` | Real-time alert relay for MCP clients (experimental). |
| `daemon8 doctor` | Diagnose configuration and environment (`--fix` to auto-repair). |
| `daemon8 init` | Scaffold a `.daemon8.toml` in the current project. |
| `daemon8 hooks` | Manage daemon8 CLI hooks across providers (`list \| remove \| update \| repair`). |
| `daemon8 features` | Interactive feature activation menu. |

## Architecture

```
Sources (inputs)                          Agents (outputs)
-----------------                         ----------------
Browser (CDP)   ---\                       /--  Codex
Applications    ---->  daemon8            ----> Claude Code
Devices (ADB)   ---/   [localhost:8888]    \--> Gemini CLI
CLI hooks       --/
```

Sources push observations into the daemon loop. Agents query, subscribe, and act through MCP (`http://localhost:8888/mcp`) or the parallel HTTP API.

Core crates in the Cargo workspace:

| Crate | Purpose |
|-------|---------|
| `daemon` | CLI binary, command dispatch, runtime wiring. |
| `types` | Shared types: `Observation`, `Filter`, severity, origin, and memory kind records. |
| `store` | SurrealDB backend for observations, curated memory, debug sessions, librarian catalog, and the `LensManager` ring buffer. |
| `api` | Axum HTTP routes: observe, stream, lens, memory, and browser actions. |
| `mcp` | MCP server: 26 tools across observe, debug-session, action, lens, memory, setup, hooks, help routers. |
| `ingest` | HTTP, UDP, and Unix socket ingest endpoints. |
| `chrome` | Chrome DevTools Protocol bridge over raw WebSocket. |
| `adb` | Android Debug Bridge transport for device logcat. |
| `parse` | Log parser trait with built-in format parsers (JSON, syslog, monolog, logfmt, CLF, grok, auto-detect). |
| `providers` | AI tool detection, hook management, and provider configuration. |

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
