// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration tests for the deliber8 specialist runtime loop.
//!
//! These tests run the loop as an in-process tokio task against an in-memory
//! SurrealStore. The CLI surface around the loop is exercised by separate
//! manual smoke tests; here we validate the substrate behavior:
//! spawn -> request -> response -> stop -> retired.
//!
//! Source layout note: the daemon8 binary crate is binary-only, so these
//! tests live in `crates/daemon/tests/` and compile against the binary's
//! source via the generated test harness. They cannot access the
//! `crate::deliber8` module directly; instead they reproduce the minimal
//! AgentCard wiring and call the same `EnvelopeStore`/`CardStore` surface
//! the runtime uses, then drive the loop via a small in-test reimplementation
//! of the same poll/answer/heartbeat shape. This keeps the integration test
//! independent of the binary's module visibility while still validating the
//! end-to-end envelope choreography.

use std::sync::Arc;
use std::time::Duration;

use daemon8_store::{AgentCardFilter, CardStore, EnvelopeFilter, EnvelopeStore, SurrealStore};
use daemon8_types::{
    AgentCard, AgentKind, AgentStatus, EnvelopeKind, EnvelopePriority, EnvelopeRecord,
    EnvelopeStatus,
};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

const HEARTBEAT_MS: u64 = 200;

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn make_card(slug: &str, address: &str) -> AgentCard {
    let now = now_ns();
    AgentCard {
        id: format!("agent_{slug}"),
        actor_ref: format!("actor:{slug}"),
        address: address.to_string(),
        slug: slug.to_string(),
        display_name: None,
        agent_kind: AgentKind::Specialist,
        status: AgentStatus::Alive,
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
        runtime_kind: Some("daemon8.deliber8".into()),
        runtime_version: None,
        launch_nonce: None,
        started_at: Some(now),
        last_seen_at: None,
        heartbeat_interval_ms: Some(HEARTBEAT_MS),
        stop_state: serde_json::json!({}),
        last_stop_request_at: None,
        last_exit_code: None,
        last_signal: None,
        cost_window_usd: 0.0,
        cost_total_usd: 0.0,
        budget_daily_usd: None,
        failure_reason: None,
        created_at: now,
        updated_at: now,
    }
}

fn request(from: &str, to: &str, subject: &str) -> EnvelopeRecord {
    let now = now_ns();
    EnvelopeRecord {
        id: String::new(),
        kind: EnvelopeKind::Request,
        status: EnvelopeStatus::Queued,
        priority: EnvelopePriority::Normal,
        from_address: from.to_string(),
        to_address: to.to_string(),
        inbox_address: to.to_string(),
        subject: Some(subject.to_string()),
        body: Some("hello".into()),
        payload: Some(serde_json::json!({"k": "v"})),
        correlation_id: Some("integration-corr".into()),
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
        tags: vec![],
        project_refs: vec![],
        team_refs: vec![],
    }
}

fn stop(inbox: &str) -> EnvelopeRecord {
    let now = now_ns();
    EnvelopeRecord {
        id: String::new(),
        kind: EnvelopeKind::Control,
        status: EnvelopeStatus::Queued,
        priority: EnvelopePriority::Urgent,
        from_address: "operator:test".into(),
        to_address: inbox.to_string(),
        inbox_address: inbox.to_string(),
        subject: Some("stop".into()),
        body: Some("stop".into()),
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
        tags: vec![],
        project_refs: vec![],
        team_refs: vec![],
    }
}

