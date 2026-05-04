// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{
    MemoryReferenceFilter, MemoryReferenceRecord, MemoryReferenceStore, ReferenceSourceKind,
    StoreError,
};

pub(crate) const MEMORY_REFERENCE_DDL: &str =
    "DEFINE TABLE IF NOT EXISTS memory_reference SCHEMAFULL;

DEFINE FIELD IF NOT EXISTS content      ON memory_reference TYPE string;
DEFINE FIELD IF NOT EXISTS source_kind  ON memory_reference TYPE string;
DEFINE FIELD IF NOT EXISTS source_url   ON memory_reference TYPE option<string>;
DEFINE FIELD IF NOT EXISTS source_hash  ON memory_reference TYPE string;
DEFINE FIELD IF NOT EXISTS scope        ON memory_reference TYPE string;
DEFINE FIELD IF NOT EXISTS tags         ON memory_reference TYPE array<string>;
DEFINE FIELD IF NOT EXISTS content_hash ON memory_reference TYPE string;
DEFINE FIELD IF NOT EXISTS refreshed_at ON memory_reference TYPE int;
DEFINE FIELD IF NOT EXISTS created_at   ON memory_reference TYPE int;
DEFINE FIELD IF NOT EXISTS updated_at   ON memory_reference TYPE int;
DEFINE FIELD IF NOT EXISTS embedding             ON memory_reference TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS embedding_profile_id  ON memory_reference TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_ref_source_hash ON memory_reference FIELDS source_hash;
DEFINE INDEX IF NOT EXISTS idx_ref_scope       ON memory_reference FIELDS scope;
DEFINE INDEX IF NOT EXISTS idx_ref_refreshed   ON memory_reference FIELDS refreshed_at;
DEFINE INDEX IF NOT EXISTS idx_ref_kind        ON memory_reference FIELDS source_kind;
DEFINE INDEX IF NOT EXISTS idx_ref_emb_pid     ON memory_reference FIELDS embedding_profile_id;";

pub struct SurrealMemoryReferenceStore {
    db: Surreal<Db>,
}

impl SurrealMemoryReferenceStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

    pub(crate) fn build_query_sql(
        filter: &MemoryReferenceFilter,
    ) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(ref kinds) = filter.source_kinds
            && !kinds.is_empty()
        {
            let strs: Vec<String> = kinds.iter().map(ReferenceSourceKind::to_string).collect();
            conditions.push("source_kind IN $allowed_kinds".to_string());
            binds.push(("allowed_kinds".into(), serde_json::json!(strs)));
        }

        if let Some(scope) = filter.scope {
            conditions.push("scope = $scope_v".to_string());
            binds.push(("scope_v".into(), serde_json::json!(scope.to_string())));
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
            "SELECT * FROM memory_reference{where_clause} ORDER BY created_at ASC{limit_clause}"
        );

        (sql, binds)
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

fn decode(mut val: serde_json::Value) -> Result<MemoryReferenceRecord, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

#[async_trait::async_trait]
impl MemoryReferenceStore for SurrealMemoryReferenceStore {
    async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .query(MEMORY_REFERENCE_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_reference schema init check: {e}")))?;
        Ok(())
    }

