// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::io::IsTerminal;

use anyhow::{Result, bail};
use clap::Args;

#[derive(Args, Default)]
pub struct FeaturesArgs {
    /// List available features without interactive prompts.
    #[arg(long)]
    pub list: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feature {
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
            Feature::ProjectInit,
            "Project init",
            "scaffold .daemon8/config.md in cwd",
        )
        .interact()?;

    if selected.is_empty() {
        println!("no features selected");
        return Ok(());
    }

    if selected.contains(&Feature::ProjectInit) {
        enable_project_init()?;
    }

    Ok(())
}

fn enable_project_init() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let target = cwd
        .join(crate::cli_config::PROJECT_CONFIG_DIR)
        .join(crate::cli_config::PROJECT_CONFIG_FILENAME);

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
    println!("  project-init   Scaffold .daemon8/config.md at current directory");
    println!("                 Defines project slug and file sources");
    println!();
    println!("Run `daemon8 features` (without --list) for interactive setup.");
}
