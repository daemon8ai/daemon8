// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 deliber8` -- CLI surface for the deliber8 specialist runtime.
//!
//! See `crate::deliber8` for the runtime-loop library function.
//!
//! Concurrency note: every subcommand here opens the on-disk SurrealKV store
//! directly. Because SurrealKV holds an exclusive lock per process, these
//! commands cannot be run concurrently with `daemon8 serve` (or with each
//! other) against the same store path. The MVP-06 model is "operator picks
//! one mode at a time"; HTTP-bridged hosting is a follow-on (likely MVP-12).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use daemon8_store::{AgentCardFilter, CardStore, EnvelopeFilter, EnvelopeStore, SurrealStore};
use daemon8_types::{AgentCard, AgentKind, AgentStatus, EnvelopeStatus};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::deliber8::{
    DEFAULT_HEARTBEAT_MS, InboxCounts, SpecialistConfig, build_stop_envelope, inbox_counts, now_ns,
    run_specialist,
};

#[derive(Subcommand)]
pub enum Deliber8Subcommand {
    /// Register a new specialist agent card
    Spawn(SpawnArgs),
    /// List specialist agents
    List(ListArgs),
    /// Inspect a specialist agent and its inbox counts
    Inspect(InspectArgs),
    /// Send a stop control envelope to a specialist's inbox
    Stop(StopArgs),
    /// Stop a specialist and reset its card to Alive (operator re-invokes `run`)
    Restart(RestartArgs),
    /// Run the specialist loop in the foreground (hidden; used by spawned processes)
    #[command(hide = true)]
    Run(RunArgs),
}

#[derive(clap::Args)]
pub struct SpawnArgs {
    /// Slug used as the AgentCard key and address suffix
    #[arg(long)]
    pub slug: String,
    /// Agent kind (specialist | steward | bookkeeper)
    #[arg(long, default_value = "specialist")]
    pub kind: String,
    /// Inbox address; defaults to "agent:<slug>"
    #[arg(long)]
    pub inbox: Option<String>,
    /// Optional human-readable display name
    #[arg(long)]
    pub display_name: Option<String>,
    /// Heartbeat interval in milliseconds (recorded on the card)
    #[arg(long, default_value_t = DEFAULT_HEARTBEAT_MS)]
    pub heartbeat_ms: u64,
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Filter by status (repeatable). Defaults to non-retired.
    #[arg(long)]
    pub status: Vec<String>,
}

#[derive(clap::Args)]
pub struct InspectArgs {
    pub slug: String,
}

#[derive(clap::Args)]
pub struct StopArgs {
    pub slug: String,
    /// Maximum seconds to wait for the specialist to acknowledge (Retired)
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,
}

#[derive(clap::Args)]
pub struct RestartArgs {
    pub slug: String,
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,
}

#[derive(clap::Args)]
pub struct RunArgs {
    #[arg(long)]
    pub slug: String,
    #[arg(long)]
    pub inbox: Option<String>,
    #[arg(long, default_value_t = DEFAULT_HEARTBEAT_MS)]
    pub heartbeat_ms: u64,
}

pub async fn cmd_deliber8(
    config_override: Option<String>,
    subcommand: Deliber8Subcommand,
) -> Result<()> {
    let cfg = config::load(config_override.as_deref()).unwrap_or_default();
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    let store = SurrealStore::open(&db_path)
        .await
        .with_context(|| format!("opening daemon8 store at {}", db_path.display()))?;
    let store = Arc::new(store);

    match subcommand {
        Deliber8Subcommand::Spawn(args) => cmd_spawn(store, args).await,
        Deliber8Subcommand::List(args) => cmd_list(store, args).await,
        Deliber8Subcommand::Inspect(args) => cmd_inspect(store, args).await,
        Deliber8Subcommand::Stop(args) => cmd_stop(store, args).await,
        Deliber8Subcommand::Restart(args) => cmd_restart(store, args).await,
        Deliber8Subcommand::Run(args) => cmd_run(store, args).await,
    }
}

