<p align="center">
  <img src="logo.svg" alt="Daemon8" width="360">
</p>

<p align="center">
  <strong>The admin layer for AI agents.</strong><br>
  Observe. Act. Coordinate.<br>
  One convention for runtime output and real-time context.
</p>

<p align="center">
  <a href="https://daemon8.ai">Website</a> ·
  <a href="https://daemon8.ai/docs">Docs</a> ·
  <a href="https://github.com/daemon8ai/daemon8/discussions">Discussions</a> ·
  <a href="mailto:mail@daemon8.ai">Contact</a>
</p>

<p align="center">
  <em>Free and open source. No tiers, no license keys, no phone-home.</em>
</p>

> [!TIP]
> Daemon8 is 'closing the loop' - agents will be able to debug anything in one MCP call - that's the objective! Care to help ensure this type of power stays open source?? Consider starring the repo!

---

> [!NOTE]
> Daemon8 is under active development.

## What is Daemon8?

As smart as LLMs are getting, how long does one reasonably expect the "logging mess" to last? Send logs to one place for agents to consume, query, and collaborate — that's what Daemon8 is.

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

| Tool                     | Purpose                                                         |
|--------------------------|-----------------------------------------------------------------|
| `query_observations`     | Query recent observations (kind, severity, origin, text match). |
| `status`                 | Health snapshot: error rate, active sources, observation count. |
| `create_checkpoint`      | Mark current stream position; subsequent reads resume from it.  |
| `list_connections`       | List connected ingestion sources and browser state.             |
| `connect_browser`        | Point the daemon at a Chrome DevTools endpoint.                 |
| `issue_command`          | Run browser/device actions: eval JS, screenshots, CSS, storage. |
| `ingest_observation`     | Record an observation from inside the agent loop.               |
| `subscribe_observations` | Subscribe to a filtered real-time alert stream.                 |

Tested clients: Claude Code, Cursor, Continue.dev. Any MCP client over HTTP/SSE or stdio works.

## Send observations from your applications

Any language with an HTTP client:

```bash
curl -X POST http://localhost:9077/ingest \
  -H 'Content-Type: application/json' \
  -d '{"kind":"query","severity":"info","app":"my-api",
       "data":{"sql":"SELECT * FROM users","duration_ms":3.2}}'
```

Batch ingest, optional UDP, and optional Unix socket listeners are documented at [daemon8.ai/docs](https://daemon8.ai/docs).

## Docs

Docs render at [daemon8.ai/docs](https://daemon8.ai/docs). Source lives in [`daemon8ai/daemon8-site`](https://github.com/daemon8ai/daemon8-site).

Contributions to the docs are welcome. Corrections, clearer examples, and new pages for edges of the surface we haven't covered yet all help. File issues at [daemon8ai/daemon8/issues](https://github.com/daemon8ai/daemon8/issues) under the `docs` label.

## Contributing

Read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before sending a PR. The [`TODO.md`](./TODO.md) list and the Testing Gauntlet in [`TESTING.md`](./TESTING.md) are both set up to be good-first-issue friendly — start there if you're looking for a first merged PR.

By participating, you agree to the [Code of Conduct](./CODE_OF_CONDUCT.md). Enforcement: `mail@daemon8.ai`.

## Signing & trust

Until `v0.1.0`, release binaries are not yet code-signed. Builds from `cargo install --git` carry an ad-hoc signature and trigger macOS Gatekeeper's "unidentified developer" prompt on first launch.

From `v0.1.0` forward, GitHub Release binaries are code-signed with an Apple Developer ID and notarized by Apple. From `v0.2.0` forward, releases ship under Havy.tech, LLC's organizational Developer ID once enrollment completes.

Verify a signed binary:

```bash
codesign -dvv $(which daemon8) | grep Authority
```

## Trademark

DAEMON8™ is a trademark of Havy.tech, LLC. U.S. trademark application pending. Policy inquiries: `mail@daemon8.ai`.

## License

[Fair Core License 1.0 with Apache 2.0 Fallback](LICENSES/FCL-1.0-ALv2.txt) (FCL-1.0-ALv2). Grants full rights for internal use, education, research, and professional services; restricts competing use. Each release relicenses under Apache 2.0 two years after publication.

## Contact

- **General / trademark / security:** mail@daemon8.ai
- **Discussion / show-and-tell:** [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
- **Bug reports & feature requests:** [issue templates](.github/ISSUE_TEMPLATE/)

Copyright © 2026 Havy.tech, LLC.
