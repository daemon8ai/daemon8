// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use daemon8_store::SurrealStore;

use crate::config;

#[derive(clap::Args)]
pub(crate) struct ResetArgs {
    #[arg(long)]
    pub yes: bool,
}

pub(crate) async fn cmd_reset(config_path: Option<String>, args: ResetArgs) -> Result<()> {
    if !args.yes {
        eprint!(
            "Wipe daemon-owned state (observations, memory, debug sessions/checkpoints, session/scope ledger, cursors, schema metadata)? [y/N] "
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());

    if !db_path.exists() {
        eprintln!(
            "No database found at {}. Nothing to reset.",
            db_path.display()
        );
        return Ok(());
    }

    let store = SurrealStore::open_for_reset(&db_path)
        .await
        .with_context(|| format!("opening database: {}", db_path.display()))?;

    let report = store.reset().await.context("reset failed")?;

    eprintln!(
        "daemon8 reset complete: cleared observations/live feed, memories, debug sessions/checkpoints, session/scope ledger, cursors, and schema metadata; {} observations dropped, {} session/scope ledger records dropped, {} cursor records dropped; schema re-initialized at {}; project files were untouched",
        report.observations_dropped,
        report.scope_ledger_records_dropped,
        report.cursor_states_dropped,
        daemon8_store::SCHEMA_VERSION,
    );
    Ok(())
}
