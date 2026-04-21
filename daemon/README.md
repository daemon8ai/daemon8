# Daemon8

A runtime observation layer for AI coding agents.

LLM coding assistants are blind to runtime. They can read files and run tests, but they can't see what happens when their code executes -- the SQL queries it generates, the HTTP responses the server returns, the console errors in the browser. The human is the feedback loop. Daemon8 eliminates that gap.

## How It Works

Daemon8 is a persistent daemon that sits between your applications, your browser, and your LLM agent. It collects runtime observations from two sources:

**Application telemetry** -- any running application pushes debug data via HTTP POST. PHP, Node, Python, anything that can send JSON.

**Browser observation** -- the daemon connects to Chrome via the DevTools Protocol and silently captures console output, network requests, and errors.

Both sources feed into a unified observation store. LLM agents query the store through MCP tools, getting structured answers about what the application is doing right now.

```
┌──────────────┐
│  Your App     │──POST──┐
│  (any lang)   │        │     ┌──────────────┐     ┌───────────┐
└──────────────┘        ├────▶│   Daemon8    │────▶│ LLM Agent │
                         │     │   (daemon)    │     │ (Claude,  │
┌──────────────┐        │     └──────────────┘     │  Cursor)  │
│  Chrome       │──CDP───┘                          └───────────┘
└──────────────┘
```

## Quick Start

```bash
cargo install --path crates/daemon

# Start the daemon
daemon8 serve

# Start with Chrome observation
daemon8 serve --browser http://localhost:9222
```

### Installation as a system service

For persistent operation across login and crash restarts:

```bash
cargo install --path crates/daemon --force --locked
daemon8 install
```

`daemon8 install` writes a `launchd` plist (macOS), `systemd` unit (Linux), or `schtasks` job (Windows) and starts the service. MCP clients connect to `http://localhost:9077/mcp` by default.

#### macOS prerequisites (Sonoma and later)

macOS 14+ requires one additional step and may surface two permission dialogs on first install:

1. **Re-sign the binary after every rebuild.** `cargo install` produces an ad-hoc signature whose identity changes with every binary hash. `launchd` caches the prior identity and will refuse the new binary with `OS_REASON_CODESIGNING`. Run `codesign --force --sign - ~/.cargo/bin/daemon8` after `cargo install` and before `daemon8 install`.

2. **Approve "Background Items Added" notification.** Standard macOS prompt for any new `launchd` agent. No action required beyond acknowledging it.

3. **Approve App Management in System Settings.** On first install, open System Settings > Privacy & Security > App Management and toggle `daemon8` on. Without this permission the daemon's outbound calls may be blocked by TCC. This prompt only appears once per unique ad-hoc signature, so it re-triggers on rebuild. A future release signed with a stable Developer ID will eliminate the re-prompt.

Canonical install sequence on macOS:

```bash
cargo install --path crates/daemon --force --locked
codesign --force --sign - ~/.cargo/bin/daemon8
daemon8 install
```

Verify:

```bash
launchctl list | grep daemon8    # expect: <PID> -15 dev.daemon8.daemon
daemon8 status                   # expect: Daemon: running
```

To remove: `daemon8 uninstall`.

### Send observations from your application

```bash
curl -X POST http://localhost:9077/ingest \
  -H "Content-Type: application/json" \
  -d '{"kind":"query","data":{"sql":"SELECT * FROM users","duration_ms":12.5},"severity":"info","app":"my-api"}'
```

Or from your application code:

```php
// PHP
@file_get_contents('http://localhost:9077/ingest', false, stream_context_create([
    'http' => ['method' => 'POST', 'header' => "Content-Type: application/json\r\n",
               'content' => json_encode(['kind' => 'exception', 'data' => ['message' => $e->getMessage()],
                   'severity' => 'error', 'file' => $e->getFile(), 'line' => $e->getLine()]),
               'timeout' => 1]]));
```

```javascript
// Node
fetch('http://localhost:9077/ingest', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ kind: 'log', data: { message: 'user created' }, severity: 'info', app: 'my-api' })
});
```

### Connect Chrome for browser observation

Start Chrome with the remote debugging port:

```bash
/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  --remote-debugging-port=9222 \
  --user-data-dir="/tmp/chrome-debug-profile"
```

The daemon auto-discovers tabs and monitors console output + network requests. New tabs are picked up automatically every 5 seconds.

## MCP Tools

Daemon8 exposes eight tools via the Model Context Protocol. Any MCP-compatible agent (Claude Code, Cursor, etc.) can use them.