async fn cmd_spawn(store: Arc<SurrealStore>, args: SpawnArgs) -> Result<()> {
    let card_store = store.card_store();
    card_store.init_schema().await.context("init card schema")?;

    if card_store.get_agent_by_slug(&args.slug).await?.is_some() {
        bail!(
            "agent '{}' already exists; use 'daemon8 deliber8 restart' to reuse",
            args.slug
        );
    }

    let kind = parse_agent_kind(&args.kind)?;
    let inbox = args
        .inbox
        .clone()
        .unwrap_or_else(|| format!("agent:{}", args.slug));
    let now = now_ns();

    let card = AgentCard {
        id: format!("agent_{}", args.slug),
        actor_ref: format!("actor:{}", args.slug),
        address: inbox.clone(),
        slug: args.slug.clone(),
        display_name: args.display_name.clone(),
        agent_kind: kind,
        status: AgentStatus::Alive,
        persona: serde_json::json!({}),
        model: serde_json::json!({}),
        capabilities: vec![],
        subjects_handled: vec![],
        project_refs: vec![],
        team_refs: vec![],
        primary_team_ref: None,
        spawned_by_actor_ref: None,
        spawned_from_cwd: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        spawned_from_project_ref: None,
        host_id: hostname_string(),
        pid: None,
        parent_pid: None,
        process_group_id: None,
        executable_path: std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        argv_hash: None,
        runtime_kind: Some("daemon8.deliber8".into()),
        runtime_version: Some(env!("CARGO_PKG_VERSION").into()),
        launch_nonce: None,
        started_at: Some(now),
        last_seen_at: None,
        heartbeat_interval_ms: Some(args.heartbeat_ms),
        stop_state: serde_json::json!({}),
        last_stop_request_at: None,
        last_exit_code: None,
        last_signal: None,
        cost_window_usd: 0.0,
        cost_total_usd: 0.0,
        budget_daily_usd: None,
        created_at: now,
        updated_at: now,
    };

    card_store
        .upsert_agent(card)
        .await
        .context("upserting agent card")?;
    println!(
        "spawned agent '{}' (kind={}, inbox={})",
        args.slug, args.kind, inbox
    );
    println!("invoke: daemon8 deliber8 run --slug {}", args.slug);
    Ok(())
}

async fn cmd_list(store: Arc<SurrealStore>, args: ListArgs) -> Result<()> {
    let card_store = store.card_store();
    card_store.init_schema().await.context("init card schema")?;

    let statuses = if args.status.is_empty() {
        // Default: everything except Retired.
        Some(vec![
            AgentStatus::Created,
            AgentStatus::Starting,
            AgentStatus::Alive,
            AgentStatus::Busy,
            AgentStatus::Paused,
            AgentStatus::Degraded,
            AgentStatus::Stopping,
            AgentStatus::Failed,
        ])
    } else {
        let mut acc = Vec::with_capacity(args.status.len());
        for s in &args.status {
            acc.push(s.parse::<AgentStatus>().map_err(|e| anyhow::anyhow!(e))?);
        }
        Some(acc)
    };

    let filter = AgentCardFilter {
        statuses,
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let agents = card_store.list_agents(&filter).await?;

    if agents.is_empty() {
        println!("no agents matching filter");
        return Ok(());
    }

    println!(
        "{:<20} {:<12} {:<12} {:<28} INBOX",
        "SLUG", "KIND", "STATUS", "LAST_SEEN_NS"
    );
    for a in agents {
        println!(
            "{:<20} {:<12} {:<12} {:<28} {}",
            a.slug,
            a.agent_kind.to_string(),
            a.status.to_string(),
            a.last_seen_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into()),
            a.address
        );
    }
    Ok(())
}

