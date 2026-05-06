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
use daemon8_store::{AgentCardFilter, CardStore, EnvelopeStore, SurrealStore};
use daemon8_types::{AgentCard, AgentKind, AgentStatus};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::deliber8::{
    DEFAULT_HEARTBEAT_MS, InboxCounts, SpecialistConfig, build_stop_envelope, inbox_counts, now_ns,
    run_specialist,
};

use super::observe::{ClientArgs, base_url, check_response, handle_reqwest_error};

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

    #[command(flatten)]
    pub client: ClientArgs,
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Filter by status (repeatable). Defaults to non-retired.
    #[arg(long)]
    pub status: Vec<String>,

    #[command(flatten)]
    pub client: ClientArgs,
}

#[derive(clap::Args)]
pub struct InspectArgs {
    pub slug: String,

    #[command(flatten)]
    pub client: ClientArgs,
}

#[derive(clap::Args)]
pub struct StopArgs {
    pub slug: String,
    /// Maximum seconds to wait for the specialist to acknowledge (Retired)
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,

    #[command(flatten)]
    pub client: ClientArgs,
}

#[derive(clap::Args)]
pub struct RestartArgs {
    pub slug: String,
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,

    #[command(flatten)]
    pub client: ClientArgs,
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
    match subcommand {
        Deliber8Subcommand::Spawn(args) => cmd_spawn(config_override, args).await,
        Deliber8Subcommand::List(args) => cmd_list(config_override, args).await,
        Deliber8Subcommand::Inspect(args) => cmd_inspect(config_override, args).await,
        Deliber8Subcommand::Stop(args) => cmd_stop(config_override, args).await,
        Deliber8Subcommand::Restart(args) => cmd_restart(config_override, args).await,
        Deliber8Subcommand::Run(args) => {
            let cfg = config::load(config_override.as_deref()).unwrap_or_default();
            let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
            let store = SurrealStore::open(&db_path)
                .await
                .with_context(|| format!("opening daemon8 store at {}", db_path.display()))?;
            cmd_run(Arc::new(store), args).await
        }
    }
}

async fn open_store(config_override: Option<String>) -> Result<Arc<SurrealStore>> {
    let cfg = config::load(config_override.as_deref()).unwrap_or_default();
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    let store = SurrealStore::open(&db_path)
        .await
        .with_context(|| format!("opening daemon8 store at {}", db_path.display()))?;
    Ok(Arc::new(store))
}

async fn cmd_spawn(config_override: Option<String>, args: SpawnArgs) -> Result<()> {
    let port = args.client.resolved_port();
    let url = format!("{}/api/deliber8/roster", base_url(port));

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

    match reqwest::Client::new()
        .post(&url)
        .json(&card)
        .send()
        .await
    {
        Ok(resp) => {
            check_response(resp).await?;
            println!(
                "spawned agent '{}' via API (kind={}, inbox={})",
                args.slug, args.kind, inbox
            );
            println!("invoke: daemon8 deliber8 run --slug {}", args.slug);
            Ok(())
        }
        Err(e) if e.is_connect() => {
            let store = open_store(config_override).await?;
            let card_store = store.card_store();

            if card_store.get_agent_by_slug(&args.slug).await?.is_some() {
                bail!(
                    "agent '{}' already exists; use 'daemon8 deliber8 restart' to reuse",
                    args.slug
                );
            }

            card_store
                .upsert_agent(card)
                .await
                .context("upserting agent card")?;
            println!(
                "spawned agent '{}' via store (kind={}, inbox={})",
                args.slug, args.kind, inbox
            );
            println!("invoke: daemon8 deliber8 run --slug {}", args.slug);
            Ok(())
        }
        Err(e) => Err(handle_reqwest_error(e, port)),
    }
}

