# Daemon8 Deployment

Build, install, and run `daemon8` as a local system service. For the
end-user overview and quick-start, see [README.md](README.md).

## Distribution

`daemon8` is a compiled Rust binary. Three install methods:

```bash
# Pre-built binary (fastest, no compile)
cargo install cargo-binstall       # one-time
cargo binstall daemon8

# Compile from crates.io
cargo install daemon8

# Compile from this repo
cargo install --git https://github.com/daemon8ai/daemon8 daemon8
```

All three produce the same binary at `~/.cargo/bin/daemon8`.

> **Note on crates.io publishing.** Workspace crates currently carry
> `publish = false` while the crates.io publish flow is stabilized.
> This will change once `daemon8` and its library crates are reserved
> and published. Until then, use `cargo install --git` or a pre-built
> release tarball.

## Building from Source

```bash
# Build and install to ~/.cargo/bin/daemon8
cargo install --path crates/daemon --force

# Or just build without installing (for testing)
cargo build -p daemon8
```

### macOS post-install codesign

Ad-hoc signatures produced by `cargo install` change their identity each
rebuild, which retriggers macOS TCC prompts. Re-sign after every `cargo
install`:

```bash
codesign --force --sign - ~/.cargo/bin/daemon8
```

Pre-built release binaries (`cargo binstall`, direct download) are signed
with a stable Developer ID so this step is not needed for those.

## Service Management

`daemon8` can run as a user-level system service that starts on login and
auto-restarts on crash.

```bash
daemon8 install     # register + start the service
daemon8 uninstall   # stop + remove the service
```

| Platform | Service type                  | File location                                                 |
| -------- | ----------------------------- | ------------------------------------------------------------- |
| macOS    | launchd user agent            | `~/Library/LaunchAgents/dev.daemon8.daemon.plist`             |
| Linux    | systemd user unit             | `~/.config/systemd/user/daemon8.service`                      |
| Windows  | Task Scheduler task `Daemon8` | —                                                             |

The launchd plist uses `KeepAlive: true`, which means `kill <pid>` does
**not** stop the daemon — launchd immediately respawns it. Use
`daemon8 uninstall` (or `launchctl bootout` on the plist) to actually
stop.

The generated service only passes `serve` as an argument. Chrome
endpoint, ADB settings, ingestion listeners — all configured via
`config.toml`, not via service-file arguments.

## Upgrading

```bash
# 1. Optional: back up current binary
cp ~/.cargo/bin/daemon8 ~/.cargo/bin/daemon8.bak

# 2. Stop and remove service
daemon8 uninstall

# 3. Build and install new binary
cargo install --path crates/daemon --force --locked
codesign --force --sign - ~/.cargo/bin/daemon8   # macOS only

# 4. Re-register and start service
daemon8 install

# 5. Verify
daemon8 status
```

Quick rollback:

```bash
daemon8 uninstall
cp ~/.cargo/bin/daemon8.bak ~/.cargo/bin/daemon8
daemon8 install
```

## Configuration

Config file location (via `directories::ProjectDirs::from("dev", "daemon8", "daemon8")`):

| Platform | Path                                                                 |
| -------- | -------------------------------------------------------------------- |
| macOS    | `~/Library/Application Support/dev.daemon8.daemon8/config.toml`      |
| Linux    | `~/.config/dev/daemon8/daemon8/config.toml`                          |
| Windows  | `%APPDATA%\dev\daemon8\daemon8\config.toml`                          |

Priority: compiled defaults < `config.toml` < environment variables < CLI args.

Environment variables use `DAEMON8_` prefix with double-underscore for nesting:

```bash
DAEMON8_SERVER__PORT=9090
DAEMON8_BROWSER__AUTO_CONNECT=true
DAEMON8_LOGGING__LEVEL=debug
```

Invalid primitives (bad IP address, malformed socket, unknown log level)
fail at load time — no silent fallbacks.

### Key options and defaults

