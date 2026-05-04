// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{MemoryShortFilter, MemoryShortRecord, MemoryShortStore, ShortContentKind, StoreError};

// memory_short schema. Mirrors the envelope-DDL pattern: the constant is
// referenced from both SurrealStore::init_schema (workspace bootstrap) and
// MemoryShortStore::init_schema (standalone construction). `IF NOT EXISTS`
// keeps repeated execution safe.
pub(crate) const MEMORY_SHORT_DDL: &str = "DEFINE TABLE IF NOT EXISTS memory_short SCHEMAFULL;

DEFINE FIELD IF NOT EXISTS content       ON memory_short TYPE string;
DEFINE FIELD IF NOT EXISTS content_kind  ON memory_short TYPE string;
DEFINE FIELD IF NOT EXISTS agent_id      ON memory_short TYPE string;
DEFINE FIELD IF NOT EXISTS thread_id     ON memory_short TYPE option<string>;
DEFINE FIELD IF NOT EXISTS scope         ON memory_short TYPE string;
DEFINE FIELD IF NOT EXISTS tags          ON memory_short TYPE array<string>;
DEFINE FIELD IF NOT EXISTS content_hash  ON memory_short TYPE string;
DEFINE FIELD IF NOT EXISTS expires_at    ON memory_short TYPE int;
DEFINE FIELD IF NOT EXISTS created_at    ON memory_short TYPE int;
DEFINE FIELD IF NOT EXISTS updated_at    ON memory_short TYPE int;
DEFINE FIELD IF NOT EXISTS embedding             ON memory_short TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS embedding_profile_id  ON memory_short TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_short_agent   ON memory_short FIELDS agent_id;
DEFINE INDEX IF NOT EXISTS idx_short_thread  ON memory_short FIELDS thread_id;
DEFINE INDEX IF NOT EXISTS idx_short_expires ON memory_short FIELDS expires_at;
DEFINE INDEX IF NOT EXISTS idx_short_hash    ON memory_short FIELDS content_hash;
DEFINE INDEX IF NOT EXISTS idx_short_scope   ON memory_short FIELDS scope;
DEFINE INDEX IF NOT EXISTS idx_short_emb_pid ON memory_short FIELDS embedding_profile_id;";

pub struct SurrealMemoryShortStore {
    db: Surreal<Db>,
}

impl SurrealMemoryShortStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

    pub(crate) fn build_query_sql(
        filter: &MemoryShortFilter,
    ) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(ref agent) = filter.agent_id {
            conditions.push("agent_id = $agent_id".to_string());
            binds.push(("agent_id".into(), serde_json::json!(agent)));
        }

        if let Some(ref thread) = filter.thread_id {
            conditions.push("thread_id = $thread_id_v".to_string());
            binds.push(("thread_id_v".into(), serde_json::json!(thread)));
        }

        if let Some(scope) = filter.scope {
            conditions.push("scope = $scope_v".to_string());
            binds.push(("scope_v".into(), serde_json::json!(scope.to_string())));
        }

        if let Some(ref kinds) = filter.content_kinds
            && !kinds.is_empty()
        {
            let strs: Vec<String> = kinds.iter().map(ShortContentKind::to_string).collect();
            conditions.push("content_kind IN $allowed_kinds".to_string());
            binds.push(("allowed_kinds".into(), serde_json::json!(strs)));
        }

        if let Some(ref tags) = filter.tags_any
            && !tags.is_empty()
        {
            conditions.push("tags ANYINSIDE $any_tags".to_string());
            binds.push(("any_tags".into(), serde_json::json!(tags)));
        }

        if let Some(ref pid) = filter.embedding_profile_id {
            conditions.push("embedding_profile_id = $emb_pid".to_string());
            binds.push(("emb_pid".into(), serde_json::json!(pid)));
        }

        if !filter.include_expired {
            conditions.push("expires_at > $now_ns".to_string());
            let now = filter
                .now_ns
                .unwrap_or_else(|| current_unix_nanos().unwrap_or(u64::MAX));
            binds.push(("now_ns".into(), serde_json::json!(now)));
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

        let sql = format!(
            "SELECT * FROM memory_short{where_clause} ORDER BY created_at ASC{limit_clause}"
        );

        (sql, binds)
    }
}

fn current_unix_nanos() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos() as u64)
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

fn decode(mut val: serde_json::Value) -> Result<MemoryShortRecord, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

