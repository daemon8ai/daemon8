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

- [What is Daemon8?](#what-is-daemon8)
- [Install](#install)
- [Quick Start](#quick-start)
- [How It Works](#how-it-works)
- [Auto-Discovery](#auto-discovery)
- [The Librarian](#the-librarian)
- [Reference](#reference)
- [Advanced: Manual Overrides](#advanced-manual-overrides)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

## What is Daemon8?

Daemon8 is a local runtime observation layer for AI coding agents. It runs on your machine, captures browser console, network, JavaScript exceptions, device logcat, application logs, and agent tool calls into one queryable observation stream, and exposes that stream to agents over MCP.

The daemon is project-aware. Point it at a project and it classifies the project type, consults its librarian for source locations it has learned about, connects to everything it knows, and asks the agent in the session to teach it about anything new. Subsequent projects of the same type onboard with zero prompting.

Everything is local. The observation stream and the librarian live in `~/.daemon8/`. Nothing leaves the machine.

> v0.3 alpha — in active development. **[Star the repo](https://github.com/daemon8ai/daemon8)** to follow along.

## Install

```bash
curl -fsSL https://daemon8.ai/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://daemon8.ai/install.ps1 | iex
```

From source:

```bash
cargo install daemon8
```

Pin a version by exporting `DAEMON8_VERSION=v0.3.4` before the curl command. The installer downloads the binary, verifies its SHA-256, and runs `daemon8 setup` to print current state.

> [!WARNING]
> **macOS:** first launch may trigger an "unidentified developer" Gatekeeper prompt and request Background Items and App Management permissions. Both are expected until signed release binaries ship.

## Quick Start

```bash
daemon8 install                 # install as a system service
cd /path/to/your/project
daemon8 setup apply             # register MCP, run discovery scan
```

`daemon8 setup apply` does two things:

1. Registers the daemon8 MCP server with detected AI coding agents (Claude Code, Codex, Gemini CLI).
2. Runs a project-aware discovery scan: classifies the project, looks up source templates the librarian has learned, presents the matched sources, and prompts once.

If the librarian already has templates for this project type, the prompt is a single confirmation. If not, daemon8 asks the agent in your session to investigate and report back via `librarian_index` — a one-time learning step per framework per machine.

Verify:

```bash
daemon8 status
daemon8 doctor
```

For best results, add a directive to your agent's instruction file (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`, `~/.gemini/GEMINI.md`) telling the agent to use daemon8 for runtime observation and debugging.

## How It Works

The core loop is three calls:

```
create_checkpoint --> make a change --> query_observations(since_checkpoint=<id>)
```

Wrap that in a debug session and the investigation — the observations consulted, the root cause identified, the fix applied — persists as searchable memory. Each resolved investigation makes the next one faster.

**Always-on stream.** Every connected source writes into one append-only stream, persisted locally in SurrealDB. HTTP, UDP, Unix socket, MCP, browser CDP, and ADB all normalize to the same `Observation` shape and arrive in the same query surface.

**Opt-in for everything else.** The stream is the foundation. Browser CDP, ADB logcat, file sources, CLI hooks, and project-aware discovery activate explicitly. No surprise data collection.

**Agent-native help.** Every tool response carries optional `next_actions` and `hint` fields that steer agents toward productive follow-up calls without burning conversation turns. `daemon8_help(topic=...)` returns small, topic-specific docs sized for LLM context windows.

## Auto-Discovery

Daemon8 ships with zero hardcoded knowledge of where any framework's logs live. The librarian is the learning store, populated by agents over time.

### What happens on `daemon8 setup apply`

1. **Classify.** Daemon8 reads root manifests and emits tags: `react-native`, `vega`, `kepler`, `nextjs`, `vite`, `tanstack-start`, `rust`, `rust-workspace`, `laravel`, `symfony`, `python`, `django`, `flask`, `fastapi`, `go`, `rails`, `expo`, plus the universal `git-repo` tag. Framework versions are extracted from `package.json`, `composer.json`, `Gemfile.lock`, and `pyproject.toml` where present.

2. **Lookup.** Daemon8 queries the librarian for `source_template` entries whose `project_types` intersect the classification tags, whose `platforms` match the current OS, and whose `version_constraint` accepts the project's framework versions.

3. **Probe.** Each matched template's `locator_pattern` is expanded against `~`, env vars, `<root>`, and globs. Paths that resolve become candidate sources.

4. **Present.** Daemon8 prints the plan and prompts once to register. Confirm registers the sources and writes a `project` node plus `has_source` edges so the next run uses the cached topology.

5. **Ask the agent if needed.** When the librarian has no templates for a project type, daemon8 emits a `discovery_hint` observation. The agent in your session reads the hint via `query_observations`, investigates with shell tools, and calls `librarian_index` with `source_template` entries. Daemon8 then re-enters the scan.

### Discovery escape hatches

```bash
daemon8 discover --complete     # stop waiting for the agent and use what's been written
daemon8 discover --skip         # write .daemon8/skip-discovery; future serves bypass the scan
daemon8 discover --rescan       # remove the skip marker; next serve re-runs discovery
```

### Drift detection

`daemon8 doctor` walks the registered sources, flags any whose paths no longer resolve, and compares the project's current framework versions against the versions captured at registration. When versions differ, the diagnosis names the upgrade as the likely cause and suggests `daemon8 discover --rescan`.

## The Librarian

The librarian is daemon8's primary memory of project topology. It is a relational graph of typed nodes — `source_template`, `source_instance`, `project`, `doc`, `fix` — connected by typed edges (`has_source`, `derived_from`, `supersedes`).

Agents teach the librarian what they discover. The first React Native project a Claude session sees on this machine: the agent investigates, writes `source_template` entries for Metro, the Kepler bridge log, the Expo cache. The second React Native project on this machine: daemon8 reads those templates and applies them without asking the agent anything. The librarian gets richer with use. Daemon8 itself stays thin.

### Template portability

`source_template.locator_pattern` is enforced portable across machines:

- Use `~` for the user home. Literal `/Users/<name>/...` and `/home/<name>/...` are rejected.
- Use `$VAR` or `${VAR}` for OS-specific roots: `$XDG_CONFIG_HOME`, `$LOCALAPPDATA`, `$TMPDIR`.
- Use `<root>` for project-relative paths.
- Glob characters (`*`, `?`, `[...]`) expand at registration.
- `platforms` is explicit. Never imply OS from the path.

Windows absolute user paths (`C:\Users\...`) and UNC paths (`\\server\share`) are rejected at write time. These rules exist so the librarian remains exportable later without schema migration.

### Tools

| Tool | Purpose |
|------|---------|
| `librarian_index` | Catalog a node with kind, tags, and optional edges. |
| `librarian_lookup` | Query by kind, tags, project, text, or hierarchy. |
| `librarian_forget` | Deprecate or remove a node. |

## Reference

<details>
<summary>MCP Tools (29 tools)</summary>

Every tool returns the standard envelope (`{result, daemon8, error}`) with optional `next_actions` and `hint` fields. Call `daemon8_help(topic="envelope")` for the full response format. Destructive operations require explicit confirmation.

| Tool | Purpose |
|------|---------|
| **Observation** | |
| `query_observations` | Filter by kind, severity, origin, text, tags, checkpoint. |
| `status` | Health snapshot: error rate, sources, observation count, version. |
| `list_connections` | Active sources and browser connection state. |
| `subscribe_observations` | Register a real-time alert filter pushed as MCP notifications. |
| `ingest_observation` | Record an observation from inside an agent loop. |
| **Debug Session** | |
| `start_debug_session` | Open an investigation. |
| `create_checkpoint` | Bookmark the stream; pair with `since_checkpoint`. |
| `resolve_debug_session` | Close with a rich summary (root cause, fix, related errors). |
| `end_debug_session` | Close without a fix. |
| `list_debug_sessions` | Enumerate sessions by status or feature. |
| **Action** | |
| `connect_browser` | Attach to a Chrome DevTools endpoint. |
| `issue_command` | Eval JS, screenshot, CSS inject, storage, navigate, viewport, network throttle, tabs. |
| **Lens** | |
| `set_lens` | Install a focused filter that buffers matches (max 1000). |
| `lens_status` | Inspect active filter and buffer depth. |
| `clear_lens` | Remove the active lens. |
| **Memory** | |
| `save_memory` | Store a pattern, decision, error signature, session summary, or user-flagged note. |
| `query_memory` | Search by kind, tags, project, or text. |
| `forget_memory` | Delete by id (requires confirmation). |
| **Librarian** | |
| `librarian_index` | Catalog a reference node with tags and edges. |
| `librarian_lookup` | Query the catalog by kind, tags, project, text, or hierarchy. |
| `librarian_forget` | Deprecate or remove a reference. |
| **Setup** | |
| `setup_status` | Current setup state for the project. |
| `setup_plan` | Preview the action plan. |
| `setup_apply` | Apply the plan (requires confirmation). |
| **Hooks** | |
| `hooks_list` | Enumerate installed hooks across providers and scopes. |
| `hooks_remove` | Uninstall daemon8 hooks from a provider. |
| `hooks_update` | Reinstall hooks (fixes drift after binary moves). |
| `hooks_repair` | Detect drift and reinstall only stale hooks. |
| **Help** | |
| `daemon8_help` | Topic-specific docs: checkpoint, debug_session, envelope, hooks, lens, librarian, memory, observations, setup. |

</details>

<details>
<summary>HTTP API</summary>

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/observe` | Query observations. |
| GET | `/api/checkpoint` | Current checkpoint id. |
| GET | `/api/summary` | Runtime summary. |
| GET | `/api/connections` | Active sources and browser state. |
| POST | `/api/connect` | Set Chrome DevTools endpoint. |
| GET | `/api/stream` | SSE observation stream with reconnection support. |
| GET / PUT / DELETE | `/api/lens` | Inspect, set, or clear the active lens. |
| POST | `/api/browser/act` | Browser actions (same surface as `issue_command`). |
| GET / POST | `/api/memory` | Query or save memory. |
| POST | `/api/memory/export` | Stream memory export as NDJSON. |
| POST | `/api/discover/complete` | Signal the running scanner to stop waiting. |
| POST | `/api/discover/skip` | Signal the running scanner to abort discovery. |
| POST | `/ingest` | Single observation ingest. |
| POST | `/ingest/batch` | Batch observation ingest. |
| GET | `/health` | Health probe. |

UDP listener on port 8889 accepts the same JSON shapes for fire-and-forget telemetry.

</details>

<details>
<summary>CLI Reference</summary>

| Command | Description |
|---------|-------------|
| `daemon8 serve` | Start the daemon (default when no subcommand). |
| `daemon8 status` | Daemon health and status. |
| `daemon8 tail` | Stream observations in real-time. |
| `daemon8 query` | Query stored observations. |
| `daemon8 connections` | List active data sources. |
| `daemon8 browser` | Browser DevTools commands. |
| `daemon8 lens` | Manage observation lens. |
| `daemon8 memory` | Export memory query results. |
| `daemon8 logs` | Show log location or tail logs. |
| `daemon8 config` | Show or modify configuration. |
| `daemon8 completions` | Generate shell completions. |
| `daemon8 setup apply` | Register MCP with detected agents, run discovery scan. |
| `daemon8 setup features` | Enable optional features interactively. |
| `daemon8 setup init` | Write `.daemon8.toml` for explicit source overrides. |
| `daemon8 service install` | Install daemon8 as a system service. |
| `daemon8 service uninstall` | Remove the system service. |
| `daemon8 hooks` | Manage CLI hooks (`list`, `remove`, `update`, `repair`). |
| `daemon8 discover --complete` | Stop waiting for the agent; use what's been written. |
| `daemon8 discover --skip` | Bypass the discovery scan for this project. |
| `daemon8 discover --rescan` | Re-run the discovery scan on next serve. |
| `daemon8 channel` | Real-time alert relay for MCP clients (experimental). |
| `daemon8 doctor` | Diagnose project state and source drift (`--fix` to auto-repair). |

</details>

## Advanced: Manual Overrides

Auto-discovery is the default path. The librarian learns what your machine's projects need over time, and most users never write a `.daemon8.toml`.

Override when you need explicit control: a non-standard log location, a private SQLite file, a one-off conversation source. Entries in `.daemon8.toml` always take precedence over auto-discovered sources.

### `.daemon8.toml` source kinds

```toml
[sources.api-logs]
type = "file"
path = "/var/log/my-api/app.log"
parser = "auto"
tags = ["api"]

[sources.codex-db]
type = "sqlite"
path = "~/.codex/conversations.db"
provider = "codex"
poll_interval_secs = 60
tags = ["conversation", "codex"]

[sources.claude]
type = "conversation"
provider = "claude"
tags = ["conversation"]
```

Three source types are supported: `file` (tails any log with built-in parsers for JSON, syslog, monolog, logfmt, CLF, grok, or auto-detect), `conversation` (watches an AI provider's transcript directory using the provider's known filesystem layout), and `sqlite` (polls a SQLite database — used for providers like Codex that write their transcripts there).

`daemon8 setup init` writes a starter `.daemon8.toml` if you prefer template-driven configuration over auto-discovery.

### ADB

ADB device monitoring is opt-in. Set `[adb] enabled = true` in `~/.config/daemon8/config.toml`. With ADB enabled, daemon8 streams logcat from connected Android devices into the observation stream.

### CLI hooks

Hooks record agent tool calls as `tool_call` observations linked via `correlation_id`. They are useful when you want a full audit trail of what an agent did and when, independent of the conversation file.

```bash
daemon8 setup apply --install-hooks local    # project-level
daemon8 setup apply --install-hooks global   # machine-level
daemon8 hooks list
daemon8 hooks repair
```

## Architecture

```
Sources (inputs)                          Agents (outputs)
-----------------                         ----------------
Browser (CDP)   ---\                       /--  Codex
Applications    ---->  daemon8            ----> Claude Code
Devices (ADB)   ---/   [localhost:8888]    \--> Gemini CLI
CLI hooks       --/
```

Cargo workspace:

| Crate | Role |
|-------|------|
| `daemon` | CLI binary, command dispatch, runtime wiring (the published `daemon8` binary). |
| `types` | Shared types: `Observation`, `Filter`, `Severity`, librarian and discovery types. |
| `store` | SurrealDB backend: observation store, librarian, debug sessions, memory, lens manager. |
| `api` | Axum HTTP routes for observation query, SSE, and health. |
| `mcp` | MCP server with the 29 tools listed above. |
| `ingest` | HTTP, UDP, and Unix socket ingestion endpoints. |
| `chrome` | Chrome DevTools Protocol bridge. |
| `adb` | Android Debug Bridge transport. |
| `parse` | Log parser trait and built-in format parsers. |
| `providers` | AI provider detection, MCP registration, project type detection. |

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
