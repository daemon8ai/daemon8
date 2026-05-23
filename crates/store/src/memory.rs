// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{Memory, MemoryFilter, MemoryStore, StoreError};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

pub struct SurrealMemoryStore {
    db: Surreal<Db>,
}

impl SurrealMemoryStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

    pub async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .use_ns(NAMESPACE)
            .use_db(DATABASE)
            .await
            .map_err(|e| StoreError::Db(format!("selecting namespace/database: {e}")))?;

        self.db
            .query(
                "DEFINE TABLE IF NOT EXISTS memory SCHEMAFULL;

                 DEFINE FIELD IF NOT EXISTS created_at          ON memory TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at          ON memory TYPE int;
                 DEFINE FIELD IF NOT EXISTS kind                ON memory TYPE string;
                 DEFINE FIELD IF NOT EXISTS content             ON memory TYPE string;
                 DEFINE FIELD IF NOT EXISTS source_observations ON memory TYPE array<int>;
                 DEFINE FIELD IF NOT EXISTS tags                ON memory TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS project_slug        ON memory TYPE string;
                 DEFINE FIELD IF NOT EXISTS session_id          ON memory TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS confidence          ON memory TYPE float;
                 DEFINE FIELD IF NOT EXISTS data                ON memory TYPE option<object> FLEXIBLE;

                 DEFINE INDEX IF NOT EXISTS idx_memory_kind    ON memory FIELDS kind;
                 DEFINE INDEX IF NOT EXISTS idx_memory_project ON memory FIELDS project_slug;
                 DEFINE INDEX IF NOT EXISTS idx_memory_tags    ON memory FIELDS tags;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("memory schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory schema init check: {e}")))?;

        Ok(())
    }

    fn build_query_sql(filter: &MemoryFilter) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(ref kinds) = filter.kinds
            && !kinds.is_empty()
        {
            let kind_strs: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
            conditions.push("kind IN $allowed_kinds".to_string());
            binds.push(("allowed_kinds".into(), serde_json::json!(kind_strs)));
        }

        if let Some(ref tags) = filter.tags
            && !tags.is_empty()
        {
            conditions.push("tags CONTAINSALL $required_tags".to_string());
            binds.push(("required_tags".into(), serde_json::json!(tags)));
        }

        if let Some(ref slug) = filter.project_slug {
            conditions.push("project_slug = $slug".to_string());
            binds.push(("slug".into(), serde_json::json!(slug)));
        }

        if let Some(ref sid) = filter.session_id {
            conditions.push("session_id = $sid".to_string());
            binds.push(("sid".into(), serde_json::json!(sid)));
        }

        if let Some(ref text) = filter.text_match {
            conditions
                .push("string::contains(string::lowercase(content), $text_lower)".to_string());
            binds.push((
                "text_lower".into(),
                serde_json::json!(text.to_ascii_lowercase()),
            ));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = match filter.limit {
            Some(n) => format!(" LIMIT {n}"),
            None => String::new(),
        };

        let sql =
            format!("SELECT * FROM memory{where_clause} ORDER BY created_at DESC{limit_clause}");

        (sql, binds)
    }
}

/// SurrealDB wraps record IDs in a `{ id: "memory:xxx", ... }` envelope.
/// After deserialization the `id` field is a Thing like `memory:abc123` --
/// strip the table prefix so callers get a bare key they can pass back to
/// `get_memory` / `forget_memory`.
fn strip_table_prefix(raw: &str) -> &str {
    raw.strip_prefix("memory:").unwrap_or(raw)
}