| Section           | Field                         | Default                  | Description                                 |
| ----------------- | ----------------------------- | ------------------------ | ------------------------------------------- |
| server            | port                          | `9077`                   | HTTP server port                            |
| server            | host                          | `127.0.0.1`              | Bind address (IpAddr)                       |
| storage           | path                          | (platform default)       | SQLite database location                    |
| storage           | screenshot_path               | (resolved from storage)  | Browser screenshot output directory         |
| browser           | auto_connect                  | `false`                  | Connect to Chrome on startup                |
| browser           | endpoint                      | `http://localhost:9222`  | Chrome DevTools Protocol URL                |
| browser           | reconnect_interval_secs       | `5`                      | Initial backoff after disconnect            |
| browser           | max_reconnect_interval_secs   | `30`                     | Max exponential backoff ceiling             |
| browser           | path                          | (auto-detect)            | Chrome binary path for auto-launch          |
| adb               | enabled                       | `true`                   | Monitor ADB/Vega devices                    |
| adb               | server_addr                   | `127.0.0.1:5037`         | ADB server address (SocketAddrV4)           |
| adb               | scan_interval_secs            | `10`                     | Device discovery interval                   |
| mcp               | stdio                         | `true`                   | Enable MCP stdio transport                  |
| mcp               | http                          | `false`                  | Enable MCP HTTP/SSE transport               |
| ingestion.udp     | enabled                       | `false`                  | UDP ingestion listener                      |
| ingestion.udp     | bind                          | `127.0.0.1:9078`         | UDP bind address (SocketAddr)               |
| ingestion.unix    | enabled                       | `false`                  | Unix socket listener                        |
| ingestion.unix    | path                          | `/tmp/daemon8.sock`      | Socket path                                 |
| logging           | level                         | `info`                   | trace / debug / info / warn / error         |
| logging           | stderr                        | `true`                   | Log to stderr                               |
| logging           | max_log_files                 | `5`                      | Daily log-rotation retention                |

Print resolved config: `daemon8 config show`.

## Chrome Connection

Chrome auto-connect is configured via `config.toml`:

```toml
[browser]
auto_connect = true
endpoint = "http://localhost:9222"
```

Or connect at runtime via the MCP tool:

```
connect_browser { endpoint: "http://localhost:9222" }
```

Chrome must be launched with remote debugging enabled:

```bash
# macOS
/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/chrome-debug-profile
```

## Troubleshooting

**Port 9077 in use** (orphaned daemon):

```bash
lsof -ti :9077 | xargs kill    # kill whatever holds the port
daemon8 uninstall              # clean up stale service
daemon8 install                # fresh start
```

**Orphaned processes**:

```bash
pgrep -fl daemon8              # find all daemon processes
daemon8 uninstall              # remove service (stops respawning)
pkill -f daemon8               # kill stragglers
```

**Logs**:

```bash
daemon8 logs                   # print path to latest log file
daemon8 logs -f                # tail -f the latest log
```

Log directory: `~/Library/Application Support/dev.daemon8.daemon8/logs/` (macOS), `~/.local/share/dev/daemon8/daemon8/logs/` (Linux).

**Doctor**:

```bash
daemon8 doctor             # run diagnostic checks
daemon8 doctor --fix       # auto-repair issues that can be repaired
```

Covers: config file existence, screenshot/data dir writability, port availability, outbound network, macOS launchd service state, App Management TCC grant.

## Release Workflow

GitHub Actions builds 5 targets on `v*` tag push:

- `x86_64-apple-darwin` (macOS Intel)
- `aarch64-apple-darwin` (macOS ARM)
- `x86_64-unknown-linux-gnu` (Linux Intel)
- `aarch64-unknown-linux-gnu` (Linux ARM)
- `x86_64-pc-windows-msvc` (Windows)

Release artifacts are attached to the GitHub Release for that tag. Users
with `cargo binstall` installed consume these tarballs directly via
`cargo binstall daemon8`.

### Tagging a release

```bash
git tag v0.1.0
git push origin v0.1.0
```

The release workflow triggers automatically from the tag.

