// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::io::IsTerminal;

use anyhow::{Result, bail};
use clap::Args;

use crate::providers::{
    HookScope, Provider, detect_ai_tools, dirs_home, install_claude_hooks, install_codex_hooks,
    install_gemini_hooks,
};

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
    let detected = detect_ai_tools();
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

    let selected: Vec<Provider> = cliclack::multiselect("Install hooks for which providers?")
        .required(false)
        .items(
            &hook_providers
                .iter()
                .map(|p| (*p, p.label(), ""))
                .collect::<Vec<_>>(),
        )
        .interact()?;

    let home = dirs_home();
    let cwd = std::env::current_dir()?;

    for provider in selected {
        match provider {
            Provider::ClaudeCode => {
                let scope: HookScope = cliclack::select("Claude Code hook scope")
                    .item(HookScope::Local, "local", ".claude/settings.local.json")
                    .item(HookScope::Shared, "shared", ".claude/settings.json")
                    .item(HookScope::Global, "global", "~/.claude/settings.json")
                    .interact()?;
                let path = install_claude_hooks(scope, &cwd, &home, false)?;
                println!("  wrote: {}", path.display());
            }
            Provider::Codex => {
                let path = install_codex_hooks(&home, false)?;
                println!("  wrote: {}", path.display());
            }
            Provider::Gemini => {
                let path = install_gemini_hooks(&home, false)?;
                println!("  wrote: {}", path.display());
            }
            Provider::OpenCode => {}
        }
    }

    Ok(())
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
