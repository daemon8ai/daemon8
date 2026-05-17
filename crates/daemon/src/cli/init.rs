// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8/config.md`.

use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use daemon8_core::control::{AlphaEnvelope, AlphaStatus};
use daemon8_core::init::{InitRequest, init_project};
use daemon8_store::{RecentScopeRecord, ScopeLedgerStore, SurrealStore};

use crate::config;

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
}

pub async fn cmd_init(config_path: Option<String>, args: InitArgs) -> Result<()> {
    let path = match args.path {
        Some(path) => path,
        None => env::current_dir()?,
    };

    let outcome = init_project(InitRequest {
        project_path: path,
        name: args.name,
        overwrite: args.force,
    });

    if let Err(err) = record_init_outcome(config_path.as_deref(), &outcome.envelope).await {
        tracing::warn!(error = %err, "scope ledger init recording failed");
    }

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

async fn record_init_outcome(config_path: Option<&str>, envelope: &AlphaEnvelope) -> Result<()> {
    if envelope.status != AlphaStatus::Success {
        return Ok(());
    }
    let Some(scope_root) = envelope_data_str(envelope, "scope_root") else {
        return Ok(());
    };

    let cfg = config::load(config_path).unwrap_or_default();
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    let store = SurrealStore::open(&db_path).await?;
    let ledger = store.scope_ledger_store();
    let now = current_ns();

    ledger
        .record_recent_scope(RecentScopeRecord {
            id: None,
            mode: envelope_data_str(envelope, "mode").unwrap_or_else(|| "project".into()),
            requested_path: envelope_data_str(envelope, "requested_path")
                .unwrap_or_else(|| scope_root.clone()),
            scope_root,
            provider: None,
            agent_name: None,
            session_id: None,
            project_name: envelope_data_str(envelope, "project_name"),
            source_count: envelope_data_u64(envelope, "source_count"),
            first_seen_at: now,
            last_seen_at: now,
        })
        .await?;
    Ok(())
}

fn envelope_data_str(envelope: &AlphaEnvelope, key: &str) -> Option<String> {
    envelope
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn envelope_data_u64(envelope: &AlphaEnvelope, key: &str) -> Option<u64> {
    envelope
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .and_then(|value| value.as_u64())
}

fn current_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}
