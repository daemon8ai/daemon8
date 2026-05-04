// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{
    LongContentKind, MemoryLongFilter, MemoryLongRecord, MemoryLongStore, MemoryScope, StoreError,
};

pub(crate) const MEMORY_LONG_DDL: &str = "DEFINE TABLE IF NOT EXISTS memory_long SCHEMAFULL;

DEFINE FIELD IF NOT EXISTS content      ON memory_long TYPE string;
DEFINE FIELD IF NOT EXISTS content_kind ON memory_long TYPE string;
DEFINE FIELD IF NOT EXISTS scope        ON memory_long TYPE string;
DEFINE FIELD IF NOT EXISTS tags         ON memory_long TYPE array<string>;
DEFINE FIELD IF NOT EXISTS content_hash ON memory_long TYPE string;
DEFINE FIELD IF NOT EXISTS provenance   ON memory_long TYPE array;
DEFINE FIELD IF NOT EXISTS provenance.*  ON memory_long TYPE object FLEXIBLE;
DEFINE FIELD IF NOT EXISTS confidence   ON memory_long TYPE float;
DEFINE FIELD IF NOT EXISTS supersedes   ON memory_long TYPE option<string>;
DEFINE FIELD IF NOT EXISTS revoked_at   ON memory_long TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_at   ON memory_long TYPE int;
DEFINE FIELD IF NOT EXISTS updated_at   ON memory_long TYPE int;

DEFINE INDEX IF NOT EXISTS idx_long_scope      ON memory_long FIELDS scope;
DEFINE INDEX IF NOT EXISTS idx_long_hash       ON memory_long FIELDS content_hash;
DEFINE INDEX IF NOT EXISTS idx_long_supersedes ON memory_long FIELDS supersedes;
DEFINE INDEX IF NOT EXISTS idx_long_revoked    ON memory_long FIELDS revoked_at;
DEFINE INDEX IF NOT EXISTS idx_long_kind       ON memory_long FIELDS content_kind;";

pub struct SurrealMemoryLongStore {
    db: Surreal<Db>,
}