async fn cmd_list(config_override: Option<String>, args: ListArgs) -> Result<()> {
    let port = args.client.resolved_port();
    let url = format!("{}/api/deliber8/roster", base_url(port));
    let mut query = Vec::new();
    if !args.status.is_empty() {
        query.push(format!("statuses={}", args.status.join(",")));
    }
    let url = if query.is_empty() {
        url
    } else {
        format!("{}?{}", url, query.join("&"))
    };

    match reqwest::get(&url).await {
        Ok(resp) => {
            let resp = check_response(resp).await?;
            if args.client.json {
                let raw: serde_json::Value = resp.json().await.unwrap_or_default();
                println!("{}", serde_json::to_string_pretty(&raw).unwrap_or_default());
                return Ok(());
            }

            #[derive(serde::Deserialize)]
            struct RosterResponse {
                agents: Vec<AgentCard>,
            }
            let data: RosterResponse = resp.json().await?;
            if data.agents.is_empty() {
                println!("no agents matching filter");
            } else {
                print_roster_table(&data.agents);
            }
            Ok(())
        }
        Err(e) if e.is_connect() => {
            let store = open_store(config_override).await?;
            let card_store = store.card_store();
            let statuses = if args.status.is_empty() {
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

            if args.client.json {
                println!("{}", serde_json::to_string_pretty(&agents)?);
            } else {
                print_roster_table(&agents);
            }
            Ok(())
        }
        Err(e) => Err(handle_reqwest_error(e, port)),
    }
}

async fn cmd_inspect(config_override: Option<String>, args: InspectArgs) -> Result<()> {
    let port = args.client.resolved_port();
    let url = format!("{}/api/deliber8/roster", base_url(port));
    
    match reqwest::get(&url).await {
        Ok(resp) => {
            let resp = check_response(resp).await?;
            #[derive(serde::Deserialize)]
            struct RosterResponse {
                agents: Vec<AgentCard>,
            }
            let data: RosterResponse = resp.json().await?;
            let card = data.agents.iter().find(|c| c.slug == args.slug).cloned();
            
            if let Some(card) = card {
                let inbox_url = format!("{}/api/deliber8/inbox/{}", base_url(port), card.address);
                let inbox_resp = reqwest::get(&inbox_url).await?;
                let inbox_resp = check_response(inbox_resp).await?;
                
                #[derive(serde::Deserialize)]
                struct InboxResponse {
                    queued: usize,
                    delivered: usize,
                    read: usize,
                    failed: usize,
                    cancelled: usize,
                }
                let counts: InboxResponse = inbox_resp.json().await?;
                let counts = InboxCounts {
                    queued: counts.queued,
                    delivered: counts.delivered,
                    read: counts.read,
                    failed: counts.failed,
                    cancelled: counts.cancelled,
                };
                
                if args.client.json {
                    println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                        "card": card,
                        "inbox": counts
                    }))?);
                } else {
                    print_inspect_report(&card, &counts);
                }
                Ok(())
            } else {
                bail!("agent '{}' not found", args.slug);
            }
        }
        Err(e) if e.is_connect() => {
            let store = open_store(config_override).await?;
            let card_store = store.card_store();
            let envelope_store = store.envelope_store();

            let card = card_store
                .get_agent_by_slug(&args.slug)
                .await?
                .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", args.slug))?;

            let counts = inbox_counts(&envelope_store, &card.address).await?;

            if args.client.json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "card": card,
                    "inbox": counts
                }))?);
            } else {
                print_inspect_report(&card, &counts);
            }
            Ok(())
        }
        Err(e) => Err(handle_reqwest_error(e, port)),
    }
}

async fn cmd_stop(config_override: Option<String>, args: StopArgs) -> Result<()> {
    stop_inner(config_override, &args.slug, args.timeout_secs, &args.client).await
}

