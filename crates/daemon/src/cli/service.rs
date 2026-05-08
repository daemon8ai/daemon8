// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Output;

use crate::providers::Provider;

#[cfg(target_os = "macos")]
const LABEL: &str = "dev.daemon8.daemon";

fn binary_path() -> Result<PathBuf> {
    std::env::current_exe()
        .context("failed to determine binary path")?
        .canonicalize()
        .context("failed to resolve binary path")
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn cargo_bin_dir() -> String {
    home_dir().join(".cargo/bin").display().to_string()
}

#[cfg(target_os = "linux")]
fn local_bin_dir() -> String {
    home_dir().join(".local/bin").display().to_string()
}

// macOS TCC preflight. macOS 14+ prompts for two permissions when launchd
// loads a new agent: Background Items Added (Login Items) and App Management
// (Privacy & Security). Surfacing both in advance sets user expectations and
// keeps the install log self-documenting for operators who hit this later.
#[cfg(target_os = "macos")]
fn macos_permission_preflight() {
    println!();
    println!("  macOS will prompt for two permissions on first install:");
    println!("    1. Background Items Added (Login Items)");
    println!("       -- click Allow in the notification that appears.");
    println!("    2. App Management");
    println!("       -- open System Settings > Privacy & Security > App Management");
    println!("       and toggle daemon8 on.");
    println!();
    println!("  Without App Management granted, outbound calls may be blocked by TCC.");
    println!();
}

// Best-effort jump to the Privacy & Security App Management pane. Fails
// silently if the URL scheme is unavailable or `open` returns non-zero --
// install has already succeeded at this point.
#[cfg(target_os = "macos")]
fn macos_open_privacy_pane() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AppBundles")
        .output();
}

pub fn service_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        plist_path().exists()
    }

    #[cfg(target_os = "linux")]
    {
        unit_path().exists()
    }

    #[cfg(windows)]
    {
        std::process::Command::new("schtasks")
            .args(["/Query", "/TN", "Daemon8"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    false
}

pub fn cmd_install() -> Result<()> {
    let binary = binary_path()?;
    let binary_str = binary.display().to_string();
    let cfg = crate::config::load(None).unwrap_or_default();
    let port = cfg.server.port;
    let chrome_endpoint = if cfg.browser.auto_connect {
        Some(cfg.browser.endpoint.clone())
    } else {
        None
    };

    println!("Installing daemon8 service...");
    println!("  Binary: {binary_str}");

    #[cfg(target_os = "macos")]
    {
        macos_permission_preflight();
        install_launchd(&binary_str, chrome_endpoint.as_deref(), port)?;
        macos_open_privacy_pane();
    }

    #[cfg(target_os = "linux")]
    install_systemd(&binary_str, chrome_endpoint.as_deref(), port)?;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        println!();
        println!("Daemon8 is now running. MCP clients connect to http://localhost:{port}/mcp");
    }

    #[cfg(windows)]
    {
        install_schtasks(&binary_str, chrome_endpoint.as_deref(), port)?;
        println!();
        println!("Daemon8 is now running. MCP clients connect to http://localhost:{port}/mcp");
    }

    Ok(())
}

pub fn cmd_uninstall() -> Result<()> {
    println!("Removing daemon8...");
    println!();

    // 1. System service
    #[cfg(target_os = "macos")]
    match uninstall_launchd() {
        Ok(()) => println!("  [ok] launchd service removed"),
        Err(e) => println!("  [--] launchd service: {e}"),
    }

    #[cfg(target_os = "linux")]
    match uninstall_systemd() {
        Ok(()) => println!("  [ok] systemd service removed"),
        Err(e) => println!("  [--] systemd service: {e}"),
    }

    #[cfg(windows)]
    match uninstall_schtasks() {
        Ok(()) => println!("  [ok] scheduled task removed"),
        Err(e) => println!("  [--] scheduled task: {e}"),
    }

    // 2. Config directory
    let cfg = crate::config::load(None).unwrap_or_default();
    remove_path(&cfg.config_dir, "config dir");

    // 3. Data directory (db, logs, screenshots)
    let db_path = crate::config::resolve_db_path(cfg.storage.path.as_deref());
    if let Some(data_dir) = db_path.parent() {
        remove_path(data_dir, "data dir");
    }

    // 4. Project-local .daemon8.toml
    let cwd = std::env::current_dir().unwrap_or_default();
    let cwd_config = cwd.join(crate::cli_config::PROJECT_CONFIG_FILENAME);
    if cwd_config.exists() {
        remove_path(&cwd_config, "project config");
    }

    // 5. Remove daemon8 from provider MCP configs
    let home = crate::providers::dirs_home();
    remove_provider_entry(Provider::ClaudeCode, &home);
    remove_provider_entry(Provider::Cursor, &home);
    remove_provider_entry(Provider::Windsurf, &home);
    remove_provider_entry(Provider::Gemini, &home);
    remove_provider_entry(Provider::Codex, &home);

    // 6. Remove hook entries from provider hook config files
    remove_cli_hooks(&home, &cwd);

    println!();
    println!("Daemon8 fully uninstalled.");
    Ok(())
}

fn remove_path(path: &std::path::Path, label: &str) {
    if !path.exists() {
        return;
    }
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => println!("  [ok] {label}: {}", path.display()),
        Err(e) => println!("  [!!] {label}: {} ({e})", path.display()),
    }
}

