// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::config;
use crate::style;

#[derive(Subcommand)]
pub(crate) enum ConfigSubcommand {
    /// Show resolved configuration (default)
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Print config file path
    Path,
    /// Set a config value (e.g. daemon8 config set browser.path "/path/to/browser")
    Set {
        /// Dotted key path (e.g. browser.path, server.port)
        key: String,
        /// Value to set
        value: String,
    },
}

pub(crate) fn cmd_config(config_path: Option<String>, sub: ConfigSubcommand) -> Result<()> {
    match sub {
        ConfigSubcommand::Show { json } => cmd_config_show(config_path, json),
        ConfigSubcommand::Path => cmd_config_path(config_path),
        ConfigSubcommand::Set { key, value } => cmd_config_set(config_path, &key, &value),
    }
}

fn cmd_config_show(config_path: Option<String>, json: bool) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&cfg).unwrap_or_default());
        return Ok(());
    }

    let path_val = |p: Option<&std::path::Path>| match p {
        None => style::dim("(default)"),
        Some(path) => path.display().to_string(),
    };
    let bval = |b: bool| {
        if b {
            style::green("true")
        } else {
            "false".to_string()
        }
    };

    println!("  {}", style::blue("Server"));
    println!("    {} {}", style::label("port"), cfg.server.port);
    println!("    {} {}", style::label("host"), cfg.server.host);
    println!();
    println!("  {}", style::blue("Browser"));
    println!("    {} {}", style::label("endpoint"), cfg.browser.endpoint);
    println!(
        "    {} {}",
        style::label("path"),
        path_val(cfg.browser.path.as_deref())
    );
    println!();
    println!("  {}", style::blue("Storage"));
    println!(
        "    {} {}",
        style::label("path"),
        path_val(cfg.storage.path.as_deref())
    );
    println!();
    println!("  {}", style::blue("Device"));
    println!(
        "    {} {}",
        style::label("adb"),
        bval(cfg.device.adb.enabled)
    );
    println!(
        "    {} {}",
        style::label("vvd"),
        bval(cfg.device.vvd.enabled)
    );
    println!(
        "    {} {}",
        style::label("vega cli"),
        path_val(cfg.device.vvd.vega_cli_path.as_deref())
    );
    println!();
    println!("  {}", style::blue("Debug Sessions"));
    println!(
        "    {} {}",
        style::label("inactivity auto-end secs"),
        cfg.debug_session
            .inactivity_auto_end_secs
            .unwrap_or(crate::cleanup::DEFAULT_INACTIVITY_AUTO_END_SECS)
    );
    println!();
    println!("  {}", style::blue("Logging"));
    println!(
        "    {} {}",
        style::label("path"),
        config::resolve_log_dir(cfg.logging.file.as_deref()).display()
    );
    println!(
        "    {} {}",
        style::label("level"),
        cfg.logging.level.as_str()
    );
    println!(
        "    {} {}",
        style::label("stderr"),
        bval(cfg.logging.stderr)
    );
    println!("    {} daily", style::label("rotation"));
    println!(
        "    {} {}",
        style::label("max files"),
        cfg.logging.max_log_files
    );

    Ok(())
}

fn cmd_config_path(config_path: Option<String>) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    println!("{}", cfg.config_dir.join("config.toml").display());
    Ok(())
}

