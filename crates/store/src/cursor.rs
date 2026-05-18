// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{CursorState, CursorStore, StoreError};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

pub struct SurrealCursorStore {
    db: Surreal<Db>,
}

impl SurrealCursorStore {
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
                "DEFINE TABLE IF NOT EXISTS cursor_state SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS scope_root      ON cursor_state TYPE string;
                 DEFINE FIELD IF NOT EXISTS source          ON cursor_state TYPE string;
                 DEFINE FIELD IF NOT EXISTS source_instance ON cursor_state TYPE string;
                 DEFINE FIELD IF NOT EXISTS position        ON cursor_state TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at      ON cursor_state TYPE int;
                 DEFINE FIELD IF NOT EXISTS metadata        ON cursor_state TYPE option<object> FLEXIBLE;
                 DEFINE INDEX IF NOT EXISTS idx_cursor_identity ON cursor_state FIELDS scope_root, source, source_instance UNIQUE;
                 DEFINE INDEX IF NOT EXISTS idx_cursor_scope ON cursor_state FIELDS scope_root;
                 DEFINE INDEX IF NOT EXISTS idx_cursor_source ON cursor_state FIELDS source;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("cursor schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("cursor schema init check: {e}")))?;

        Ok(())
    }

    async fn select_one(&self, key: &str) -> Result<Option<CursorState>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('cursor_state', $id)")
            .bind(("id", serde_json::json!(key)))
            .await
            .map_err(|e| StoreError::Db(format!("select cursor_state: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("select cursor_state read: {e}")))?;

        match row {
            Some(row) => Ok(Some(serde_json::from_value(rehydrate_id(row))?)),
            None => Ok(None),
        }
    }
}

#[async_trait::async_trait]
impl CursorStore for SurrealCursorStore {
    async fn upsert_cursor(&self, mut cursor: CursorState) -> Result<(), StoreError> {
        validate_cursor_identity(&cursor.scope_root, &cursor.source, &cursor.source_instance)?;
        let key = cursor_key(&cursor.scope_root, &cursor.source, &cursor.source_instance);
        cursor.id = None;
        self.db
            .query("UPSERT type::record('cursor_state', $id) CONTENT $content")
            .bind(("id", serde_json::json!(key)))
            .bind(("content", serde_json::to_value(cursor)?))
            .await
            .map_err(|e| StoreError::Db(format!("upsert cursor_state: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("upsert cursor_state check: {e}")))?;
        Ok(())
    }

    async fn get_cursor(
        &self,
        scope_root: &str,
        source: &str,
        source_instance: &str,
    ) -> Result<Option<CursorState>, StoreError> {
        validate_cursor_identity(scope_root, source, source_instance)?;
        let key = cursor_key(scope_root, source, source_instance);
        self.select_one(&key).await
    }

    async fn list_cursors_for_scope(
        &self,
        scope_root: &str,
    ) -> Result<Vec<CursorState>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM cursor_state WHERE scope_root = $scope_root ORDER BY updated_at DESC")
            .bind(("scope_root", serde_json::json!(scope_root)))
            .await
            .map_err(|e| StoreError::Db(format!("list cursor_state: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("list cursor_state read: {e}")))?;

        rows.into_iter()
            .map(|row| serde_json::from_value(rehydrate_id(row)).map_err(StoreError::from))
            .collect()
    }
}

fn validate_cursor_identity(
    scope_root: &str,
    source: &str,
    source_instance: &str,
) -> Result<(), StoreError> {
    for (field, value) in [
        ("scope_root", scope_root),
        ("source", source),
        ("source_instance", source_instance),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::Other(format!(
                "cursor identity field `{field}` must not be empty"
            )));
        }
    }

    Ok(())
}

fn cursor_key(scope_root: &str, source: &str, source_instance: &str) -> String {
    let mut hasher = Sha256::new();
    for part in ["cursor_state", scope_root, source, source_instance] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn rehydrate_id(mut row: serde_json::Value) -> serde_json::Value {
    if let Some(id_val) = row.get("id")
        && let Some(bare) = extract_record_id(id_val)
    {
        row["id"] = serde_json::Value::String(bare);
    }
    row
}

fn extract_record_id(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(
            s.strip_prefix("cursor_state:")
                .unwrap_or(s.as_str())
                .to_string(),
        ),
        serde_json::Value::Object(obj) => match obj.get("id")? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(inner) => {
                inner.get("String")?.as_str().map(ToString::to_string)
            }
            _ => None,
        },
        _ => None,
    }
}
