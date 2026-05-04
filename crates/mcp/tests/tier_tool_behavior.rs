// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_mcp::{
    MemoryDedupeLongParams, MemorySweepShortParams, QueryMemoryTierParams,
    memory_dedupe_long_inner, memory_sweep_short_inner, query_memory_tier_inner,
};
use daemon8_store::{
    MemoryLongRecord, MemoryLongStore, MemoryReferenceStore, MemoryShortRecord, MemoryShortStore,
    SurrealStore,
};
use daemon8_types::{LongContentKind, MemoryScope, ShortContentKind};

async fn setup() -> (
    Arc<SurrealStore>,
    daemon8_store::SurrealMemoryShortStore,
    daemon8_store::SurrealMemoryReferenceStore,
    daemon8_store::SurrealMemoryLongStore,
    daemon8_store::SurrealBookkeeperStore,
) {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let short = store.memory_short_store();
    short.init_schema().await.unwrap();
    let reference = store.memory_reference_store();
    reference.init_schema().await.unwrap();
    let long = store.memory_long_store();
    long.init_schema().await.unwrap();
    let bookkeeper = store.bookkeeper_store();
    (store, short, reference, long, bookkeeper)
}

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("response must be valid JSON")
}

fn short_record(id: &str, agent: &str, content: &str, expires_at: u64) -> MemoryShortRecord {
    MemoryShortRecord {
        id: Some(id.into()),
        content: content.into(),
        content_kind: ShortContentKind::Scratch,
        agent_id: agent.into(),
        thread_id: None,
        scope: MemoryScope::Agent,
        tags: vec![],
        content_hash: format!("hash:{id}"),
        expires_at,
        created_at: 1_000,
        updated_at: 1_000,
        embedding: None,
        embedding_profile_id: None,
    }
}

fn long_record(id: &str, content_hash: &str, confidence: f32) -> MemoryLongRecord {
    MemoryLongRecord {
        id: Some(id.into()),
        content: format!("content for {id}"),
        content_kind: LongContentKind::Fact,
        scope: MemoryScope::Global,
        tags: vec![],
        content_hash: content_hash.into(),
        provenance: vec![],
        confidence,
        supersedes: None,
        revoked_at: None,
        created_at: 1_000,
        updated_at: 1_000,
        embedding: None,
        embedding_profile_id: None,
    }
}

#[tokio::test]
async fn sweep_short_dry_run_reports_without_deleting() {
    let (_store, short, _, _, bookkeeper) = setup().await;
    short
        .save(short_record("a", "agent:x", "stale", 100))
        .await
        .unwrap();
    short
        .save(short_record("b", "agent:x", "fresh", u64::MAX))
        .await
        .unwrap();

    let params = MemorySweepShortParams {
        agent_id: None,
        apply: false,
    };
    let res = memory_sweep_short_inner(&bookkeeper, params).await;
    let v = parse(&res);
    assert_eq!(v["expired"], 1);
    assert_eq!(
        v["deleted_ids"].as_array().unwrap().len(),
        0,
        "dry run must not delete: {res}"
    );
}

#[tokio::test]
async fn sweep_short_apply_deletes_expired() {
    let (_store, short, _, _, bookkeeper) = setup().await;
    short
        .save(short_record("a", "agent:x", "stale", 100))
        .await
        .unwrap();
    short
        .save(short_record("b", "agent:x", "fresh", u64::MAX))
        .await
        .unwrap();

    let params = MemorySweepShortParams {
        agent_id: None,
        apply: true,
    };
    let res = memory_sweep_short_inner(&bookkeeper, params).await;
    let v = parse(&res);
    assert_eq!(v["expired"], 1);
    assert_eq!(v["deleted_ids"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn dedupe_long_dry_run_reports_redundant_without_deleting() {
    let (_store, _, _, long, bookkeeper) = setup().await;
    long.save(long_record("k1", "hash:dup", 0.9)).await.unwrap();
    long.save(long_record("k2", "hash:dup", 0.5)).await.unwrap();
    long.save(long_record("k3", "hash:unique", 0.7))
        .await
        .unwrap();

    let params = MemoryDedupeLongParams {
        scope: None,
        apply: false,
    };
    let res = memory_dedupe_long_inner(&bookkeeper, params).await;
    let v = parse(&res);
    assert_eq!(v["groups"], 1);
    assert_eq!(v["redundant"], 1);
    assert_eq!(
        v["removed_ids"].as_array().unwrap().len(),
        0,
        "dry run must not delete"
    );
}

#[tokio::test]
async fn dedupe_long_unknown_scope_errors() {
    let (_store, _, _, _, bookkeeper) = setup().await;
    let params = MemoryDedupeLongParams {
        scope: Some("session".into()),
        apply: false,
    };
    let res = memory_dedupe_long_inner(&bookkeeper, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "unknown memory scope: session");
}

#[tokio::test]
async fn query_tier_short_returns_records() {
    let (_store, short, reference, long, _) = setup().await;
    short
        .save(short_record("s1", "agent:x", "hello", u64::MAX))
        .await
        .unwrap();

    let params = QueryMemoryTierParams {
        tier: "short".into(),
        agent_id: Some("agent:x".into()),
        scope: None,
        tags_any: None,
        embedding_profile_id: None,
        include_expired: false,
        include_revoked: false,
        limit: None,
    };
    let res = query_memory_tier_inner(Some(&short), Some(&reference), Some(&long), params).await;
    let v = parse(&res);
    assert_eq!(v["tier"], "short");
    assert_eq!(v["total"], 1);
}

#[tokio::test]
async fn query_tier_unknown_tier_errors() {
    let (_store, short, reference, long, _) = setup().await;
    let params = QueryMemoryTierParams {
        tier: "ephemeral".into(),
        agent_id: None,
        scope: None,
        tags_any: None,
        embedding_profile_id: None,
        include_expired: false,
        include_revoked: false,
        limit: None,
    };
    let res = query_memory_tier_inner(Some(&short), Some(&reference), Some(&long), params).await;
    let v = parse(&res);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("unknown tier: ephemeral"),
        "got: {res}"
    );
}

#[tokio::test]
async fn query_tier_default_limit_caps_at_twenty() {
    let (_store, short, reference, long, _) = setup().await;
    for i in 0..25 {
        short
            .save(short_record(
                &format!("s{i:02}"),
                "agent:cap",
                "x",
                u64::MAX,
            ))
            .await
            .unwrap();
    }
    let params = QueryMemoryTierParams {
        tier: "short".into(),
        agent_id: Some("agent:cap".into()),
        scope: None,
        tags_any: None,
        embedding_profile_id: None,
        include_expired: false,
        include_revoked: false,
        limit: None,
    };
    let res = query_memory_tier_inner(Some(&short), Some(&reference), Some(&long), params).await;
    let v = parse(&res);
    assert_eq!(v["total"], 20);
}