#[async_trait::async_trait]
impl MemoryShortStore for SurrealMemoryShortStore {
    async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .query(MEMORY_SHORT_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("memory_short schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_short schema init check: {e}")))?;
        Ok(())
    }

    async fn save(&self, record: MemoryShortRecord) -> Result<String, StoreError> {
        let mut content = serde_json::to_value(&record)?;
        if let serde_json::Value::Object(ref mut obj) = content {
            obj.remove("id");
        }

        let explicit_id = record
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        match explicit_id {
            Some(id) => {
                let mut result = self
                    .db
                    .query("UPSERT type::record('memory_short', $id) CONTENT $content RETURN AFTER")
                    .bind(("id", serde_json::json!(id)))
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("memory_short upsert: {e}")))?;

                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("memory_short upsert read: {e}")))?;

                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("memory_short save: no id returned".into()))
            }
            None => {
                let mut result = self
                    .db
                    .query("CREATE memory_short CONTENT $content")
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("memory_short create: {e}")))?;

                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("memory_short create read: {e}")))?;

                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("memory_short save: no id returned".into()))
            }
        }
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryShortRecord>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('memory_short', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_short get: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_short get read: {e}")))?;

        row.map(decode).transpose()
    }

    async fn list(&self, filter: &MemoryShortFilter) -> Result<Vec<MemoryShortRecord>, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);

        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("memory_short list: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_short list read: {e}")))?;

        rows.into_iter().map(decode).collect()
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let exists = self.get(id).await?.is_some();

        self.db
            .query("DELETE type::record('memory_short', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_short delete: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_short delete check: {e}")))?;

        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryScope, ShortContentKind, SurrealStore};

    async fn setup() -> (SurrealStore, SurrealMemoryShortStore) {
        let store = SurrealStore::memory().await.unwrap();
        let s = store.memory_short_store();
        s.init_schema().await.unwrap();
        (store, s)
    }

    fn make(
        agent: &str,
        content: &str,
        kind: ShortContentKind,
        created_at: u64,
        expires_at: u64,
    ) -> MemoryShortRecord {
        MemoryShortRecord {
            id: None,
            content: content.into(),
            content_kind: kind,
            agent_id: agent.into(),
            thread_id: None,
            scope: MemoryScope::Agent,
            tags: vec![],
            content_hash: format!("h-{content}"),
            expires_at,
            created_at,
            updated_at: created_at,
            embedding: None,
            embedding_profile_id: None,
        }
    }

    #[tokio::test]
    async fn schema_init_is_idempotent() {
        let (_s, store) = setup().await;
        store.init_schema().await.unwrap();
        store.init_schema().await.unwrap();
    }

    #[tokio::test]
    async fn crud_roundtrip_preserves_all_fields() {
        let (_s, store) = setup().await;

        let mut rec = make(
            "alice",
            "scratchpad note",
            ShortContentKind::Scratch,
            100,
            999_999,
        );
        rec.thread_id = Some("thread-1".into());
        rec.scope = MemoryScope::Team;
        rec.tags = vec!["t1".into(), "t2".into()];

        let id = store.save(rec.clone()).await.unwrap();
        assert!(!id.is_empty());

        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.content, rec.content);
        assert_eq!(got.content_kind, rec.content_kind);
        assert_eq!(got.agent_id, rec.agent_id);
        assert_eq!(got.thread_id, rec.thread_id);
        assert_eq!(got.scope, rec.scope);
        let mut got_tags = got.tags.clone();
        got_tags.sort();
        let mut want = rec.tags.clone();
        want.sort();
        assert_eq!(got_tags, want);
        assert_eq!(got.content_hash, rec.content_hash);
        assert_eq!(got.expires_at, rec.expires_at);
        assert_eq!(got.created_at, rec.created_at);
    }

    #[tokio::test]
    async fn list_filters_by_agent_id() {
        let (_s, store) = setup().await;
        store
            .save(make("alice", "a1", ShortContentKind::Fact, 10, 999_999))
            .await
            .unwrap();
        store
            .save(make("alice", "a2", ShortContentKind::Fact, 20, 999_999))
            .await
            .unwrap();
        store
            .save(make("bob", "b1", ShortContentKind::Fact, 30, 999_999))
            .await
            .unwrap();

        let alices = store
            .list(&MemoryShortFilter {
                agent_id: Some("alice".into()),
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(alices.len(), 2);
        assert!(alices.iter().all(|r| r.agent_id == "alice"));
    }

    #[tokio::test]
    async fn list_filters_by_content_kinds() {
        let (_s, store) = setup().await;
        store
            .save(make("a", "scratch", ShortContentKind::Scratch, 1, 999_999))
            .await
            .unwrap();
        store
            .save(make("a", "fact1", ShortContentKind::Fact, 2, 999_999))
            .await
            .unwrap();
        store
            .save(make("a", "fact2", ShortContentKind::Fact, 3, 999_999))
            .await
            .unwrap();
        store
            .save(make("a", "summary", ShortContentKind::Summary, 4, 999_999))
            .await
            .unwrap();

        let facts = store
            .list(&MemoryShortFilter {
                content_kinds: Some(vec![ShortContentKind::Fact]),
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(facts.len(), 2);

        let multi = store
            .list(&MemoryShortFilter {
                content_kinds: Some(vec![ShortContentKind::Fact, ShortContentKind::Summary]),
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(multi.len(), 3);
    }

    #[tokio::test]
    async fn list_filters_by_thread_id() {
        let (_s, store) = setup().await;
        let mut r1 = make("a", "in-thread", ShortContentKind::Fact, 1, 999_999);
        r1.thread_id = Some("t-abc".into());
        store.save(r1).await.unwrap();
        store
            .save(make("a", "no-thread", ShortContentKind::Fact, 2, 999_999))
            .await
            .unwrap();

        let in_thread = store
            .list(&MemoryShortFilter {
                thread_id: Some("t-abc".into()),
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(in_thread.len(), 1);
        assert_eq!(in_thread[0].thread_id.as_deref(), Some("t-abc"));
    }

    #[tokio::test]
    async fn list_filters_by_tags_any() {
        let (_s, store) = setup().await;
        let mut r1 = make("a", "x", ShortContentKind::Fact, 1, 999_999);
        r1.tags = vec!["alpha".into(), "beta".into()];
        store.save(r1).await.unwrap();
        let mut r2 = make("a", "y", ShortContentKind::Fact, 2, 999_999);
        r2.tags = vec!["beta".into()];
        store.save(r2).await.unwrap();
        let mut r3 = make("a", "z", ShortContentKind::Fact, 3, 999_999);
        r3.tags = vec!["gamma".into()];
        store.save(r3).await.unwrap();

        let any = store
            .list(&MemoryShortFilter {
                tags_any: Some(vec!["alpha".into(), "gamma".into()]),
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(any.len(), 2);
    }

    #[tokio::test]
    async fn list_excludes_expired_by_default_and_includes_when_asked() {
        let (_s, store) = setup().await;
        store
            .save(make("a", "live", ShortContentKind::Fact, 1, 1_000))
            .await
            .unwrap();
        store
            .save(make("a", "dead", ShortContentKind::Fact, 2, 50))
            .await
            .unwrap();

        let live_only = store
            .list(&MemoryShortFilter {
                now_ns: Some(500),
                include_expired: false,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(live_only.len(), 1);
        assert_eq!(live_only[0].content, "live");

        let with_expired = store
            .list(&MemoryShortFilter {
                now_ns: Some(500),
                include_expired: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(with_expired.len(), 2);
    }

    #[tokio::test]
    async fn embedding_fields_roundtrip_when_set() {
        let (_s, store) = setup().await;
        let mut r = make("a", "v", ShortContentKind::Fact, 1, 999_999);
        r.embedding = Some(vec![0.1, 0.2, 0.3]);
        r.embedding_profile_id = Some("openai:text-embedding-3-small".into());
        let id = store.save(r.clone()).await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.embedding, r.embedding);
        assert_eq!(got.embedding_profile_id, r.embedding_profile_id);
    }

    #[tokio::test]
    async fn list_filters_by_embedding_profile_id() {
        let (_s, store) = setup().await;
        let mut r1 = make("a", "with", ShortContentKind::Fact, 1, 999_999);
        r1.embedding_profile_id = Some("openai:small".into());
        store.save(r1).await.unwrap();
        let mut r2 = make("a", "different", ShortContentKind::Fact, 2, 999_999);
        r2.embedding_profile_id = Some("local:onnx".into());
        store.save(r2).await.unwrap();
        store
            .save(make("a", "no-pid", ShortContentKind::Fact, 3, 999_999))
            .await
            .unwrap();

        let openai_only = store
            .list(&MemoryShortFilter {
                embedding_profile_id: Some("openai:small".into()),
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(openai_only.len(), 1);
        assert_eq!(openai_only[0].content, "with");
    }

    #[tokio::test]
    async fn delete_returns_true_when_existed_false_otherwise() {
        let (_s, store) = setup().await;
        let id = store
            .save(make("a", "x", ShortContentKind::Scratch, 1, 999_999))
            .await
            .unwrap();
        assert!(store.delete(&id).await.unwrap());
        assert!(!store.delete(&id).await.unwrap());
    }
}
