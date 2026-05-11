// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::env;

use anyhow::Result;
use clap::Subcommand;

use daemon8_providers::hook_management::{
    list_all_hooks, parse_hook_provider, parse_scope, remove_hooks, repair_hooks, update_hooks,
};

#[derive(Subcommand)]
pub enum HooksSubcommand {
    /// List installed daemon8 hooks across providers and scopes.
    List,
    /// Remove daemon8 hooks. Provider required; scope optional (claude only).
    Remove {
        /// claude | codex | gemini
        provider: String,
        /// local | shared | global  (claude only; others have no scope)
        #[arg(long)]
        scope: Option<String>,
    },
    /// Reinstall daemon8 hooks for the given provider/scope (binary path drift fix).
    Update {
        /// claude | codex | gemini
        provider: String,
        /// local | shared | global  (claude only)
        #[arg(long)]
        scope: Option<String>,
    },
    /// Detect drift across all providers and reinstall as needed.
    Repair,
}

pub async fn cmd_hooks_mcp(action: daemon8_mcp::HooksToolAction) -> String {
    let cwd = match env::current_dir() {
        Ok(p) => p,
        Err(e) => return error_payload(&format!("cwd unavailable: {e}")),
    };
    let result: anyhow::Result<serde_json::Value> = (|| -> anyhow::Result<serde_json::Value> {
        Ok(match action.action.as_str() {
            "list" => serde_json::to_value(list_all_hooks(&cwd, &crate::cli_config::SERVICE)?)?,
            "remove" => {
                let provider = action
                    .provider
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("provider is required for remove"))?;
                let provider = parse_hook_provider(provider)?;
                let scope = action.scope.as_deref().map(parse_scope).transpose()?;
                serde_json::to_value(remove_hooks(
                    provider,
                    scope,
                    &cwd,
                    &crate::cli_config::SERVICE,
                )?)?
            }
            "update" => {
                let provider = action
                    .provider
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("provider is required for update"))?;
                let provider = parse_hook_provider(provider)?;
                let scope = action.scope.as_deref().map(parse_scope).transpose()?;
                serde_json::to_value(update_hooks(
                    provider,
                    scope,
                    &cwd,
                    &crate::cli_config::SERVICE,
                )?)?
            }
            "repair" => serde_json::to_value(repair_hooks(&cwd, &crate::cli_config::SERVICE)?)?,
            other => anyhow::bail!(
                "unknown hooks action '{other}' (valid: list, remove, update, repair)"
            ),
        })
    })();
    match result {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|e| error_payload(&e.to_string())),
        Err(e) => error_payload(&e.to_string()),
    }
}

fn error_payload(msg: &str) -> String {
    serde_json::to_string(&serde_json::json!({"error": msg})).unwrap_or_default()
}

pub async fn cmd_hooks(sub: HooksSubcommand) -> Result<()> {
    let cwd = env::current_dir()?;

    let payload = match sub {
        HooksSubcommand::List => {
            serde_json::to_value(list_all_hooks(&cwd, &crate::cli_config::SERVICE)?)?
        }
        HooksSubcommand::Remove { provider, scope } => {
            let provider = parse_hook_provider(&provider)?;
            let scope = scope.as_deref().map(parse_scope).transpose()?;
            serde_json::to_value(remove_hooks(
                provider,
                scope,
                &cwd,
                &crate::cli_config::SERVICE,
            )?)?
        }
        HooksSubcommand::Update { provider, scope } => {
            let provider = parse_hook_provider(&provider)?;
            let scope = scope.as_deref().map(parse_scope).transpose()?;
            serde_json::to_value(update_hooks(
                provider,
                scope,
                &cwd,
                &crate::cli_config::SERVICE,
            )?)?
        }
        HooksSubcommand::Repair => {
            serde_json::to_value(repair_hooks(&cwd, &crate::cli_config::SERVICE)?)?
        }
    };

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
