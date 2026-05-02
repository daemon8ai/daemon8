// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};

use crate::config;

pub(crate) fn cmd_logs(config_path: Option<String>, follow: bool) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    let log_dir = config::resolve_log_dir(cfg.logging.file.as_deref());

    if !log_dir.exists() {
        anyhow::bail!("log directory does not exist: {}", log_dir.display());
    }

    let mut logs: Vec<_> = std::fs::read_dir(&log_dir)
        .with_context(|| format!("reading log directory {}", log_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            let is_log = path.extension().map(|ext| ext == "log").unwrap_or(false);
            let is_daemon8 = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("daemon8."));
            is_log && is_daemon8
        })
        .collect();

    logs.sort_by_key(|e| std::cmp::Reverse(e.metadata().ok().and_then(|m| m.modified().ok())));

    let latest = logs.first().map(|e| e.path());

    match latest {
        Some(path) => {
            println!("{}", path.display());
            if follow {
                #[cfg(unix)]
                {
                    let status = std::process::Command::new("tail")
                        .args(["-f", "-n", "100"])
                        .arg(&path)
                        .status()
                        .context("failed to run tail")?;
                    std::process::exit(status.code().unwrap_or(1));
                }
                #[cfg(windows)]
                {
                    let path_str = path.display().to_string();
                    let status = std::process::Command::new("powershell")
                        .args([
                            "-NoProfile",
                            "-NonInteractive",
                            "-Command",
                            &format!(
                                "Get-Content -LiteralPath '{}' -Tail 100 -Wait",
                                path_str.replace('\'', "''")
                            ),
                        ])
                        .status()
                        .context("failed to run PowerShell Get-Content")?;
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
        }
        None => {
            println!("no log files found in {}", log_dir.display());
        }
    }
    Ok(())
}