    async fn save(&self, record: MemoryReferenceRecord) -> Result<String, StoreError> {
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
                    .query(
                        "UPSERT type::record('memory_reference', $id) CONTENT $content RETURN AFTER",
                    )
                    .bind(("id", serde_json::json!(id)))
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("memory_reference upsert: {e}")))?;
                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("memory_reference upsert read: {e}")))?;
                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("memory_reference save: no id returned".into()))
            }
            None => {
                let mut result = self
                    .db
                    .query("CREATE memory_reference CONTENT $content")
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("memory_reference create: {e}")))?;
                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("memory_reference create read: {e}")))?;
                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("memory_reference save: no id returned".into()))
            }
        }
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryReferenceRecord>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('memory_reference', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference get: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_reference get read: {e}")))?;

        row.map(decode).transpose()
    }

    async fn list(
        &self,
        filter: &MemoryReferenceFilter,
    ) -> Result<Vec<MemoryReferenceRecord>, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);
        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }
        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference list: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_reference list read: {e}")))?;
        rows.into_iter().map(decode).collect()
    }

    async fn find_by_source_hash(
        &self,
        source_hash: &str,
    ) -> Result<Option<MemoryReferenceRecord>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM memory_reference WHERE source_hash = $h LIMIT 1")
            .bind(("h", serde_json::json!(source_hash)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference find_by_source_hash: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_reference find read: {e}")))?;

        rows.into_iter().next().map(decode).transpose()
    }

    async fn mark_refreshed(&self, id: &str, refreshed_at: u64) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query(
                "UPDATE type::record('memory_reference', $id) \
                 SET refreshed_at = $at, updated_at = $at \
                 RETURN AFTER",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(refreshed_at)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference mark_refreshed: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_reference mark_refreshed read: {e}")))?;

        if rows.is_empty() {
            return Err(StoreError::Other(format!(
                "memory_reference '{id}' not found"
            )));
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let exists = self.get(id).await?.is_some();
        self.db
            .query("DELETE type::record('memory_reference', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference delete: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_reference delete check: {e}")))?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryScope, ReferenceSourceKind, SurrealStore};

    async fn setup() -> (SurrealStore, SurrealMemoryReferenceStore) {
        let store = SurrealStore::memory().await.unwrap();
        let s = store.memory_reference_store();
        s.init_schema().await.unwrap();
        (store, s)
    }

    fn make(content: &str, kind: ReferenceSourceKind, source_hash: &str) -> MemoryReferenceRecord {
        MemoryReferenceRecord {
            id: None,
            content: content.into(),
            source_kind: kind,
            source_url: None,
            source_hash: source_hash.into(),
            scope: MemoryScope::Project,
            tags: vec![],
            content_hash: format!("ch-{content}"),
            refreshed_at: 100,
            created_at: 100,
            updated_at: 100,
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
        let mut r = make("rust docs excerpt", ReferenceSourceKind::Doc, "sh-abc");
        r.source_url = Some("https://doc.rust-lang.org/std".into());
        r.tags = vec!["rust".into(), "std".into()];
        r.scope = MemoryScope::Global;

        let id = store.save(r.clone()).await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();

        assert_eq!(got.content, r.content);
        assert_eq!(got.source_kind, r.source_kind);
        assert_eq!(got.source_url, r.source_url);
        assert_eq!(got.source_hash, r.source_hash);
        assert_eq!(got.scope, r.scope);
        assert_eq!(got.refreshed_at, r.refreshed_at);
    }

    #[tokio::test]
    async fn find_by_source_hash_returns_matching_row() {
        let (_s, store) = setup().await;
        store
            .save(make("a", ReferenceSourceKind::Doc, "h-a"))
            .await
            .unwrap();
        store
            .save(make("b", ReferenceSourceKind::Doc, "h-b"))
            .await
            .unwrap();

        let hit = store.find_by_source_hash("h-b").await.unwrap();
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().content, "b");

        let miss = store.find_by_source_hash("h-missing").await.unwrap();
        assert!(miss.is_none());
    }

    #[tokio::test]
    async fn mark_refreshed_updates_refreshed_and_updated() {
        let (_s, store) = setup().await;
        let id = store
            .save(make("doc", ReferenceSourceKind::Doc, "h-doc"))
            .await
            .unwrap();

        store.mark_refreshed(&id, 555).await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.refreshed_at, 555);
        assert_eq!(got.updated_at, 555);
    }

    #[tokio::test]
    async fn mark_refreshed_on_missing_id_errors() {
        let (_s, store) = setup().await;
        let err = store
            .mark_refreshed("ghost", 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost"));
    }

    #[tokio::test]
    async fn embedding_fields_roundtrip_when_set() {
        let (_s, store) = setup().await;
        let mut r = make("note", ReferenceSourceKind::Note, "h-note");
        r.embedding = Some(vec![0.5, -0.5]);
        r.embedding_profile_id = Some("local:onnx-small".into());
        let id = store.save(r.clone()).await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.embedding, r.embedding);
        assert_eq!(got.embedding_profile_id, r.embedding_profile_id);
    }

    #[tokio::test]
    async fn list_filters_by_embedding_profile_id() {
        let (_s, store) = setup().await;
        let mut a = make("a", ReferenceSourceKind::Doc, "h1");
        a.embedding_profile_id = Some("openai:small".into());
        store.save(a).await.unwrap();
        let mut b = make("b", ReferenceSourceKind::Doc, "h2");
        b.embedding_profile_id = Some("local:onnx".into());
        store.save(b).await.unwrap();

        let openai = store
            .list(&MemoryReferenceFilter {
                embedding_profile_id: Some("openai:small".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(openai.len(), 1);
        assert_eq!(openai[0].content, "a");
    }

    #[tokio::test]
    async fn list_filters_by_source_kind_and_scope() {
        let (_s, store) = setup().await;
        let mut a = make("rfc-x", ReferenceSourceKind::Rfc, "h1");
        a.scope = MemoryScope::Global;
        store.save(a).await.unwrap();
        let mut b = make("doc-y", ReferenceSourceKind::Doc, "h2");
        b.scope = MemoryScope::Project;
        store.save(b).await.unwrap();
        let mut c = make("doc-z", ReferenceSourceKind::Doc, "h3");
        c.scope = MemoryScope::Global;
        store.save(c).await.unwrap();

        let docs_global = store
            .list(&MemoryReferenceFilter {
                source_kinds: Some(vec![ReferenceSourceKind::Doc]),
                scope: Some(MemoryScope::Global),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(docs_global.len(), 1);
        assert_eq!(docs_global[0].content, "doc-z");
    }
}