fn remove_provider_entry(provider: Provider, home: &std::path::Path) {
    let config_path = provider.config_path(home);
    if !config_path.exists() {
        return;
    }

    if provider == Provider::Codex {
        let contents = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut table: toml::Table = match contents.parse() {
            Ok(t) => t,
            Err(_) => return,
        };
        if let Some(mcp) = table.get_mut("mcp_servers").and_then(|v| v.as_table_mut())
            && mcp.remove("daemon8").is_some()
            && let Ok(s) = toml::to_string_pretty(&table)
        {
            let _ = std::fs::write(&config_path, s);
            println!("  [ok] removed daemon8 from {}", config_path.display());
        }
        return;
    }

    // Claude Code: try CLI first
    if provider == Provider::ClaudeCode {
        let ok = crate::providers::shim_command("claude")
            .args(["mcp", "remove", "--scope", "user", "daemon8"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            println!("  [ok] removed daemon8 MCP from Claude Code");
        }
        let _ = crate::providers::shim_command("claude")
            .args(["mcp", "remove", "--scope", "user", "daemon8-channel"])
            .output();
        return;
    }

    // Gemini: try CLI first
    if provider == Provider::Gemini {
        let ok = crate::providers::shim_command("gemini")
            .args(["mcp", "remove", "daemon8"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            println!("  [ok] removed daemon8 MCP from Gemini");
            return;
        }
    }

    // JSON-based providers: remove the daemon8 key from mcpServers
    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut json: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(_) => return,
    };
    if let Some(servers) = json.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        let had_daemon8 = servers.remove("daemon8").is_some();
        let had_channel = servers.remove("daemon8-channel").is_some();
        if (had_daemon8 || had_channel)
            && let Ok(s) = serde_json::to_string_pretty(&json)
        {
            let _ = std::fs::write(&config_path, s);
            println!("  [ok] removed daemon8 from {}", config_path.display());
        }
    }
}