### debug_observe

Query runtime observations with filtering.

```
Parameters:
  kinds          -- log, query, http_exchange, exception, state_snapshot, metric, custom, js_exception, lifecycle
  severity_min   -- trace, debug, info, warn, error
  origins        -- "app:my-api", "browser", "browser:<tab_id>"
  text_match     -- substring search in observation data
  since_checkpoint -- only observations after this checkpoint
  limit          -- max results (default 50)
```

### debug_summary

Health snapshot: observation count, error rate, active channels, connected sources.

### debug_checkpoint

Mark current position. Subsequent `debug_observe` calls with `since_checkpoint` return only new data.

### debug_connections

List active data source connections (applications and browser tabs).

### debug_connect

Connect to a Chrome instance at runtime: `debug_connect({ endpoint: "http://localhost:9222" })`.

### debug_act

Drive the browser: evaluate JavaScript, take screenshots, inject/revert CSS, navigate, set viewport, inspect DOM, manage storage, throttle network.

### debug_ingest

Push an observation from inside the agent loop.

### debug_subscribe

Filter the real-time alert stream delivered over the MCP channel.

## Observation Types

Every observation has an origin, kind, severity, optional source location, and arbitrary JSON data.

| Kind | Structured Fields | Use Case |
|------|------------------|----------|
| `log` | -- | General logging |
| `query` | `sql`, `duration_ms` | Database queries |
| `http_exchange` | `method`, `url`, `status`, `duration_ms` | HTTP requests/responses |
| `exception` | `message`, `trace` | Caught/uncaught exceptions |
| `state_snapshot` | `label` | Variable dumps |
| `metric` | `name`, `value` | Numeric measurements |
| `custom` | `channel` | Anything else |

Severities: `trace`, `debug`, `info`, `warn`, `error`.

## Ingestion Protocol

`POST /ingest` accepts any JSON. Recognized fields are extracted; everything else becomes the data payload.

```json
{
  "kind": "query",
  "severity": "info",
  "app": "my-api",
  "file": "/src/UserRepository.php",
  "line": 42,
  "data": {
    "sql": "SELECT * FROM users WHERE id = ?",
    "duration_ms": 3.2
  }
}
```

If `kind` is omitted, the observation defaults to `log`. If `channel` is provided instead of `kind`, it wraps in `custom`. If `data` is omitted, the entire JSON body (minus meta fields) becomes the payload.

## Configuration

Layered configuration via [figment](https://docs.rs/figment): compiled defaults < TOML config file < environment variables < CLI args.

```toml
# ~/.config/daemon8/config.toml (Linux)
# ~/Library/Application Support/dev.daemon8.daemon8/config.toml (macOS)

version = 1

[server]
port = 9077
host = "127.0.0.1"

[browser]
auto_connect = false
endpoint = "http://localhost:9222"

[mcp]
stdio = true
```

Environment variables use the `DAEMON8_` prefix with double-underscore nesting: `DAEMON8_SERVER__PORT=9090`, `DAEMON8_BROWSER__AUTO_CONNECT=true`.

## Architecture

Rust workspace:

```
crates/types/    shared types (Observation, Filter, Origin)
crates/store/    StateModel trait + SQLite WAL implementation
crates/ingest/   Axum HTTP ingestion endpoint
crates/chrome/   Chrome DevTools Protocol bridge
crates/mcp/      MCP tool server (rmcp)
crates/adb/      ADB transport + logcat streaming
crates/api/      REST + SSE streaming endpoints
crates/daemon/   binary entrypoint + configuration
```

Storage is behind the `StateModel` trait -- swappable without touching anything else. SQLite WAL mode gives concurrent reads during writes with zero configuration.

## Building

```bash
cargo build --release                      # optimized binary
cargo test --workspace -- --test-threads=1 # all tests
cargo run -p daemon8 -- -v serve           # debug logging
```

## License

Fair Core License 1.0 with Apache 2.0 Fallback (FCL-1.0-ALv2). See [`LICENSE`](LICENSE) for the full text and [`LICENSES/FCL-1.0-ALv2.txt`](LICENSES/FCL-1.0-ALv2.txt) for the canonical template.

The FCL is a source-available license that grants full rights for any Permitted Purpose (internal use, non-commercial education/research, professional services using the Software) and restricts Competing Use. After two years from the date we make each release available, that release automatically becomes available under Apache 2.0.

Copyright (c) 2026 Havy.tech, LLC.
