// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Deliber8 specialist runtime loop.
//!
//! A specialist is an addressable agent that polls its inbox for envelopes,
//! produces stub responses (real LLM work is deferred to a later MVP), writes
//! heartbeats, and exits cleanly when it receives a control envelope with
//! body `"stop"` or when the host process is signalled.
//!
//! Architectural constraint: SurrealKV is exclusive — a single process can
//! hold the on-disk store at a time. The MVP-06 model is therefore "operator
//! invokes `daemon8 deliber8 run --slug X` directly; it is not invoked
//! concurrently with `daemon8 serve`". Future work adds HTTP envelope
//! routes so `serve` can host specialists as tokio tasks; until then the
//! runtime is library-shaped to permit both shapes once that lands.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use daemon8_store::{
    CardStore, EnvelopeFilter, EnvelopeStore, StoreError, SurrealCardStore, SurrealEnvelopeStore,
    SurrealStore,
};
use daemon8_types::{AgentStatus, EnvelopeKind, EnvelopePriority, EnvelopeRecord, EnvelopeStatus};
use tokio::select;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_HEARTBEAT_MS: u64 = 5_000;
pub const STOP_BODY: &str = "stop";
const POLL_BATCH: usize = 32;

/// Configuration for a specialist run.
#[derive(Debug, Clone)]
pub struct SpecialistConfig {
    pub slug: String,
    pub inbox_address: String,
    pub heartbeat_interval_ms: u64,
}

impl SpecialistConfig {
    pub fn new(slug: impl Into<String>, inbox_address: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            inbox_address: inbox_address.into(),
            heartbeat_interval_ms: DEFAULT_HEARTBEAT_MS,
        }
    }

    pub fn heartbeat_interval(mut self, ms: u64) -> Self {
        self.heartbeat_interval_ms = ms.max(100);
        self
    }
}

/// Outcome of a single specialist run, useful for tests and logging.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpecialistOutcome {
    pub processed: u64,
    pub responded: u64,
    pub heartbeats: u64,
    pub stopped_by_control: bool,
    pub cancelled: bool,
}

/// Run the specialist loop against the given store. Returns when:
/// - a Control envelope with body "stop" is processed, or
/// - the cancellation token is cancelled (typically from a signal handler).
///
/// The agent identified by `cfg.slug` must already exist in the card registry
/// (typically via `daemon8 deliber8 spawn`). On clean control-stop the agent
/// status transitions to `Retired`; on cancellation it stays in `Alive` so a
/// subsequent run can resume.
pub async fn run_specialist(
    store: Arc<SurrealStore>,
    cfg: SpecialistConfig,
    cancel: CancellationToken,
) -> Result<SpecialistOutcome> {
    let envelope_store = store.envelope_store();
    let card_store = store.card_store();

    let card = card_store
        .get_agent_by_slug(&cfg.slug)
        .await
        .with_context(|| format!("looking up agent card '{}'", cfg.slug))?
        .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in card registry", cfg.slug))?;
    let agent_id = card.id.clone();

    card_store
        .update_agent_status(&agent_id, AgentStatus::Alive, now_ns())
        .await
        .context("marking agent alive at run start")?;

    // First heartbeat at start so external observers see liveness immediately
    // rather than waiting one full interval.
    if let Err(e) = card_store.record_agent_heartbeat(&agent_id, now_ns()).await {
        tracing::warn!(error = %e, slug = %cfg.slug, "initial heartbeat failed");
    }

    let mut outcome = SpecialistOutcome::default();
    let poll_interval = Duration::from_millis(cfg.heartbeat_interval_ms / 2);

    loop {
        let pending = envelope_store
            .list_pending(&cfg.inbox_address, Some(now_ns()), Some(POLL_BATCH))
            .await
            .with_context(|| format!("listing pending envelopes for {}", cfg.inbox_address))?;

        for env in pending {
            outcome.processed += 1;

            if env.kind == EnvelopeKind::Control && is_stop_envelope(&env) {
                handle_stop(&envelope_store, &card_store, &agent_id, &env).await?;
                outcome.stopped_by_control = true;
                return Ok(outcome);
            }

            // Mark delivered before producing the response so observers see the
            // delivery boundary even if response generation is slow.
            if let Err(e) = envelope_store.mark_delivered(&env.id, now_ns()).await {
                tracing::warn!(error = %e, env_id = %env.id, "mark_delivered failed");
                continue;
            }

            if env.kind == EnvelopeKind::Request {
                let response = build_stub_response(&cfg, &env);
                match envelope_store.enqueue_envelope(response).await {
                    Ok(_) => outcome.responded += 1,
                    Err(e) => {
                        tracing::warn!(error = %e, request_id = %env.id, "response enqueue failed");
                        if let Err(fe) = envelope_store
                            .mark_failed(
                                &env.id,
                                &format!("response enqueue failed: {e}"),
                                now_ns(),
                            )
                            .await
                        {
                            tracing::warn!(error = %fe, env_id = %env.id, "mark_failed after enqueue failure also failed");
                        }
                        continue;
                    }
                }
            }

            if let Err(e) = envelope_store.mark_read(&env.id, now_ns()).await {
                tracing::warn!(error = %e, env_id = %env.id, "mark_read failed");
            }
        }

        if let Err(e) = card_store.record_agent_heartbeat(&agent_id, now_ns()).await {
            tracing::warn!(error = %e, slug = %cfg.slug, "heartbeat write failed");
        } else {
            outcome.heartbeats += 1;
        }

        select! {
            _ = cancel.cancelled() => {
                outcome.cancelled = true;
                return Ok(outcome);
            }
            _ = sleep(poll_interval) => {}
        }
    }
}