fn remove_cli_hooks(home: &std::path::Path, cwd: &std::path::Path) {
    for settings_path in [
        home.join(".claude/settings.json"),
        home.join(".claude/settings.local.json"),
        home.join(".codex/hooks.json"),
        cwd.join(".claude/settings.json"),
        cwd.join(".claude/settings.local.json"),
    ] {
        if !settings_path.exists() {
            continue;
        }
        let contents = match std::fs::read_to_string(&settings_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut json: serde_json::Value = match serde_json::from_str(&contents) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mut modified = false;
        if let Some(hooks) = json.get_mut("hooks").and_then(|v| v.as_object_mut()) {
            for (_event, matchers) in hooks.iter_mut() {
                if let Some(arr) = matchers.as_array_mut() {
                    let before = arr.len();
                    arr.retain(|entry| !hook_group_contains_daemon8(entry));
                    if arr.len() < before {
                        modified = true;
                    }
                }
            }
        }
        if modified && let Ok(s) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(&settings_path, s);
            println!(
                "  [ok] removed daemon8 hooks from {}",
                settings_path.display()
            );
        }
    }
}

fn hook_group_contains_daemon8(entry: &serde_json::Value) -> bool {
    entry
        .get("command")
        .and_then(|command| command.as_str())
        .is_some_and(|command| command.contains("daemon8"))
        || entry
            .get("hooks")
            .and_then(|hooks| hooks.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("command")
                        .and_then(|command| command.as_str())
                        .is_some_and(|command| command.contains("daemon8"))
                })
            })
}

// ---------------------------------------------------------------------------
// macOS: launchd user agent
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn launchd_domain() -> String {
    // User LaunchAgents live in the per-login GUI domain.
    format!("gui/{}", unsafe { libc::geteuid() })
}

#[cfg(target_os = "macos")]
fn launchctl_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("exit status {}", output.status)
}

#[cfg(target_os = "macos")]
fn install_launchd(binary: &str, chrome_endpoint: Option<&str>, port: u16) -> Result<()> {
    let path = plist_path();
    let path_str = path.display().to_string();
    let cargo_bin = cargo_bin_dir();
    let domain = launchd_domain();
    let service_target = format!("{domain}/{LABEL}");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let chrome_args = chrome_endpoint
        .map(|ep| format!("\n        <string>--browser</string>\n        <string>{ep}</string>"))
        .unwrap_or_default();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>serve</string>{chrome_args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/dev/null</string>
    <key>StandardErrorPath</key>
    <string>/dev/null</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:{cargo_bin}</string>
    </dict>
</dict>
</plist>"#
    );

    std::fs::write(&path, &plist).with_context(|| format!("writing {}", path.display()))?;
    println!("  Service: {}", path.display());

    // If already installed, boot it out first (upgrade/reinstall path).
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &domain, &path_str])
        .output();

    let output = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &path_str])
        .output()
        .context("failed to run launchctl bootstrap")?;

    if output.status.success() {
        println!("  Loaded: launchctl bootstrap");
    } else {
        anyhow::bail!("launchctl bootstrap failed: {}", launchctl_message(&output));
    }

    let enable = std::process::Command::new("launchctl")
        .args(["enable", &service_target])
        .output()
        .context("failed to run launchctl enable")?;
    if !enable.status.success() {
        anyhow::bail!("launchctl enable failed: {}", launchctl_message(&enable));
    }

    let kickstart = std::process::Command::new("launchctl")
        .args(["kickstart", "-k", &service_target])
        .output()
        .context("failed to run launchctl kickstart")?;
    if !kickstart.status.success() {
        anyhow::bail!(
            "launchctl kickstart failed: {}",
            launchctl_message(&kickstart)
        );
    }

    // Wait for it to start
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            println!("  Status: running");
            return Ok(());
        }
    }

    println!("  Status: started (verifying health timed out, check 'daemon8 logs')");
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<()> {
    let path = plist_path();
    let path_str = path.display().to_string();
    let domain = launchd_domain();

    if !path.exists() {
        println!("  Not installed (no plist found)");
        return Ok(());
    }

    let output = std::process::Command::new("launchctl")
        .args(["bootout", &domain, &path_str])
        .output()
        .context("failed to run launchctl bootout")?;

    if output.status.success() {
        println!("  Unloaded: launchctl bootout");
    } else {
        println!(
            "  Warning: launchctl bootout: {}",
            launchctl_message(&output)
        );
    }

    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    println!("  Removed: {}", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux: systemd user unit
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn unit_path() -> PathBuf {
    home_dir()
        .join(".config/systemd/user")
        .join("daemon8.service")
}

#[cfg(target_os = "linux")]
fn install_systemd(binary: &str, chrome_endpoint: Option<&str>, port: u16) -> Result<()> {
    let path = unit_path();
    let cargo_bin = cargo_bin_dir();
    let local_bin = local_bin_dir();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let chrome_args = chrome_endpoint
        .map(|ep| format!(" --browser {ep}"))
        .unwrap_or_default();

    let unit = format!(
        r#"[Unit]
Description=Daemon8 — the admin layer for AI agents
After=network.target

[Service]
Type=simple
ExecStart={binary} serve{chrome_args}
Restart=always
RestartSec=5
Environment=PATH=/usr/local/bin:/usr/bin:/bin:{cargo_bin}:{local_bin}

[Install]
WantedBy=default.target
"#
    );

    std::fs::write(&path, &unit).with_context(|| format!("writing {}", path.display()))?;
    println!("  Service: {}", path.display());

    let reload = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output()
        .context("failed to run systemctl daemon-reload")?;

    if !reload.status.success() {
        let stderr = String::from_utf8_lossy(&reload.stderr);
        anyhow::bail!("systemctl daemon-reload failed: {stderr}");
    }

    let enable = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "daemon8"])
        .output()
        .context("failed to run systemctl enable")?;

    if enable.status.success() {
        println!("  Enabled: systemctl --user enable --now daemon8");
    } else {
        let stderr = String::from_utf8_lossy(&enable.stderr);
        anyhow::bail!("systemctl enable failed: {stderr}");
    }

    // Wait for it to start
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            println!("  Status: running");
            return Ok(());
        }
    }

    println!("  Status: started (verifying health timed out, check 'daemon8 logs')");
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> Result<()> {
    let path = unit_path();

    if !path.exists() {
        println!("  Not installed (no unit file found)");
        return Ok(());
    }

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "daemon8"])
        .output();
    println!("  Disabled: systemctl --user disable --now daemon8");

    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    println!("  Removed: {}", path.display());

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    Ok(())
}

