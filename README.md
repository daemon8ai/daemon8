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
- [Features](#features)
- [Reference](#reference)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

## What is Daemon8?

AI agents write and run code without seeing what happens at runtime. Console errors, network failures, device crashes, and application exceptions happen out of view -- leaving agents to guess, or ask the user to check.

Daemon8 is a local runtime layer that captures these signals into one observation stream as code is being written and executed. Browser console output, network traffic, application logs, device events, and agent tool calls all land in the same queryable feed. Agents connect over MCP and see what's happening without leaving their workflow.

But observation is just the starting point. As agents investigate bugs, daemon8 records the investigation -- which observations led to the root cause, what fix was applied, which commands were tried. These records persist as searchable memory. A built-in reference catalog links fixes, documentation, and project context into a graph that spans across projects. Over time, similar errors get diagnosed faster because the path to the fix is already indexed.

Situational awareness is the foundation of a larger system. Some of what daemon8 is being built toward is public; some isn't yet.

> v0.3 alpha -- in active development. If the concept resonates, **[star the repo](https://github.com/daemon8ai/daemon8)** to follow along.

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

Pin a specific version with `DAEMON8_VERSION=v0.3.4` before the curl command.

The installer downloads the binary, verifies its SHA-256 checksum, and runs `daemon8 setup` to show current state.

> [!WARNING]
> **macOS:** first launch may trigger an "unidentified developer" Gatekeeper prompt and request Background Items and App Management permissions. Both are expected until signed release binaries ship.

## Quick Start

```bash
daemon8 install        # register as a system service
daemon8 init           # create .daemon8.toml in the current project
daemon8 setup apply    # register sources and configure AI providers
```

`daemon8 features` opens an interactive menu for enabling hooks and project scaffolding.

For best results, add daemon8 instructions to your AI context file (`~/.claude/CLAUDE.md`, `~/.gemini/GEMINI.md`, or `~/.codex/AGENTS.md`) telling the agent to use daemon8 for runtime observation, debugging, and note-taking.

Verify everything is running:

```bash
daemon8 status
daemon8 doctor
```

## How It Works

**Everything is local.** The daemon runs on your machine. Runtime data and curated knowledge stay local. Nothing phones home.

**Opt-in by design.** The observation stream is always on -- it's the foundation. Every other feature is activated through the CLI when you need it. No surprise data collection, no background scanning. You choose what feeds the stream and what the agent can act on.

**Agent-native help.** Daemon8 includes a built-in help system designed for LLM context windows -- small, topic-specific documents rather than large reference pages. Every tool response carries optional steering hints that guide agents toward productive next steps without extra conversation. When an agent saves a memory about a fix, daemon8 suggests cataloging it as a reference. When a checkpoint is created outside a debug session, daemon8 suggests starting one. As the catalog grows, daemon8 prompts agents to engage the user -- naming recurring bugs, reviewing duplicate context, organizing references into a structure that makes sense for the project.

**The core loop:**

```
checkpoint --> make a change --> query what happened
```

Wrap that in a debug session and the investigation -- observations, root cause, fix, resolution -- persists as searchable memory. Each resolved investigation makes the next one faster.

## Features

### Observation Stream

All runtime signals land in one feed: application logs, browser console output, network failures, device events, CLI hook telemetry, and agent-emitted notes. The stream is always on and accepts observations over HTTP, UDP, and Unix socket.

```bash
daemon8 tail                                    # live-stream
daemon8 query --kinds log,exception --limit 20  # query stored
```

<details>
<summary>More on observations</summary>

#### Observation kinds

`log`, `query`, `http_exchange`, `exception`, `js_exception`, `state_snapshot`, `metric`, `lifecycle`, `tool_call`, `custom`.

#### Ingestion

```bash
curl -X POST http://localhost:8888/ingest -H 'Content-Type: application/json' -d '{"kind":"log","severity":"info","app":"my-api","data":{"message":"deploy complete"}}'
```

HTTP (`POST /ingest`), UDP (port 8889), and Unix socket endpoints all accept the same JSON shape. Batch ingestion is available at `POST /ingest/batch`.

#### Subscriptions

`subscribe_observations` registers a real-time alert filter. Matching observations push directly into agent sessions as MCP notifications -- no polling required.

#### Replay

The SSE stream supports reconnection without missing events.

</details>

### Checkpoints

Bookmark the stream before making a change, then query only what happened after. This is the simplest and most frequently used pattern in daemon8.

```
create_checkpoint --> make a change --> query_observations(since_checkpoint=<id>)
```

No setup required. Available immediately.

### Debug Sessions

A debug session wraps one or more checkpoints into a structured investigation. Observations captured during the session are linked to it. On resolution, the root cause, fix, and related errors are written as searchable memory -- so when the same error surfaces again, the path to the fix is already recorded.

Multiple agents -- Claude Code, Codex, Gemini CLI -- can each run their own debug session on the same project simultaneously. Agents discover overlapping work by querying sessions filtered by feature.

<details>
<summary>More on debug sessions</summary>

#### Lifecycle

1. `start_debug_session` -- open an investigation, scoped to this agent's MCP session
2. `create_checkpoint` -- bookmark moments within the session
3. `resolve_debug_session` -- close with a rich summary (root cause, fix diff, commands tried, related errors)
4. `end_debug_session` -- close without a fix

#### Safety nets

Sessions with no activity for 4 hours are automatically marked abandoned. Observations linked to active sessions are never cleaned up, regardless of age.

#### Multi-agent coordination

Declare a feature when starting: `start_debug_session(feature="auth")`. Other agents discover this with `list_debug_sessions(feature="auth")` -- preventing duplicate investigations across concurrent sessions.

</details>

### Memory

Verified fixes, error signatures, patterns, and architectural decisions persist in a local memory store. They survive observation cleanup and compound across sessions.

New error signatures are cataloged with a normalized hash. When an agent resolves a debug session that references the error, the fix and the signature are linked -- making the resolution retrievable by hash across projects.

```bash
daemon8 memory export --kind pattern --project my-app
```

<details>
<summary>More on memory</summary>

#### Kinds

- `pattern` -- recurring code or architecture pattern
- `decision` -- architectural decision and its rationale
- `error_signature` -- normalized error with hash tag for cross-referencing
- `session_summary` -- written by `resolve_debug_session` (rich) or `end_debug_session` (thin)
- `user_flagged` -- general-purpose "remember this"

#### Error signature linking

Every error observation carries a normalized `error_hash`. Tag `hash:<x>` joins error signatures with their fix summaries. `query_memory(tags=["hash:<x>"])` returns both the signature and any session summaries whose resolution referenced that hash.

#### Tools

- `save_memory` -- persist an insight with kind, tags, and project scope
- `query_memory` -- search by kind, tags, project, or text
- `forget_memory` -- delete by id (requires confirmation)

</details>

### Librarian

The librarian is a reference catalog -- not for storing information, but for knowing where to find it. Think of it as the index system for a library: it knows which shelf every book is on, which chapter covers the topic you need, and which edition supersedes the last.

Agents index pointers to documentation, source configurations, known fixes, and projects as nodes in a relational graph. Typed edges link them: a project has sources, sources are documented by docs, fixes resolve specific errors. The catalog spans projects -- a fix indexed while working on one codebase is queryable from any other. As context builds, the graph enables increasingly specific lookups, moving toward the kind of granular retrieval where "this function throws this exception with this message" resolves to a specific fix with specific steps.

Available by default with `daemon8 serve`.

<details>
<summary>More on the librarian</summary>

#### What gets cataloged

- **doc** -- documentation, READMEs, wiki pages, API references
- **source_template** -- log source configurations, parser templates
- **fix** -- known fix recipes, workarounds, error resolutions
- **project** -- project entry points, workspace roots

#### Relationships

Nodes connect through typed edges: `has_source`, `documented_by`, `fixes`, `supersedes`, `child_of`. The graph model enables traversal -- from a project to its docs, from an error to its fix, from a fix to the investigation that produced it.

#### Versioning and lifecycle

Re-indexing the same reference creates a new version, deprecates the old one, and links them with a `supersedes` edge. Every node tracks when it was created, last updated, last accessed, and deprecated. Stale entries -- those not accessed in 30+ days -- surface automatically for review.

#### Hierarchy

Nodes can be organized under parent nodes, forming a navigable tree. Daemon8 prompts agents to suggest organization when categories grow large, and flags when nesting gets too deep.

#### Tools

- `librarian_index` -- catalog a reference with tags and optional edges
- `librarian_lookup` -- query by kind, tags, project, text, or browse the hierarchy
- `librarian_forget` -- deprecate (default) or remove a reference

</details>

### Lenses

A lens is a focused filter that collects matching observations between queries. Set it before making a change, and subsequent queries automatically include buffered matches -- without polling.

No setup required.

```bash
daemon8 lens set --kinds exception --severity-min warn
daemon8 lens status
daemon8 lens clear
```

<details>
<summary>More on lenses</summary>

A lens buffers up to 1000 matching observations. When you call `query_observations`, the buffered matches appear in a `lens_observations` array alongside the regular query results, then the buffer resets.

Use a lens when the observation stream is high-volume but you only care about a narrow slice -- exceptions during a deploy, warnings from a specific service, network failures to a particular endpoint.

</details>

### Browser Actions

Connect to Chrome DevTools Protocol to observe console output, network requests, JS exceptions, and lifecycle events. Run JavaScript, capture screenshots, inject CSS, navigate, and inspect storage -- all from the agent's MCP session.

**Enable:** `daemon8 browser connect` or call the `connect_browser` MCP tool.

### CLI Hooks

Capture what your agent does. Hooks record tool calls, file edits, and command runs as structured observations -- giving agents cause-and-effect context across their own actions.

**Enable:**

```bash
daemon8 setup apply --install-hooks local   # project-level
daemon8 setup apply --install-hooks global  # machine-level
```

<details>
<summary>More on hooks</summary>

Hooks support Claude Code, Codex, Gemini CLI, and OpenCode. Each tool call becomes a `tool_call` observation linked via `correlation_id`, enabling full agent audit trails.

Manage installed hooks:

```bash
daemon8 hooks list
daemon8 hooks update
daemon8 hooks repair
```

</details>

### File Sources

Tail log files and parse them into structured observations. Built-in parsers handle JSON, syslog, monolog, logfmt, CLF, grok patterns, and auto-detection.

**Enable:** add `[sources.*]` entries to `.daemon8.toml`:

```toml
[sources.api-logs]
path = "/var/log/my-api/app.log"
parser = "auto"
tags = ["api"]
```

### Device Monitoring (ADB)

Stream logcat from Android devices connected via ADB.

**Enable:** set `[adb] enabled = true` in `~/.config/daemon8/config.toml`.

---

## Reference

<details>
<summary>MCP Tools (29 tools)</summary>

Every tool returns the standard envelope (`{result, daemon8, error}`) with optional `next_actions` and `hint` fields for agent steering. Call `daemon8_help(topic="envelope")` for the full response format. Destructive operations require explicit confirmation.

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
| GET&nbsp;/&nbsp;PUT&nbsp;/&nbsp;DELETE | `/api/lens` | Inspect, set, or clear the active lens. |
| POST | `/api/browser/act` | Browser actions (same surface as `issue_command`). |
| GET&nbsp;/&nbsp;POST | `/api/memory` | Query or save memory. |
| POST | `/api/memory/export` | Stream memory export as NDJSON. |
| POST | `/ingest` | Single observation ingest. |
| POST | `/ingest/batch` | Batch observation ingest. |
| GET | `/health` | Health probe. |

UDP listener on port 8889 accepts the same JSON shapes for fire-and-forget telemetry.

</details>

<details>
<summary>CLI Reference</summary>

| Command | Description |
|---------|-------------|
| `daemon8 serve` | Start the daemon. |
| `daemon8 status` | Daemon health and status. |
| `daemon8 tail` | Stream observations in real-time. |
| `daemon8 query` | Query stored observations. |
| `daemon8 connections` | List active data sources. |
| `daemon8 browser` | Browser DevTools commands. |
| `daemon8 lens` | Manage observation lens. |
| `daemon8 memory` | Export memory to Markdown files. |
| `daemon8 logs` | Show log location or tail logs. |
| `daemon8 config` | Show or modify configuration. |
| `daemon8 completions` | Generate shell completions. |
| `daemon8 install` | Install as a system service. |
| `daemon8 uninstall` | Remove the system service. |
| `daemon8 setup` | Guided setup. |
| `daemon8 channel` | Real-time alert relay for MCP clients. |
| `daemon8 doctor` | Diagnose environment (`--fix` to auto-repair). |
| `daemon8 init` | Scaffold `.daemon8.toml` in the current project. |
| `daemon8 hooks` | Manage CLI hooks (`list \| remove \| update \| repair`). |
| `daemon8 features` | Interactive feature activation menu. |

</details>

## Architecture

```
Sources (inputs)                          Agents (outputs)
-----------------                         ----------------
Browser (CDP)   ---\                       /--  Codex
Applications    ---->  daemon8            ----> Claude Code
Devices (ADB)   ---/   [localhost:8888]    \--> Gemini CLI
CLI hooks       --/
```

Sources push observations into the daemon. Agents query, subscribe, and act through MCP or the HTTP API.

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