fn is_stop_envelope(env: &EnvelopeRecord) -> bool {
    env.body.as_deref().map(str::trim) == Some(STOP_BODY)
}

async fn handle_stop(
    envelope_store: &SurrealEnvelopeStore,
    card_store: &SurrealCardStore,
    agent_id: &str,
    env: &EnvelopeRecord,
) -> Result<(), StoreError> {
    envelope_store.mark_delivered(&env.id, now_ns()).await.ok();
    envelope_store.mark_read(&env.id, now_ns()).await.ok();
    card_store
        .update_agent_status(agent_id, AgentStatus::Retired, now_ns())
        .await?;
    Ok(())
}

fn build_stub_response(cfg: &SpecialistConfig, request: &EnvelopeRecord) -> EnvelopeRecord {
    let now = now_ns();
    let reply_inbox = if request.from_address.is_empty() {
        format!("agent:{}-replies", cfg.slug)
    } else {
        request.from_address.clone()
    };
    EnvelopeRecord {
        id: String::new(),
        kind: EnvelopeKind::Response,
        status: EnvelopeStatus::Queued,
        priority: EnvelopePriority::Normal,
        from_address: cfg.inbox_address.clone(),
        to_address: reply_inbox.clone(),
        inbox_address: reply_inbox,
        subject: request
            .subject
            .as_ref()
            .map(|s| format!("re: {s}"))
            .or_else(|| Some(format!("re: {}", request.id))),
        body: Some(format!(
            "stub response from {} for request {} at {} ns",
            cfg.slug, request.id, now
        )),
        payload: None,
        correlation_id: request.correlation_id.clone(),
        thread_id: request
            .thread_id
            .clone()
            .or_else(|| Some(request.id.clone())),
        reply_to: Some(request.id.clone()),
        created_at: now,
        updated_at: now,
        deliver_after: None,
        delivered_at: None,
        read_at: None,
        expires_at: None,
        failed_at: None,
        failure_reason: None,
        tags: vec!["deliber8.stub".into()],
        project_refs: request.project_refs.clone(),
        team_refs: request.team_refs.clone(),
    }
}

pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Build a Control envelope with body "stop" addressed to the given inbox.
/// Used by `daemon8 deliber8 stop` and by tests.
pub fn build_stop_envelope(inbox_address: &str, from: &str) -> EnvelopeRecord {
    let now = now_ns();
    EnvelopeRecord {
        id: String::new(),
        kind: EnvelopeKind::Control,
        status: EnvelopeStatus::Queued,
        priority: EnvelopePriority::Urgent,
        from_address: from.to_string(),
        to_address: inbox_address.to_string(),
        inbox_address: inbox_address.to_string(),
        subject: Some("stop".into()),
        body: Some(STOP_BODY.into()),
        payload: None,
        correlation_id: None,
        thread_id: None,
        reply_to: None,
        created_at: now,
        updated_at: now,
        deliver_after: None,
        delivered_at: None,
        read_at: None,
        expires_at: None,
        failed_at: None,
        failure_reason: None,
        tags: vec!["deliber8.control".into()],
        project_refs: vec![],
        team_refs: vec![],
    }
}

/// Inbox counts surfaced by `daemon8 deliber8 inspect`.
#[derive(Debug, Default, Clone, Copy)]
pub struct InboxCounts {
    pub queued: usize,
    pub delivered: usize,
    pub read: usize,
    pub failed: usize,
    pub cancelled: usize,
}

pub async fn inbox_counts(
    envelope_store: &SurrealEnvelopeStore,
    inbox_address: &str,
) -> Result<InboxCounts, StoreError> {
    let mut counts = InboxCounts::default();
    for (status, slot) in [
        (EnvelopeStatus::Queued, &mut counts.queued),
        (EnvelopeStatus::Delivered, &mut counts.delivered),
        (EnvelopeStatus::Read, &mut counts.read),
        (EnvelopeStatus::Failed, &mut counts.failed),
        (EnvelopeStatus::Cancelled, &mut counts.cancelled),
    ] {
        let filter = EnvelopeFilter {
            inbox_address: Some(inbox_address.to_string()),
            statuses: Some(vec![status]),
            ..Default::default()
        };
        *slot = envelope_store.query_inbox(&filter).await?.len();
    }
    Ok(counts)
}

/// Stuck-agent classification used by `daemon8 doctor`.
///
/// Returns the AgentCard ids whose `last_seen_at` is older than
/// `multiplier * heartbeat_interval_ms` (default multiplier 3 = three
/// missed beats) and whose status is `Alive`. Cards with no
/// `last_seen_at` are considered stuck after the same window measured from
/// `started_at`, falling through to "stuck" if neither is set.
pub fn classify_stuck(
    cards: &[daemon8_types::AgentCard],
    now_ns: u64,
    multiplier: u64,
) -> Vec<String> {
    let mut stuck = Vec::new();
    for card in cards {
        if card.status != AgentStatus::Alive {
            continue;
        }
        let interval_ms = card.heartbeat_interval_ms.unwrap_or(DEFAULT_HEARTBEAT_MS);
        let threshold_ns = interval_ms
            .saturating_mul(multiplier)
            .saturating_mul(1_000_000);
        let reference = card.last_seen_at.or(card.started_at);
        let is_stuck = match reference {
            Some(t) => now_ns.saturating_sub(t) > threshold_ns,
            None => true,
        };
        if is_stuck {
            stuck.push(card.slug.clone());
        }
    }
    stuck
}

