// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{EmbeddingProfile, EmbeddingProfileStore, StoreError};

pub(crate) const EMBEDDING_PROFILE_DDL: &str =
    "DEFINE TABLE IF NOT EXISTS embedding_profile SCHEMAFULL;

DEFINE FIELD IF NOT EXISTS provider   ON embedding_profile TYPE string;
DEFINE FIELD IF NOT EXISTS model      ON embedding_profile TYPE string;
DEFINE FIELD IF NOT EXISTS dimensions ON embedding_profile TYPE int;
DEFINE FIELD IF NOT EXISTS created_at ON embedding_profile TYPE int;

DEFINE INDEX IF NOT EXISTS idx_emb_provider_model
    ON embedding_profile FIELDS provider, model UNIQUE;";

pub struct SurrealEmbeddingProfileStore {
    db: Surreal<Db>,
}

impl SurrealEmbeddingProfileStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }
}

fn strip_table_prefix(raw: &str) -> &str {
    raw.split_once(':')
        .map_or(raw, |(_, id)| id)
        .trim_matches('`')
}

fn extract_record_id(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(strip_table_prefix(s).to_string()),
        serde_json::Value::Object(obj) => {
            let id_field = obj.get("id")?;
            match id_field {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(inner) => {
                    inner.get("String")?.as_str().map(|s| s.to_string())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn decode(mut val: serde_json::Value) -> Result<EmbeddingProfile, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

#[async_trait::async_trait]
impl EmbeddingProfileStore for SurrealEmbeddingProfileStore {
    async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .query(EMBEDDING_PROFILE_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("embedding_profile schema init check: {e}")))?;
        Ok(())
    }

    async fn upsert(&self, profile: EmbeddingProfile) -> Result<String, StoreError> {
        let mut content = serde_json::to_value(&profile)?;
        if let serde_json::Value::Object(ref mut obj) = content {
            obj.remove("id");
        }

        let trimmed = profile.id.trim();
        if trimmed.is_empty() {
            return Err(StoreError::Other(
                "embedding_profile.id must be non-empty (use '<provider>:<model>')".into(),
            ));
        }

        let mut result = self
            .db
            .query("UPSERT type::record('embedding_profile', $id) CONTENT $content RETURN AFTER")
            .bind(("id", serde_json::json!(trimmed)))
            .bind(("content", content))
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile upsert: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("embedding_profile upsert read: {e}")))?;

        row.as_ref()
            .and_then(|v| v.get("id"))
            .and_then(extract_record_id)
            .ok_or_else(|| StoreError::Db("embedding_profile upsert: no id returned".into()))
    }

    async fn get(&self, id: &str) -> Result<Option<EmbeddingProfile>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('embedding_profile', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile get: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("embedding_profile get read: {e}")))?;

        row.map(decode).transpose()
    }

    async fn list(&self) -> Result<Vec<EmbeddingProfile>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM embedding_profile ORDER BY created_at ASC")
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile list: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("embedding_profile list read: {e}")))?;
        rows.into_iter().map(decode).collect()
    }

    async fn find_by_provider_and_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Option<EmbeddingProfile>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM embedding_profile WHERE provider = $p AND model = $m LIMIT 1")
            .bind(("p", serde_json::json!(provider)))
            .bind(("m", serde_json::json!(model)))
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile find: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("embedding_profile find read: {e}")))?;
        rows.into_iter().next().map(decode).transpose()
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let exists = self.get(id).await?.is_some();
        self.db
            .query("DELETE type::record('embedding_profile', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile delete: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("embedding_profile delete check: {e}")))?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurrealStore;

    async fn setup() -> (SurrealStore, SurrealEmbeddingProfileStore) {
        let store = SurrealStore::memory().await.unwrap();
        let s = store.embedding_profile_store();
        s.init_schema().await.unwrap();
        (store, s)
    }

    fn make(provider: &str, model: &str, dims: u32) -> EmbeddingProfile {
        EmbeddingProfile {
            id: format!("{provider}:{model}"),
            provider: provider.into(),
            model: model.into(),
            dimensions: dims,
            created_at: 100,
        }
    }

    #[tokio::test]
    async fn schema_init_is_idempotent() {
        let (_s, store) = setup().await;
        store.init_schema().await.unwrap();
        store.init_schema().await.unwrap();
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let (_s, store) = setup().await;
        let p = make("openai", "text-embedding-3-small", 1536);
        let id = store.upsert(p.clone()).await.unwrap();
        assert_eq!(id, p.id);

        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got, p);
    }

    #[tokio::test]
    async fn upsert_overwrites_in_place() {
        let (_s, store) = setup().await;
        let mut p = make("openai", "text-embedding-3-small", 1536);
        store.upsert(p.clone()).await.unwrap();

        p.dimensions = 1024;
        store.upsert(p.clone()).await.unwrap();
        let got = store.get(&p.id).await.unwrap().unwrap();
        assert_eq!(got.dimensions, 1024);
    }

    #[tokio::test]
    async fn empty_id_is_rejected() {
        let (_s, store) = setup().await;
        let p = EmbeddingProfile {
            id: "   ".into(),
            provider: "p".into(),
            model: "m".into(),
            dimensions: 1,
            created_at: 0,
        };
        let err = store.upsert(p).await.unwrap_err().to_string();
        assert!(err.contains("non-empty"));
    }

    #[tokio::test]
    async fn find_by_provider_and_model_returns_match() {
        let (_s, store) = setup().await;
        store.upsert(make("openai", "small", 384)).await.unwrap();
        store.upsert(make("openai", "large", 3072)).await.unwrap();
        store.upsert(make("local", "small", 384)).await.unwrap();

        let hit = store
            .find_by_provider_and_model("openai", "small")
            .await
            .unwrap();
        assert!(hit.is_some());
        let p = hit.unwrap();
        assert_eq!(p.provider, "openai");
        assert_eq!(p.model, "small");

        let miss = store
            .find_by_provider_and_model("anthropic", "claude")
            .await
            .unwrap();
        assert!(miss.is_none());
    }

    #[tokio::test]
    async fn list_returns_all_profiles() {
        let (_s, store) = setup().await;
        store.upsert(make("openai", "a", 1)).await.unwrap();
        store.upsert(make("openai", "b", 2)).await.unwrap();
        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn delete_returns_true_when_existed_false_otherwise() {
        let (_s, store) = setup().await;
        store.upsert(make("p", "m", 1)).await.unwrap();
        assert!(store.delete("p:m").await.unwrap());
        assert!(!store.delete("p:m").await.unwrap());
    }

    #[tokio::test]
    async fn unique_index_prevents_duplicate_provider_model_under_different_id() {
        // The UNIQUE index on (provider, model) means even a different
        // record id collides if it would create a second row with the same
        // (provider, model) pair.
        let (_s, store) = setup().await;
        store.upsert(make("openai", "small", 1536)).await.unwrap();

        let dup = EmbeddingProfile {
            id: "alias:openai-small".into(),
            provider: "openai".into(),
            model: "small".into(),
            dimensions: 1536,
            created_at: 200,
        };
        let err = store.upsert(dup).await;
        assert!(err.is_err(), "duplicate (provider, model) must be rejected");
    }
}
