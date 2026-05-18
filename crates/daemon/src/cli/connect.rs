// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 connect` -- classify an explicit alpha scope and report the next step.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use daemon8_core::control::{
    AlphaEnvelope, AlphaStatus, ConnectOutcome, ConnectRequest, ScopeMode, SessionConnection,
    connect, normalize_provider_for_connect, resolve_connect_transcript,
};
use daemon8_ingest::source_sync::{
    ActiveTranscriptSource, ConfiguredSourceTrigger, SourceSyncReport, SourceTrigger,
    SourceTriggerRequest,
};
use daemon8_providers::dirs_home;
use daemon8_store::{
    ObservationHashCache, ScopeConnectFailureRecord, ScopeLedgerStore, ScopeSessionRecord,
    StateModel, SurrealStore,
};
use serde_json::json;
use tokio::sync::broadcast;

use crate::cli::serve::{ObservationWriteService, StoreWriterCtx};
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

    /// Emit the common alpha JSON envelope.
    #[arg(long)]
    pub json: bool,
}

pub async fn cmd_connect(config_path: Option<String>, args: ConnectArgs) -> Result<()> {
    let provider_input = args.provider;
    let project_path = args.path;
    let agent_name = args.agent_name;
    let transcript_path = args.transcript_path;
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
    let mut outcome = resolve_connect_transcript(outcome, transcript_path.as_deref(), &dirs_home());

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

    if outcome.envelope.status == AlphaStatus::Success
        && let Some(store) = connect_store.as_ref()
    {
        match trigger_project_sources(store, &outcome.connection).await {
            Ok(Some(report)) => {
                let mut data = outcome.envelope.data.take().unwrap_or_else(|| json!({}));
                data["triggered_ingestion"] = source_report_value(&report);
                outcome.envelope.data = Some(data);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(error = %err, "configured source trigger failed after connect");
            }
        }
    }

    if args.json {
        println!("{}", outcome.envelope.render());
        return Ok(());
    }

    match outcome.envelope.status {
        AlphaStatus::Success
        | AlphaStatus::SetupRequired
        | AlphaStatus::ConnectRequired
        | AlphaStatus::Blocked => {
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

async fn trigger_project_sources(
    surreal_store: &Arc<SurrealStore>,
    connection: &Option<SessionConnection>,
) -> Result<Option<SourceSyncReport>> {
    let Some(connection) = connection else {
        return Ok(None);
    };
    if connection.mode != ScopeMode::Project {
        return Ok(None);
    }
    let Some(scope_root) = connection.scope_root.as_ref() else {
        return Ok(None);
    };

    let store: Arc<dyn StateModel> = surreal_store.clone();
    let (broadcast_tx, _broadcast_rx) = broadcast::channel(1);
    let writer = Arc::new(ObservationWriteService::new(Arc::new(StoreWriterCtx {
        store,
        memory_store: None,
        debug_session_store: None,
        broadcast_tx,
        node_id: Arc::from("cli-connect"),
        hash_cache: ObservationHashCache::new(),
    })));
    let trigger = ConfiguredSourceTrigger::new(Arc::new(surreal_store.cursor_store()), writer);
    let active_transcript =
        connection
            .transcript_path
            .as_ref()
            .map(|path| ActiveTranscriptSource {
                provider: connection.provider.clone(),
                path: PathBuf::from(path),
            });

    Ok(Some(
        trigger
            .trigger_sources(SourceTriggerRequest {
                scope_root: PathBuf::from(scope_root),
                active_transcript,
            })
            .await,
    ))
}

fn source_report_value(report: &SourceSyncReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or_else(|_| json!({}))
}

fn print_envelope_guidance(envelope: &AlphaEnvelope) {
    println!("{}", envelope.message);
    if let Some(why) = &envelope.why {
        println!("{why}");
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
    envelope: &AlphaEnvelope,
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
    envelope: &AlphaEnvelope,
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
        status: alpha_status_str(envelope.status).into(),
        code: envelope.code.clone(),
        message: envelope.message.clone(),
        why: envelope.why.clone(),
        attempt_count: 1,
        first_seen_at: now,
        last_seen_at: now,
    }
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

fn alpha_status_str(status: AlphaStatus) -> &'static str {
    match status {
        AlphaStatus::Success => "success",
        AlphaStatus::Error => "error",
        AlphaStatus::ConnectRequired => "connect_required",
        AlphaStatus::SetupRequired => "setup_required",
        AlphaStatus::Blocked => "blocked",
    }
}

fn current_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default()
}
