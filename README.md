<p align="center">
  <img src="logo.svg" alt="Daemon8" width="360">
</p>

<p align="center">
  <strong>A runtime observation layer for AI coding agents.</strong><br>
  One local stream for logs, network, browser, and device output — queried by agents over MCP.
</p>

<p align="center">
  <a href="https://daemon8.ai">Website</a> ·
  <a href="https://daemon8.ai/docs">Docs</a> ·
  <a href="#sdk-libraries">SDKs</a> ·
  <a href="https://github.com/daemon8ai/daemon8/discussions">Discussions</a> ·
  <a href="mailto:mail@daemon8.ai">Contact</a>
</p>

<p align="center">
  <em>Free and open source. No tiers, no license keys, no phone-home.</em>
</p>

---

> [!IMPORTANT]
> **Daemon8 is under active development.** APIs, docs, and tooling are moving
> quickly. See [`TODO.md`](./TODO.md) for the current help-wanted list —
> documentation, testing gauntlet tracks, and SDK demos all have work sized
> for contributors.

## What is Daemon8?

LLM coding agents can read files and run tests, but they cannot see what happens when your code actually executes — the SQL queries, the HTTP responses, the console errors in the browser, the exceptions. The developer becomes the feedback loop, describing runtime behavior in chat.

Daemon8 is a small local service that removes that loop. It collects runtime output from your applications (any language, over HTTP), from your browser (via the DevTools Protocol), and from connected devices (Android, Fire TV) into one queryable stream on your machine. Any AI agent that speaks MCP — Claude Code, Cursor, Continue.dev — connects to the stream and gets real-time visibility into what your program is doing.

Everything stays on your machine. Nothing is sent anywhere.

## Getting started

Two install paths. Pick whichever matches your comfort level.

### Option A — Fastest (coming soon)

> Signed release binaries ship with `v0.1.0`. Until then, use Option B.

```bash
cargo install cargo-binstall
cargo binstall daemon8
daemon8 setup
```

### Option B — Build from source (works today, 4 commands)

```bash
# 1. Build and install the daemon binary from this repo.
cargo install --git https://github.com/daemon8ai/daemon8 daemon8

# 2. macOS only: sign your local build so Gatekeeper and launchd accept it.
codesign --force --sign - ~/.cargo/bin/daemon8

# 3. Register daemon8 as a user-level system service.
#    (launchd on macOS, systemd on Linux, Task Scheduler on Windows.)
daemon8 install

# 4. One-shot wizard: finds your browser, configures Claude Code / Cursor /
#    Windsurf / Gemini CLI, and confirms the daemon is responsive.
daemon8 setup
```

> [!WARNING]
> **macOS:** the first launch will show an "unidentified developer" Gatekeeper
> warning and may ask for two permissions (Background Items, App Management).
> Both are expected until we ship signed release binaries. Click "Allow" — the
> daemon is entirely local and nothing leaves your machine.

> [!NOTE]
> **Windows:** the build-from-source path works (Task Scheduler gets a
> user-level task at logon). Pre-built signed binaries for Windows are still a
> work in progress.

## Verify and connect a client

```bash
daemon8 status           # expect: Daemon: running
daemon8 doctor           # expect: No issues detected
```

Any MCP-compatible client connects to:

```
http://localhost:9077/mcp
```

`daemon8 setup` auto-configures the common ones. Eight tools are exposed:

| Tool                | Purpose                                                         |
|---------------------|-----------------------------------------------------------------|
| `debug_observe`     | Query recent observations (kind, severity, origin, text match). |
| `debug_summary`     | Health snapshot: error rate, active sources, observation count. |
| `debug_checkpoint`  | Mark current stream position; subsequent reads resume from it.  |
| `debug_connections` | List connected ingestion sources and browser state.             |
| `debug_connect`     | Point the daemon at a Chrome DevTools endpoint.                 |
| `debug_act`         | Drive the browser: eval JS, screenshot, inject CSS, navigate.   |
| `debug_ingest`      | Record an observation from inside the agent loop.               |
| `debug_subscribe`   | Subscribe to a filtered real-time alert stream.                 |

Tested clients: Claude Code, Cursor, Continue.dev. Any MCP client over HTTP/SSE or stdio works.

## Send observations from your applications

Any language with an HTTP client:

```bash
curl -X POST http://localhost:9077/ingest \
  -H 'Content-Type: application/json' \
  -d '{"kind":"query","severity":"info","app":"my-api",
       "data":{"sql":"SELECT * FROM users","duration_ms":3.2}}'
```

Batch ingest, optional UDP, and optional Unix socket listeners are documented in [`daemon/README.md`](./daemon/README.md).

## SDK libraries

<details>
<summary><strong>Available and on-roadmap SDKs</strong> (click to expand)</summary>

<br>

**Available today:**

