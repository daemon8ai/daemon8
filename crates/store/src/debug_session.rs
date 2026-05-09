// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use daemon8_types::{DebugSessionOutcome, DebugSessionStatus};

use crate::{DebugCheckpoint, DebugSession, DebugSessionStore, StoreError};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

pub struct SurrealDebugSessionStore {
    db: Surreal<Db>,
}

impl SurrealDebugSessionStore {
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
                "DEFINE TABLE IF NOT EXISTS debug_session SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS started_at        ON debug_session TYPE int;
                 DEFINE FIELD IF NOT EXISTS ended_at          ON debug_session TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS last_activity     ON debug_session TYPE int;
                 DEFINE FIELD IF NOT EXISTS project_slug      ON debug_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS description       ON debug_session TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS status            ON debug_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS outcome           ON debug_session TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS summary_memory_id ON debug_session TYPE option<string>;
                 DEFINE INDEX IF NOT EXISTS idx_ds_status  ON debug_session FIELDS status;
                 DEFINE INDEX IF NOT EXISTS idx_ds_project ON debug_session FIELDS project_slug;

                 DEFINE TABLE IF NOT EXISTS checkpoint SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS debug_session_id ON checkpoint TYPE string;
                 DEFINE FIELD IF NOT EXISTS description      ON checkpoint TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS created_at       ON checkpoint TYPE int;
                 DEFINE FIELD IF NOT EXISTS seq_at_creation  ON checkpoint TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_cp_session ON checkpoint FIELDS debug_session_id;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("debug_session schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("debug_session schema init check: {e}")))?;

        Ok(())
    }
}