// ---------------------------------------------------------------------------
// Windows: Task Scheduler user task
// ---------------------------------------------------------------------------

// schtasks consumes the XML literally -- any `&` in a path or URL is interpreted
// as an entity reference and aborts parsing. Order matters: `&` must be escaped
// first so the replacement entities aren't double-escaped.
#[cfg(windows)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
fn install_schtasks(binary: &str, chrome_endpoint: Option<&str>, port: u16) -> Result<()> {
    let chrome_args = chrome_endpoint
        .map(|ep| format!(" --browser {ep}"))
        .unwrap_or_default();

    // On domain-joined machines schtasks wants `DOMAIN\User`; standalone boxes
    // accept `.\User`. Bare `USERNAME` silently fails under AD policy.
    let raw_user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let username = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{raw_user}"),
        _ => format!(".\\{raw_user}"),
    };
    let username = xml_escape(&username);
    let binary = xml_escape(binary);
    let chrome_args = xml_escape(&chrome_args);

    // Task XML: run at logon for current user, restart on failure up to 10
    // times at 1-minute intervals, no execution time limit.
    // schtasks requires UTF-16 LE with BOM when reading from a file.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals>
    <Principal id="Author">
      <UserId>{username}</UserId>
      <LogonType>InteractiveToken</LogonType>
    </Principal>
  </Principals>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{username}</UserId>
    </LogonTrigger>
  </Triggers>
  <Settings>
    <RestartOnFailure>
      <Count>10</Count>
      <Interval>PT1M</Interval>
    </RestartOnFailure>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{binary}</Command>
      <Arguments>serve{chrome_args}</Arguments>
    </Exec>
  </Actions>
