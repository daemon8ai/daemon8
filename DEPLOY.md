# Install and Release Artifacts

This file documents public install paths, local service commands, and release artifact behavior for daemon8.

## Install

macOS and Linux:

```bash
curl -fsSL https://daemon8.ai/install.sh | bash
```

Windows PowerShell:

```powershell
iwr https://daemon8.ai/install.ps1 -UseB | iex
```

The release installers download the matching GitHub Release artifact, verify `checksums.sha256`, install the `daemon8` binary, and run `daemon8 service install`.

Install destination:

- macOS/Linux: `~/.cargo/bin` when available, otherwise `~/.local/bin`
- Windows: `%LOCALAPPDATA%\Programs\daemon8`

`DAEMON8_INSTALL_DIR` overrides the default destination.

## Build From Source

Install from this repository:

```bash
cargo install --git https://github.com/daemon8ai/daemon8 daemon8
```

Install from a checkout:

```bash
cargo install --path crates/daemon --force
```

Build without installing:

```bash
cargo build -p daemon8
```

The workspace crates are not published independently. Use the release installers, git install, or checkout install paths above.

## Service Commands

Register and start daemon8 as a user-level service:

```bash
daemon8 service install
```

Stop and remove the service:

```bash
daemon8 service uninstall
```

Check current daemon state:

```bash
daemon8 status
```

Show or tail daemon logs:

```bash
daemon8 logs
daemon8 logs -f
```

The service runs `daemon8 serve`. The installer can also configure supported MCP providers and add the daemon8 instruction block to provider instruction files.

## Configuration

daemon8 has two configuration surfaces:

- Global daemon settings live in `config.toml` under the platform app-config directory.
- Project setup lives in `.daemon8/config.md` inside each project and is created with `daemon8 init`.

Print the resolved global daemon config:

```bash
daemon8 config show
```

Initialize a project:

```bash
daemon8 init --path .
```

Connect a session to a project:

```bash
daemon8 connect --path . --provider codex
```

## Release Artifacts

The release workflow builds these targets for `v*` tags:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

Artifacts are packaged as `.tar.gz` on Unix targets and `.zip` on Windows. Each release also includes `checksums.sha256`.

The workflow creates the GitHub Release after the server artifact upload step succeeds. Required server upload secrets are validated in the workflow before release creation.

## Tagging a Release

Create and push a version tag that matches the workspace version:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

For prereleases, include the prerelease suffix in both `Cargo.toml` and the tag:

```bash
git tag vX.Y.Z-alpha.N
git push origin vX.Y.Z-alpha.N
```

The release workflow runs automatically on matching `v*` tags.
