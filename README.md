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

Send runtime output to one local stream for agents to consume, query, and collaborate.

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

## SDK libraries

<details>
<summary><strong>Available and on-roadmap SDKs</strong> (click to expand)</summary>

<br>

**Available today:**

- **PHP** — [daemon8ai/daemon8-php](https://github.com/daemon8ai/daemon8-php) — `composer require daemon8/php`
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
| [`daemon8ai/daemon8`](https://github.com/daemon8ai/daemon8) | Rust daemon binary and canonical documentation (this repo) |
| [`daemon8ai/daemon8-php`](https://github.com/daemon8ai/daemon8-php) | Framework-agnostic PHP SDK |
| [`daemon8ai/daemon8-laravel`](https://github.com/daemon8ai/daemon8-laravel) | Laravel integration |
| [`daemon8ai/daemon8-symfony`](https://github.com/daemon8ai/daemon8-symfony) | Symfony bundle |
| [`daemon8ai/daemon8-demo-php`](https://github.com/daemon8ai/daemon8-demo-php) | PHP demo app |
| [`daemon8ai/daemon8-demo-laravel`](https://github.com/daemon8ai/daemon8-demo-laravel) | Laravel demo app |
| [`daemon8ai/daemon8-demo-symfony`](https://github.com/daemon8ai/daemon8-demo-symfony) | Symfony demo app |

## Docs

Docs render at [daemon8.ai/docs](https://daemon8.ai/docs). Source lives in [`daemon8ai/daemon8-site`](https://github.com/daemon8ai/daemon8-site).

Contributions to the docs are welcome. Corrections, clearer examples, and new pages for edges of the surface we haven't covered yet all help. File issues at [daemon8ai/daemon8/issues](https://github.com/daemon8ai/daemon8/issues) under the `docs` label.

## Contributing

Read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before sending a PR. The [`TODO.md`](./TODO.md) list and the Testing Gauntlet in [`TESTING.md`](./TESTING.md) are both set up to be good-first-issue friendly — start there if you're looking for a first merged PR.

By participating, you agree to the [Code of Conduct](./CODE_OF_CONDUCT.md). Enforcement: `mail@daemon8.ai`.

## Signing & trust

Builds from `cargo install --git` carry an ad-hoc signature and trigger macOS Gatekeeper's "unidentified developer" prompt on first launch.

GitHub Release binaries will be code-signed with an Apple Developer ID and notarized by Apple once the release workflow is finalized.

Verify a signed binary:

```bash
codesign -dvv $(which daemon8) | grep Authority
```

## Trademark

DAEMON8™ is a trademark of Havy.tech, LLC. U.S. trademark application pending. Policy inquiries: `mail@daemon8.ai`.

## License

[Fair Core License 1.0 with Apache 2.0 Fallback](LICENSES/FCL-1.0-ALv2.txt) (FCL-1.0-ALv2). Grants full rights for internal use, education, research, and professional services; restricts competing use. Each release relicenses under Apache 2.0 two years after publication.

SDK packages ([daemon8-php](https://github.com/daemon8ai/daemon8-php), [daemon8-laravel](https://github.com/daemon8ai/daemon8-laravel), [daemon8-symfony](https://github.com/daemon8ai/daemon8-symfony)) are MIT-licensed.

## Contact

- **General / trademark / security:** mail@daemon8.ai
- **Discussion / show-and-tell:** [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
- **Bug reports & feature requests:** [issue templates](.github/ISSUE_TEMPLATE/)

Copyright © 2026 Havy.tech, LLC.
