// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

#[cfg(target_os = "macos")]
use std::io::IsTerminal;
use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::Subcommand;
use daemon8_adb::transport::{AdbTransport, DeviceTransport};

use crate::config;
use crate::style;

#[derive(Subcommand)]
pub(crate) enum FeatureSubcommand {
    /// Show all device feature gates.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Manage generic Android/ADB device support.
    Adb {
        #[command(subcommand)]
        command: ToggleCommand,
    },
    /// Manage Vega Virtual Device support.
    Vvd {
        #[command(subcommand)]
        command: VvdCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum ToggleCommand {
    /// Enable the feature.
    Enable,
    /// Disable the feature.
    Disable,
    /// Show feature status.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum VvdCommand {
    /// Enable Vega Virtual Device support after preflight checks.
    Enable {
        /// Explicit path to the Vega CLI. If omitted, daemon8 uses config or PATH.
        #[arg(long)]
        vega_cli: Option<PathBuf>,
    },
    /// Disable Vega Virtual Device support.
    Disable,
    /// Show feature status.
    Status {
        #[arg(long)]
        json: bool,
    },
}

pub(crate) async fn cmd_feature(
    config_path: Option<String>,
    subcommand: FeatureSubcommand,
) -> Result<()> {
    match subcommand {
        FeatureSubcommand::Status { json } => cmd_status(config_path, json).await,
        FeatureSubcommand::Adb { command } => match command {
            ToggleCommand::Enable => {
                write_feature_config(config_path.as_deref(), |table| {
                    set_toml_bool(table, &["device", "adb"], "enabled", true)?;
                    Ok(())
                })?;
                println!("Enabled generic Android/ADB device support.");
                print_restart_hint();
                Ok(())
            }
            ToggleCommand::Disable => {
                write_feature_config(config_path.as_deref(), |table| {
                    set_toml_bool(table, &["device", "adb"], "enabled", false)?;
                    set_toml_bool(table, &["device", "vvd"], "enabled", false)?;
                    Ok(())
                })?;
                println!("Disabled generic Android/ADB device support.");
                println!("Disabled VVD support because it depends on ADB transport.");
                print_restart_hint();
                Ok(())
            }
            ToggleCommand::Status { json } => cmd_status(config_path, json).await,
        },
        FeatureSubcommand::Vvd { command } => match command {
            VvdCommand::Enable { vega_cli } => cmd_vvd_enable(config_path, vega_cli).await,
            VvdCommand::Disable => {
                write_feature_config(config_path.as_deref(), |table| {
                    set_toml_bool(table, &["device", "vvd"], "enabled", false)?;
                    Ok(())
                })?;
                println!("Disabled Vega Virtual Device support.");
                print_restart_hint();
                Ok(())
            }
            VvdCommand::Status { json } => cmd_status(config_path, json).await,
        },
    }
}

async fn cmd_vvd_enable(config_path: Option<String>, vega_cli: Option<PathBuf>) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;
    let vega_cli_path = resolve_vega_cli(vega_cli.or(cfg.device.vvd.vega_cli_path.clone()))?;
    let version = run_vega_version(&vega_cli_path)?;

    println!("Vega CLI detected.");
    println!("  {} {}", style::label("path"), vega_cli_path.display());
    println!("  {} {}", style::label("version"), version.trim());
    println!();
    println!("VVD screenshots use host window capture on macOS.");
    println!("daemon8 will request Screen Recording permission now if macOS has not granted it.");
    println!("Final permission proof still comes from a daemon-run screenshot after restart.");

    #[cfg(target_os = "macos")]
    {
        if crate::cli::service::request_macos_screen_recording_permission() {
            println!();
            println!("Screen Recording permission request completed for this daemon8 binary.");
        } else {
            println!();
            println!("Screen Recording permission is not granted yet.");
        }
        if std::io::stdin().is_terminal() {
            println!("Opening Screen Recording settings so you can verify daemon8 after restart.");
            crate::cli::service::macos_open_privacy_pane();
        } else {
            println!("Verify daemon8 in System Settings > Privacy & Security > Screen Recording.");
        }
    }

    probe_vega_devices(cfg.device.adb.server_addr).await;

    write_feature_config(config_path.as_deref(), |table| {
        set_toml_bool(table, &["device", "adb"], "enabled", true)?;
        set_toml_bool(table, &["device", "vvd"], "enabled", true)?;
        set_toml_string(
            table,
            &["device", "vvd"],
            "vega_cli_path",
            &vega_cli_path.display().to_string(),
        )?;
        Ok(())
    })?;

    println!();
    println!("Enabled Vega Virtual Device support.");
    println!("Enabled generic Android/ADB transport because VVD depends on it.");
    print_restart_hint();
    Ok(())
}

async fn cmd_status(config_path: Option<String>, json: bool) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;
    let vega_cli = resolve_vega_cli(cfg.device.vvd.vega_cli_path.clone()).ok();
    let vega_version = vega_cli
        .as_deref()
        .and_then(|path| run_vega_version(path).ok());

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "device": {
                    "adb": {
                        "enabled": cfg.device.adb.enabled,
                        "server_addr": cfg.device.adb.server_addr,
                        "scan_interval_secs": cfg.device.adb.scan_interval_secs,
                    },
                    "vvd": {
                        "enabled": cfg.device.vvd.enabled,
                        "vega_cli_path": vega_cli.as_ref().map(|path| path.display().to_string()),
                        "vega_cli_version": vega_version,
                        "screen_recording": screen_recording_state(),
                    }
                }
            }))?
        );
        return Ok(());
    }

    let bval = |enabled: bool| {
        if enabled {
            style::green("enabled")
        } else {
            "disabled".to_string()
        }
    };
    let path_val = |path: Option<&Path>| {
        path.map(|p| p.display().to_string())
            .unwrap_or_else(|| style::dim("(not found)"))
    };

    println!("  {}", style::blue("Device Features"));
    println!(
        "    {} {}",
        style::label("adb"),
        bval(cfg.device.adb.enabled)
    );
    println!(
        "    {} {}",
        style::label("adb server"),
        cfg.device.adb.server_addr
    );
    println!(
        "    {} {}",
        style::label("vvd"),
        bval(cfg.device.vvd.enabled)
    );
    println!(
        "    {} {}",
        style::label("vega cli"),
        path_val(vega_cli.as_deref())
    );
    if let Some(version) = vega_version {
        println!("    {} {}", style::label("vega version"), version.trim());
    }
    println!(
        "    {} {}",
        style::label("screen recording"),
        screen_recording_state()
    );

    Ok(())
}

