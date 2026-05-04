// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_mcp::{
    RegisterEmbeddingProfileParams, list_embedding_profiles_inner, register_embedding_profile_inner,
};
use daemon8_store::{EmbeddingProfileStore, SurrealStore};

async fn setup() -> (
    Arc<SurrealStore>,
    daemon8_store::SurrealEmbeddingProfileStore,
) {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let profiles = store.embedding_profile_store();
    profiles.init_schema().await.unwrap();
    (store, profiles)
}

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("response must be valid JSON")
}

#[tokio::test]
async fn register_with_default_id() {
    let (_store, profiles) = setup().await;
    let params = RegisterEmbeddingProfileParams {
        provider: "openai".into(),
        model: "text-embedding-3-small".into(),
        dimensions: 1536,
        id: None,
    };
    let res = register_embedding_profile_inner(&profiles, params).await;
    let v = parse(&res);
    assert_eq!(v["id"], "openai:text-embedding-3-small");
    assert_eq!(v["provider"], "openai");
    assert_eq!(v["dimensions"], 1536);
}

#[tokio::test]
async fn register_with_explicit_id() {
    let (_store, profiles) = setup().await;
    let params = RegisterEmbeddingProfileParams {
        provider: "fastembed".into(),
        model: "bge-small-en-v1.5".into(),
        dimensions: 384,
        id: Some("fast:bge".into()),
    };
    let res = register_embedding_profile_inner(&profiles, params).await;
    let v = parse(&res);
    assert_eq!(v["id"], "fast:bge");
}

#[tokio::test]
async fn register_is_idempotent_on_provider_model() {
    let (_store, profiles) = setup().await;
    let params = || RegisterEmbeddingProfileParams {
        provider: "openai".into(),
        model: "text-embedding-3-large".into(),
        dimensions: 3072,
        id: None,
    };
    let first = register_embedding_profile_inner(&profiles, params()).await;
    let _second = register_embedding_profile_inner(&profiles, params()).await;
    let listing = list_embedding_profiles_inner(&profiles).await;
    let v = parse(&listing);
    assert_eq!(v["total"], 1, "duplicate register must not create row");
    assert_eq!(v["profiles"][0]["id"], parse(&first)["id"]);
}

#[tokio::test]
async fn register_rejects_empty_provider() {
    let (_store, profiles) = setup().await;
    let params = RegisterEmbeddingProfileParams {
        provider: "  ".into(),
        model: "x".into(),
        dimensions: 16,
        id: None,
    };
    let res = register_embedding_profile_inner(&profiles, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "provider must be non-empty");
}

#[tokio::test]
async fn register_rejects_zero_dimensions() {
    let (_store, profiles) = setup().await;
    let params = RegisterEmbeddingProfileParams {
        provider: "x".into(),
        model: "y".into(),
        dimensions: 0,
        id: None,
    };
    let res = register_embedding_profile_inner(&profiles, params).await;
    let v = parse(&res);
    assert_eq!(v["error"], "dimensions must be greater than zero");
}

#[tokio::test]
async fn list_returns_total_and_profiles() {
    let (_store, profiles) = setup().await;
    register_embedding_profile_inner(
        &profiles,
        RegisterEmbeddingProfileParams {
            provider: "a".into(),
            model: "x".into(),
            dimensions: 8,
            id: None,
        },
    )
    .await;
    register_embedding_profile_inner(
        &profiles,
        RegisterEmbeddingProfileParams {
            provider: "b".into(),
            model: "y".into(),
            dimensions: 16,
            id: None,
        },
    )
    .await;
    let res = list_embedding_profiles_inner(&profiles).await;
    let v = parse(&res);
    assert_eq!(v["total"], 2);
    assert_eq!(v["profiles"].as_array().unwrap().len(), 2);
}
