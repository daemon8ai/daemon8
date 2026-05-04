// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_mcp::{Deliber8RosterParams, deliber8_roster_inner};
use daemon8_store::{CardStore, SurrealStore};
use daemon8_types::{AgentCard, AgentKind, AgentStatus};

async fn setup() -> (Arc<SurrealStore>, daemon8_store::SurrealCardStore) {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let card_store = store.card_store();
    card_store.init_schema().await.unwrap();
    (store, card_store)
}

fn agent(slug: &str, kind: AgentKind, status: AgentStatus) -> AgentCard {
    AgentCard {
        id: format!("agent_{slug}"),
        actor_ref: format!("actor:{slug}"),
        address: format!("agent:{slug}"),
        slug: slug.to_string(),
        display_name: None,
        agent_kind: kind,
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
        last_seen_at: Some(2_000),
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

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("response must be valid JSON")
}

#[tokio::test]
async fn defaults_to_alive_only() {
    let (_store, card_store) = setup().await;
    card_store
        .upsert_agent(agent("alice", AgentKind::Specialist, AgentStatus::Alive))
        .await
        .unwrap();
    card_store
        .upsert_agent(agent("bob", AgentKind::Specialist, AgentStatus::Retired))
        .await
        .unwrap();

    let params = Deliber8RosterParams {
        kinds: None,
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 1);
    assert_eq!(v["agents"][0]["slug"], "alice");
}

#[tokio::test]
async fn explicit_status_returns_all() {
    let (_store, card_store) = setup().await;
    card_store
        .upsert_agent(agent("alice", AgentKind::Specialist, AgentStatus::Alive))
        .await
        .unwrap();
    card_store
        .upsert_agent(agent("bob", AgentKind::Specialist, AgentStatus::Retired))
        .await
        .unwrap();

    let params = Deliber8RosterParams {
        kinds: None,
        statuses: Some(vec!["alive".into(), "retired".into()]),
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 2);
}

#[tokio::test]
async fn kind_filter_isolates() {
    let (_store, card_store) = setup().await;
    card_store
        .upsert_agent(agent("alice", AgentKind::Specialist, AgentStatus::Alive))
        .await
        .unwrap();
    card_store
        .upsert_agent(agent("bk", AgentKind::Bookkeeper, AgentStatus::Alive))
        .await
        .unwrap();

    let params = Deliber8RosterParams {
        kinds: Some(vec!["specialist".into()]),
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 1);
    assert_eq!(v["agents"][0]["slug"], "alice");
}

#[tokio::test]
async fn unknown_kind_errors() {
    let (_store, card_store) = setup().await;
    let params = Deliber8RosterParams {
        kinds: Some(vec!["dragon".into()]),
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "unknown agent kind: dragon");
}

#[tokio::test]
async fn unknown_status_errors() {
    let (_store, card_store) = setup().await;
    let params = Deliber8RosterParams {
        kinds: None,
        statuses: Some(vec!["zombie".into()]),
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "unknown agent status: zombie");
}

#[tokio::test]
async fn respects_limit() {
    let (_store, card_store) = setup().await;
    for i in 0..4 {
        card_store
            .upsert_agent(agent(
                &format!("a{i}"),
                AgentKind::Specialist,
                AgentStatus::Alive,
            ))
            .await
            .unwrap();
    }
    let params = Deliber8RosterParams {
        kinds: None,
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: Some(2),
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 2);
}

#[tokio::test]
async fn default_limit_caps_at_fifty() {
    let (_store, card_store) = setup().await;
    for i in 0..60 {
        card_store
            .upsert_agent(agent(
                &format!("a{i:02}"),
                AgentKind::Specialist,
                AgentStatus::Alive,
            ))
            .await
            .unwrap();
    }
    let params = Deliber8RosterParams {
        kinds: None,
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 50);
}

#[tokio::test]
async fn empty_status_list_falls_back_to_alive_default() {
    let (_store, card_store) = setup().await;
    card_store
        .upsert_agent(agent("alive", AgentKind::Specialist, AgentStatus::Alive))
        .await
        .unwrap();
    card_store
        .upsert_agent(agent(
            "retired",
            AgentKind::Specialist,
            AgentStatus::Retired,
        ))
        .await
        .unwrap();

    let params = Deliber8RosterParams {
        kinds: None,
        statuses: Some(vec![]),
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 1);
    assert_eq!(v["agents"][0]["slug"], "alive");
}

#[tokio::test]
async fn empty_kind_list_treated_as_omitted() {
    let (_store, card_store) = setup().await;
    card_store
        .upsert_agent(agent("a", AgentKind::Specialist, AgentStatus::Alive))
        .await
        .unwrap();
    card_store
        .upsert_agent(agent("b", AgentKind::Bookkeeper, AgentStatus::Alive))
        .await
        .unwrap();

    let params = Deliber8RosterParams {
        kinds: Some(vec![]),
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 2);
}

#[tokio::test]
async fn explicit_limit_clamped_at_500() {
    let (_store, card_store) = setup().await;
    let params = Deliber8RosterParams {
        kinds: None,
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: Some(10_000),
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert!(v["error"].is_null(), "unexpected error: {res}");
    assert_eq!(v["total"], 0);
}

#[tokio::test]
async fn kind_filter_runs_before_limit() {
    let (_store, card_store) = setup().await;
    for i in 0..40 {
        card_store
            .upsert_agent(agent(
                &format!("spec_{i:02}"),
                AgentKind::Specialist,
                AgentStatus::Alive,
            ))
            .await
            .unwrap();
    }
    for i in 0..3 {
        card_store
            .upsert_agent(agent(
                &format!("bk_{i}"),
                AgentKind::Bookkeeper,
                AgentStatus::Alive,
            ))
            .await
            .unwrap();
    }
    let params = Deliber8RosterParams {
        kinds: Some(vec!["bookkeeper".into()]),
        statuses: None,
        project_ref: None,
        team_ref: None,
        limit: Some(10),
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 3, "expected 3 bookkeepers, got: {res}");
}

#[tokio::test]
async fn team_ref_filter_isolates() {
    let (_store, card_store) = setup().await;
    let mut card_a = agent("with_team", AgentKind::Specialist, AgentStatus::Alive);
    card_a.team_refs = vec!["team:alpha".into()];
    card_a.primary_team_ref = Some("team:alpha".into());
    let card_b = agent("no_team", AgentKind::Specialist, AgentStatus::Alive);
    card_store.upsert_agent(card_a).await.unwrap();
    card_store.upsert_agent(card_b).await.unwrap();

    let params = Deliber8RosterParams {
        kinds: None,
        statuses: None,
        project_ref: None,
        team_ref: Some("team:alpha".into()),
        limit: None,
    };
    let res = deliber8_roster_inner(&card_store, params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 1);
    assert_eq!(v["agents"][0]["slug"], "with_team");
}
