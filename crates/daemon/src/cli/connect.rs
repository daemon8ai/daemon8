// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 connect` -- classify an explicit alpha scope and report the next step.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use daemon8_core::control::{AlphaStatus, ConnectRequest, connect};

#[derive(clap::Args, Default)]
pub struct ConnectArgs {
    /// Project or general directory to connect this session to.
    #[arg(long)]
    pub path: PathBuf,

    /// Calling agent/provider name, e.g. codex, claude, gemini.
    #[arg(long)]
    pub provider: String,

    /// Optional human-readable agent name.
    #[arg(long)]
    pub agent_name: Option<String>,

    /// Optional provider transcript path for future conversation-source binding.
    #[arg(long)]
    pub transcript_path: Option<PathBuf>,

    /// Emit the common alpha JSON envelope.
    #[arg(long)]
    pub json: bool,
}

pub fn cmd_connect(args: ConnectArgs) -> Result<()> {
    let outcome = connect(ConnectRequest {
        session_id: next_cli_session_id(),
        provider: args.provider,
        project_path: args.path,
        agent_name: args.agent_name,
        transcript_path: args.transcript_path,
    });

    if args.json {
        println!("{}", outcome.envelope.render());
        return Ok(());
    }

    match outcome.envelope.status {
        AlphaStatus::Success | AlphaStatus::SetupRequired | AlphaStatus::ConnectRequired => {
            println!("{}", outcome.envelope.message);
            if let Some(why) = &outcome.envelope.why {
                println!("{why}");
            }
            for action in &outcome.envelope.next_actions {
                println!("next: {} ({})", action.tool, action.reason);
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

fn next_cli_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("cli-{nanos}")
}
