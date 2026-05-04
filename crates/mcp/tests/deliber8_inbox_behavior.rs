// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_mcp::{Deliber8InboxParams, deliber8_inbox_inner};
use daemon8_store::{EnvelopeStore, SurrealStore};
use daemon8_types::{EnvelopeKind, EnvelopePriority, EnvelopeRecord, EnvelopeStatus};

async fn setup() -> (Arc<SurrealStore>, daemon8_store::SurrealEnvelopeStore) {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let env_store = store.envelope_store();
    env_store.init_schema().await.unwrap();
    (store, env_store)
}

fn envelope(id: &str, address: &str, status: EnvelopeStatus) -> EnvelopeRecord {
    EnvelopeRecord {
        id: id.to_string(),
        kind: EnvelopeKind::Message,
        status,
        priority: EnvelopePriority::Normal,
        from_address: "agent:sender".into(),
        to_address: address.into(),
        inbox_address: address.into(),
        subject: None,
        body: Some("hello".into()),
        payload: None,
        correlation_id: None,
        thread_id: None,
        reply_to: None,
        created_at: 1_000_000,
        updated_at: 1_000_000,
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

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("response must be valid JSON")
}

#[tokio::test]
async fn rejects_empty_address() {
    let (_store, env_store) = setup().await;
    let params = Deliber8InboxParams {
        address: "  ".into(),
        statuses: None,
        limit: None,
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "deliber8_inbox requires a non-empty address");
}

#[tokio::test]
async fn rejects_unknown_status() {
    let (_store, env_store) = setup().await;
    let params = Deliber8InboxParams {
        address: "agent:x".into(),
        statuses: Some(vec!["bogus".into()]),
        limit: None,
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "unknown envelope status: bogus");
}

#[tokio::test]
async fn returns_counts_and_envelopes() {
    let (_store, env_store) = setup().await;
    let addr = "agent:counts";
    env_store
        .enqueue_envelope(envelope("env_a", addr, EnvelopeStatus::Queued))
        .await
        .unwrap();
    env_store
        .enqueue_envelope(envelope("env_b", addr, EnvelopeStatus::Queued))
        .await
        .unwrap();

    let params = Deliber8InboxParams {
        address: addr.into(),
        statuses: None,
        limit: None,
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);

    assert_eq!(v["address"], addr);
    assert_eq!(v["total"], 2);
    assert_eq!(v["queued"], 2);
    assert_eq!(v["delivered"], 0);
    assert_eq!(v["envelopes"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn status_filter_isolates() {
    let (_store, env_store) = setup().await;
    let addr = "agent:filter";
    env_store
        .enqueue_envelope(envelope("env_q", addr, EnvelopeStatus::Queued))
        .await
        .unwrap();
    env_store
        .enqueue_envelope(envelope("env_d", addr, EnvelopeStatus::Queued))
        .await
        .unwrap();
    env_store.mark_delivered("env_d", 2_000_000).await.unwrap();

    let params = Deliber8InboxParams {
        address: addr.into(),
        statuses: Some(vec!["queued".into()]),
        limit: None,
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);

    assert_eq!(v["total"], 1);
    assert_eq!(v["queued"], 1);
    assert_eq!(v["delivered"], 0);
}

#[tokio::test]
async fn respects_limit() {
    let (_store, env_store) = setup().await;
    let addr = "agent:limit";
    for i in 0..5 {
        env_store
            .enqueue_envelope(envelope(&format!("env_{i}"), addr, EnvelopeStatus::Queued))
            .await
            .unwrap();
    }

    let params = Deliber8InboxParams {
        address: addr.into(),
        statuses: None,
        limit: Some(2),
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);

    assert_eq!(v["total"], 2);
    assert_eq!(v["envelopes"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn default_limit_caps_at_twenty() {
    let (_store, env_store) = setup().await;
    let addr = "agent:default";
    for i in 0..25 {
        env_store
            .enqueue_envelope(envelope(&format!("env_{i}"), addr, EnvelopeStatus::Queued))
            .await
            .unwrap();
    }

    let params = Deliber8InboxParams {
        address: addr.into(),
        statuses: None,
        limit: None,
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);

    assert_eq!(v["total"], 20);
    assert_eq!(v["envelopes"].as_array().unwrap().len(), 20);
}

#[tokio::test]
async fn empty_status_list_returns_all() {
    let (_store, env_store) = setup().await;
    let addr = "agent:empty";
    env_store
        .enqueue_envelope(envelope("env_q", addr, EnvelopeStatus::Queued))
        .await
        .unwrap();
    env_store
        .enqueue_envelope(envelope("env_d", addr, EnvelopeStatus::Queued))
        .await
        .unwrap();
    env_store.mark_delivered("env_d", 2_000_000).await.unwrap();

    let params = Deliber8InboxParams {
        address: addr.into(),
        statuses: Some(vec![]),
        limit: None,
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);

    assert_eq!(v["total"], 2);
}

#[tokio::test]
async fn explicit_limit_clamped_at_500() {
    let (_store, env_store) = setup().await;
    let params = Deliber8InboxParams {
        address: "agent:none".into(),
        statuses: None,
        limit: Some(10_000),
    };
    let res = deliber8_inbox_inner(&env_store, params).await;
    let v = parse(&res);

    assert!(v["error"].is_null(), "unexpected error: {res}");
    assert_eq!(v["total"], 0);
}
