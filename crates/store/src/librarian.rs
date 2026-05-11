// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{LibrarianEdge, LibrarianFilter, LibrarianNode, LibrarianStore, StoreError};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

pub struct SurrealLibrarianStore {
    db: Surreal<Db>,
}

impl SurrealLibrarianStore {
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
                "DEFINE TABLE IF NOT EXISTS catalog_node SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS kind          ON catalog_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS label         ON catalog_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS locator_kind  ON catalog_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS locator       ON catalog_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS tags          ON catalog_node TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS project_slug  ON catalog_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS version       ON catalog_node TYPE int;
                 DEFINE FIELD IF NOT EXISTS parent_id     ON catalog_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS created_at    ON catalog_node TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at    ON catalog_node TYPE int;
                 DEFINE FIELD IF NOT EXISTS last_read_at  ON catalog_node TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS deprecated_at ON catalog_node TYPE option<int>;

                 DEFINE INDEX IF NOT EXISTS idx_cn_kind       ON catalog_node FIELDS kind;
                 DEFINE INDEX IF NOT EXISTS idx_cn_project    ON catalog_node FIELDS project_slug;
                 DEFINE INDEX IF NOT EXISTS idx_cn_tags       ON catalog_node FIELDS tags;
                 DEFINE INDEX IF NOT EXISTS idx_cn_locator    ON catalog_node FIELDS locator_kind, locator;
                 DEFINE INDEX IF NOT EXISTS idx_cn_parent     ON catalog_node FIELDS parent_id;
                 DEFINE INDEX IF NOT EXISTS idx_cn_deprecated ON catalog_node FIELDS deprecated_at;

                 DEFINE TABLE IF NOT EXISTS catalog_edge SCHEMAFULL TYPE RELATION
                   FROM catalog_node TO catalog_node;
                 DEFINE FIELD IF NOT EXISTS kind       ON catalog_edge TYPE string;
                 DEFINE FIELD IF NOT EXISTS created_at ON catalog_edge TYPE int;

                 DEFINE INDEX IF NOT EXISTS idx_ce_kind ON catalog_edge FIELDS kind;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("librarian schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("librarian schema init check: {e}")))?;

        Ok(())
    }

    fn build_query_sql(filter: &LibrarianFilter) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if !filter.include_deprecated {
            conditions.push("deprecated_at IS NONE".to_string());
        }

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

        if let Some(ref text) = filter.text_match {
            conditions.push(
                "(string::contains(string::lowercase(label), $text_lower) OR string::contains(string::lowercase(locator), $text_lower))"
                    .to_string(),
            );
            binds.push((
                "text_lower".into(),
                serde_json::json!(text.to_ascii_lowercase()),
            ));
        }

        if let Some(threshold) = filter.stale_before {
            conditions
                .push("(last_read_at IS NONE OR last_read_at < $stale_threshold)".to_string());
            binds.push(("stale_threshold".into(), serde_json::json!(threshold)));
        }

        if let Some(ref pid) = filter.parent_id {
            conditions.push("parent_id = $parent_id".to_string());
            binds.push(("parent_id".into(), serde_json::json!(pid)));
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
            "SELECT * FROM catalog_node{where_clause} ORDER BY updated_at DESC{limit_clause}"
        );

        (sql, binds)
    }
}

fn strip_table_prefix(raw: &str, table: &str) -> String {
    raw.strip_prefix(&format!("{table}:"))
        .unwrap_or(raw)
        .to_string()
}

fn extract_record_id(val: &serde_json::Value, table: &str) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(strip_table_prefix(s, table)),
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

fn patch_node_id(val: &mut serde_json::Value) {
    if let Some(id_val) = val.get("id")
        && let Some(bare) = extract_record_id(id_val, "catalog_node")
    {
        val["id"] = serde_json::Value::String(bare);
    }
}

fn parse_node(mut val: serde_json::Value) -> Result<LibrarianNode, StoreError> {
    patch_node_id(&mut val);
    serde_json::from_value(val).map_err(StoreError::from)
}

