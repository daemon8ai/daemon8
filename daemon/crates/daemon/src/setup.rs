// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config;

// `claude` and `gemini` install as `.cmd` shims on Windows, which `Command::new`
// does not auto-resolve the way cmd.exe does. Wrap through `cmd /C` so invocation
// matches what the user would type in a shell.
#[cfg(windows)]
fn shim_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/C").arg(program);
    cmd
}

#[cfg(not(windows))]
fn shim_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

fn print_banner() {
    let bold = "\x1b[1m";
    let blue = "\x1b[34;1m";
    let reset = "\x1b[0m";
    eprintln!();
    eprintln!("  {bold}daemon{blue}8{reset}");
    eprintln!();
}

pub async fn cmd_setup() -> Result<()> {
    // cliclack::intro initializes VT processing on legacy cmd.exe; run it first
    // so the banner's ANSI escapes actually render as colors rather than raw text.
    cliclack::intro("Setup")?;
    print_banner();

    let cfg = crate::config::load(None).unwrap_or_default();
    let config_dir = &cfg.config_dir;
    let config_path = config_dir.join("config.toml");

    setup_browser(&config_path)?;

    setup_mcp_configs()?;

    setup_service()?;

    run_health_check().await;

    let screenshot_path = config::resolve_screenshot_path(&config::load(None).unwrap_or_default());
    eprintln!();
    eprintln!("  Screenshots: {}", screenshot_path.display());

    eprintln!();
    eprintln!("  Daemon8 is written by Havy.tech LLC.");
    eprintln!("  Terms:    https://daemon8.ai/terms");
    eprintln!("  Privacy:  https://daemon8.ai/privacy");
    eprintln!();

    cliclack::outro("Daemon8 is successfully installed! The time to build is now!!")?;

    eprintln!();
    eprintln!("  Quick commands:");
    eprintln!("    daemon8 status                         check if daemon is running");
    eprintln!("    daemon8 config set browser.path \"...\"  change default browser");
    eprintln!();

    Ok(())
}

fn setup_browser(config_path: &Path) -> Result<()> {
    let existing = read_config_value(config_path, "browser", "path");
    let browsers = daemon8_chrome::find_all_chromium_browsers();

    if let Some(ref path) = existing
        && !path.is_empty()
        && std::path::Path::new(path).exists()
    {
        cliclack::log::info(format!("Browser: {path}"))?;
        return Ok(());
    }
    if existing.as_ref().is_some_and(|p| !p.is_empty()) {
        cliclack::log::warning("Configured browser path no longer exists. Reselecting.")?;
    }

    if browsers.is_empty() {
        cliclack::log::warning(
            "No Chromium-based browser detected. Browser observation will be unavailable.",
        )?;
        return Ok(());
    }

    if browsers.len() == 1 {
        let (name, path) = &browsers[0];
        cliclack::log::info(format!("Browser: {name}"))?;
        write_config_value(
            config_path,
            "browser",
            "path",
            path.to_string_lossy().as_ref(),
        )?;
        return Ok(());
    }

    let names: Vec<&str> = browsers.iter().map(|(n, _)| n.as_str()).collect();
    cliclack::log::info(format!("Found on this machine: {}", names.join(", ")))?;

    let mut items: Vec<(String, String, String)> = browsers
        .iter()
        .map(|(name, path)| {
            (
                path.to_string_lossy().to_string(),
                name.clone(),
                String::new(),
            )
        })
        .collect();
    items.push((
        "none".to_string(),
        "None".to_string(),
        "skip browser integration".to_string(),
    ));

    let selected: String = cliclack::select("Which browser should Daemon8 use by default?")
        .items(
            &items
                .iter()
                .map(|(v, l, h)| (v.as_str(), l.as_str(), h.as_str()))
                .collect::<Vec<_>>(),
        )
        .interact()?
        .to_string();

    if selected != "none" {
        write_config_value(config_path, "browser", "path", &selected)?;
    }

    Ok(())
}