/// Extract the bare record key from a SurrealDB `id` field, which may be
/// serialized as either a plain string (`"memory:abc"`) or a structured
/// object (`{ "tb": "memory", "id": { "String": "abc" } }`).
fn extract_record_id(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(strip_table_prefix(s).to_string()),
        serde_json::Value::Object(obj) => {
            // Structured Thing: { "tb": "memory", "id": { "String": "key" } }
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

#[async_trait::async_trait]
impl MemoryStore for SurrealMemoryStore {
    async fn save_memory(&self, memory: Memory) -> Result<String, StoreError> {
        let json_content = serde_json::to_value(&memory)?;

        let mut result = self
            .db
            .query("CREATE memory CONTENT $content")
            .bind(("content", json_content))
            .await
            .map_err(|e| StoreError::Db(format!("save_memory: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("save_memory read result: {e}")))?;

        let id = row
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(extract_record_id)
            .ok_or_else(|| StoreError::Db("save_memory: no id returned".into()))?;

        Ok(id)
    }

    async fn query_memory(&self, filter: &MemoryFilter) -> Result<Vec<Memory>, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);

        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("query_memory: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("query_memory read results: {e}")))?;

        let mut memories = Vec::with_capacity(raw.len());
        for mut val in raw {
            // Patch the id field from SurrealDB's Thing to a bare string
            if let Some(id_val) = val.get("id")
                && let Some(bare) = extract_record_id(id_val)
            {
                val["id"] = serde_json::Value::String(bare);
            }
            let mem: Memory = serde_json::from_value(val)?;
            memories.push(mem);
        }

        Ok(memories)
    }

    async fn get_memory(&self, id: &str) -> Result<Option<Memory>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('memory', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_memory: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_memory read result: {e}")))?;

        match row {
            Some(mut val) => {
                if let Some(id_val) = val.get("id")
                    && let Some(bare) = extract_record_id(id_val)
                {
                    val["id"] = serde_json::Value::String(bare);
                }
                let mem: Memory = serde_json::from_value(val)?;
                Ok(Some(mem))
            }
            None => Ok(None),
        }
    }

    async fn update_memory(&self, memory: Memory) -> Result<(), StoreError> {
        let id = memory
            .id
            .as_deref()
            .ok_or_else(|| StoreError::Other("update_memory requires an id".into()))?;

        let json_content = serde_json::to_value(&memory)?;

        self.db
            .query("UPDATE type::record('memory', $id) CONTENT $content")
            .bind(("id", serde_json::json!(id)))
            .bind(("content", json_content))
            .await
            .map_err(|e| StoreError::Db(format!("update_memory: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("update_memory check: {e}")))?;

        Ok(())
    }

    async fn forget_memory(&self, id: &str) -> Result<bool, StoreError> {
        // Check existence first so we can return false for nonexistent IDs
        let exists = self.get_memory(id).await?.is_some();

        self.db
            .query("DELETE type::record('memory', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("forget_memory: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("forget_memory check: {e}")))?;

        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurrealStore;
    use daemon8_types::MemoryKind;

    async fn setup() -> (SurrealStore, SurrealMemoryStore) {
        let store = SurrealStore::memory().await.unwrap();
        let mem_store = store.memory_store();
        mem_store.init_schema().await.unwrap();
        (store, mem_store)
    }

    fn make_memory(kind: MemoryKind, content: &str, project: &str) -> Memory {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        Memory {
            id: None,
            created_at: now,
            updated_at: now,
            kind,
            content: content.to_string(),
            source_observations: vec![],
            tags: vec![],
            project_slug: project.to_string(),
            session_id: None,
            confidence: 1.0,
            data: None,
        }
    }

    #[tokio::test]
    async fn crud_round_trip() {
        let (_store, mem_store) = setup().await;

        let mut mem = make_memory(MemoryKind::Pattern, "retry with backoff", "daemon8");
        mem.tags = vec!["resilience".into()];

        let id = mem_store.save_memory(mem.clone()).await.unwrap();
        assert!(!id.is_empty());

        let fetched = mem_store.get_memory(&id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "retry with backoff");
        assert_eq!(fetched.kind, MemoryKind::Pattern);
        assert_eq!(fetched.tags, vec!["resilience"]);

        let mut updated = fetched;
        updated.content = "retry with exponential backoff".to_string();
        updated.updated_at += 1_000_000;
        mem_store.update_memory(updated).await.unwrap();

        let fetched2 = mem_store.get_memory(&id).await.unwrap().unwrap();
        assert_eq!(fetched2.content, "retry with exponential backoff");
    }

    #[tokio::test]
    async fn query_by_kind() {
        let (_store, mem_store) = setup().await;

        mem_store
            .save_memory(make_memory(MemoryKind::Pattern, "pattern 1", "p1"))
            .await
            .unwrap();
        mem_store
            .save_memory(make_memory(MemoryKind::Decision, "decision 1", "p1"))
            .await
            .unwrap();
        mem_store
            .save_memory(make_memory(MemoryKind::ErrorSignature, "error sig", "p1"))
            .await
            .unwrap();

        let filter = MemoryFilter {
            kinds: Some(vec![MemoryKind::Pattern]),
            ..Default::default()
        };
        let results = mem_store.query_memory(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MemoryKind::Pattern);
    }

    #[tokio::test]
    async fn query_by_tags() {
        let (_store, mem_store) = setup().await;

        let mut m1 = make_memory(MemoryKind::Pattern, "tagged", "p1");
        m1.tags = vec!["network".into(), "retry".into()];
        mem_store.save_memory(m1).await.unwrap();

        let mut m2 = make_memory(MemoryKind::Pattern, "other tags", "p1");
        m2.tags = vec!["database".into()];
        mem_store.save_memory(m2).await.unwrap();

        let filter = MemoryFilter {
            tags: Some(vec!["network".into()]),
            ..Default::default()
        };
        let results = mem_store.query_memory(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "tagged");
    }

    #[tokio::test]
    async fn query_by_project_slug() {
        let (_store, mem_store) = setup().await;

        mem_store
            .save_memory(make_memory(MemoryKind::Decision, "for alpha", "alpha"))
            .await
            .unwrap();
        mem_store
            .save_memory(make_memory(MemoryKind::Decision, "for beta", "beta"))
            .await
            .unwrap();

        let filter = MemoryFilter {
            project_slug: Some("alpha".into()),
            ..Default::default()
        };
        let results = mem_store.query_memory(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].project_slug, "alpha");
    }

    #[tokio::test]
    async fn query_by_text_match() {
        let (_store, mem_store) = setup().await;

        mem_store
            .save_memory(make_memory(
                MemoryKind::Pattern,
                "Connection timeout on port 443",
                "p1",
            ))
            .await
            .unwrap();
        mem_store
            .save_memory(make_memory(
                MemoryKind::Pattern,
                "Successful deployment",
                "p1",
            ))
            .await
            .unwrap();

        let filter = MemoryFilter {
            text_match: Some("TIMEOUT".into()),
            ..Default::default()
        };
        let results = mem_store.query_memory(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("timeout"));
    }

    #[tokio::test]
    async fn forget_existing() {
        let (_store, mem_store) = setup().await;

        let id = mem_store
            .save_memory(make_memory(MemoryKind::UserFlagged, "remove me", "p1"))
            .await
            .unwrap();

        let deleted = mem_store.forget_memory(&id).await.unwrap();
        assert!(deleted);

        let gone = mem_store.get_memory(&id).await.unwrap();
        assert!(gone.is_none());
    }

    #[tokio::test]
    async fn forget_nonexistent() {
        let (_store, mem_store) = setup().await;

        let deleted = mem_store.forget_memory("does_not_exist_42").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn cleanup_isolation_memories_survive_observation_cleanup() {
        use crate::StateModel;
        use daemon8_types::{Observation, ObservationKind, Origin, Severity};

        let (store, mem_store) = setup().await;

        // Insert observations with old timestamps
        let obs = Observation {
            id: 0,
            origin: Origin::Application {
                name: "test".into(),
            },
            kind: ObservationKind::Log,
            data: serde_json::json!({"msg": "old"}),
            severity: Severity::Info,
            source_location: None,
            service: None,
            source: None,
            source_instance: None,
            timestamp_ns: 1_000,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            debug_session_id: None,
            checkpoint_id: None,
            error_hash: None,
        };
        store.insert(&obs).await.unwrap();

        // Insert a memory
        let mem_id = mem_store
            .save_memory(make_memory(MemoryKind::Pattern, "should survive", "p1"))
            .await
            .unwrap();

        // Cleanup observations older than now (removes all observations)
        let deleted = store.cleanup_before(u64::MAX).await.unwrap();
        assert_eq!(deleted, 1);

        // Memory must still be there
        let mem = mem_store.get_memory(&mem_id).await.unwrap();
        assert!(mem.is_some(), "memory must survive observation cleanup");
        assert_eq!(mem.unwrap().content, "should survive");
    }
}