// Minimal reimplementation of the runtime loop body. Mirrors
// `crate::deliber8::run_specialist` semantics so we can drive it against an
// in-memory store without depending on the binary's module visibility.
async fn run_loop(
    store: Arc<SurrealStore>,
    slug: String,
    inbox: String,
    cancel: CancellationToken,
) -> (u64, u64, u64, bool) {
    let envelope_store = store.envelope_store();
    let card_store = store.card_store();
    let agent_id = format!("agent_{slug}");
    card_store
        .update_agent_status(&agent_id, AgentStatus::Alive, now_ns())
        .await
        .unwrap();
    card_store
        .record_agent_heartbeat(&agent_id, now_ns())
        .await
        .ok();

    let mut processed = 0u64;
    let mut responded = 0u64;
    let mut heartbeats = 0u64;
    let poll = Duration::from_millis(HEARTBEAT_MS / 2);

    loop {
        let pending = envelope_store
            .list_pending(&inbox, Some(now_ns()), Some(32))
            .await
            .unwrap();
        for env in pending {
            processed += 1;
            if env.kind == EnvelopeKind::Control && env.body.as_deref() == Some("stop") {
                envelope_store.mark_delivered(&env.id, now_ns()).await.ok();
                envelope_store.mark_read(&env.id, now_ns()).await.ok();
                card_store
                    .update_agent_status(&agent_id, AgentStatus::Retired, now_ns())
                    .await
                    .unwrap();
                return (processed, responded, heartbeats, true);
            }

            envelope_store.mark_delivered(&env.id, now_ns()).await.ok();
            if env.kind == EnvelopeKind::Request {
                let response = EnvelopeRecord {
                    id: String::new(),
                    kind: EnvelopeKind::Response,
                    status: EnvelopeStatus::Queued,
                    priority: EnvelopePriority::Normal,
                    from_address: inbox.clone(),
                    to_address: env.from_address.clone(),
                    inbox_address: env.from_address.clone(),
                    subject: env.subject.as_ref().map(|s| format!("re: {s}")),
                    body: Some(format!("stub for {}", env.id)),
                    payload: None,
                    correlation_id: env.correlation_id.clone(),
                    thread_id: env.thread_id.clone().or_else(|| Some(env.id.clone())),
                    reply_to: Some(env.id.clone()),
                    created_at: now_ns(),
                    updated_at: now_ns(),
                    deliver_after: None,
                    delivered_at: None,
                    read_at: None,
                    expires_at: None,
                    failed_at: None,
                    failure_reason: None,
                    tags: vec!["deliber8.stub".into()],
                    project_refs: vec![],
                    team_refs: vec![],
                };
                envelope_store.enqueue_envelope(response).await.unwrap();
                responded += 1;
            }
            envelope_store.mark_read(&env.id, now_ns()).await.ok();
        }

        if card_store
            .record_agent_heartbeat(&agent_id, now_ns())
            .await
            .is_ok()
        {
            heartbeats += 1;
        }

        tokio::select! {
            _ = cancel.cancelled() => return (processed, responded, heartbeats, false),
            _ = sleep(poll) => {}
        }
    }
}

async fn setup_store_with_agent(slug: &str, inbox: &str) -> Arc<SurrealStore> {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let card_store = store.card_store();
    card_store.init_schema().await.unwrap();
    let card = make_card(slug, inbox);
    card_store.upsert_agent(card).await.unwrap();
    store
}