impl SurrealMemoryLongStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

    pub(crate) fn build_query_sql(
        filter: &MemoryLongFilter,
    ) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(scope) = filter.scope {
            conditions.push("scope = $scope_v".to_string());
            binds.push(("scope_v".into(), serde_json::json!(scope.to_string())));
        }

        if let Some(ref kinds) = filter.content_kinds
            && !kinds.is_empty()
        {
            let strs: Vec<String> = kinds.iter().map(LongContentKind::to_string).collect();
            conditions.push("content_kind IN $allowed_kinds".to_string());
            binds.push(("allowed_kinds".into(), serde_json::json!(strs)));
        }

        if let Some(ref tags) = filter.tags_any
            && !tags.is_empty()
        {
            conditions.push("tags ANYINSIDE $any_tags".to_string());
            binds.push(("any_tags".into(), serde_json::json!(tags)));
        }

        if !filter.include_revoked {
            conditions.push("revoked_at IS NONE".to_string());
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
            "SELECT * FROM memory_long{where_clause} ORDER BY created_at ASC{limit_clause}"
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

fn decode(mut val: serde_json::Value) -> Result<MemoryLongRecord, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

#[async_trait::async_trait]
impl MemoryLongStore for SurrealMemoryLongStore {
    async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .query(MEMORY_LONG_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("memory_long schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_long schema init check: {e}")))?;
        Ok(())
    }

    async fn save(&self, record: MemoryLongRecord) -> Result<String, StoreError> {
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
                    .query("UPSERT type::record('memory_long', $id) CONTENT $content RETURN AFTER")
                    .bind(("id", serde_json::json!(id)))
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("memory_long upsert: {e}")))?;
                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("memory_long upsert read: {e}")))?;
                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("memory_long save: no id returned".into()))
            }
            None => {
                let mut result = self
                    .db
                    .query("CREATE memory_long CONTENT $content")
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("memory_long create: {e}")))?;
                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("memory_long create read: {e}")))?;
                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("memory_long save: no id returned".into()))
            }
        }
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryLongRecord>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('memory_long', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_long get: {e}")))?;
        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_long get read: {e}")))?;
        row.map(decode).transpose()
    }

    async fn list(&self, filter: &MemoryLongFilter) -> Result<Vec<MemoryLongRecord>, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);
        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }
        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("memory_long list: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_long list read: {e}")))?;
        rows.into_iter().map(decode).collect()
    }

    async fn find_by_content_hash(
        &self,
        content_hash: &str,
        scope: Option<MemoryScope>,
    ) -> Result<Vec<MemoryLongRecord>, StoreError> {
        let (sql, binds) = match scope {
            Some(s) => (
                "SELECT * FROM memory_long WHERE content_hash = $h AND scope = $s ORDER BY created_at ASC".to_string(),
                vec![
                    ("h".to_string(), serde_json::json!(content_hash)),
                    ("s".to_string(), serde_json::json!(s.to_string())),
                ],
            ),
            None => (
                "SELECT * FROM memory_long WHERE content_hash = $h ORDER BY created_at ASC"
                    .to_string(),
                vec![("h".to_string(), serde_json::json!(content_hash))],
            ),
        };

        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }
        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("memory_long find_by_content_hash: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_long find read: {e}")))?;
        rows.into_iter().map(decode).collect()
    }

    async fn supersede(&self, old_id: &str, new_id: &str, at_ns: u64) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query(
                "UPDATE type::record('memory_long', $new_id) \
                 SET supersedes = $old_id, updated_at = $at \
                 RETURN AFTER",
            )
            .bind(("new_id", serde_json::json!(new_id)))
            .bind(("old_id", serde_json::json!(old_id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_long supersede: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_long supersede read: {e}")))?;

        if rows.is_empty() {
            return Err(StoreError::Other(format!(
                "memory_long '{new_id}' not found"
            )));
        }
        Ok(())
    }

    async fn revoke(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query(
                "UPDATE type::record('memory_long', $id) \
                 SET revoked_at = $at, updated_at = $at \
                 WHERE revoked_at IS NONE RETURN AFTER",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_long revoke: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory_long revoke read: {e}")))?;

        if rows.is_empty() {
            return Err(StoreError::Other(format!(
                "memory_long '{id}' not found or already revoked"
            )));
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let exists = self.get(id).await?.is_some();
        self.db
            .query("DELETE type::record('memory_long', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("memory_long delete: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_long delete check: {e}")))?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LongContentKind, MemoryScope, ProvenanceEntry, SurrealStore};

    async fn setup() -> (SurrealStore, SurrealMemoryLongStore) {
        let store = SurrealStore::memory().await.unwrap();
        let s = store.memory_long_store();
        s.init_schema().await.unwrap();
        (store, s)
    }

    fn make(content: &str, kind: LongContentKind, content_hash: &str) -> MemoryLongRecord {
        MemoryLongRecord {
            id: None,
            content: content.into(),
            content_kind: kind,
            scope: MemoryScope::Project,
            tags: vec![],
            content_hash: content_hash.into(),
            provenance: vec![],
            confidence: 0.5,
            supersedes: None,
            revoked_at: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    #[tokio::test]
    async fn schema_init_is_idempotent() {
        let (_s, store) = setup().await;
        store.init_schema().await.unwrap();
        store.init_schema().await.unwrap();
    }

    #[tokio::test]
    async fn crud_roundtrip_preserves_provenance_and_confidence() {
        let (_s, store) = setup().await;
        let mut r = make(
            "the api returns paginated by default",
            LongContentKind::Fact,
            "h1",
        );
        r.confidence = 0.9;
        r.provenance = vec![
            ProvenanceEntry {
                source_kind: "envelope".into(),
                source_id: "env_abc".into(),
                validated_at: 500,
                validated_by: "agent:bookkeeper".into(),
            },
            ProvenanceEntry {
                source_kind: "memory_short".into(),
                source_id: "ms_xyz".into(),
                validated_at: 600,
                validated_by: "agent:bookkeeper".into(),
            },
        ];
        r.tags = vec!["api".into()];

        let id = store.save(r.clone()).await.unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.content, r.content);
        assert_eq!(got.content_kind, r.content_kind);
        assert_eq!(got.scope, r.scope);
        assert!((got.confidence - r.confidence).abs() < 1e-6);
        assert_eq!(got.provenance.len(), 2);
        assert_eq!(got.provenance[0].source_kind, "envelope");
        assert_eq!(got.provenance[1].source_id, "ms_xyz");
    }

    #[tokio::test]
    async fn find_by_content_hash_returns_matching_rows() {
        let (_s, store) = setup().await;
        store
            .save(make("a", LongContentKind::Fact, "shared"))
            .await
            .unwrap();
        store
            .save(make("b", LongContentKind::Fact, "shared"))
            .await
            .unwrap();
        store
            .save(make("c", LongContentKind::Fact, "different"))
            .await
            .unwrap();

        let shared = store.find_by_content_hash("shared", None).await.unwrap();
        assert_eq!(shared.len(), 2);

        let none = store.find_by_content_hash("missing", None).await.unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn find_by_content_hash_respects_scope() {
        let (_s, store) = setup().await;
        let mut a = make("a", LongContentKind::Fact, "h");
        a.scope = MemoryScope::Agent;
        store.save(a).await.unwrap();
        let mut b = make("b", LongContentKind::Fact, "h");
        b.scope = MemoryScope::Project;
        store.save(b).await.unwrap();

        let agent_only = store
            .find_by_content_hash("h", Some(MemoryScope::Agent))
            .await
            .unwrap();
        assert_eq!(agent_only.len(), 1);
        assert_eq!(agent_only[0].scope, MemoryScope::Agent);
    }

    #[tokio::test]
    async fn supersede_links_old_into_new() {
        let (_s, store) = setup().await;
        let old = store
            .save(make("old fact", LongContentKind::Fact, "h-old"))
            .await
            .unwrap();
        let newer = store
            .save(make("new fact", LongContentKind::Fact, "h-new"))
            .await
            .unwrap();

        store.supersede(&old, &newer, 999).await.unwrap();
        let got = store.get(&newer).await.unwrap().unwrap();
        assert_eq!(got.supersedes.as_deref(), Some(old.as_str()));
        assert_eq!(got.updated_at, 999);
    }

    #[tokio::test]
    async fn revoke_marks_revoked_at() {
        let (_s, store) = setup().await;
        let id = store
            .save(make("x", LongContentKind::Fact, "h"))
            .await
            .unwrap();
        store.revoke(&id, 777).await.unwrap();

        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.revoked_at, Some(777));

        let again = store.revoke(&id, 888).await;
        assert!(again.is_err(), "second revoke must fail (already revoked)");
    }

    #[tokio::test]
    async fn list_excludes_revoked_by_default_and_includes_when_asked() {
        let (_s, store) = setup().await;
        let live = store
            .save(make("live", LongContentKind::Fact, "h-l"))
            .await
            .unwrap();
        let dead = store
            .save(make("dead", LongContentKind::Fact, "h-d"))
            .await
            .unwrap();
        store.revoke(&dead, 100).await.unwrap();

        let live_only = store.list(&MemoryLongFilter::default()).await.unwrap();
        assert_eq!(live_only.len(), 1);
        assert_eq!(live_only[0].id.as_deref(), Some(live.as_str()));

        let all = store
            .list(&MemoryLongFilter {
                include_revoked: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
    }
}
