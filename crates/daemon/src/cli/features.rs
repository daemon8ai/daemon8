// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Args;

use daemon8_providers::hook_management::{self, InstalledHookGroup, scope_label};
use daemon8_providers::traits::HookScope;
use daemon8_providers::{Provider, ServiceIdentity, detect_ai_tools, dirs_home};

#[derive(Args, Default)]
pub struct FeaturesArgs {
    /// List available features without interactive prompts.
    #[arg(long)]
    pub list: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feature {
    Hooks,
    ProjectInit,
}

pub(crate) struct HookInstallEntry {
    pub provider: Provider,
    pub scope: HookScope,
}

pub(crate) struct HookInstallPlan {
    pub entries: Vec<HookInstallEntry>,
}

pub(crate) struct HookInstallResult {
    pub provider: &'static str,
    pub scope: &'static str,
    pub path: Option<PathBuf>,
    pub status: InstallStatus,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallStatus {
    Installed,
    Updated,
    Failed(String),
}

pub(crate) struct HookInstallSummary {
    pub results: Vec<HookInstallResult>,
}

pub fn cmd_features(args: FeaturesArgs) -> Result<()> {
    if args.list {
        print_feature_list();
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        bail!("daemon8 features requires an interactive terminal (use --list for non-interactive)");
    }

    let selected: Vec<Feature> = cliclack::multiselect("Which features do you want to enable?")
        .required(false)
        .item(
            Feature::Hooks,
            "CLI hooks",
            "capture tool calls as observations",
        )
        .item(
            Feature::ProjectInit,
            "Project init",
            "scaffold .daemon8.toml in cwd",
        )
        .interact()?;

    if selected.is_empty() {
        println!("no features selected");
        return Ok(());
    }

    if selected.contains(&Feature::Hooks) {
        enable_hooks()?;
    }

    if selected.contains(&Feature::ProjectInit) {
        enable_project_init()?;
    }

    Ok(())
}

fn enable_hooks() -> Result<()> {
    let service = &crate::cli_config::SERVICE;
    let detected = detect_ai_tools(service);
    let hook_providers: Vec<Provider> = detected
        .iter()
        .filter(|d| d.provider.as_hook_provider().is_some())
        .map(|d| d.provider)
        .collect();

    if hook_providers.is_empty() {
        println!(
            "no hook-capable providers detected; install Claude Code, Codex, or Gemini CLI first"
        );
        return Ok(());
    }

    let home = dirs_home();
    let cwd = std::env::current_dir()?;

    let existing = hook_management::list_all_hooks(&cwd, service)?;
    if !existing.is_empty() {
        println!("\nexisting daemon8 hooks:");
        for group in &existing {
            println!(
                "  {} ({}) -- {} hook(s) at {}",
                group.provider,
                group.scope.unwrap_or("?"),
                group.entries.len(),
                group.settings_path.display(),
            );
        }
        println!();
    }

    let selected: Vec<Provider> = cliclack::multiselect("Install hooks for which providers?")
        .required(false)
        .items(
            &hook_providers
                .iter()
                .map(|p| (*p, p.label(), ""))
                .collect::<Vec<_>>(),
        )
        .interact()?;

    let mut plan = HookInstallPlan {
        entries: Vec::new(),
    };

    for provider in selected {
        let Some(hp) = provider.as_hook_provider() else {
            continue;
        };
        let scopes = hp.supported_scopes();
        let scope = if scopes.len() == 1 {
            scopes[0]
        } else {
            let mut select = cliclack::select(format!("{} hook scope", provider.label()));
            for &s in scopes {
                select = select.item(s, scope_label(s), hp.scope_display_hint(s, &cwd, &home));
            }
            select.interact()?
        };

        let conflicts = detect_conflicts(provider, scope, &existing);
        if !conflicts.is_empty() {
            for msg in &conflicts {
                println!("  warning: {msg}");
            }
            if !cliclack::confirm("Continue anyway?")
                .initial_value(false)
                .interact()?
            {
                continue;
            }
        }

        plan.entries.push(HookInstallEntry { provider, scope });
    }

    if plan.entries.is_empty() {
        println!("no hooks to install");
        return Ok(());
    }

    let summary = execute_hook_plan(&plan, &cwd, &home, service);
    print_install_summary(&summary);

    Ok(())
}

pub(crate) fn detect_conflicts(
    provider: Provider,
    chosen_scope: HookScope,
    existing: &[InstalledHookGroup],
) -> Vec<String> {
    let label = provider.label();
    existing
        .iter()
        .filter(|g| g.provider == label && g.scope != Some(scope_label(chosen_scope)))
        .map(|g| {
            format!(
                "daemon8 hooks already exist at {} scope for {} -- \
                 installing at {} scope will cause double-firing",
                g.scope.unwrap_or("?"),
                label,
                scope_label(chosen_scope),
            )
        })
        .collect()
}

pub(crate) fn execute_hook_plan(
    plan: &HookInstallPlan,
    cwd: &Path,
    home: &Path,
    service: &ServiceIdentity,
) -> HookInstallSummary {
    let mut results = Vec::new();

    for entry in &plan.entries {
        let Some(hp) = entry.provider.as_hook_provider() else {
            continue;
        };

        let already_installed = hp
            .list_hooks(entry.scope, cwd, home, service)
            .map(|e| !e.is_empty())
            .unwrap_or(false);

        match hp.install_hooks(entry.scope, cwd, home, already_installed, service) {
            Ok(path) => {
                results.push(HookInstallResult {
                    provider: entry.provider.label(),
                    scope: scope_label(entry.scope),
                    path: Some(path),
                    status: if already_installed {
                        InstallStatus::Updated
                    } else {
                        InstallStatus::Installed
                    },
                });
            }
            Err(e) => {
                results.push(HookInstallResult {
                    provider: entry.provider.label(),
                    scope: scope_label(entry.scope),
                    path: None,
                    status: InstallStatus::Failed(e.to_string()),
                });
            }
        }
    }

    HookInstallSummary { results }
}

fn print_install_summary(summary: &HookInstallSummary) {
    println!();
    for result in &summary.results {
        let status = match &result.status {
            InstallStatus::Installed => "new",
            InstallStatus::Updated => "updated",
            InstallStatus::Failed(e) => {
                println!("  [ERR] {} ({}) -- {e}", result.provider, result.scope);
                continue;
            }
        };
        if let Some(ref path) = result.path {
            println!(
                "  [{}] {} ({}) -> {}",
                status,
                result.provider,
                result.scope,
                path.display()
            );
        }
    }
}

fn enable_project_init() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let target = cwd.join(crate::cli_config::PROJECT_CONFIG_FILENAME);