fn cmd_config_set(config_path: Option<String>, key: &str, value: &str) -> Result<()> {
    validate_config_key_value(key, value)?;

    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    let file_path = config_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cfg.config_dir.join("config.toml"));

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let mut table: toml::Table = if file_path.exists() {
        let contents = std::fs::read_to_string(&file_path).context("reading config file")?;
        contents.parse().context("parsing config file")?
    } else {
        toml::Table::new()
    };

    let parts: Vec<&str> = key.split('.').collect();

    // Navigate to the parent table, creating sections as needed
    let mut current = &mut table;
    for section in &parts[..parts.len() - 1] {
        current = current
            .entry(section.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("{section} is not a table"))?;
    }

    let field = parts
        .last()
        .expect("non-empty parts guaranteed by validate_config_key_value");

    // Type-aware TOML value based on the key's expected type
    let toml_value = if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if value == "true" || value == "false" {
        toml::Value::Boolean(value == "true")
    } else {
        toml::Value::String(value.to_string())
    };

    current.insert(field.to_string(), toml_value);

    // Atomic write: tmp file then rename
    let tmp_path = file_path.with_extension("toml.tmp");
    let serialized = toml::to_string_pretty(&table)?;
    std::fs::write(&tmp_path, &serialized).context("writing temp config file")?;
    std::fs::rename(&tmp_path, &file_path).context("renaming temp config file")?;

    eprintln!("Set {key} = {value}");

    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    let daemon_running =
        std::net::TcpStream::connect(format!("127.0.0.1:{}", cfg.server.port)).is_ok();

    if daemon_running {
        eprintln!();
        eprintln!("  Restart the daemon for this change to take effect:");
        eprintln!("  daemon8 service install");
    }

    Ok(())
}

/// Validate a config key exists and the value is acceptable before writing anything.
fn validate_config_key_value(key: &str, value: &str) -> Result<()> {
    match key {
        "server.port" => {
            let port: u16 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("server.port must be a valid u16, got: '{value}'"))?;
            if port == 0 {
                anyhow::bail!("server.port cannot be 0");
            }
            if port < 1024 {
                eprintln!("Warning: port {port} is in the privileged range (< 1024)");
            }
        }
        "server.host" => {
            if value.is_empty() || value.contains(char::is_whitespace) {
                anyhow::bail!("server.host must be a non-empty string with no whitespace");
            }
        }
        "storage.path" => {
            if !value.is_empty() {
                let p = std::path::Path::new(value);
                if let Some(parent) = p.parent()
                    && !parent.as_os_str().is_empty()
                    && !parent.exists()
                {
                    anyhow::bail!("parent directory does not exist: {}", parent.display());
                }
            }
        }
        "storage.screenshot_path" => {
            if value.is_empty() {
                anyhow::bail!(
                    "storage.screenshot_path must be non-empty (omit the key to use the default)"
                );
            }
        }
        "browser.path" => {
            if !value.is_empty() {
                let p = std::path::Path::new(value);
                if !p.exists() {
                    anyhow::bail!("browser path does not exist: {value}");
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let meta = std::fs::metadata(p)
                        .with_context(|| format!("cannot stat browser path: {value}"))?;
                    if meta.permissions().mode() & 0o111 == 0 {
                        anyhow::bail!("browser path is not executable: {value}");
                    }
                }
            }
        }
        "browser.endpoint" => {
            let Some(after_scheme) = value
                .strip_prefix("https://")
                .or_else(|| value.strip_prefix("http://"))
            else {
                anyhow::bail!(
                    "browser.endpoint must start with http:// or https://, got: '{value}'"
                );
            };
            if after_scheme.is_empty() {
                anyhow::bail!("browser.endpoint has no host after scheme");
            }
        }
        "browser.auto_connect"
        | "ingestion.udp.enabled"
        | "ingestion.unix.enabled"
        | "logging.stderr"
        | "mcp.stdio" => {
            if value != "true" && value != "false" {
                anyhow::bail!("{key} must be 'true' or 'false', got: '{value}'");
            }
        }
        "device.adb.enabled" => {
            anyhow::bail!(
                "device.adb.enabled is managed by feature preflight; use `daemon8 feature adb {}`",
                if value == "true" { "enable" } else { "disable" }
            );
        }
        "device.vvd.enabled" => {
            anyhow::bail!(
                "device.vvd.enabled is managed by feature preflight; use `daemon8 feature vvd {}`",
                if value == "true" { "enable" } else { "disable" }
            );
        }
        "device.adb.server_addr" | "ingestion.udp.bind" => {
            validate_host_port(key, value)?;
        }
        "device.adb.scan_interval_secs" => {
            let n: u64 = value.parse().map_err(|_| {
                anyhow::anyhow!(
                    "device.adb.scan_interval_secs must be a positive integer, got: '{value}'"
                )
            })?;
            if n == 0 {
                anyhow::bail!("device.adb.scan_interval_secs must be greater than 0");
            }
        }
        "device.vvd.vega_cli_path" => {
            if value.is_empty() {
                anyhow::bail!(
                    "device.vvd.vega_cli_path must be non-empty (omit the key to use PATH)"
                );
            }
            let p = std::path::Path::new(value);
            if !p.exists() {
                anyhow::bail!("Vega CLI path does not exist: {value}");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let meta = std::fs::metadata(p)
                    .with_context(|| format!("cannot stat Vega CLI path: {value}"))?;
                if meta.permissions().mode() & 0o111 == 0 {
                    anyhow::bail!("Vega CLI path is not executable: {value}");
                }
            }
        }
        "ingestion.unix.path" => {
            if cfg!(windows) {
                anyhow::bail!(
                    "ingestion.unix.path is not supported on Windows (unix sockets unavailable)"
                );
            }
            if value.is_empty() {
                anyhow::bail!("ingestion.unix.path must be non-empty");
            }
        }
        "logging.level" => match value {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => anyhow::bail!(
                "logging.level must be one of: trace, debug, info, warn, error -- got: '{value}'"
            ),
        },
        "logging.max_log_files" => {
            let n: usize = value.parse().map_err(|_| {
                anyhow::anyhow!("logging.max_log_files must be a positive integer, got: '{value}'")
            })?;
            if n == 0 {
                anyhow::bail!("logging.max_log_files must be greater than 0");
            }
        }
        "debug_session.inactivity_auto_end_secs" => {
            let n: u64 = value.parse().map_err(|_| {
                anyhow::anyhow!(
                    "debug_session.inactivity_auto_end_secs must be a positive integer, got: '{value}'"
                )
            })?;
            if n == 0 {
                anyhow::bail!("debug_session.inactivity_auto_end_secs must be greater than 0");
            }
        }
        "logging.file" => {
            if !value.is_empty() {
                let p = std::path::Path::new(value);
                if let Some(parent) = p.parent()
                    && !parent.as_os_str().is_empty()
                    && !parent.exists()
                {
                    anyhow::bail!("parent directory does not exist: {}", parent.display());
                }
            }
        }
        _ => anyhow::bail!("unknown config key: {key}"),
    }
    Ok(())
}