/// Identify alive specialists whose oldest queued envelope predates their
/// heartbeat cadence by more than `multiplier`× — a signal that the
/// consumer side is alive but not draining.
///
/// Each input pair is `(card, oldest_queued_created_at_ns)`. A `None`
/// second element means "no queued envelopes" and is never flagged.
/// Returns `(slug, age_ms)` tuples for each backed-up specialist.
pub fn classify_inbox_backlog(
    cards: &[(daemon8_types::AgentCard, Option<u64>)],
    now_ns: u64,
    multiplier: u64,
) -> Vec<(String, u64)> {
    let mut backlog = Vec::new();
    for (card, oldest) in cards {
        if card.status != AgentStatus::Alive {
            continue;
        }
        let Some(oldest_ns) = *oldest else {
            continue;
        };
        let interval_ms = card.heartbeat_interval_ms.unwrap_or(DEFAULT_HEARTBEAT_MS);
        let threshold_ns = interval_ms
            .saturating_mul(multiplier)
            .saturating_mul(1_000_000);
        let age_ns = now_ns.saturating_sub(oldest_ns);
        if age_ns > threshold_ns {
            backlog.push((card.slug.clone(), age_ns / 1_000_000));
        }
    }
    backlog
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_types::{ActorKind, AgentCard, AgentKind};

    fn agent(slug: &str, status: AgentStatus, last_seen_at: Option<u64>) -> AgentCard {
        AgentCard {
            id: format!("agent_{slug}"),
            actor_ref: format!("actor:{slug}"),
            address: format!("agent:{slug}"),
            slug: slug.to_string(),
            display_name: None,
            agent_kind: AgentKind::Specialist,
            status,
            persona: serde_json::json!({}),
            model: serde_json::json!({}),
            capabilities: vec![],
            subjects_handled: vec![],
            project_refs: vec![],
            team_refs: vec![],
            primary_team_ref: None,
            spawned_by_actor_ref: None,
            spawned_from_cwd: None,
            spawned_from_project_ref: None,
            host_id: None,
            pid: None,
            parent_pid: None,
            process_group_id: None,
            executable_path: None,
            argv_hash: None,
            runtime_kind: None,
            runtime_version: None,
            launch_nonce: None,
            started_at: Some(1_000),
            last_seen_at,
            heartbeat_interval_ms: Some(1_000),
            stop_state: serde_json::json!({}),
            last_stop_request_at: None,
            last_exit_code: None,
            last_signal: None,
            cost_window_usd: 0.0,
            cost_total_usd: 0.0,
            budget_daily_usd: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    // Suppress unused-import warning for ActorKind which is used elsewhere
    // when AgentCard tests grow.
    #[test]
    fn _actor_kind_link() {
        let _ = ActorKind::Agent;
    }

    #[test]
    fn stuck_classification_skips_non_alive() {
        let cards = vec![agent("retired", AgentStatus::Retired, None)];
        assert!(classify_stuck(&cards, 1_000_000_000_000, 3).is_empty());
    }

    #[test]
    fn stuck_classification_flags_no_heartbeat() {
        let cards = vec![agent("never", AgentStatus::Alive, None)];
        // last_seen_at is None, started_at = Some(1_000) ns. now = 1s. threshold
        // = 3 * 1000 ms * 1_000_000 ns/ms = 3_000_000_000 ns. Elapsed since
        // started_at = 1_000_000_000 - 1_000 ~= 1s, less than 3s, so NOT stuck.
        assert!(classify_stuck(&cards, 1_000_000_000, 3).is_empty());
        // Push past the threshold.
        assert_eq!(
            classify_stuck(&cards, 5_000_000_000, 3),
            vec!["never".to_string()]
        );
    }

    #[test]
    fn stuck_classification_flags_stale_heartbeat() {
        let cards = vec![
            agent("fresh", AgentStatus::Alive, Some(1_000_000_000_000)),
            agent("stale", AgentStatus::Alive, Some(1_000_000_000)),
        ];
        let now = 1_000_000_000_000_u64;
        let stuck = classify_stuck(&cards, now, 3);
        assert_eq!(stuck, vec!["stale".to_string()]);
    }

    #[test]
    fn inbox_backlog_skips_non_alive() {
        // Retired card with old queued envelope should not flag.
        let pairs = vec![(
            agent("retired", AgentStatus::Retired, Some(1_000_000_000_000)),
            Some(0_u64),
        )];
        assert!(classify_inbox_backlog(&pairs, 1_000_000_000_000, 10).is_empty());
    }

    #[test]
    fn inbox_backlog_skips_empty_inbox() {
        // Alive card with no queued envelopes (None) is never flagged.
        let pairs = vec![(agent("idle", AgentStatus::Alive, Some(0)), None)];
        assert!(classify_inbox_backlog(&pairs, 1_000_000_000_000, 10).is_empty());
    }

    #[test]
    fn inbox_backlog_passes_below_threshold() {
        // heartbeat = 1_000ms, multiplier = 10 => threshold = 10s = 10_000_000_000 ns.
        // Oldest queued at age 9s => below threshold, not flagged.
        let now = 100_000_000_000_u64;
        let pairs = vec![(
            agent("draining", AgentStatus::Alive, Some(now)),
            Some(now - 9_000_000_000),
        )];
        assert!(classify_inbox_backlog(&pairs, now, 10).is_empty());
    }

    #[test]
    fn inbox_backlog_flags_above_threshold() {
        // Same setup, oldest queued at age 11s => above 10s threshold.
        let now = 100_000_000_000_u64;
        let pairs = vec![(
            agent("backed-up", AgentStatus::Alive, Some(now)),
            Some(now - 11_000_000_000),
        )];
        let backlog = classify_inbox_backlog(&pairs, now, 10);
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].0, "backed-up");
        assert_eq!(backlog[0].1, 11_000); // 11_000 ms.
    }

    #[test]
    fn inbox_backlog_boundary_exactly_at_threshold_passes() {
        // threshold = 10_000ms in ns. age_ns == threshold_ns is NOT flagged
        // (strict greater-than). One ns over the line is flagged.
        let now = 100_000_000_000_u64;
        let threshold_ns: u64 = 10_u64 * 1_000 * 1_000_000;

        let exact = vec![(
            agent("exact", AgentStatus::Alive, Some(now)),
            Some(now - threshold_ns),
        )];
        assert!(classify_inbox_backlog(&exact, now, 10).is_empty());

        let over = vec![(
            agent("over", AgentStatus::Alive, Some(now)),
            Some(now - threshold_ns - 1),
        )];
        assert_eq!(classify_inbox_backlog(&over, now, 10).len(), 1);
    }

    #[test]
    fn stop_envelope_round_trip_via_is_stop() {
        let env = build_stop_envelope("agent:foo", "operator");
        assert_eq!(env.kind, EnvelopeKind::Control);
        assert_eq!(env.priority, EnvelopePriority::Urgent);
        assert!(is_stop_envelope(&env));
    }

    #[test]
    fn build_stub_response_carries_correlation_and_reply_to() {
        let cfg = SpecialistConfig::new("specialist-a", "agent:specialist-a");
        let request = EnvelopeRecord {
            id: "env_request_1".into(),
            kind: EnvelopeKind::Request,
            status: EnvelopeStatus::Queued,
            priority: EnvelopePriority::Normal,
            from_address: "agent:supervisor".into(),
            to_address: "agent:specialist-a".into(),
            inbox_address: "agent:specialist-a".into(),
            subject: Some("inspect".into()),
            body: Some("walk repo".into()),
            payload: None,
            correlation_id: Some("corr-9".into()),
            thread_id: None,
            reply_to: None,
            created_at: 100,
            updated_at: 100,
            deliver_after: None,
            delivered_at: None,
            read_at: None,
            expires_at: None,
            failed_at: None,
            failure_reason: None,
            tags: vec![],
            project_refs: vec!["proj:p1".into()],
            team_refs: vec![],
        };
        let response = build_stub_response(&cfg, &request);
        assert_eq!(response.kind, EnvelopeKind::Response);
        assert_eq!(response.from_address, "agent:specialist-a");
        assert_eq!(response.to_address, "agent:supervisor");
        assert_eq!(response.inbox_address, "agent:supervisor");
        assert_eq!(response.correlation_id.as_deref(), Some("corr-9"));
        assert_eq!(response.reply_to.as_deref(), Some("env_request_1"));
        assert_eq!(response.thread_id.as_deref(), Some("env_request_1"));
        assert_eq!(response.subject.as_deref(), Some("re: inspect"));
        assert_eq!(response.project_refs, vec!["proj:p1".to_string()]);
    }
}
