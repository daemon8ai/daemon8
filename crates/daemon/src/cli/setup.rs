// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::Path;

use anyhow::Result;
use clap::Args;
use serde::Serialize;

use daemon8_providers::{DetectedProvider, detect_ai_tools, dirs_home, write_provider_config};

#[derive(Args, Default)]
pub struct SetupArgs {
    /// Emit machine-readable JSON and skip the discovery report.
    #[arg(long)]
    pub json: bool,

    /// Comma-separated providers to register (overrides auto-detection).
    #[arg(long)]
    pub providers: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetupResult {
    providers: Vec<ProviderResult>,
    daemon_running: bool,
    service_installed: bool,
    issues: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProviderResult {
    name: &'static str,
    config_path: String,
    was_configured: bool,
    action: &'static str,
}

pub async fn cmd_setup(_config_path: Option<String>, args: SetupArgs) -> Result<()> {
    let cwd = std::env::current_dir().ok();
    let result = run_setup(args.providers.as_deref(), cwd.as_deref(), false)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_human(&result);
    }

    Ok(())
}

fn run_setup(
    providers_override: Option<&str>,
    cwd: Option<&Path>,
    dry_run: bool,
) -> Result<SetupResult> {
    let detected = detect_ai_tools(&crate::cli_config::SERVICE);
    let targets = resolve_targets(providers_override, &detected)?;

    let port = crate::config::load(None).unwrap_or_default().server.port;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");

    let mut results = Vec::new();
    let mut issues = Vec::new();

    for target in &targets {
        let config_path = target.config_path.clone();
        let was_configured = target.already_configured;

        if was_configured {
            results.push(ProviderResult {
                name: target.provider.label(),
                config_path: config_path.display().to_string(),
                was_configured: true,
                action: "already_configured",
            });
            continue;
        }

        if dry_run {
            results.push(ProviderResult {
                name: target.provider.label(),
                config_path: config_path.display().to_string(),
                was_configured: false,
                action: "would_configure",
            });
            continue;
        }

        match write_provider_config(
            target.provider,
            &config_path,
            &mcp_url,
            cwd,
            &crate::cli_config::SERVICE,
        ) {
            Ok(()) => {
                results.push(ProviderResult {
                    name: target.provider.label(),
                    config_path: config_path.display().to_string(),
                    was_configured: false,
                    action: "configured",
                });
            }
            Err(e) => {
                issues.push(format!("{}: {e}", target.provider.label()));
                results.push(ProviderResult {
                    name: target.provider.label(),
                    config_path: config_path.display().to_string(),
                    was_configured: false,
                    action: "failed",
                });
            }
        }
    }

    if targets.is_empty() {
        issues.push("no AI coding tools detected".into());
    }

    let daemon_running = probe_daemon();
    let service_installed = super::service::service_installed();

    Ok(SetupResult {
        providers: results,
        daemon_running,
        service_installed,
        issues,
    })
}

fn resolve_targets(
    providers_override: Option<&str>,
    detected: &[DetectedProvider],
) -> Result<Vec<DetectedProvider>> {
    if let Some(raw) = providers_override {
        let requested = daemon8_providers::parse_provider_list(raw)?;
        let home = dirs_home();
        return Ok(requested
            .into_iter()
            .map(|p| DetectedProvider {
                provider: p,
                config_path: p.config_path(&home),
                already_configured: detected
                    .iter()
                    .find(|d| d.provider == p)
                    .map(|d| d.already_configured)
                    .unwrap_or(false),
            })
            .collect());
    }
    Ok(detected.to_vec())
}

fn probe_daemon() -> bool {
    let port = crate::config::load(None).unwrap_or_default().server.port;
    std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok()
}

fn print_human(result: &SetupResult) {
    let connected: Vec<&str> = result
        .providers
        .iter()
        .filter(|p| p.action == "configured" || p.action == "already_configured")
        .map(|p| p.name)
        .collect();

    if connected.is_empty() {
        println!("daemon8 setup: no providers configured");
    } else {
        println!("daemon8 is registered with: {}", connected.join(", "));
    }

    if !result.daemon_running {
        println!();
        println!("  daemon is not running. Start with:");
        println!("    daemon8 install   (system service, recommended)");
        println!("    daemon8 serve     (foreground, for testing)");
    }

    for p in &result.providers {
        if p.action == "configured" {
            println!("  wrote: {}", p.config_path);
        }
    }

    if !result.issues.is_empty() {
        println!();
        for issue in &result.issues {
            println!("  warning: {issue}");
        }
    }

    println!();
    println!("To get the most out of daemon8:");
    println!();
    println!("  1. Add instructions to your provider's context file:");
    for p in &connected {
        if let Some(provider) = daemon8_providers::Provider::from_label(p) {
            println!(
                "       {:<12} {}",
                p,
                provider.as_provider().instruction_file_name()
            );
        }
    }
    println!();
    println!("     Tell your AI to use daemon8 for debugging and observation.");
    println!();
    println!("  2. Optional project config:");
    println!("       daemon8 setup features    (interactive menu)");
    println!();
    println!("  3. Inspect:");
    println!("       daemon8 setup init        (write .daemon8/config.md)");
    println!("       daemon8 doctor            (check daemon8 config and source paths)");
    println!();
    println!("  Docs: https://daemon8.ai/docs");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_setup_with_no_providers_reports_issue() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let result = run_setup(None, None, false).unwrap();
        assert!(
            result
                .issues
                .iter()
                .any(|i| i.contains("no AI coding tools"))
        );
    }

    #[test]
    fn run_setup_explicit_provider_attempts_config() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let result = run_setup(Some("claude-code"), Some(tmp.path()), false).unwrap();
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].name, "Claude Code");
    }

    #[test]
    fn run_setup_dry_run_does_not_write() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let result = run_setup(Some("claude-code"), Some(tmp.path()), true).unwrap();
        assert_eq!(result.providers[0].action, "would_configure");
        let config_path = std::path::Path::new(&result.providers[0].config_path);
        assert!(!config_path.exists());
    }
}