    if target.exists() {
        println!("  {} already exists", target.display());
        return Ok(());
    }

    let args = crate::cli::init::InitArgs {
        yes: true,
        ..Default::default()
    };
    crate::cli::init::cmd_init(args)
}

fn print_feature_list() {
    println!("available daemon8 features:");
    println!();
    println!("  hooks          Install CLI hooks to capture tool calls as observations");
    println!("                 Providers: Claude Code, Codex, Gemini CLI");
    println!();
    println!("  project-init   Scaffold .daemon8.toml at current directory");
    println!("                 Defines project slug and file sources");
    println!();
    println!("Run `daemon8 features` (without --list) for interactive setup.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_providers::hook_management::InstalledHookGroup;
    use daemon8_providers::traits::InstalledHookEntry;

    fn mock_group(provider: &'static str, scope: &'static str, count: usize) -> InstalledHookGroup {
        InstalledHookGroup {
            provider,
            scope: Some(scope),
            settings_path: PathBuf::from("/tmp/test"),
            entries: (0..count)
                .map(|i| InstalledHookEntry {
                    event: format!("event_{i}"),
                    command: "daemon8 cli-hook".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn detect_conflicts_finds_scope_mismatch() {
        let existing = vec![mock_group("Claude Code", "local", 3)];
        let conflicts = detect_conflicts(Provider::ClaudeCode, HookScope::Global, &existing);
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("double-firing"));
    }

    #[test]
    fn detect_conflicts_same_scope_is_clean() {
        let existing = vec![mock_group("Claude Code", "local", 3)];
        let conflicts = detect_conflicts(Provider::ClaudeCode, HookScope::Local, &existing);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_different_provider_ignored() {
        let existing = vec![mock_group("Codex", "global", 2)];
        let conflicts = detect_conflicts(Provider::ClaudeCode, HookScope::Global, &existing);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn execute_plan_installs_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        let svc = crate::cli_config::SERVICE;

        let plan = HookInstallPlan {
            entries: vec![HookInstallEntry {
                provider: Provider::ClaudeCode,
                scope: HookScope::Local,
            }],
        };

        let summary = execute_hook_plan(&plan, &cwd, &home, &svc);
        assert_eq!(summary.results.len(), 1);
        assert_eq!(summary.results[0].status, InstallStatus::Installed);
        assert!(summary.results[0].path.is_some());
    }
}