</Task>"#
    );

    // PID-suffixed so concurrent installers don't race on the same temp path.
    let tmp = std::env::temp_dir().join(format!("daemon8-task-{}.xml", std::process::id()));
    {
        let utf16: Vec<u16> = std::iter::once(0xFEFF_u16)
            .chain(xml.encode_utf16())
            .collect();
        let bytes: Vec<u8> = utf16.iter().flat_map(|c| c.to_le_bytes()).collect();
        std::fs::write(&tmp, bytes)
            .with_context(|| format!("writing task XML to {}", tmp.display()))?;
    }

    // Remove any existing task first (idempotent upgrade path).
    // `.output()` already captures both streams; no redirection needed.
    let _ = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", "Daemon8", "/F"])
        .output();

    let create = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            "Daemon8",
            "/XML",
            &tmp.display().to_string(),
            "/F",
        ])
        .output()
        .context("failed to run schtasks /Create")?;

    let _ = std::fs::remove_file(&tmp);

    if !create.status.success() {
        let stderr = String::from_utf8_lossy(&create.stderr);
        anyhow::bail!("schtasks /Create failed: {stderr}");
    }
    println!("  Service: Task Scheduler task 'Daemon8' registered (restarts on crash)");

    let start = std::process::Command::new("schtasks")
        .args(["/Run", "/TN", "Daemon8"])
        .output()
        .context("failed to run schtasks /Run")?;

    if !start.status.success() {
        let stderr = String::from_utf8_lossy(&start.stderr);
        anyhow::bail!("schtasks /Run failed: {stderr}");
    }

    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            println!("  Status: running");
            return Ok(());
        }
    }
    println!("  Status: started (verifying health timed out, check 'daemon8 logs')");
    Ok(())
}

#[cfg(windows)]
fn uninstall_schtasks() -> Result<()> {
    let query = std::process::Command::new("schtasks")
        .args(["/Query", "/TN", "Daemon8"])
        .output()
        .context("failed to run schtasks /Query")?;

    if !query.status.success() {
        println!("  Service: not installed (nothing to remove)");
        return Ok(());
    }

    let output = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", "Daemon8", "/F"])
        .output()
        .context("failed to run schtasks /Delete")?;

    if output.status.success() {
        println!("  Removed: Task Scheduler task 'Daemon8'");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("schtasks /Delete failed: {stderr}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_json(path: &std::path::Path) -> serde_json::Value {
        let text = std::fs::read_to_string(path).expect("read hook config");
        serde_json::from_str(&text).expect("parse hook config")
    }

    #[test]
    fn remove_cli_hooks_removes_nested_daemon8_groups_and_preserves_user_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let claude_dir = home.join(".claude");
        let codex_dir = home.join(".codex");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::create_dir_all(&codex_dir).unwrap();

        let hook_config = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{ "type": "command", "command": "user-hook" }]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [{ "type": "command", "command": "/bin/daemon8 cli-hook" }]
                    }
                ]
            }
        });
        std::fs::write(
            claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&hook_config).unwrap(),
        )
        .unwrap();
        std::fs::write(
            codex_dir.join("hooks.json"),
            serde_json::to_string_pretty(&hook_config).unwrap(),
        )
        .unwrap();

        let project = tmp.path().join("project");
        let project_claude_dir = project.join(".claude");
        std::fs::create_dir_all(&project_claude_dir).unwrap();
        std::fs::write(
            project_claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&hook_config).unwrap(),
        )
        .unwrap();

        remove_cli_hooks(home, &project);

        for path in [
            claude_dir.join("settings.local.json"),
            codex_dir.join("hooks.json"),
            project_claude_dir.join("settings.local.json"),
        ] {
            let parsed = read_json(&path);
            let groups = parsed["hooks"]["PreToolUse"].as_array().unwrap();
            assert_eq!(groups.len(), 1, "{path:?}");
            assert_eq!(groups[0]["hooks"][0]["command"].as_str(), Some("user-hook"));
        }
    }
}