- **PHP** — [daemon8ai/daemon8-php](https://github.com/daemon8ai/daemon8-php) — `composer require daemon8/sdk`
- **Laravel** — [daemon8ai/daemon8-laravel](https://github.com/daemon8ai/daemon8-laravel) — `composer require daemon8/laravel`
- **Symfony** — [daemon8ai/daemon8-symfony](https://github.com/daemon8ai/daemon8-symfony) — `composer require daemon8/symfony`

**On roadmap:**

- JavaScript / TypeScript — on roadmap
- Python — on roadmap
- Rust (client SDK, distinct from the daemon binary) — on roadmap
- .NET — on roadmap
- Go — on roadmap

Community SDK contributions welcome — open a discussion at
[GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions) before starting a new SDK, so the observation envelope stays consistent across languages.

</details>

## Example projects

Minimal runnable demos showing how to wire each SDK into a real application:

- **PHP:** [daemon8ai/daemon8-demo-php](https://github.com/daemon8ai/daemon8-demo-php)
- **Laravel:** [daemon8ai/daemon8-demo-laravel](https://github.com/daemon8ai/daemon8-demo-laravel)
- **Symfony:** [daemon8ai/daemon8-demo-symfony](https://github.com/daemon8ai/daemon8-demo-symfony)

Demo improvements are tracked in [`TODO.md`](./TODO.md#sdk-demo-improvements).

## The daemon8 ecosystem

Every public repository under the [`daemon8ai`](https://github.com/daemon8ai) GitHub organization:

| Repository | Purpose |
|---|---|
| [`daemon8ai/daemon8`](https://github.com/daemon8ai/daemon8) | Rust daemon binary (this repo) |
| [`daemon8ai/ui`](https://github.com/daemon8ai/ui) | Marketing site and canonical documentation |
| [`daemon8ai/daemon8-php`](https://github.com/daemon8ai/daemon8-php) | Framework-agnostic PHP SDK |
| [`daemon8ai/daemon8-laravel`](https://github.com/daemon8ai/daemon8-laravel) | Laravel integration |
| [`daemon8ai/daemon8-symfony`](https://github.com/daemon8ai/daemon8-symfony) | Symfony bundle |
| [`daemon8ai/daemon8-demo-php`](https://github.com/daemon8ai/daemon8-demo-php) | PHP demo app |
| [`daemon8ai/daemon8-demo-laravel`](https://github.com/daemon8ai/daemon8-demo-laravel) | Laravel demo app |
| [`daemon8ai/daemon8-demo-symfony`](https://github.com/daemon8ai/daemon8-demo-symfony) | Symfony demo app |

## Docs

[Docs](https://daemon8.ai/docs) — source code at [Docs — Source Code](https://github.com/daemon8ai/ui/tree/main/content/docs/).

<details>
<summary><strong>Help us finish the docs</strong> (click to expand)</summary>

<br>

Parts of the daemon's real surface aren't documented yet. Each item below is a good-first-issue-sized piece of work. The full breakdown lives in [`TODO.md`](./TODO.md#documentation-gaps); issues are filed at [daemon8ai/ui/issues](https://github.com/daemon8ai/ui/issues) under the `docs` label.

**CLI gaps** — `daemon8 init`, `daemon8 doctor`, `daemon8 channel`, `daemon8 completions`, `daemon8 tail`, `daemon8 query`, `daemon8 connections`, `daemon8 logs`, `daemon8 config`.

**HTTP / transport gaps** — `POST /ingest/batch`, `GET /api/stream` (with `Last-Event-ID` resume and gap markers), `GET /health`, `POST /api/connect`, the optional UDP listener, the optional Unix socket listener.

**Schema gaps** — field specs for `HttpExchange`, `JsException`, `Lifecycle`, `Query`, `Exception`, `StateSnapshot`, `Metric`, `Custom`; origin patterns `app:<name>`, `browser:<tab_id>`, `device:<serial>`.

**Stale content** — delete `free-vs-pro.mdx`; correct `reference.mdx` (remove non-existent license subcommands); correct `mcp-tools.mdx` (drop Free/Pro split); correct `configuration.mdx` (retention is 24h, remove telemetry language); rewrite `telemetry-and-privacy.mdx` (local-only, no phone-home).

</details>

## Contributing

Read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before sending a PR. The [`TODO.md`](./TODO.md) list and the Testing Gauntlet in [`TESTING.md`](./TESTING.md) are both set up to be good-first-issue friendly — start there if you're looking for a first merged PR.

By participating, you agree to the [Code of Conduct](./CODE_OF_CONDUCT.md). Enforcement: `mail@daemon8.ai`.

## Signing & trust

Until `v0.1.0`, release binaries are not yet code-signed. Builds from `cargo install --git` carry an ad-hoc signature and trigger macOS Gatekeeper's "unidentified developer" prompt on first launch.

From `v0.1.0` forward, GitHub Release binaries are code-signed with an Apple Developer ID (Team `4WT356MQPL`, Jonathan Havens) and notarized by Apple. From `v0.2.0` forward, releases ship under Havy.tech, LLC's organizational Developer ID once enrollment completes.

Verify a signed binary:

```bash
codesign -dvv $(which daemon8) | grep Authority
```

## License

- `daemon/` — [Fair Core License 1.0 with Apache 2.0 Fallback](LICENSES/FCL-1.0-ALv2.txt) (FCL-1.0-ALv2). Grants full rights for internal use, education, research, and professional services; restricts competing use. Each release relicenses under Apache 2.0 two years after publication.
- SDKs (sibling repos) — MIT.

## Contact

- **General / security:** mail@daemon8.ai
- **Discussion / show-and-tell:** [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
- **Bug reports & feature requests:** [issue templates](.github/ISSUE_TEMPLATE/)

Copyright © 2026 Havy.tech, LLC.