async fn probe_vega_devices(addr: SocketAddrV4) {
    let transport = AdbTransport::new(addr);
    let devices = match transport.list_devices().await {
        Ok(devices) => devices,
        Err(e) => {
            println!();
            println!("ADB device probe skipped: {e}");
            return;
        }
    };

    let mut vvd_count = 0usize;
    for device in devices {
        if let Ok(output) = transport
            .shell_command(&device.serial, "which loggingctl")
            .await
            && !output.trim().is_empty()
            && !output.contains("not found")
        {
            vvd_count += 1;
        }
    }

    println!();
    if vvd_count == 0 {
        println!("No active VVD was detected. The feature is still enabled for the next launch.");
    } else {
        println!("Detected {vvd_count} active VVD device(s).");
    }
}

fn resolve_vega_cli(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        validate_executable(&path, "Vega CLI")?;
        return Ok(path);
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("vega");
        if validate_executable(&candidate, "Vega CLI").is_ok() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "Vega CLI not found. Install the Vega SDK or pass --vega-cli /path/to/vega before enabling VVD support."
    );
}

fn validate_executable(path: &Path, label: &str) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} path does not exist: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("{label} path is not a file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta =
            std::fs::metadata(path).with_context(|| format!("cannot stat {}", path.display()))?;
        if meta.permissions().mode() & 0o111 == 0 {
            anyhow::bail!("{label} path is not executable: {}", path.display());
        }
    }
    Ok(())
}

fn run_vega_version(path: &Path) -> Result<String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Vega CLI version check failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn screen_recording_state() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "requires_daemon_screenshot_verification"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "not_applicable"
    }
}

fn write_feature_config<F>(config_path: Option<&str>, update: F) -> Result<()>
where
    F: FnOnce(&mut toml::Table) -> Result<()>,
{
    let cfg = config::load(config_path).unwrap_or_default();
    let file_path = config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| cfg.config_dir.join("config.toml"));

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory: {}", parent.display()))?;
    }

    let mut table: toml::Table = if file_path.exists() {
        let contents = std::fs::read_to_string(&file_path)
            .with_context(|| format!("reading config file: {}", file_path.display()))?;
        contents
            .parse()
            .with_context(|| format!("parsing config file: {}", file_path.display()))?
    } else {
        toml::Table::new()
    };

    update(&mut table)?;

    let tmp_path = file_path.with_extension("toml.tmp");
    let serialized = toml::to_string_pretty(&table)?;
    std::fs::write(&tmp_path, serialized)
        .with_context(|| format!("writing temp config file: {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &file_path)
        .with_context(|| format!("renaming config file: {}", file_path.display()))?;
    Ok(())
}

fn set_toml_bool(
    table: &mut toml::Table,
    section_path: &[&str],
    field: &str,
    value: bool,
) -> Result<()> {
    table_at_mut(table, section_path)?.insert(field.into(), toml::Value::Boolean(value));
    Ok(())
}

fn set_toml_string(
    table: &mut toml::Table,
    section_path: &[&str],
    field: &str,
    value: &str,
) -> Result<()> {
    table_at_mut(table, section_path)?.insert(field.into(), toml::Value::String(value.into()));
    Ok(())
}

fn table_at_mut<'a>(mut table: &'a mut toml::Table, path: &[&str]) -> Result<&'a mut toml::Table> {
    for section in path {
        table = table
            .entry((*section).to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("{section} is not a table"))?;
    }
    Ok(table)
}

fn print_restart_hint() {
    println!();
    println!("Restart daemon8 for the feature change to take effect:");
    println!("  daemon8 service install --yes --no-provider-setup --no-instruction-setup");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_feature_config_creates_device_sections_without_dropping_existing_keys() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[server]
port = 9999
"#,
        )
        .unwrap();

        write_feature_config(config_path.to_str(), |table| {
            set_toml_bool(table, &["device", "adb"], "enabled", true)?;
            set_toml_bool(table, &["device", "vvd"], "enabled", true)?;
            set_toml_string(table, &["device", "vvd"], "vega_cli_path", "/tmp/vega")?;
            Ok(())
        })
        .unwrap();

        let cfg = config::load(config_path.to_str()).unwrap();
        assert_eq!(cfg.server.port, 9999);
        assert!(cfg.device.adb.enabled);
        assert!(cfg.device.vvd.enabled);
        assert_eq!(
            cfg.device.vvd.vega_cli_path,
            Some(PathBuf::from("/tmp/vega"))
        );
    }

    #[test]
    fn resolve_vega_cli_rejects_missing_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-vega");

        let err = resolve_vega_cli(Some(missing)).unwrap_err();

        assert!(err.to_string().contains("does not exist"));
    }
}
