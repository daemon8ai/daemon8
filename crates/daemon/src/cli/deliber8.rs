// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 deliber8` -- namespace for the upcoming background-agent runtime.

use anyhow::{Result, bail};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Deliber8Subcommand {
    /// Manage deliber8 agent definitions and lifecycles
    Agent,
    /// Ask a deliber8 agent or team and receive work through an inbox
    Ask,
    /// Inspect pending deliber8 messages
    Inbox,
    /// Inspect or manage deliber8 memory context
    Memory,
    /// Diagnose deliber8 runtime state
    Doctor,
}

pub fn cmd_deliber8(subcommand: Deliber8Subcommand) -> Result<()> {
    let name = match subcommand {
        Deliber8Subcommand::Agent => "agent",
        Deliber8Subcommand::Ask => "ask",
        Deliber8Subcommand::Inbox => "inbox",
        Deliber8Subcommand::Memory => "memory",
        Deliber8Subcommand::Doctor => "doctor",
    };

    bail!("daemon8 deliber8 {name} is not implemented yet")
}