fn setup_mcp_configs() -> Result<()> {
    let tools = detect_ai_tools();

    if tools.is_empty() {
        cliclack::log::info("No AI tools detected. You can configure MCP manually later.")?;
        return Ok(());
    }

    let mut configured = 0u32;
    let mut skipped = 0u32;

    for (name, config_path, already) in &tools {
        if *already {
            cliclack::log::info(format!("{name}: already configured"))?;
            skipped += 1;
            continue;
        }

        let should_configure: bool = cliclack::confirm(format!("Configure {name}?"))
            .initial_value(true)
            .interact()?;

        if should_configure {
            if let Err(e) = write_mcp_config(name, config_path) {
                cliclack::log::error(format!("{name}: {e}"))?;
            } else {
                cliclack::log::success(format!("{name}: MCP config written"))?;
                configured += 1;
            }
        }
    }

    if configured > 0 || skipped > 0 {
        cliclack::log::info(format!("{configured} configured, {skipped} already set up"))?;
    }

    Ok(())
}

fn detect_ai_tools() -> Vec<(&'static str, PathBuf, bool)> {
    let home = dirs_home();
    let mut tools = Vec::new();

    // All four tools resolve their global config via `os.homedir()` without
    // platform branching (verified against current docs and source):
    //   - Claude Code: ~/.claude.json
    //   - Cursor:      ~/.cursor/mcp.json
    //   - Windsurf:    ~/.codeium/windsurf/mcp_config.json
    //   - Gemini CLI:  ~/.gemini/settings.json
    // On Windows these resolve under %USERPROFILE%; Path::join normalizes
    // the forward-slash separators.
    let checks: &[(&str, &str, &str)] = &[
        ("Claude Code", ".claude.json", ".claude"),
        ("Cursor", ".cursor/mcp.json", ".cursor"),
        (
            "Windsurf",
            ".codeium/windsurf/mcp_config.json",
            ".codeium/windsurf",
        ),
        ("Gemini", ".gemini/settings.json", ".gemini"),
    ];

    for (name, config_rel, detect_dir) in checks {
        let config_path = home.join(config_rel);
        if home_dir_exists(detect_dir) {
            let already = config_path.exists()
                && std::fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                    .and_then(|v| {
                        v.get("mcpServers")?
                            .as_object()
                            .map(|m| m.contains_key("daemon8"))
                    })
                    .unwrap_or(false);
            tools.push((*name, config_path.clone(), already));
        }
    }

    tools
}

