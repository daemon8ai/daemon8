// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use daemon8_core::control::status_envelope;
use daemon8_store::{ScopeLedgerStore, ScopeLedgerSummary, SurrealStore};
use daemon8_types::{HealthStatus, RuntimeSummary};
use owo_colors::OwoColorize;
use std::path::PathBuf;

use super::{base_url, format_number};

pub async fn cmd_status(config_path: Option<String>, args: super::ClientArgs) -> Result<()> {
    use crate::config;
    use crate::style;

    let cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;
    let config_file = config_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| cfg.config_dir.join("config.toml"));

    let config_exists = config_file.exists();
    let config_label = if config_exists {
        style::green("exists")
    } else {
        style::dim("not found")
    };

    let data_dir = config::resolve_db_path(cfg.storage.path.as_deref());
    let data_dir_display = data_dir.parent().unwrap_or(&data_dir).display().to_string();

    let screenshot_dir = config::resolve_screenshot_path(&cfg);

    let port = args.port.unwrap_or(cfg.server.port);
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);
    let running =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(1)).is_ok();
    let process_label = if running {
        style::green("running")
    } else {
        style::dim("stopped")
    };

    if args.json {
        let scope_ledger = load_scope_ledger_summary(&data_dir).await;
        let mut data = serde_json::json!({
            "config_path": config_file.display().to_string(),
            "config_exists": config_exists,
            "data_dir": data_dir_display,
            "screenshot_dir": screenshot_dir.display().to_string(),
            "daemon": if running { "running" } else { "stopped" },
            "port": port,
            "daemon_version": env!("CARGO_PKG_VERSION"),
            "connection": serde_json::Value::Null,
            "scope_authority": "none",
            "scope_ledger": scope_ledger.as_ref().ok().cloned().unwrap_or_default(),
        });
        if let Err(err) = scope_ledger {
            data["scope_ledger_error"] = serde_json::json!(err);
        }
        println!("{}", status_envelope(data).render());
        return Ok(());
    }

    println!();
    println!("  {}", style::blue("Daemon8 Status"));
    println!("    {} {}", style::label("Config"), config_file.display());
    println!("    {}   {config_label}", style::label("      "));
    println!("    {} {data_dir_display}", style::label("Data   "));
    println!(
        "    {} {}",
        style::label("Screens"),
        screenshot_dir.display()
    );
    println!("    {} {process_label}", style::label("Daemon "));

    // If daemon is running, also fetch live summary
    if running {
        let url = format!("{}/api/summary", base_url(port));
        if let Ok(resp) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap_or_default()
            .get(&url)
            .send()
            .await
            && resp.status().is_success()
            && let Ok(summary) = resp.json::<RuntimeSummary>().await
        {
            let health_str = match summary.health {
                HealthStatus::Ok => style::green("ok"),
                HealthStatus::ErrorsDetected => "errors_detected".yellow().to_string(),
                HealthStatus::NoSources => style::dim("no_sources"),
            };
            println!();
            println!("    {} {health_str}", style::label("Health "));
            println!(
                "    {} {}",
                style::label("Obs    "),
                format_number(summary.observation_count)
            );
            println!(
                "    {} {}",
                style::label("Errors "),
                summary.error_count_last_60s
            );
            if !summary.active_channels.is_empty() {
                println!(
                    "    {} {}",
                    style::label("Sources"),
                    summary.active_channels.join(", ")
                );
            }
        }
    }

    println!();
    Ok(())
}

async fn load_scope_ledger_summary(
    db_path: &std::path::Path,
) -> Result<ScopeLedgerSummary, String> {
    if !db_path.exists() {
        return Ok(ScopeLedgerSummary::default());
    }

    let store = SurrealStore::open(db_path)
        .await
        .map_err(|err| err.to_string())?;
    store
        .scope_ledger_store()
        .scope_ledger_summary(5)
        .await
        .map_err(|err| err.to_string())
}
