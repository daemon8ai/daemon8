// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 connect` -- classify a project scope and report the next step.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use daemon8_core::control::{
    ConnectOutcome, ConnectRequest, Envelope, ScopeMode, SessionConnection, Status, connect,
    normalize_provider_for_connect, resolve_connect_transcript,
};
use daemon8_providers::{conversation_since_ms, dirs_home};
use daemon8_store::{
    ScopeConnectFailureRecord, ScopeLedgerStore, ScopeSessionRecord, SurrealStore,
};

use crate::config;

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

    /// Optional provider transcript path for runtime conversation binding.
    #[arg(long)]
    pub transcript_path: Option<PathBuf>,

    /// Optional conversation discovery lookback window in hours. Defaults to 24.
    #[arg(long)]
    pub conversation_lookback_hours: Option<u64>,

    /// Emit the common JSON envelope.
    #[arg(long)]
    pub json: bool,
}

pub async fn cmd_connect(config_path: Option<String>, args: ConnectArgs) -> Result<()> {
    let provider_input = args.provider;
    let project_path = args.path;
    let agent_name = args.agent_name;
    let transcript_path = args.transcript_path;
    let conversation_lookback_hours = args.conversation_lookback_hours;
    let session_id = next_cli_session_id();
    let requested_path = project_path.display().to_string();
    let transcript_path_display = transcript_path
        .as_ref()
        .map(|path| path.display().to_string());
    let provider =
        match normalize_provider_for_connect(&session_id, &provider_input, &requested_path) {
            Ok(provider) => provider,
            Err(envelope) => {
                let envelope = *envelope;
                let outcome = ConnectOutcome {
                    envelope,
                    connection: None,
                };
                if let Err(err) = record_connect_outcome(
                    config_path.as_deref(),
                    &session_id,
                    &provider_input,
                    &requested_path,
                    agent_name.as_deref(),
                    transcript_path_display.as_deref(),
                    &outcome,
                )
                .await
                {
                    tracing::warn!(error = %err, "scope ledger connect recording failed");
                }
                if args.json {
                    println!("{}", outcome.envelope.render());
                    return Ok(());
                }
                bail!(
                    "{}: {}",
                    outcome.envelope.code,
                    outcome
                        .envelope
                        .why
                        .as_deref()
                        .unwrap_or(&outcome.envelope.message)
                );
            }
        };
    let outcome = connect(ConnectRequest {
        session_id: session_id.clone(),
        provider: provider.clone(),
        project_path: project_path.clone(),
        agent_name: agent_name.clone(),
        transcript_path: transcript_path.clone(),
    });
    let home = dirs_home();
    let conversation_since_ms = conversation_since_ms(conversation_lookback_hours);
    let outcome = resolve_connect_transcript(
        outcome,
        transcript_path.as_deref(),
        &home,
        conversation_since_ms,
    );

    let connect_store = match open_connect_store(config_path.as_deref()).await {
        Ok(store) => Some(store),
        Err(err) => {
            tracing::warn!(error = %err, "scope ledger store unavailable");
            None
        }
    };
    if let Some(store) = connect_store.as_ref()
        && let Err(err) = record_connect_outcome_with_store(
            store,
            &session_id,
            &provider,
            &requested_path,
            agent_name.as_deref(),
            transcript_path_display.as_deref(),
            &outcome,
        )
        .await
    {
        tracing::warn!(error = %err, "scope ledger connect recording failed");
    }

    if args.json {
        println!("{}", outcome.envelope.render());
        return Ok(());
    }

    match outcome.envelope.status {
        Status::Success | Status::SetupRequired | Status::ConnectRequired | Status::Blocked => {
            print_envelope_guidance(&outcome.envelope);
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

fn print_envelope_guidance(envelope: &Envelope) {
    println!("{}", envelope.message);
    if let Some(why) = &envelope.why {
        println!("{why}");
    }
    for requirement in &envelope.requirements {
        println!("{requirement}");
    }
    for action in &envelope.next_actions {
        println!("next: {} ({})", action.tool, action.reason);
    }
}

fn next_cli_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("cli-{nanos}")
}

async fn record_connect_outcome(
    config_path: Option<&str>,
    session_id: &str,
    provider: &str,
    requested_path: &str,
    agent_name: Option<&str>,
    transcript_path: Option<&str>,
    outcome: &ConnectOutcome,
) -> Result<()> {
    let store = open_connect_store(config_path).await?;
    record_connect_outcome_with_store(
        &store,
        session_id,
        provider,
        requested_path,
        agent_name,
        transcript_path,
        outcome,
    )
    .await
}

async fn open_connect_store(config_path: Option<&str>) -> Result<Arc<SurrealStore>> {
    let cfg = config::load(config_path)?;
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    Ok(Arc::new(SurrealStore::open(&db_path).await?))
}

async fn record_connect_outcome_with_store(
    store: &SurrealStore,
    session_id: &str,
    provider: &str,
    requested_path: &str,
    agent_name: Option<&str>,
    transcript_path: Option<&str>,
    outcome: &ConnectOutcome,
) -> Result<()> {
    let ledger = store.scope_ledger_store();
    let now = current_ns();

    match &outcome.connection {
        Some(connection) => {
            ledger
                .record_connect_success(scope_session_record(connection, &outcome.envelope, now))
                .await?;
        }
        None => {
            ledger
                .record_connect_failure(scope_failure_record(
                    session_id,
                    provider,
                    requested_path,
                    agent_name,
                    transcript_path,
                    &outcome.envelope,
                    now,
                ))
                .await?;
        }
    }

    Ok(())
}

fn scope_session_record(
    connection: &SessionConnection,
    envelope: &Envelope,
    now: u64,
) -> ScopeSessionRecord {
    ScopeSessionRecord {
        id: None,
        session_id: connection.session_id.clone(),
        provider: connection.provider.clone(),
        agent_name: connection.agent_name.clone(),
        mode: connection.mode.as_str().into(),
        requested_path: connection.requested_path.clone(),
        scope_root: connection.scope_root.clone(),
        transcript_path: connection.transcript_path.clone(),
        project_name: envelope_data_str(envelope, "project_name"),
        source_count: envelope_data_u64(envelope, "source_count"),
        connected_at: now,
        last_seen_at: now,
    }
}

fn scope_failure_record(
    session_id: &str,
    provider: &str,
    requested_path: &str,
    agent_name: Option<&str>,
    transcript_path: Option<&str>,
    envelope: &Envelope,
    now: u64,
) -> ScopeConnectFailureRecord {
    ScopeConnectFailureRecord {
        id: None,
        session_id: session_id.into(),
        provider: provider.into(),
        agent_name: agent_name.map(Into::into),
        requested_path: requested_path.into(),
        scope_root: envelope_data_str(envelope, "scope_root"),
        transcript_path: transcript_path.map(Into::into),
        mode: envelope_data_str(envelope, "mode")
            .unwrap_or_else(|| ScopeMode::Invalid.as_str().into()),
        status: status_str(envelope.status).into(),
        code: envelope.code.clone(),
        message: envelope.message.clone(),
        why: envelope.why.clone(),
        attempt_count: 1,
        first_seen_at: now,
        last_seen_at: now,
    }
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

fn status_str(status: Status) -> &'static str {
    match status {
        Status::Success => "success",
        Status::Error => "error",
        Status::ConnectRequired => "connect_required",
        Status::SetupRequired => "setup_required",
        Status::Blocked => "blocked",
    }
}

fn current_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}