async fn cmd_inspect(store: Arc<SurrealStore>, args: InspectArgs) -> Result<()> {
    let card_store = store.card_store();
    let envelope_store = store.envelope_store();
    card_store.init_schema().await.context("init card schema")?;
    envelope_store
        .init_schema()
        .await
        .context("init envelope schema")?;

    let card = card_store
        .get_agent_by_slug(&args.slug)
        .await?
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", args.slug))?;

    let counts = inbox_counts(&envelope_store, &card.address)
        .await
        .context("counting envelopes")?;

    let now = now_ns();
    let last_seen_age_ms = card
        .last_seen_at
        .map(|t| (now.saturating_sub(t)) / 1_000_000);

    println!("slug:                {}", card.slug);
    println!("kind:                {}", card.agent_kind);
    println!("status:              {}", card.status);
    println!("address:             {}", card.address);
    println!(
        "display_name:        {}",
        card.display_name.as_deref().unwrap_or("-")
    );
    println!(
        "started_at_ns:       {}",
        card.started_at
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "last_seen_age_ms:    {}",
        last_seen_age_ms
            .map(|m| m.to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "heartbeat_ms:        {}",
        card.heartbeat_interval_ms
            .map(|m| m.to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!();
    println!("inbox counts ({}):", card.address);
    print_counts(&counts);

    Ok(())
}

fn print_counts(counts: &InboxCounts) {
    println!("  queued:    {}", counts.queued);
    println!("  delivered: {}", counts.delivered);
    println!("  read:      {}", counts.read);
    println!("  failed:    {}", counts.failed);
    println!("  cancelled: {}", counts.cancelled);
}

async fn cmd_stop(store: Arc<SurrealStore>, args: StopArgs) -> Result<()> {
    stop_inner(store, &args.slug, args.timeout_secs).await
}

async fn stop_inner(store: Arc<SurrealStore>, slug: &str, timeout_secs: u64) -> Result<()> {
    let card_store = store.card_store();
    let envelope_store = store.envelope_store();
    card_store.init_schema().await.context("init card schema")?;
    envelope_store
        .init_schema()
        .await
        .context("init envelope schema")?;

    let card = card_store
        .get_agent_by_slug(slug)
        .await?
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", slug))?;

    let stop_env = build_stop_envelope(&card.address, "operator:cli");
    envelope_store
        .enqueue_envelope(stop_env)
        .await
        .context("enqueueing stop envelope")?;

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    let card_store_arc = card_store;
    loop {
        sleep(Duration::from_millis(500)).await;
        if let Some(c) = card_store_arc.get_agent_by_slug(slug).await? {
            if c.status == AgentStatus::Retired {
                println!("specialist '{}' acknowledged stop (status=retired)", slug);
                return Ok(());
            }
        } else {
            // get_agent_by_slug filters out retired; treat absence as success
            println!("specialist '{}' acknowledged stop (no longer alive)", slug);
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            // Inspect pending stop envelopes so the operator can see whether
            // the message was even consumed.
            let pending = envelope_store
                .query_inbox(&EnvelopeFilter {
                    inbox_address: Some(card.address.clone()),
                    statuses: Some(vec![EnvelopeStatus::Queued]),
                    ..Default::default()
                })
                .await?
                .len();
            bail!(
                "timed out after {}s waiting for '{}' to retire (queued envelopes in inbox: {}). \
                 The specialist may not be running; start it with: daemon8 deliber8 run --slug {}",
                timeout_secs,
                slug,
                pending,
                slug,
            );
        }
    }
}

async fn cmd_restart(store: Arc<SurrealStore>, args: RestartArgs) -> Result<()> {
    stop_inner(store.clone(), &args.slug, args.timeout_secs).await?;

    let card_store = store.card_store();
    let card_id = format!("agent_{}", args.slug);
    card_store
        .update_agent_status(&card_id, AgentStatus::Alive, now_ns())
        .await
        .context("resetting agent status to alive")?;
    println!("agent '{}' reset to alive", args.slug);
    println!("re-invoke: daemon8 deliber8 run --slug {}", args.slug);
    Ok(())
}

async fn cmd_run(store: Arc<SurrealStore>, args: RunArgs) -> Result<()> {
    let card_store = store.card_store();
    let envelope_store = store.envelope_store();
    card_store.init_schema().await.context("init card schema")?;
    envelope_store
        .init_schema()
        .await
        .context("init envelope schema")?;

    let card = card_store
        .get_agent_by_slug(&args.slug)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "agent '{}' not found; spawn it first with: daemon8 deliber8 spawn --slug {}",
                args.slug,
                args.slug
            )
        })?;
    let inbox = args.inbox.clone().unwrap_or_else(|| card.address.clone());

    let cfg = SpecialistConfig::new(args.slug.clone(), inbox).heartbeat_interval(args.heartbeat_ms);

    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_cancel.cancel();
    });

    let outcome = run_specialist(store, cfg, cancel).await?;
    println!(
        "specialist '{}' exited (processed={}, responded={}, heartbeats={}, stopped_by_control={}, cancelled={})",
        args.slug,
        outcome.processed,
        outcome.responded,
        outcome.heartbeats,
        outcome.stopped_by_control,
        outcome.cancelled,
    );
    Ok(())
}

fn parse_agent_kind(s: &str) -> Result<AgentKind> {
    s.parse::<AgentKind>().map_err(|e| anyhow::anyhow!(e))
}

fn hostname_string() -> Option<String> {
    // Cheap best-effort hostname capture without adding a dep. Falls back to
    // None if the env var isn't set.
    std::env::var("HOST")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kinds() {
        assert!(matches!(
            parse_agent_kind("specialist").unwrap(),
            AgentKind::Specialist
        ));
        assert!(matches!(
            parse_agent_kind("steward").unwrap(),
            AgentKind::Steward
        ));
        assert!(matches!(
            parse_agent_kind("bookkeeper").unwrap(),
            AgentKind::Bookkeeper
        ));
        assert!(parse_agent_kind("queen").is_err());
    }
}