fn write_mcp_config(tool_name: &str, config_path: &Path) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let port = crate::config::load(None).unwrap_or_default().server.port;

    // Claude Code's restricted PATH won't resolve bare "daemon8"; use the absolute path.
    let binary_path =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("daemon8"));

    let mcp_url = format!("http://localhost:{port}/mcp");
    // Gemini CLI uses "httpUrl" for streamable HTTP; other tools use type/url.
    let daemon8_entry = if tool_name == "Gemini" {
        serde_json::json!({ "httpUrl": mcp_url })
    } else {
        serde_json::json!({ "type": "http", "url": mcp_url })
    };

    let channel_entry = serde_json::json!({
        "command": binary_path.to_string_lossy(),
        "args": ["channel"]
    });

    let include_channel = tool_name == "Claude Code";

    // For Gemini, prefer the official CLI which writes to the user-scope settings.json.
    if tool_name == "Gemini" {
        let ok = shim_command("gemini")
            .args([
                "mcp",
                "add",
                "daemon8",
                &mcp_url,
                "--transport",
                "http",
                "--scope",
                "user",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
    }

    if tool_name == "Claude Code" {
        let http_ok = shim_command("claude")
            .args([
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "http",
                "daemon8",
                &mcp_url,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if http_ok {
            let _ = shim_command("claude")
                .args([
                    "mcp",
                    "add",
                    "--scope",
                    "user",
                    "--transport",
                    "stdio",
                    "daemon8-channel",
                    binary_path.to_str().unwrap_or("daemon8"),
                    "--",
                    "channel",
                ])
                .status();
            return Ok(());
        }
    }

    // Manual JSON patch: surgical edit of mcpServers only, atomic rename to avoid
    // corrupting the file if the write is interrupted.
    if config_path.exists() {
        let contents = std::fs::read_to_string(config_path)?;
        let mut json: serde_json::Value =
            serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));

        let servers = json
            .as_object_mut()
            .context("config is not a JSON object")?
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));

        if let Some(obj) = servers.as_object_mut() {
            obj.insert("daemon8".to_string(), daemon8_entry);
            if include_channel {
                obj.insert("daemon8-channel".to_string(), channel_entry);
            }
        }

        let tmp = config_path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&json)?)?;
        std::fs::rename(&tmp, config_path)?;
    } else {
        let mut servers = serde_json::json!({ "daemon8": daemon8_entry });
        if include_channel {
            servers
                .as_object_mut()
                .unwrap()
                .insert("daemon8-channel".to_string(), channel_entry);
        }
        let json = serde_json::json!({ "mcpServers": servers });
        std::fs::write(config_path, serde_json::to_string_pretty(&json)?)?;
    }

    Ok(())
}

fn setup_service() -> Result<()> {
    if service_installed() {
        let port = crate::config::load(None).unwrap_or_default().server.port;
        let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let running =
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(1)).is_ok();

        let binary_stale = service_binary_stale();

        if running && !binary_stale {
            cliclack::log::info("System service: running")?;
            return Ok(());
        }

        let reason = if binary_stale && !running {
            "System service is registered but the binary path has changed and it is not running. Re-register?"
        } else if binary_stale {
            "System service is running but points to a different binary. Re-register?"
        } else {
            "System service is registered but not running. Repair?"
        };

        let repair: bool = cliclack::confirm(reason).initial_value(true).interact()?;

        if !repair {
            cliclack::log::warning("Run 'daemon8 install' to repair later.")?;
            return Ok(());
        }

        let spinner = cliclack::spinner();
        spinner.start("Repairing system service...");
        match crate::service::cmd_install() {
            Ok(()) => spinner.stop("System service repaired"),
            Err(e) => {
                spinner.stop(format!("Repair failed: {e}"));
                cliclack::log::warning("Run 'daemon8 install' to try again.")?;
            }
        }
        return Ok(());
    }

    cliclack::log::info("Recommended: starts automatically at login and restarts on crash.")?;
    let should_install: bool = cliclack::confirm("Install Daemon8 as a system service?")
        .initial_value(true)
        .interact()?;

    if should_install {
        let spinner = cliclack::spinner();
        spinner.start("Registering system service...");
        match crate::service::cmd_install() {
            Ok(()) => spinner.stop("System service registered"),
            Err(e) => {
                spinner.stop(format!("Service registration failed: {e}"));
                cliclack::log::warning("Run 'daemon8 install' later to try again.")?;
            }
        }
    }

    Ok(())
}