fn parse_edge(mut val: serde_json::Value) -> Result<LibrarianEdge, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(bare) = extract_record_id(id_val, "catalog_edge")
    {
        val["id"] = serde_json::Value::String(bare);
    }
    if let Some(in_val) = val.get("in")
        && let Some(bare) = extract_record_id(in_val, "catalog_node")
    {
        val["from_node"] = serde_json::Value::String(bare);
    }
    if let Some(out_val) = val.get("out")
        && let Some(bare) = extract_record_id(out_val, "catalog_node")
    {
        val["to_node"] = serde_json::Value::String(bare);
    }
    // SurrealDB relation records carry `in`/`out` which don't map to our struct
    if let Some(obj) = val.as_object_mut() {
        obj.remove("in");
        obj.remove("out");
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

#[async_trait::async_trait]
impl LibrarianStore for SurrealLibrarianStore {
    async fn index_node(&self, mut node: LibrarianNode) -> Result<String, StoreError> {
        // Check for existing non-deprecated node with same locator
        let mut existing = self
            .db
            .query(
                "SELECT * FROM catalog_node WHERE locator_kind = $lk AND locator = $loc AND deprecated_at IS NONE LIMIT 1",
            )
            .bind(("lk", node.locator_kind.to_string()))
            .bind(("loc", node.locator.clone()))
            .await
            .map_err(|e| StoreError::Db(format!("index_node check existing: {e}")))?;

        let old_row: Option<serde_json::Value> = existing
            .take(0)
            .map_err(|e| StoreError::Db(format!("index_node read existing: {e}")))?;

        let mut old_id = None;
        if let Some(ref row) = old_row
            && let Some(bare) = row
                .get("id")
                .and_then(|v| extract_record_id(v, "catalog_node"))
        {
            let old_version: u32 = row.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            node.version = old_version + 1;
            old_id = Some(bare);
        }

        if node.version == 0 {
            node.version = 1;
        }

        let json_content = serde_json::to_value(&node)?;
        let mut result = self
            .db
            .query("CREATE catalog_node CONTENT $content")
            .bind(("content", json_content))
            .await
            .map_err(|e| StoreError::Db(format!("index_node create: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("index_node read result: {e}")))?;

        let new_id = row
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(|v| extract_record_id(v, "catalog_node"))
            .ok_or_else(|| StoreError::Db("index_node: no id returned".into()))?;

        // Deprecate old and create supersedes edge
        if let Some(ref old) = old_id {
            let now = current_ns();
            self.db
                .query("UPDATE type::record('catalog_node', $id) SET deprecated_at = $now")
                .bind(("id", serde_json::json!(old)))
                .bind(("now", serde_json::json!(now)))
                .await
                .map_err(|e| StoreError::Db(format!("index_node deprecate old: {e}")))?
                .check()
                .map_err(|e| StoreError::Db(format!("index_node deprecate check: {e}")))?;

            let edge = LibrarianEdge {
                id: None,
                kind: daemon8_types::LibrarianEdgeKind::Supersedes,
                from_node: new_id.clone(),
                to_node: old.clone(),
                created_at: now,
            };
            self.index_edge(edge).await?;
        }

        Ok(new_id)
    }

    async fn index_edge(&self, edge: LibrarianEdge) -> Result<String, StoreError> {
        let now = if edge.created_at == 0 {
            current_ns()
        } else {
            edge.created_at
        };

        let sql = format!(
            "RELATE catalog_node:`{}`->catalog_edge->catalog_node:`{}` SET kind = $kind, created_at = $created_at",
            edge.from_node, edge.to_node
        );
        let mut result = self
            .db
            .query(&sql)
            .bind(("kind", edge.kind.to_string()))
            .bind(("created_at", serde_json::json!(now)))
            .await
            .map_err(|e| StoreError::Db(format!("index_edge: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("index_edge read result: {e}")))?;

        let id = row
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(|v| extract_record_id(v, "catalog_edge"))
            .ok_or_else(|| StoreError::Db("index_edge: no id returned".into()))?;

        Ok(id)
    }

    async fn lookup(&self, filter: &LibrarianFilter) -> Result<Vec<LibrarianNode>, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);

        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("lookup: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("lookup read results: {e}")))?;

        let mut nodes = Vec::with_capacity(raw.len());
        for val in raw {
            nodes.push(parse_node(val)?);
        }

        Ok(nodes)
    }

    async fn get_node(&self, id: &str) -> Result<Option<LibrarianNode>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('catalog_node', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_node: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_node read result: {e}")))?;

        match row {
            Some(val) => {
                let node = parse_node(val)?;
                let _ = self.touch_read(id).await;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    async fn get_edges(&self, node_id: &str) -> Result<Vec<LibrarianEdge>, StoreError> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM catalog_edge WHERE in = type::record('catalog_node', $id) OR out = type::record('catalog_node', $id)",
            )
            .bind(("id", serde_json::json!(node_id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_edges: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_edges read results: {e}")))?;

        let mut edges = Vec::with_capacity(raw.len());
        for val in raw {
            edges.push(parse_edge(val)?);
        }

        Ok(edges)
    }

    async fn get_children(&self, parent_id: &str) -> Result<Vec<LibrarianNode>, StoreError> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM catalog_node WHERE parent_id = $pid AND deprecated_at IS NONE ORDER BY label ASC",
            )
            .bind(("pid", serde_json::json!(parent_id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_children: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_children read results: {e}")))?;

        raw.into_iter().map(parse_node).collect()
    }

    async fn touch_read(&self, id: &str) -> Result<(), StoreError> {
        let now = current_ns();
        self.db
            .query("UPDATE type::record('catalog_node', $id) SET last_read_at = $now")
            .bind(("id", serde_json::json!(id)))
            .bind(("now", serde_json::json!(now)))
            .await
            .map_err(|e| StoreError::Db(format!("touch_read: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("touch_read check: {e}")))?;
        Ok(())
    }

    async fn deprecate_node(&self, id: &str) -> Result<bool, StoreError> {
        let exists = self.get_node(id).await?.is_some();
        if !exists {
            return Ok(false);
        }
        let now = current_ns();
        self.db
            .query("UPDATE type::record('catalog_node', $id) SET deprecated_at = $now")
            .bind(("id", serde_json::json!(id)))
            .bind(("now", serde_json::json!(now)))
            .await
            .map_err(|e| StoreError::Db(format!("deprecate_node: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("deprecate_node check: {e}")))?;
        Ok(true)
    }

    async fn forget_node(&self, id: &str) -> Result<bool, StoreError> {
        let exists = self.get_node(id).await?.is_some();
        if !exists {
            return Ok(false);
        }

        // Delete edges first
        self.db
            .query(
                "DELETE catalog_edge WHERE in = type::record('catalog_node', $id) OR out = type::record('catalog_node', $id)",
            )
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("forget_node delete edges: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("forget_node delete edges check: {e}")))?;

        self.db
            .query("DELETE type::record('catalog_node', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("forget_node: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("forget_node check: {e}")))?;

        Ok(true)
    }

    async fn forget_edge(&self, id: &str) -> Result<bool, StoreError> {
        // SurrealDB DELETE on a nonexistent record silently succeeds; check first
        let mut result = self
            .db
            .query("SELECT * FROM type::record('catalog_edge', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("forget_edge check: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("forget_edge read: {e}")))?;

        if row.is_none() {
            return Ok(false);
        }

        self.db
            .query("DELETE type::record('catalog_edge', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("forget_edge: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("forget_edge check: {e}")))?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurrealStore;
    use daemon8_types::{LibrarianEdgeKind, LibrarianNodeKind, LocatorKind};

    async fn setup() -> (SurrealStore, SurrealLibrarianStore) {
        let store = SurrealStore::memory().await.unwrap();
        let lib_store = store.librarian_store();
        lib_store.init_schema().await.unwrap();
        (store, lib_store)
    }

    fn make_node(
        kind: LibrarianNodeKind,
        label: &str,
        locator: &str,
        project: &str,
    ) -> LibrarianNode {
        let now = current_ns();
        LibrarianNode {
            id: None,
            kind,
            label: label.to_string(),
            locator_kind: LocatorKind::File,
            locator: locator.to_string(),
            tags: vec![],
            project_slug: project.to_string(),
            version: 0,
            parent_id: None,
            created_at: now,
            updated_at: now,
            last_read_at: None,
            deprecated_at: None,
        }
    }

    #[tokio::test]
    async fn index_and_get_round_trip() {
        let (_store, lib) = setup().await;

        let node = make_node(
            LibrarianNodeKind::Doc,
            "SurrealDB docs",
            "/docs/surreal",
            "daemon8",
        );
        let id = lib.index_node(node).await.unwrap();
        assert!(!id.is_empty());

        let fetched = lib.get_node(&id).await.unwrap().unwrap();
        assert_eq!(fetched.label, "SurrealDB docs");
        assert_eq!(fetched.kind, LibrarianNodeKind::Doc);
        assert_eq!(fetched.version, 1);
    }

    #[tokio::test]
    async fn index_edge_and_get_edges() {
        let (_store, lib) = setup().await;

        let proj_id = lib
            .index_node(make_node(
                LibrarianNodeKind::Project,
                "daemon8",
                "/code/daemon8",
                "daemon8",
            ))
            .await
            .unwrap();
        let doc_id = lib
            .index_node(make_node(
                LibrarianNodeKind::Doc,
                "API docs",
                "https://docs.example.com",
                "daemon8",
            ))
            .await
            .unwrap();

        let edge = LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::DocumentedBy,
            from_node: proj_id.clone(),
            to_node: doc_id.clone(),
            created_at: current_ns(),
        };
        let edge_id = lib.index_edge(edge).await.unwrap();
        assert!(!edge_id.is_empty());

        let edges = lib.get_edges(&proj_id).await.unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, LibrarianEdgeKind::DocumentedBy);
    }

    #[tokio::test]
    async fn reindex_same_locator_bumps_version() {
        let (_store, lib) = setup().await;

        let node1 = make_node(LibrarianNodeKind::Doc, "v1 docs", "/docs/api", "p1");
        let id1 = lib.index_node(node1).await.unwrap();

        let node2 = make_node(LibrarianNodeKind::Doc, "v2 docs", "/docs/api", "p1");
        let id2 = lib.index_node(node2).await.unwrap();
        assert_ne!(id1, id2);

        let v2 = lib.get_node(&id2).await.unwrap().unwrap();
        assert_eq!(v2.version, 2);
        assert_eq!(v2.label, "v2 docs");

        // Old node should be deprecated
        let filter = LibrarianFilter {
            include_deprecated: true,
            ..Default::default()
        };
        let all = lib.lookup(&filter).await.unwrap();
        let old = all.iter().find(|n| n.id.as_deref() == Some(&id1)).unwrap();
        assert!(old.deprecated_at.is_some());

        // Supersedes edge should exist
        let edges = lib.get_edges(&id2).await.unwrap();
        assert!(
            edges
                .iter()
                .any(|e| e.kind == LibrarianEdgeKind::Supersedes)
        );
    }

    #[tokio::test]
    async fn lookup_by_kind() {
        let (_store, lib) = setup().await;

        lib.index_node(make_node(LibrarianNodeKind::Doc, "doc1", "/a", "p1"))
            .await
            .unwrap();
        lib.index_node(make_node(LibrarianNodeKind::Fix, "fix1", "/b", "p1"))
            .await
            .unwrap();

        let filter = LibrarianFilter {
            kinds: Some(vec![LibrarianNodeKind::Doc]),
            ..Default::default()
        };
        let results = lib.lookup(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, LibrarianNodeKind::Doc);
    }

    #[tokio::test]
    async fn lookup_excludes_deprecated_by_default() {
        let (_store, lib) = setup().await;

        let id = lib
            .index_node(make_node(LibrarianNodeKind::Doc, "old doc", "/old", "p1"))
            .await
            .unwrap();
        lib.deprecate_node(&id).await.unwrap();

        let filter = LibrarianFilter::default();
        let results = lib.lookup(&filter).await.unwrap();
        assert!(results.is_empty());

        let filter_with = LibrarianFilter {
            include_deprecated: true,
            ..Default::default()
        };
        let results = lib.lookup(&filter_with).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn lookup_by_text_match() {
        let (_store, lib) = setup().await;

        lib.index_node(make_node(
            LibrarianNodeKind::Doc,
            "React 19 Compiler Guide",
            "/docs/react",
            "p1",
        ))
        .await
        .unwrap();
        lib.index_node(make_node(
            LibrarianNodeKind::Doc,
            "Rust Style",
            "/docs/rust",
            "p1",
        ))
        .await
        .unwrap();

        let filter = LibrarianFilter {
            text_match: Some("react".into()),
            ..Default::default()
        };
        let results = lib.lookup(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].label.contains("React"));
    }

    #[tokio::test]
    async fn lookup_by_parent_id() {
        let (_store, lib) = setup().await;

        let parent_id = lib
            .index_node(make_node(
                LibrarianNodeKind::Project,
                "daemon8",
                "/daemon8",
                "daemon8",
            ))
            .await
            .unwrap();

        let mut child = make_node(LibrarianNodeKind::Doc, "child doc", "/child", "daemon8");
        child.parent_id = Some(parent_id.clone());
        lib.index_node(child).await.unwrap();

        lib.index_node(make_node(
            LibrarianNodeKind::Doc,
            "orphan doc",
            "/orphan",
            "daemon8",
        ))
        .await
        .unwrap();

        let filter = LibrarianFilter {
            parent_id: Some(parent_id.clone()),
            ..Default::default()
        };
        let results = lib.lookup(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "child doc");
    }

    #[tokio::test]
    async fn get_children_returns_non_deprecated() {
        let (_store, lib) = setup().await;

        let parent_id = lib
            .index_node(make_node(LibrarianNodeKind::Project, "proj", "/proj", "p1"))
            .await
            .unwrap();

        let mut c1 = make_node(LibrarianNodeKind::Doc, "active child", "/c1", "p1");
        c1.parent_id = Some(parent_id.clone());
        let c1_id = lib.index_node(c1).await.unwrap();

        let mut c2 = make_node(LibrarianNodeKind::Doc, "deprecated child", "/c2", "p1");
        c2.parent_id = Some(parent_id.clone());
        let c2_id = lib.index_node(c2).await.unwrap();
        lib.deprecate_node(&c2_id).await.unwrap();

        let children = lib.get_children(&parent_id).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id.as_deref(), Some(c1_id.as_str()));
    }

    #[tokio::test]
    async fn touch_read_updates_timestamp() {
        let (_store, lib) = setup().await;

        let id = lib
            .index_node(make_node(LibrarianNodeKind::Doc, "readable", "/read", "p1"))
            .await
            .unwrap();

        let before = lib.get_node(&id).await.unwrap().unwrap();
        let first_read = before.last_read_at;

        // Small delay to get a different timestamp
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        lib.touch_read(&id).await.unwrap();
        // get_node also touches, but we already set it explicitly
        let after_raw = {
            let mut result = lib
                .db
                .query("SELECT * FROM type::record('catalog_node', $id)")
                .bind(("id", serde_json::json!(&id)))
                .await
                .unwrap();
            let row: Option<serde_json::Value> = result.take(0).unwrap();
            parse_node(row.unwrap()).unwrap()
        };

        assert!(after_raw.last_read_at.is_some());
        assert!(after_raw.last_read_at > first_read);
    }

    #[tokio::test]
    async fn forget_node_cascades_edges() {
        let (_store, lib) = setup().await;

        let a = lib
            .index_node(make_node(LibrarianNodeKind::Project, "proj", "/a", "p1"))
            .await
            .unwrap();
        let b = lib
            .index_node(make_node(LibrarianNodeKind::Doc, "doc", "/b", "p1"))
            .await
            .unwrap();

        lib.index_edge(LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::DocumentedBy,
            from_node: a.clone(),
            to_node: b.clone(),
            created_at: current_ns(),
        })
        .await
        .unwrap();

        let deleted = lib.forget_node(&a).await.unwrap();
        assert!(deleted);

        // Edge should be gone too
        let edges = lib.get_edges(&b).await.unwrap();
        assert!(edges.is_empty());
    }

    #[tokio::test]
    async fn forget_nonexistent_returns_false() {
        let (_store, lib) = setup().await;

        let deleted = lib.forget_node("does_not_exist_42").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn deprecate_node_soft_deletes() {
        let (_store, lib) = setup().await;

        let id = lib
            .index_node(make_node(
                LibrarianNodeKind::Doc,
                "to deprecate",
                "/dep",
                "p1",
            ))
            .await
            .unwrap();

        let result = lib.deprecate_node(&id).await.unwrap();
        assert!(result);

        // Still fetchable by ID
        let node = lib.get_node(&id).await.unwrap().unwrap();
        assert!(node.deprecated_at.is_some());

        // But excluded from default lookup
        let filter = LibrarianFilter::default();
        let results = lib.lookup(&filter).await.unwrap();
        let found = results.iter().any(|n| n.id.as_deref() == Some(id.as_str()));
        assert!(!found);
    }

    #[tokio::test]
    async fn lookup_stale_before_finds_unread_nodes() {
        let (_store, lib) = setup().await;

        let id = lib
            .index_node(make_node(
                LibrarianNodeKind::Doc,
                "never read",
                "/stale",
                "p1",
            ))
            .await
            .unwrap();

        let read_id = lib
            .index_node(make_node(
                LibrarianNodeKind::Doc,
                "recently read",
                "/fresh",
                "p1",
            ))
            .await
            .unwrap();
        // get_node touches last_read_at
        lib.get_node(&read_id).await.unwrap();

        let future_threshold = current_ns() + 1_000_000_000;
        let filter = LibrarianFilter {
            stale_before: Some(future_threshold),
            ..Default::default()
        };
        let both = lib.lookup(&filter).await.unwrap();
        assert_eq!(both.len(), 2, "far-future threshold matches all nodes");

        let near_threshold = current_ns().saturating_sub(1_000_000_000);
        let strict_filter = LibrarianFilter {
            stale_before: Some(near_threshold),
            ..Default::default()
        };
        let strict_results = lib.lookup(&strict_filter).await.unwrap();
        assert_eq!(strict_results.len(), 1, "only the never-read node is stale");
        assert_eq!(strict_results[0].id.as_deref(), Some(id.as_str()));
    }

    #[tokio::test]
    async fn lookup_by_tags() {
        let (_store, lib) = setup().await;

        let mut tagged = make_node(LibrarianNodeKind::Doc, "tagged doc", "/tagged", "p1");
        tagged.tags = vec!["rust".into(), "async".into()];
        lib.index_node(tagged).await.unwrap();

        lib.index_node(make_node(
            LibrarianNodeKind::Doc,
            "untagged",
            "/untagged",
            "p1",
        ))
        .await
        .unwrap();

        let filter = LibrarianFilter {
            tags: Some(vec!["rust".into()]),
            ..Default::default()
        };
        let results = lib.lookup(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "tagged doc");
    }

    #[tokio::test]
    async fn text_match_searches_locator() {
        let (_store, lib) = setup().await;

        lib.index_node(make_node(
            LibrarianNodeKind::Doc,
            "API Reference",
            "https://docs.example.com/api",
            "p1",
        ))
        .await
        .unwrap();

        let filter = LibrarianFilter {
            text_match: Some("example.com".into()),
            ..Default::default()
        };
        let results = lib.lookup(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].label, "API Reference");
    }

    #[tokio::test]
    async fn lookup_does_not_touch_read_timestamp() {
        let (_store, lib) = setup().await;

        let id = lib
            .index_node(make_node(LibrarianNodeKind::Doc, "untouched", "/ut", "p1"))
            .await
            .unwrap();

        let filter = LibrarianFilter::default();
        lib.lookup(&filter).await.unwrap();

        // Read directly without get_node (which touches)
        let mut result = lib
            .db
            .query("SELECT * FROM type::record('catalog_node', $id)")
            .bind(("id", serde_json::json!(&id)))
            .await
            .unwrap();
        let row: Option<serde_json::Value> = result.take(0).unwrap();
        let node = parse_node(row.unwrap()).unwrap();
        assert!(
            node.last_read_at.is_none(),
            "lookup must not update last_read_at"
        );
    }
}
