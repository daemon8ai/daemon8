// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8/config.md`.

use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use daemon8_core::control::{Envelope, Status};
use daemon8_core::init::{InitRequest, RemoveRequest, init_project, remove_project_config};
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

    /// Remove `.daemon8/` from this project directory.
    #[arg(long)]
    pub remove: bool,

    /// Skip the confirmation prompt for destructive operations.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

pub async fn cmd_init(config_path: Option<String>, args: InitArgs) -> Result<()> {
    let path = match &args.path {
        Some(path) => path.clone(),
        None => env::current_dir()?,
    };

    if args.remove {
        return cmd_remove(config_path, &path, args.yes, args.json).await;
    }

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
        Status::Success => {
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
            print_next_actions(&outcome.envelope);
            Ok(())
        }
        Status::Blocked => {
            println!("{}", outcome.envelope.message);
            if let Some(why) = &outcome.envelope.why {
                println!("{why}");
            }
            print_next_actions(&outcome.envelope);
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

async fn cmd_remove(
    config_path: Option<String>,
    project_path: &Path,
    yes: bool,
    json: bool,
) -> Result<()> {
    let request = RemoveRequest {
        project_path: project_path.to_path_buf(),
    };

    let check = remove_project_config(request.clone(), false);

    match check.envelope.code.as_str() {
        "already_removed" => {
            if json {
                println!("{}", check.envelope.render());
            } else {
                println!("nothing to remove");
            }
            return Ok(());
        }
        "remove_pending" => {}
        _ => {
            if json {
                println!("{}", check.envelope.render());
                return Ok(());
            }
            println!("{}", check.envelope.message);
            if let Some(why) = &check.envelope.why {
                println!("{why}");
            }
            return Ok(());
        }
    }

    let scope_root = envelope_data_str(&check.envelope, "scope_root")
        .unwrap_or_else(|| project_path.display().to_string());

    if !yes {
        eprint!("Delete .daemon8/ from {scope_root}? This removes the project config. [y/N] ");
        io::stderr().flush()?;
        let confirmed = if io::stdin().is_terminal() {
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            input.trim().eq_ignore_ascii_case("y")
        } else {
            false
        };
        if !confirmed {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    let outcome = remove_project_config(request, true);

    if outcome.envelope.status == Status::Success
        && outcome.envelope.code == "removed"
        && let Err(err) = cleanup_removed_scope(config_path.as_deref(), &scope_root).await
    {
        tracing::warn!(error = %err, "scope ledger cleanup failed");
    }

    if json {
        println!("{}", outcome.envelope.render());
    } else {
        println!("{}", outcome.envelope.message);
    }
    Ok(())
}

async fn cleanup_removed_scope(config_path: Option<&str>, scope_root: &str) -> Result<()> {
    let cfg = config::load(config_path)?;
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    if !db_path.exists() {
        return Ok(());
    }
    let store = SurrealStore::open(&db_path).await?;
    let ledger = store.scope_ledger_store();
    let removed = ledger.remove_scope_records(scope_root).await?;
    if removed > 0 {
        tracing::debug!(removed, scope_root, "cleaned scope ledger records");
    }
    Ok(())
}

fn print_next_actions(envelope: &Envelope) {
    for action in &envelope.next_actions {
        println!("next: {} ({})", action.tool, action.reason);
    }
}

async fn record_init_outcome(config_path: Option<&str>, envelope: &Envelope) -> Result<()> {
    if envelope.status != Status::Success {
        return Ok(());
    }
    let Some(scope_root) = envelope_data_str(envelope, "scope_root") else {
        return Ok(());
    };

    let cfg = config::load(config_path)?;
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

fn envelope_data_str(envelope: &Envelope, key: &str) -> Option<String> {
    envelope
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn envelope_data_u64(envelope: &Envelope, key: &str) -> Option<u64> {
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
