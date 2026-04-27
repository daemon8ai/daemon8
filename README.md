<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-light.svg">
    <img src="https://raw.githubusercontent.com/daemon8ai/daemon8/main/mark-dark.svg" alt="Daemon8" width="240">
  </picture>
</p>

<p align="center">
  <em>Everything stays on your machine.</em>
</p>

<p align="center">
  <a href="https://daemon8.ai">Website</a> ·
  <a href="https://daemon8.ai/docs">Docs</a> ·
  <a href="https://github.com/daemon8ai/daemon8/discussions">Discussions</a>
</p>

---

Daemon8 is centralized awareness for agentic programming. It runs as a local system service and presents a unified loop where **sources feed in** and **agents see out**.

**Sources** -- your browser (via CDP), devices (via ADB), applications (via HTTP/UDP ingestion), and CLI tool hooks -- continuously stream observations into daemon8. **Agents** -- any MCP-connected AI coding tool -- connect to that same loop and gain full visibility: what the browser is doing, what the app just logged, what the device is reporting, all queryable and subscribable from a single endpoint.

The result is agents that can observe, reason about, and act on your entire runtime in real time. No tab-switching. No copy-pasting logs. No "check the console."

No cloud. No account. No telemetry. Everything stays on your machine.

## Install

```bash
cargo install daemon8
```

macOS -- sign the binary so Gatekeeper and launchd accept it:

```bash
codesign --force --sign - ~/.cargo/bin/daemon8
```

Register as a system service and configure your AI clients:

```bash
daemon8 install
daemon8 setup
```

`daemon8 install` registers a user-level service (launchd on macOS, systemd on Linux, Task Scheduler on Windows). `daemon8 setup` detects your browser and auto-configures Claude Code, Cursor, Windsurf, Gemini CLI, and Codex.

> [!WARNING]
> **macOS:** first launch triggers an "unidentified developer" Gatekeeper prompt
> and may request Background Items and App Management permissions. Both are
> expected until signed release binaries ship. The daemon is entirely local.

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

Sources push observations into the loop. Agents query, subscribe, and act through MCP tools on `http://localhost:8888/mcp`.

## MCP tools

Fourteen tools across four capabilities:

**Observe** -- query and subscribe to the observation stream.

| Tool | Purpose |
|------|---------|
| `query_observations` | Query by kind, severity, origin, text, tags, or checkpoint. |
| `status` | Health snapshot: error rate, sources, observation count, version. |
| `create_checkpoint` | Mark current stream position; subsequent queries resume from it. |
| `list_connections` | List active sources and browser connection state. |
| `subscribe_observations` | Subscribe to a filtered real-time alert stream. |

**Act** -- control the browser and connected devices.

| Tool | Purpose |
|------|---------|
| `connect_browser` | Point the daemon at a Chrome DevTools Protocol endpoint. |
| `issue_command` | Eval JS, screenshot, CSS inject, storage, navigate, network throttle. |

**Lens** -- per-session reactive filters with a ring buffer.

| Tool | Purpose |
|------|---------|
| `set_lens` | Create a focused filter that buffers matching observations (up to 1000). |
| `clear_lens` | Remove the active lens. |
| `lens_status` | Inspect the current lens filter and buffer state. |

**Memory** -- persist and recall across sessions.

| Tool | Purpose |
|------|---------|
| `save_memory` | Persist a memory entry (user-flagged, pattern, insight). |
| `query_memory` | Search stored memories by kind, tags, project, or text. |
| `forget_memory` | Delete a memory entry by ID. |

**Ingest** -- agents can write into the loop too.

| Tool | Purpose |
|------|---------|
| `ingest_observation` | Record an observation from inside the agent loop. |

## CLI

| Command | Description |
|---------|-------------|
| `daemon8 serve` | Start the daemon server (default when no subcommand given). |
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
| `daemon8 setup` | Interactive first-run wizard. |
| `daemon8 channel` | Real-time alert relay for Claude Code (experimental). |
| `daemon8 agent` | Run a stateless background or one-shot agent. |
| `daemon8 doctor` | Diagnose configuration and environment issues (`--fix` to auto-repair). |
| `daemon8 init` | Scaffold a `.daemon8.toml` in the current project. |

## Ingestion

Any language with an HTTP client can feed the loop:

```bash
curl -X POST http://localhost:8888/ingest -H 'Content-Type: application/json' -d '{"kind":"query","severity":"info","app":"my-api","data":{"sql":"SELECT * FROM users","duration_ms":3.2}}'
```

Batch endpoint: `POST /ingest/batch` (JSON array). UDP listener on port 8889.

## Architecture

Ten crates in a Cargo workspace:

| Crate | Purpose |
|-------|---------|
| `daemon` | CLI binary and command dispatch. |
| `types` | `Observation`, `Filter`, `Kind`, `Origin`, `Severity` -- shared types. |
| `store` | `SurrealStore` (embedded SurrealDB) + `LensManager` ring buffer. |
| `api` | Axum HTTP routes: ingest, observe, stream, checkpoint, connections, lens, health. |
| `mcp` | MCP server: 14 tools across observe, action, lens, and memory routers. |
| `ingest` | HTTP, UDP, and Unix socket ingest routers + normalization. |
| `chrome` | Chrome DevTools Protocol bridge (raw WebSocket). |
| `adb` | Android Debug Bridge device transport. |
| `embed` | Embedding support (fastembed). |
| `parse` | Observation parsing and extraction. |

## Contributing

Read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before sending a PR. [`TODO.md`](./TODO.md) and the testing gauntlet in [`TESTING.md`](./TESTING.md) are good-first-issue friendly.

By participating, you agree to the [Code of Conduct](./CODE_OF_CONDUCT.md).

## License

[Fair Core License 1.0 with Apache 2.0 Fallback](LICENSES/FCL-1.0-ALv2.txt) (FCL-1.0-ALv2). Full rights for internal use, education, research, and professional services. Restricts competing use. Each release relicenses under Apache 2.0 two years after publication.

DAEMON8 is a trademark of Havy.tech, LLC.

## Contact

- **General / security:** mail@daemon8.ai
- **Discussion:** [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
- **Bugs / features:** [Issues](https://github.com/daemon8ai/daemon8/issues)

Copyright 2026 Havy.tech, LLC.