/// SurrealDB returns record ids as either `"table:key"` strings or
/// `{tb, id: {String: "key"}}` objects depending on the driver path.
/// Strip down to a bare key string the caller can pass back to get/end/etc.
fn extract_record_id(val: &serde_json::Value, table: &str) -> Option<String> {
    let prefix = format!("{table}:");
    match val {
        serde_json::Value::String(s) => Some(s.strip_prefix(&prefix).unwrap_or(s).to_string()),
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

fn rehydrate_id(mut row: serde_json::Value, table: &str) -> serde_json::Value {
    if let Some(id_val) = row.get("id")
        && let Some(bare) = extract_record_id(id_val, table)
    {
        row["id"] = serde_json::Value::String(bare);
    }
    row
}

#[async_trait::async_trait]
impl DebugSessionStore for SurrealDebugSessionStore {
    async fn start_debug_session(&self, session: DebugSession) -> Result<String, StoreError> {
        let json_content = serde_json::to_value(&session)?;

        let mut result = self
            .db
            .query("CREATE debug_session CONTENT $content")
            .bind(("content", json_content))
            .await
            .map_err(|e| StoreError::Db(format!("start_debug_session: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("start_debug_session read: {e}")))?;

        row.as_ref()
            .and_then(|v| v.get("id"))
            .and_then(|id| extract_record_id(id, "debug_session"))
            .ok_or_else(|| StoreError::Db("start_debug_session: no id returned".into()))
    }

    async fn get_debug_session(&self, id: &str) -> Result<Option<DebugSession>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('debug_session', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_debug_session: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_debug_session read: {e}")))?;

        match row {
            Some(val) => {
                let val = rehydrate_id(val, "debug_session");
                let session: DebugSession = serde_json::from_value(val)?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    async fn list_debug_sessions(
        &self,
        status: Option<DebugSessionStatus>,
    ) -> Result<Vec<DebugSession>, StoreError> {
        let (sql, bind_status) = match status {
            Some(s) => (
                "SELECT * FROM debug_session WHERE status = $status ORDER BY started_at DESC",
                Some(s.to_string()),
            ),
            None => ("SELECT * FROM debug_session ORDER BY started_at DESC", None),
        };

        let mut q = self.db.query(sql);
        if let Some(s) = bind_status {
            q = q.bind(("status", serde_json::json!(s)));
        }

        let mut result = q
            .await
            .map_err(|e| StoreError::Db(format!("list_debug_sessions: {e}")))?;
        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("list_debug_sessions read: {e}")))?;

        let mut sessions = Vec::with_capacity(raw.len());
        for val in raw {
            let val = rehydrate_id(val, "debug_session");
            sessions.push(serde_json::from_value(val)?);
        }
        Ok(sessions)
    }

    async fn end_debug_session(
        &self,
        id: &str,
        status: DebugSessionStatus,
        outcome: Option<DebugSessionOutcome>,
        summary_memory_id: Option<String>,
        ended_at: u64,
    ) -> Result<(), StoreError> {
        // SurrealDB's option<string> rejects JSON null — only NONE or a real
        // string. Build the SET clause conditionally so absent fields just
        // aren't touched (which preserves prior values; for end-of-session
        // they were already None).
        let mut set_clauses = vec![
            "status = $status".to_string(),
            "ended_at = $ended_at".to_string(),
            "last_activity = $ended_at".to_string(),
        ];
        if outcome.is_some() {
            set_clauses.push("outcome = $outcome".to_string());
        }
        if summary_memory_id.is_some() {
            set_clauses.push("summary_memory_id = $summary".to_string());
        }

        let sql = format!(
            "UPDATE type::record('debug_session', $id) SET {}",
            set_clauses.join(", ")
        );

        let mut q = self
            .db
            .query(&sql)
            .bind(("id", serde_json::json!(id)))
            .bind(("status", serde_json::json!(status.to_string())))
            .bind(("ended_at", serde_json::json!(ended_at)));
        if let Some(o) = outcome {
            q = q.bind(("outcome", serde_json::json!(o.to_string())));
        }
        if let Some(s) = summary_memory_id {
            q = q.bind(("summary", serde_json::json!(s)));
        }

        q.await
            .map_err(|e| StoreError::Db(format!("end_debug_session: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("end_debug_session check: {e}")))?;
        Ok(())
    }

    async fn touch_debug_session(&self, id: &str, last_activity: u64) -> Result<(), StoreError> {
        self.db
            .query("UPDATE type::record('debug_session', $id) SET last_activity = $ts")
            .bind(("id", serde_json::json!(id)))
            .bind(("ts", serde_json::json!(last_activity)))
            .await
            .map_err(|e| StoreError::Db(format!("touch_debug_session: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("touch_debug_session check: {e}")))?;
        Ok(())
    }

    async fn find_stale_active(&self, threshold_ns: u64) -> Result<Vec<DebugSession>, StoreError> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM debug_session
                 WHERE status = 'active' AND last_activity < $threshold
                 ORDER BY last_activity ASC",
            )
            .bind(("threshold", serde_json::json!(threshold_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("find_stale_active: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("find_stale_active read: {e}")))?;

        let mut sessions = Vec::with_capacity(raw.len());
        for val in raw {
            let val = rehydrate_id(val, "debug_session");
            sessions.push(serde_json::from_value(val)?);
        }
        Ok(sessions)
    }

    async fn create_checkpoint(&self, checkpoint: DebugCheckpoint) -> Result<String, StoreError> {
        let json_content = serde_json::to_value(&checkpoint)?;

        let mut result = self
            .db
            .query("CREATE checkpoint CONTENT $content")
            .bind(("content", json_content))
            .await
            .map_err(|e| StoreError::Db(format!("create_checkpoint: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("create_checkpoint read: {e}")))?;

        row.as_ref()
            .and_then(|v| v.get("id"))
            .and_then(|id| extract_record_id(id, "checkpoint"))
            .ok_or_else(|| StoreError::Db("create_checkpoint: no id returned".into()))
    }

    async fn get_checkpoint(&self, id: &str) -> Result<Option<DebugCheckpoint>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('checkpoint', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_checkpoint: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_checkpoint read: {e}")))?;

        match row {
            Some(val) => {
                let val = rehydrate_id(val, "checkpoint");
                let cp: DebugCheckpoint = serde_json::from_value(val)?;
                Ok(Some(cp))
            }
            None => Ok(None),
        }
    }

    async fn list_checkpoints(
        &self,
        debug_session_id: &str,
    ) -> Result<Vec<DebugCheckpoint>, StoreError> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM checkpoint
                 WHERE debug_session_id = $sid
                 ORDER BY created_at ASC",
            )
            .bind(("sid", serde_json::json!(debug_session_id)))
            .await
            .map_err(|e| StoreError::Db(format!("list_checkpoints: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("list_checkpoints read: {e}")))?;

        let mut checkpoints = Vec::with_capacity(raw.len());
        for val in raw {
            let val = rehydrate_id(val, "checkpoint");
            checkpoints.push(serde_json::from_value(val)?);
        }
        Ok(checkpoints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurrealStore;

    async fn setup() -> (SurrealStore, SurrealDebugSessionStore) {
        let store = SurrealStore::memory().await.unwrap();
        let ds_store = store.debug_session_store();
        ds_store.init_schema().await.unwrap();
        (store, ds_store)
    }

    fn fresh_session(project: &str, ts: u64) -> DebugSession {
        DebugSession {
            id: None,
            started_at: ts,
            ended_at: None,
            last_activity: ts,
            project_slug: project.into(),
            description: Some("fix flaky test".into()),
            status: DebugSessionStatus::Active,
            outcome: None,
            summary_memory_id: None,
        }
    }

    #[tokio::test]
    async fn start_and_get_round_trip() {
        let (_store, ds) = setup().await;
        let id = ds
            .start_debug_session(fresh_session("daemon8", 1_000))
            .await
            .unwrap();
        let fetched = ds.get_debug_session(&id).await.unwrap().unwrap();
        assert_eq!(fetched.project_slug, "daemon8");
        assert_eq!(fetched.status, DebugSessionStatus::Active);
        assert_eq!(fetched.description.as_deref(), Some("fix flaky test"));
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let (_store, ds) = setup().await;
        let active_id = ds
            .start_debug_session(fresh_session("p", 1_000))
            .await
            .unwrap();
        let other_id = ds
            .start_debug_session(fresh_session("p", 2_000))
            .await
            .unwrap();
        ds.end_debug_session(
            &other_id,
            DebugSessionStatus::Completed,
            Some(DebugSessionOutcome::Resolved),
            None,
            3_000,
        )
        .await
        .unwrap();

        let active = ds
            .list_debug_sessions(Some(DebugSessionStatus::Active))
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id.as_deref(), Some(active_id.as_str()));

        let all = ds.list_debug_sessions(None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn end_marks_status_and_outcome() {
        let (_store, ds) = setup().await;
        let id = ds
            .start_debug_session(fresh_session("p", 1_000))
            .await
            .unwrap();
        ds.end_debug_session(
            &id,
            DebugSessionStatus::Completed,
            Some(DebugSessionOutcome::Resolved),
            Some("memory_42".into()),
            5_000,
        )
        .await
        .unwrap();

        let fetched = ds.get_debug_session(&id).await.unwrap().unwrap();
        assert_eq!(fetched.status, DebugSessionStatus::Completed);
        assert_eq!(fetched.outcome, Some(DebugSessionOutcome::Resolved));
        assert_eq!(fetched.summary_memory_id.as_deref(), Some("memory_42"));
        assert_eq!(fetched.ended_at, Some(5_000));
    }

    #[tokio::test]
    async fn touch_updates_last_activity() {
        let (_store, ds) = setup().await;
        let id = ds
            .start_debug_session(fresh_session("p", 1_000))
            .await
            .unwrap();
        ds.touch_debug_session(&id, 9_999).await.unwrap();
        let fetched = ds.get_debug_session(&id).await.unwrap().unwrap();
        assert_eq!(fetched.last_activity, 9_999);
    }

    #[tokio::test]
    async fn find_stale_active_returns_only_old_active() {
        let (_store, ds) = setup().await;
        let stale = ds
            .start_debug_session(fresh_session("p", 1_000))
            .await
            .unwrap();
        let _fresh = ds
            .start_debug_session(fresh_session("p", 100_000))
            .await
            .unwrap();
        let _ended = ds
            .start_debug_session(fresh_session("p", 500))
            .await
            .unwrap();
        ds.end_debug_session(
            &_ended,
            DebugSessionStatus::Completed,
            Some(DebugSessionOutcome::Resolved),
            None,
            600,
        )
        .await
        .unwrap();

        let stale_sessions = ds.find_stale_active(50_000).await.unwrap();
        assert_eq!(stale_sessions.len(), 1);
        assert_eq!(stale_sessions[0].id.as_deref(), Some(stale.as_str()));
    }

    #[tokio::test]
    async fn checkpoint_round_trip_and_listing() {
        let (_store, ds) = setup().await;
        let session_id = ds
            .start_debug_session(fresh_session("p", 1_000))
            .await
            .unwrap();

        for (i, desc) in ["before fix", "after refactor", "verify"]
            .iter()
            .enumerate()
        {
            let cp = DebugCheckpoint {
                id: None,
                debug_session_id: session_id.clone(),
                description: Some((*desc).into()),
                created_at: 2_000 + i as u64,
                seq_at_creation: 10 + i as u64,
            };
            ds.create_checkpoint(cp).await.unwrap();
        }

        let listed = ds.list_checkpoints(&session_id).await.unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].description.as_deref(), Some("before fix"));
        assert_eq!(listed[2].seq_at_creation, 12);

        // Different session should have no checkpoints
        let other = ds
            .start_debug_session(fresh_session("p", 5_000))
            .await
            .unwrap();
        let listed = ds.list_checkpoints(&other).await.unwrap();
        assert!(listed.is_empty());
    }
}