fn validate_host_port(key: &str, value: &str) -> Result<()> {
    let Some((host, port_str)) = value.rsplit_once(':') else {
        anyhow::bail!("{key} must be host:port format, got: '{value}'");
    };
    if host.is_empty() {
        anyhow::bail!("{key} host part must not be empty");
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| anyhow::anyhow!("{key} port must be 1-65535, got: '{port_str}'"))?;
    if port == 0 {
        anyhow::bail!("{key} port must be 1-65535, got: 0");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_config_key_value_accepts_stdio_bool() {
        validate_config_key_value("mcp.stdio", "true").unwrap();
        validate_config_key_value("mcp.stdio", "false").unwrap();
    }

    #[test]
    fn validate_config_key_value_accepts_device_feature_keys() {
        validate_config_key_value("device.adb.server_addr", "127.0.0.1:5037").unwrap();
        validate_config_key_value("device.adb.scan_interval_secs", "10").unwrap();
    }

    #[test]
    fn validate_config_key_value_rejects_feature_gate_bypass() {
        let err = validate_config_key_value("device.vvd.enabled", "true").unwrap_err();
        assert!(err.to_string().contains("daemon8 feature vvd enable"));

        let err = validate_config_key_value("device.adb.enabled", "false").unwrap_err();
        assert!(err.to_string().contains("daemon8 feature adb disable"));
    }

    #[test]
    fn validate_config_key_value_accepts_debug_session_timeout() {
        validate_config_key_value("debug_session.inactivity_auto_end_secs", "86400").unwrap();
    }

    #[test]
    fn validate_config_key_value_rejects_zero_debug_session_timeout() {
        let err =
            validate_config_key_value("debug_session.inactivity_auto_end_secs", "0").unwrap_err();
        assert!(err.to_string().contains("must be greater than 0"));
    }

    #[test]
    fn validate_config_key_value_rejects_removed_http_toggle() {
        let err = validate_config_key_value("mcp.http", "true").unwrap_err();
        assert!(err.to_string().contains("unknown config key"));
    }
}
