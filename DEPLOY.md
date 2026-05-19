# Daemon8 Deployment

Build, install, and run `daemon8` as a local system service. For the
end-user overview and quick-start, see [README.md](README.md).

## Distribution

`daemon8` is a compiled Rust binary. Current install paths:

```bash
curl -fsSL https://daemon8.ai/install.sh | bash
```

Compile from git while crates remain unpublished:

```bash
cargo install --git https://github.com/daemon8ai/daemon8 daemon8
```

Compile from this checkout:

```bash
cargo install --path crates/daemon --force
```

The release shell installer uses `~/.cargo/bin` when present and otherwise
falls back to `~/.local/bin`. The PowerShell installer defaults to
`%LOCALAPPDATA%\Programs\daemon8`. After installing the binary, release
installers run `daemon8 service install` so daemon8 starts as the local MCP
server immediately. `DAEMON8_INSTALL_DIR` overrides the release installer
destination.

> **Publish gate.** Workspace crates currently carry `publish = false`.
> Use the release installer, git install, or checkout install paths above.

## Building from Source

```bash
cargo install --path crates/daemon --force
```

Build without installing:

```bash
cargo build -p daemon8
```

### macOS post-install codesign

Ad-hoc signatures produced by `cargo install` change their identity each
rebuild, which retriggers macOS TCC prompts. Re-sign after every `cargo
install`:

```bash
codesign --force --sign - ~/.cargo/bin/daemon8
```

Pre-built release binaries may be signed separately from local source
builds; verify the release artifact before assuming macOS trust behavior.

## Service Management

`daemon8` can run as a user-level system service that starts on login and
auto-restarts on crash.

```bash
daemon8 service install     # register + start the service
daemon8 service uninstall   # stop + remove the service
```

| Platform | Service type                  | File location                                                 |
| -------- | ----------------------------- | ------------------------------------------------------------- |
| macOS    | launchd user agent            | `~/Library/LaunchAgents/dev.daemon8.daemon.plist`             |
| Linux    | systemd user unit             | `~/.config/systemd/user/daemon8.service`                      |
| Windows  | Task Scheduler task `Daemon8` | —                                                             |

The launchd plist uses `KeepAlive: true`, which means `kill <pid>` does
**not** stop the daemon — launchd immediately respawns it. Use
`daemon8 service uninstall` (or `launchctl bootout` on the plist) to
actually stop.

The generated service runs `daemon8 serve`. When `browser.auto_connect`
is true at install time, the generated service also snapshots the current
browser endpoint as `--browser <endpoint>`, which overrides later
`config.toml` browser endpoint changes until the service is reinstalled.
ADB settings and ingestion listeners stay configured through `config.toml`.

## Upgrading

```bash
cp ~/.cargo/bin/daemon8 ~/.cargo/bin/daemon8.bak
```

Stop and remove the service:

```bash
daemon8 service uninstall
```

Build, install, and codesign on macOS:

```bash
cargo install --path crates/daemon --force --locked
```

```bash
codesign --force --sign - ~/.cargo/bin/daemon8
```

Re-register and start the service:

```bash
daemon8 service install
```

Verify:

```bash
daemon8 status
```

Quick rollback:

```bash
daemon8 service uninstall
cp ~/.cargo/bin/daemon8.bak ~/.cargo/bin/daemon8
daemon8 service install
```

## Configuration

Config file location (via `directories::ProjectDirs::from("dev", "daemon8", "daemon8")`):

| Platform | Path                                                                 |
| -------- | -------------------------------------------------------------------- |
| macOS    | `~/Library/Application Support/dev.daemon8.daemon8/config.toml`      |
| Linux    | `~/.config/daemon8/config.toml`                                      |
| Windows  | `%APPDATA%\daemon8\daemon8\config\config.toml`                       |

Debug builds use the `daemon8-dev` app slug so local development state stays isolated from release binaries.

Priority: compiled defaults < `config.toml` < environment variables < CLI args.

Environment variables use `DAEMON8_` prefix with double-underscore for nesting:

```bash
DAEMON8_SERVER__PORT=9090
DAEMON8_BROWSER__AUTO_CONNECT=true
DAEMON8_LOGGING__LEVEL=debug
DAEMON8_DEBUG_SESSION__INACTIVITY_AUTO_END_SECS=14400
```

Invalid primitives (bad IP address, malformed socket, unknown log level)
fail at load time — no silent fallbacks.

### Key options and defaults

| Section           | Field                         | Default                  | Description                                 |
| ----------------- | ----------------------------- | ------------------------ | ------------------------------------------- |
| server            | port                          | `8888`                   | HTTP server port                            |
| server            | host                          | `127.0.0.1`              | Bind address (IpAddr)                       |
| storage           | path                          | (platform default)       | SurrealDB/SurrealKV store location          |
| storage           | screenshot_path               | (resolved from storage)  | Browser screenshot output directory         |
| browser           | auto_connect                  | `false`                  | Connect to Chrome on startup                |
| browser           | endpoint                      | `http://localhost:9222`  | Chrome DevTools Protocol URL                |
| browser           | reconnect_interval_secs       | `5`                      | Initial backoff after disconnect            |
| browser           | max_reconnect_interval_secs   | `30`                     | Max exponential backoff ceiling             |
| browser           | path                          | (auto-detect)            | Chrome binary path for auto-launch          |
| adb               | enabled                       | `false`                  | Monitor ADB/Vega devices                    |
| adb               | server_addr                   | `127.0.0.1:5037`         | ADB server address (SocketAddrV4)           |
| adb               | scan_interval_secs            | `10`                     | Device scan interval                        |
| mcp               | stdio                         | `true`                   | Enable MCP stdio transport                  |
| debug_session     | inactivity_auto_end_secs      | `14400`                  | Active-session idle timeout                 |
| ingestion.udp     | enabled                       | `false`                  | UDP ingestion listener                      |
| ingestion.udp     | bind                          | `127.0.0.1:8889`         | UDP bind address (SocketAddr)               |
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
/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --remote-debugging-port=9222 --user-data-dir=/tmp/chrome-debug-profile
```

## Troubleshooting

**Port 8888 in use** (orphaned daemon):

```bash
daemon8 service uninstall
```

Then identify the exact daemon process:

```bash
lsof -nP -iTCP:8888 -sTCP:LISTEN
```

Only if the listed process is daemon8:

```bash
kill <exact-pid>
```

Fresh start:

```bash
daemon8 service install
```

**Orphaned processes**:

```bash
pgrep -fl daemon8
```

Remove the service so it stops respawning:

```bash
daemon8 service uninstall
```

Only for confirmed daemon8 processes:

```bash
kill <exact-pid>
```

**Logs**:

```bash
daemon8 logs
```

Tail the latest log:

```bash
daemon8 logs -f
```

Log directory: `~/Library/Application Support/dev.daemon8.daemon8/logs/` (macOS release), `~/.local/share/daemon8/logs/` (Linux release).

**Status**:

```bash
daemon8 status
```

## Release Workflow

GitHub Actions builds 5 targets on `v*` tag push:

- `x86_64-apple-darwin` (macOS Intel)
- `aarch64-apple-darwin` (macOS ARM)
- `x86_64-unknown-linux-gnu` (Linux Intel)
- `aarch64-unknown-linux-gnu` (Linux ARM)
- `x86_64-pc-windows-msvc` (Windows)

Release artifacts are attached to the GitHub Release for that tag. The
root install scripts consume those tarballs and verify `checksums.sha256`
before installing.

### Tagging a release

```bash
git tag v0.4.1
git push origin v0.4.1
```

The release workflow triggers automatically from the tag.
