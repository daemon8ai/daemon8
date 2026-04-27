// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use daemon8_embed::EmbedProvider;

use crate::config;
use crate::providers::{
    ProviderWriteSummary, detect_ai_tools, provider_map, summarize_restarts, write_provider_config,
};

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

    setup_embeddings(&config_path)?;

    setup_mcp_configs()?;

    setup_service()?;

    run_health_check().await;

    let screenshot_path = config::resolve_screenshot_path(&config::load(None).unwrap_or_default());
    eprintln!();
    eprintln!("  Screenshots: {}", screenshot_path.display());

    eprintln!();
    eprintln!("  Daemon8 is written by Havy.tech LLC.");
    eprintln!("  Docs:     https://daemon8.ai/docs");
    eprintln!("  Terms:    https://daemon8.ai/terms");
    eprintln!("  Privacy:  https://daemon8.ai/privacy");
    eprintln!();

    cliclack::outro("Daemon8 is successfully installed! The time to build is now!!")?;

    eprintln!();
    eprintln!("  Quick commands:");
    eprintln!("    daemon8 status                                check if daemon is running");
    eprintln!("    daemon8 config set browser.path \"...\"          change default browser");
    eprintln!("    daemon8 config set embeddings.provider \"...\"   change embedding provider");
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

fn setup_embeddings(config_path: &Path) -> Result<()> {
    let existing = read_config_value(config_path, "embeddings", "provider");
    if let Some(ref provider) = existing
        && provider != "none"
        && !provider.is_empty()
    {
        let model = read_config_value(config_path, "embeddings", "model")
            .unwrap_or_else(|| "default".into());
        cliclack::log::info(format!("Embeddings: {provider} (model: {model})"))?;
        return Ok(());
    }

    let selected: &str = cliclack::select("Enable semantic search for observations?")
        .items(&[
            (
                "fastembed",
                "Built-in embeddings",
                "downloads ~24 MB model on first use",
            ),
            (
                "ollama",
                "Ollama",
                "requires running Ollama instance",
            ),
            (
                "openai",
                "OpenAI",
                "requires API key",
            ),
            (
                "none",
                "No, skip for now",
                "",
            ),
        ])
        .interact()?;

    let provider: EmbedProvider = selected.parse().unwrap_or(EmbedProvider::None);

    match provider {
        EmbedProvider::Fastembed => {
            write_config_value(config_path, "embeddings", "provider", "fastembed")?;
            write_config_value(
                config_path,
                "embeddings",
                "model",
                "BAAI/bge-small-en-v1.5",
            )?;
            cliclack::log::info(
                "The embedding model (~24 MB) will download on first daemon start.",
            )?;
        }
        EmbedProvider::Ollama => {
            let endpoint: String = cliclack::input("Ollama endpoint")
                .default_input("http://localhost:11434")
                .interact()?;
            let model: String = cliclack::input("Ollama embedding model")
                .default_input("nomic-embed-text")
                .interact()?;
            write_config_value(config_path, "embeddings", "provider", "ollama")?;
            write_config_value(config_path, "embeddings", "model", &model)?;
            write_config_value(config_path, "embeddings", "endpoint", &endpoint)?;
        }
        EmbedProvider::Openai => {
            let api_key: String = cliclack::password("OpenAI API key").interact()?;
            let model: String = cliclack::input("OpenAI embedding model")
                .default_input("text-embedding-3-small")
                .interact()?;
            let base_url: String = cliclack::input("OpenAI base URL (leave empty for default)")
                .default_input("")
                .interact()?;
            write_config_value(config_path, "embeddings", "provider", "openai")?;
            write_config_value(config_path, "embeddings", "model", &model)?;
            write_config_value(config_path, "embeddings", "api_key", &api_key)?;
            if !base_url.is_empty() {
                write_config_value(config_path, "embeddings", "base_url", &base_url)?;
            }
        }
        EmbedProvider::None => {}
    }

    Ok(())
}

fn setup_mcp_configs() -> Result<()> {
    let tools = detect_ai_tools();

    if tools.is_empty() {
        cliclack::log::info("No AI tools detected. You can configure MCP manually later.")?;
        return Ok(());
    }

    let detected = provider_map(&tools);
    let mut picker = cliclack::multiselect("Select AI tools to configure").required(false);

    for tool in &tools {
        let hint = if tool.already_configured {
            "already configured"
        } else {
            "recommended"
        };
        picker = picker.item(tool.provider, tool.provider.label(), hint);
    }

    let selected = picker
        .initial_values(
            tools
                .iter()
                .filter(|tool| !tool.already_configured)
                .map(|tool| tool.provider)
                .collect::<Vec<_>>(),
        )
        .interact()?;

    if selected.is_empty() {
        cliclack::log::info("No provider configs selected.")?;
        return Ok(());
    }

    let mut summary = ProviderWriteSummary::default();
    let mut configured = 0u32;
    let mut failed = 0u32;

    for provider in selected {
        let Some(tool) = detected.get(&provider) else {
            continue;
        };
        match write_provider_config(provider, &tool.config_path, None) {
            Ok(()) => {
                cliclack::log::success(format!(
                    "{}: config written at {}",
                    provider.label(),
                    tool.config_path.display()
                ))?;
                summary.provider_files.push(tool.config_path.clone());
                summary.note_restart(provider);
                configured += 1;
            }
            Err(e) => {
                cliclack::log::error(format!("{}: {e}", provider.label()))?;
                failed += 1;
            }
        }
    }

    cliclack::log::info(format!("{configured} configured, {failed} failed"))?;
    for message in summarize_restarts(&summary) {
        cliclack::log::warning(message)?;
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
        match super::service::cmd_install() {
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
        match super::service::cmd_install() {
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
    super::service::service_installed()
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
        std::fs::write(&config_path, "[server]\nport = 8888\n").unwrap();

        assert!(read_config_value(&config_path, "browser", "path").is_none());
        assert!(read_config_value(&config_path, "server", "host").is_none());
    }
}