#[tokio::test]
async fn specialist_responds_to_request_and_stops_on_control() {
    let slug = "test-specialist-a";
    let inbox = "agent:test-specialist-a";
    let supervisor = "agent:supervisor";
    let store = setup_store_with_agent(slug, inbox).await;

    // Pre-load an inbox request before the loop starts.
    store
        .envelope_store()
        .enqueue_envelope(request(supervisor, inbox, "inspect"))
        .await
        .unwrap();

    let cancel = CancellationToken::new();
    let store_loop = store.clone();
    let slug_loop = slug.to_string();
    let inbox_loop = inbox.to_string();
    let loop_handle =
        tokio::spawn(async move { run_loop(store_loop, slug_loop, inbox_loop, cancel).await });

    // Wait long enough for the loop to drain the request, write the response,
    // and beat at least once.
    sleep(Duration::from_millis(HEARTBEAT_MS * 3)).await;

    // Supervisor inbox should now hold a Response.
    let supervisor_inbox = store
        .envelope_store()
        .query_inbox(&EnvelopeFilter {
            inbox_address: Some(supervisor.into()),
            kinds: Some(vec![EnvelopeKind::Response]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !supervisor_inbox.is_empty(),
        "expected at least one response envelope in supervisor inbox"
    );
    let resp = &supervisor_inbox[0];
    assert_eq!(resp.kind, EnvelopeKind::Response);
    assert_eq!(resp.from_address, inbox);
    assert_eq!(resp.correlation_id.as_deref(), Some("integration-corr"));
    assert!(resp.reply_to.is_some());

    // AgentCard last_seen_at should have advanced.
    let card = store
        .card_store()
        .get_agent_by_slug(slug)
        .await
        .unwrap()
        .expect("card present");
    assert!(
        card.last_seen_at.is_some(),
        "heartbeat must have advanced last_seen_at"
    );

    // Now send a stop control envelope and assert clean exit + retired status.
    store
        .envelope_store()
        .enqueue_envelope(stop(inbox))
        .await
        .unwrap();

    let outcome = timeout(Duration::from_secs(5), loop_handle)
        .await
        .expect("loop did not exit within 5s")
        .expect("loop task panicked");

    assert!(outcome.3, "loop must report stopped_by_control");
    assert!(
        outcome.0 >= 2,
        "loop should have processed at least request + stop"
    );
    assert!(outcome.1 >= 1, "loop should have responded at least once");

    // Card status should be Retired (filter that out via list_agents Alive).
    let alive_after = store
        .card_store()
        .list_agents(&AgentCardFilter {
            statuses: Some(vec![AgentStatus::Alive]),
            project_ref: None,
            team_ref: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(
        alive_after.iter().all(|a| a.slug != slug),
        "specialist must no longer appear as Alive after stop"
    );
}

#[tokio::test]
async fn cancellation_preserves_alive_status() {
    let slug = "test-specialist-cancel";
    let inbox = "agent:test-specialist-cancel";
    let store = setup_store_with_agent(slug, inbox).await;

    let cancel = CancellationToken::new();
    let store_loop = store.clone();
    let cancel_for_loop = cancel.clone();
    let slug_loop = slug.to_string();
    let inbox_loop = inbox.to_string();
    let loop_handle =
        tokio::spawn(
            async move { run_loop(store_loop, slug_loop, inbox_loop, cancel_for_loop).await },
        );

    sleep(Duration::from_millis(HEARTBEAT_MS * 2)).await;
    cancel.cancel();

    let outcome = timeout(Duration::from_secs(5), loop_handle)
        .await
        .expect("loop did not exit within 5s")
        .expect("loop task panicked");
    assert!(
        !outcome.3,
        "loop must NOT report stopped_by_control after cancellation"
    );

    let card = store
        .card_store()
        .get_agent_by_slug(slug)
        .await
        .unwrap()
        .expect("card present");
    assert_eq!(card.status, AgentStatus::Alive);
}

#[tokio::test]
async fn back_to_back_requests_each_get_a_response() {
    let slug = "test-specialist-batch";
    let inbox = "agent:test-specialist-batch";
    let supervisor = "agent:supervisor-batch";
    let store = setup_store_with_agent(slug, inbox).await;

    for n in 0..5 {
        let mut req = request(supervisor, inbox, &format!("req-{n}"));
        req.correlation_id = Some(format!("c-{n}"));
        store.envelope_store().enqueue_envelope(req).await.unwrap();
    }

    let cancel = CancellationToken::new();
    let store_loop = store.clone();
    let slug_loop = slug.to_string();
    let inbox_loop = inbox.to_string();
    let loop_handle =
        tokio::spawn(async move { run_loop(store_loop, slug_loop, inbox_loop, cancel).await });

    sleep(Duration::from_millis(HEARTBEAT_MS * 4)).await;

    let responses = store
        .envelope_store()
        .query_inbox(&EnvelopeFilter {
            inbox_address: Some(supervisor.into()),
            kinds: Some(vec![EnvelopeKind::Response]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(responses.len(), 5, "should have one response per request");

    // Send stop then drain.
    store
        .envelope_store()
        .enqueue_envelope(stop(inbox))
        .await
        .unwrap();
    let _ = timeout(Duration::from_secs(5), loop_handle)
        .await
        .expect("loop exit");
}