async fn stop_inner(
    config_override: Option<String>,
    slug: &str,
    timeout_secs: u64,
    client_args: &ClientArgs,
) -> Result<()> {
    let port = client_args.resolved_port();
    let roster_url = format!("{}/api/deliber8/roster", base_url(port));

    // 1. Find agent (API first)
    let card = match reqwest::get(&roster_url).await {
        Ok(resp) => {
            if let Ok(resp) = check_response(resp).await {
                #[derive(serde::Deserialize)]
                struct RosterResponse {
                    agents: Vec<AgentCard>,
                }
                if let Ok(data) = resp.json::<RosterResponse>().await {
                    data.agents.into_iter().find(|c| c.slug == slug)
                } else {
                    None
                }
            } else {
                None
            }
        }
        Err(e) if e.is_connect() => {
            let store = open_store(config_override.clone()).await?;
            store.card_store().get_agent_by_slug(slug).await?
        }
        Err(e) => return Err(handle_reqwest_error(e, port)),
    };

    let Some(card) = card else {
        bail!("agent '{}' not found", slug);
    };

    if card.status == AgentStatus::Retired {
        println!("agent '{}' is already retired", slug);
        return Ok(());
    }

    // 2. Send stop envelope (API first)
    let stop_env = build_stop_envelope(&card.address, "operator:cli");
    let enqueue_url = format!("{}/api/deliber8/enqueue", base_url(port));
    let mut api_sent = false;

    match reqwest::Client::new()
        .post(&enqueue_url)
        .json(&stop_env)
        .send()
        .await
    {
        Ok(resp) => {
            check_response(resp).await?;
            api_sent = true;
        }
        Err(e) if e.is_connect() => {
            let store = open_store(config_override.clone()).await?;
            store.envelope_store().enqueue_envelope(stop_env).await?;
        }
        Err(e) => return Err(handle_reqwest_error(e, port)),
    }

    println!("stop signal sent to '{}'; waiting for retirement...", slug);

    // 3. Wait for retirement
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    while start.elapsed() < timeout {
        let status = if api_sent {
            match reqwest::get(&roster_url).await {
                Ok(resp) => {
                    if let Ok(resp) = check_response(resp).await {
                        #[derive(serde::Deserialize)]
                        struct RosterResponse {
                            agents: Vec<AgentCard>,
                        }
                        if let Ok(data) = resp.json::<RosterResponse>().await {
                            data.agents.iter().find(|c| c.slug == slug).map(|c| c.status)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        } else {
            let store = open_store(config_override.clone()).await?;
            store
                .card_store()
                .get_agent_by_slug(slug)
                .await?
                .map(|c| c.status)
        };

        if let Some(AgentStatus::Retired) | None = status {
            println!("agent '{}' has retired", slug);
            return Ok(());
        }
        sleep(Duration::from_millis(1000)).await;
    }

    bail!(
        "timed out after {}s waiting for '{}' to retire. \
         The specialist may not be running; start it with: daemon8 deliber8 run --slug {}",
        timeout_secs,
        slug,
        slug
    );
}

async fn cmd_restart(config_override: Option<String>, args: RestartArgs) -> Result<()> {
    stop_inner(
        config_override.clone(),
        &args.slug,
        args.timeout_secs,
        &args.client,
    )
    .await?;

    // Resetting status is a write op; attempt API first
    let port = args.client.resolved_port();
    let roster_url = format!("{}/api/deliber8/roster", base_url(port));
    let mut api_reset = false;

    // We don't have a direct "update status" API yet, but we can re-spawn the card
    // which uses UPSERT logic.
    let store = open_store(config_override.clone()).await?;
    let card = store
        .card_store()
        .get_agent_by_slug(&args.slug)
        .await?
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", args.slug))?;

    let mut new_card = card.clone();
    new_card.status = AgentStatus::Alive;
    new_card.updated_at = now_ns();

    match reqwest::Client::new()
        .post(&roster_url)
        .json(&new_card)
        .send()
        .await
    {
        Ok(resp) => {
            check_response(resp).await?;
            api_reset = true;
        }
        Err(e) if e.is_connect() => {
            store
                .card_store()
                .update_agent_status(&card.id, AgentStatus::Alive, now_ns())
                .await?;
        }
        Err(e) => return Err(handle_reqwest_error(e, port)),
    }

    println!(
        "agent '{}' reset to alive ({})",
        args.slug,
        if api_reset { "via API" } else { "via store" }
    );
    println!("re-invoke: daemon8 deliber8 run --slug {}", args.slug);
    Ok(())
}

async fn cmd_run(store: Arc<SurrealStore>, args: RunArgs) -> Result<()> {
    let card_store = store.card_store();
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

fn print_roster_table(agents: &[AgentCard]) {
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
}

fn print_inspect_report(card: &AgentCard, counts: &InboxCounts) {
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
    println!("  queued:    {}", counts.queued);
    println!("  delivered: {}", counts.delivered);
    println!("  read:      {}", counts.read);
    println!("  failed:    {}", counts.failed);
    println!("  cancelled: {}", counts.cancelled);
}

fn parse_agent_kind(s: &str) -> Result<AgentKind> {
    s.parse::<AgentKind>().map_err(|e| anyhow::anyhow!(e))
}

fn hostname_string() -> Option<String> {
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
