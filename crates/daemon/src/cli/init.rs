// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8/config.md`.

use std::env;
use std::path::PathBuf;

use anyhow::{Result, bail};
use daemon8_core::control::AlphaStatus;
use daemon8_core::init::{InitRequest, init_project};

#[derive(clap::Args, Default)]
pub struct InitArgs {
    /// Overwrite an existing `.daemon8/config.md` at this location.
    #[arg(long)]
    pub force: bool,

    /// Explicit project name. Defaults to the project path basename.
    #[arg(long)]
    pub name: Option<String>,

    /// Project directory to initialize. Defaults to cwd.
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Emit the common alpha JSON envelope.
    #[arg(long)]
    pub json: bool,

    /// Accept defaults without prompting.
    #[arg(short = 'y', long, visible_alias = "no-interaction")]
    pub yes: bool,
}

pub fn cmd_init(args: InitArgs) -> Result<()> {
    let path = match args.path {
        Some(path) => path,
        None => env::current_dir()?,
    };

    let outcome = init_project(InitRequest {
        project_path: path,
        name: args.name,
        overwrite: args.force,
    });

    if args.json {
        println!("{}", outcome.envelope.render());
        return Ok(());
    }

    match outcome.envelope.status {
        AlphaStatus::Success => {
            if let Some(path) = outcome.config_path {
                println!("wrote {}", path.display());
            } else {
                println!("{}", outcome.envelope.message);
            }
            if let Some(name) = outcome
                .envelope
                .data
                .as_ref()
                .and_then(|data| data.get("project_name"))
                .and_then(|name| name.as_str())
            {
                println!("name: {name}");
            }
            Ok(())
        }
        AlphaStatus::Blocked => {
            println!("{}", outcome.envelope.message);
            if let Some(why) = &outcome.envelope.why {
                println!("{why}");
            }
            Ok(())
        }
        _ => bail!(
            "{}: {}",
            outcome.envelope.code,
            outcome
                .envelope
                .why
                .as_deref()
                .unwrap_or(&outcome.envelope.message)
        ),
    }
}