fn service_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        let plist = dirs_home().join("Library/LaunchAgents/dev.daemon8.daemon.plist");
        plist.exists()
    }

    #[cfg(target_os = "linux")]
    {
        let unit = dirs_home().join(".config/systemd/user/daemon8.service");
        unit.exists()
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

fn service_binary_stale() -> bool {
    let current = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(_) => return false,
    };

    #[cfg(target_os = "macos")]
    {
        let plist = dirs_home().join("Library/LaunchAgents/dev.daemon8.daemon.plist");
        if let Ok(contents) = std::fs::read_to_string(&plist)
            && let Some(start) = contents.find("<key>ProgramArguments</key>")
        {
            let rest = &contents[start..];
            if let Some(s) = rest.find("<string>") {
                let after = &rest[s + 8..];
                if let Some(e) = after.find("</string>") {
                    let registered = &after[..e];
                    let registered_path = std::path::Path::new(registered);
                    if let Ok(canonical) = registered_path.canonicalize() {
                        return canonical != current;
                    }
                    return !registered_path.exists();
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let unit = dirs_home().join(".config/systemd/user/daemon8.service");
        if let Ok(contents) = std::fs::read_to_string(&unit) {
            for line in contents.lines() {
                if let Some(exec) = line.strip_prefix("ExecStart=") {
                    let binary = exec.split_whitespace().next().unwrap_or("");
                    let registered_path = std::path::Path::new(binary);
                    if let Ok(canonical) = registered_path.canonicalize() {
                        return canonical != current;
                    }
                    return !registered_path.exists();
                }
            }
        }
    }

    #[cfg(windows)]
    {
        let output = std::process::Command::new("schtasks")
            .args(["/Query", "/TN", "Daemon8", "/XML"])
            .output();
        let Ok(output) = output else { return false };
        if !output.status.success() {
            return true;
        }
        let xml = String::from_utf8_lossy(&output.stdout);
        if let Some(start) = xml.find("<Command>")
            && let Some(end) = xml[start + 9..].find("</Command>")
        {
            let registered = xml[start + 9..start + 9 + end].trim();
            let registered_path = std::path::Path::new(registered);
            if let Ok(canonical) = registered_path.canonicalize() {
                return canonical != current;
            }
            return !registered_path.exists();
        }
    }

    false
}

async fn run_health_check() {
    let port = crate::config::load(None).unwrap_or_default().server.port;
    let spinner = cliclack::spinner();
    spinner.start("Verifying daemon is running...");

    for attempt in 1..=8u8 {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        if let Ok(resp) = reqwest::get(format!("http://127.0.0.1:{port}/health")).await
            && resp.status().is_success()
        {
            spinner.stop(format!("Daemon8 is running on port {port}"));
            return;
        }
        if attempt < 8 {
            spinner.start(format!("Waiting for daemon to start ({attempt}/8)..."));
        }
    }

    spinner.stop("Daemon8 is not running yet");
    let _ = cliclack::log::warning(
        "The system service may need a moment to start. Run 'daemon8 status' to check.",
    );
}

fn read_config_value(config_path: &Path, section: &str, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(config_path).ok()?;
    let table: toml::Table = contents.parse().ok()?;
    table
        .get(section)?
        .as_table()?
        .get(key)?
        .as_str()
        .map(String::from)
}

fn write_config_value(config_path: &Path, section: &str, key: &str, value: &str) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut table: toml::Table = if config_path.exists() {
        let contents = std::fs::read_to_string(config_path)?;
        contents.parse().unwrap_or_default()
    } else {
        toml::Table::new()
    };

    let sect = table
        .entry(section)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("section is not a table")?;
    sect.insert(key.to_string(), toml::Value::String(value.to_string()));

    std::fs::write(config_path, toml::to_string_pretty(&table)?)?;
    Ok(())
}

fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn home_dir_exists(relative: &str) -> bool {
    dirs_home().join(relative).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_config_value_creates_section() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");

        write_config_value(&config_path, "browser", "path", "/usr/bin/chrome").unwrap();
        write_config_value(&config_path, "browser", "auto", "true").unwrap();

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let table: toml::Table = contents.parse().unwrap();
        let browser = table["browser"].as_table().unwrap();

        assert_eq!(browser["path"].as_str(), Some("/usr/bin/chrome"));
        assert_eq!(browser["auto"].as_str(), Some("true"));
    }

    #[test]
    fn read_config_value_returns_none_for_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, "[server]\nport = 9077\n").unwrap();

        assert!(read_config_value(&config_path, "browser", "path").is_none());
        assert!(read_config_value(&config_path, "server", "host").is_none());
    }
}
