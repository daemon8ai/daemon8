// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{
    RecentScopeRecord, ScopeConnectFailureRecord, ScopeIgnoreRecord, ScopeLedgerStore,
    ScopeLedgerSummary, ScopeSessionRecord, StoreError,
};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";
const MAX_FAILURE_RECORDS: usize = 200;

pub struct SurrealScopeLedgerStore {
    db: Surreal<Db>,
}

impl SurrealScopeLedgerStore {
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
                "DEFINE TABLE IF NOT EXISTS scope_session SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS session_id      ON scope_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS provider        ON scope_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS agent_name      ON scope_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS mode            ON scope_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS requested_path  ON scope_session TYPE string;
                 DEFINE FIELD IF NOT EXISTS scope_root      ON scope_session TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS transcript_path ON scope_session TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS project_name    ON scope_session TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS source_count    ON scope_session TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS connected_at    ON scope_session TYPE int;
                 DEFINE FIELD IF NOT EXISTS last_seen_at    ON scope_session TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_scope_session_session ON scope_session FIELDS session_id UNIQUE;
                 DEFINE INDEX IF NOT EXISTS idx_scope_session_provider ON scope_session FIELDS provider;
                 DEFINE INDEX IF NOT EXISTS idx_scope_session_root ON scope_session FIELDS scope_root;

                 DEFINE TABLE IF NOT EXISTS recent_scope SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS mode           ON recent_scope TYPE string;
                 DEFINE FIELD IF NOT EXISTS requested_path ON recent_scope TYPE string;
                 DEFINE FIELD IF NOT EXISTS scope_root     ON recent_scope TYPE string;
                 DEFINE FIELD IF NOT EXISTS provider       ON recent_scope TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS agent_name     ON recent_scope TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS session_id     ON recent_scope TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS project_name   ON recent_scope TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS source_count   ON recent_scope TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS first_seen_at  ON recent_scope TYPE int;
                 DEFINE FIELD IF NOT EXISTS last_seen_at   ON recent_scope TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_recent_scope_root ON recent_scope FIELDS scope_root UNIQUE;
                 DEFINE INDEX IF NOT EXISTS idx_recent_scope_seen ON recent_scope FIELDS last_seen_at;

                 DEFINE TABLE IF NOT EXISTS scope_connect_failure SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS session_id      ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS provider        ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS agent_name      ON scope_connect_failure TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS requested_path  ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS scope_root      ON scope_connect_failure TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS transcript_path ON scope_connect_failure TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS mode            ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS status          ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS code            ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS message         ON scope_connect_failure TYPE string;
                 DEFINE FIELD IF NOT EXISTS why             ON scope_connect_failure TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS attempt_count   ON scope_connect_failure TYPE int;
                 DEFINE FIELD IF NOT EXISTS first_seen_at   ON scope_connect_failure TYPE int;
                 DEFINE FIELD IF NOT EXISTS last_seen_at    ON scope_connect_failure TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_scope_failure_identity ON scope_connect_failure FIELDS provider, requested_path, code UNIQUE;
                 DEFINE INDEX IF NOT EXISTS idx_scope_failure_seen ON scope_connect_failure FIELDS last_seen_at;

                 DEFINE TABLE IF NOT EXISTS scope_ignore SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS scope_root  ON scope_ignore TYPE string;
                 DEFINE FIELD IF NOT EXISTS ignored_at  ON scope_ignore TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_scope_ignore_root ON scope_ignore FIELDS scope_root UNIQUE;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("scope ledger schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("scope ledger schema init check: {e}")))?;

        Ok(())
    }

    async fn select_one<T>(&self, table: &str, key: &str) -> Result<Option<T>, StoreError>
    where
        T: DeserializeOwned,
    {
        let sql = format!("SELECT * FROM type::record('{table}', $id)");
        let mut result = self
            .db
            .query(sql)
            .bind(("id", serde_json::json!(key)))
            .await
            .map_err(|e| StoreError::Db(format!("select {table}: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("select {table} read: {e}")))?;

        match row {
            Some(val) => Ok(Some(serde_json::from_value(rehydrate_id(val, table))?)),
            None => Ok(None),
        }
    }

    async fn upsert_content<T>(&self, table: &str, key: &str, content: &T) -> Result<(), StoreError>
    where
        T: serde::Serialize,
    {
        let sql = format!("UPSERT type::record('{table}', $id) CONTENT $content");
        self.db
            .query(sql)
            .bind(("id", serde_json::json!(key)))
            .bind(("content", serde_json::to_value(content)?))
            .await
            .map_err(|e| StoreError::Db(format!("upsert {table}: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("upsert {table} check: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ScopeLedgerStore for SurrealScopeLedgerStore {
    async fn record_connect_success(
        &self,
        mut record: ScopeSessionRecord,
    ) -> Result<(), StoreError> {
        let key = ledger_key(&["scope_session", &record.session_id]);
        if let Some(existing) = self
            .select_one::<ScopeSessionRecord>("scope_session", &key)
            .await?
        {
            record.connected_at = existing.connected_at;
        }
        record.id = None;
        self.upsert_content("scope_session", &key, &record).await?;

        if let Some(scope_root) = &record.scope_root {
            let recent = RecentScopeRecord {
                id: None,
                mode: record.mode,
                requested_path: record.requested_path,
                scope_root: scope_root.clone(),
                provider: Some(record.provider),
                agent_name: Some(record.agent_name),
                session_id: Some(record.session_id),
                project_name: record.project_name,
                source_count: record.source_count,
                first_seen_at: record.connected_at,
                last_seen_at: record.last_seen_at,
            };
            self.record_recent_scope(recent).await?;
        }

        Ok(())
    }

    async fn record_recent_scope(&self, mut record: RecentScopeRecord) -> Result<(), StoreError> {
        let key = ledger_key(&["recent_scope", &record.scope_root]);
        if let Some(existing) = self
            .select_one::<RecentScopeRecord>("recent_scope", &key)
            .await?
        {
            record.first_seen_at = existing.first_seen_at;
            record.provider = record.provider.or(existing.provider);
            record.agent_name = record.agent_name.or(existing.agent_name);
            record.session_id = record.session_id.or(existing.session_id);
            record.project_name = record.project_name.or(existing.project_name);
            record.source_count = record.source_count.or(existing.source_count);
        }
        record.id = None;
        self.upsert_content("recent_scope", &key, &record).await
    }

    async fn record_connect_failure(
        &self,
        mut record: ScopeConnectFailureRecord,
    ) -> Result<(), StoreError> {
        let key = ledger_key(&[
            "scope_connect_failure",
            &record.provider,
            &record.requested_path,
            &record.code,
        ]);
        if let Some(existing) = self
            .select_one::<ScopeConnectFailureRecord>("scope_connect_failure", &key)
            .await?
        {
            record.first_seen_at = existing.first_seen_at;
            record.attempt_count = existing.attempt_count.saturating_add(1);
        }
        record.id = None;
        self.upsert_content("scope_connect_failure", &key, &record)
            .await?;
        self.prune_failures().await
    }

    async fn is_scope_ignored(&self, scope_root: &str) -> Result<bool, StoreError> {
        let key = ledger_key(&["scope_ignore", scope_root]);
        Ok(self
            .select_one::<ScopeIgnoreRecord>("scope_ignore", &key)
            .await?
            .is_some())
    }

    async fn set_scope_ignored(&self, scope_root: &str) -> Result<(), StoreError> {
        let key = ledger_key(&["scope_ignore", scope_root]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let record = ScopeIgnoreRecord {
            id: None,
            scope_root: scope_root.to_string(),
            ignored_at: now,
        };
        self.upsert_content("scope_ignore", &key, &record).await
    }

    async fn remove_scope_ignored(&self, scope_root: &str) -> Result<(), StoreError> {
        let key = ledger_key(&["scope_ignore", scope_root]);
        self.db
            .query("DELETE type::record('scope_ignore', $id)")
            .bind(("id", serde_json::json!(key)))
            .await
            .map_err(|e| StoreError::Db(format!("delete scope_ignore: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("delete scope_ignore check: {e}")))?;
        Ok(())
    }

    async fn scope_ledger_summary(&self, limit: usize) -> Result<ScopeLedgerSummary, StoreError> {
        let limit = limit.clamp(1, 50);
        let recent_sql =
            format!("SELECT * FROM recent_scope ORDER BY last_seen_at DESC LIMIT {limit}");
        let failure_sql =
            format!("SELECT * FROM scope_connect_failure ORDER BY last_seen_at DESC LIMIT {limit}");

        let mut result = self
            .db
            .query(format!("{recent_sql}; {failure_sql};"))
            .await
            .map_err(|e| StoreError::Db(format!("scope ledger summary: {e}")))?;

        let recent_raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("scope ledger recent read: {e}")))?;
        let failure_raw: Vec<serde_json::Value> = result
            .take(1)
            .map_err(|e| StoreError::Db(format!("scope ledger failure read: {e}")))?;

        let recent_scopes = recent_raw
            .into_iter()
            .map(|val| serde_json::from_value(rehydrate_id(val, "recent_scope")))
            .collect::<Result<Vec<_>, _>>()?;
        let recent_failures = failure_raw
            .into_iter()
            .map(|val| serde_json::from_value(rehydrate_id(val, "scope_connect_failure")))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ScopeLedgerSummary {
            recent_scopes,
            recent_failures,
        })
    }

    async fn list_recent_scopes(&self) -> Result<Vec<RecentScopeRecord>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM recent_scope ORDER BY last_seen_at DESC")
            .await
            .map_err(|e| StoreError::Db(format!("list recent scopes: {e}")))?;

        let recent_raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("list recent scopes read: {e}")))?;

        recent_raw
            .into_iter()
            .map(|val| serde_json::from_value(rehydrate_id(val, "recent_scope")))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    async fn remove_scope_records(&self, scope_root: &str) -> Result<usize, StoreError> {
        let sql = "
            DELETE FROM recent_scope WHERE scope_root = $root;
            DELETE FROM scope_session WHERE scope_root = $root;
            DELETE FROM scope_connect_failure WHERE scope_root = $root;
            DELETE FROM scope_ignore WHERE scope_root = $root;
        ";
        let mut result = self
            .db
            .query(sql)
            .bind(("root", scope_root.to_string()))
            .await
            .map_err(|e| StoreError::Db(format!("remove scope records: {e}")))?;

        let mut total = 0usize;
        for i in 0..4 {
            let rows: Vec<serde_json::Value> = result
                .take(i)
                .map_err(|e| StoreError::Db(format!("remove scope records read {i}: {e}")))?;
            total += rows.len();
        }
        Ok(total)
    }
}

impl SurrealScopeLedgerStore {
    async fn prune_failures(&self) -> Result<(), StoreError> {
        let sql = format!(
            "SELECT id, last_seen_at FROM scope_connect_failure ORDER BY last_seen_at DESC START {MAX_FAILURE_RECORDS}"
        );
        let mut result = self
            .db
            .query(sql)
            .await
            .map_err(|e| StoreError::Db(format!("scope failure prune select: {e}")))?;
        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("scope failure prune read: {e}")))?;

        for row in raw {
            let Some(id) = row
                .get("id")
                .and_then(|id| extract_record_id(id, "scope_connect_failure"))
            else {
                continue;
            };
            self.db
                .query("DELETE type::record('scope_connect_failure', $id)")
                .bind(("id", serde_json::json!(id)))
                .await
                .map_err(|e| StoreError::Db(format!("scope failure prune delete: {e}")))?
                .check()
                .map_err(|e| StoreError::Db(format!("scope failure prune delete check: {e}")))?;
        }

        Ok(())
    }
}

fn ledger_key(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn extract_record_id(val: &serde_json::Value, table: &str) -> Option<String> {
    let prefix = format!("{table}:");
    match val {
        serde_json::Value::String(s) => Some(s.strip_prefix(&prefix).unwrap_or(s).to_string()),
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

fn rehydrate_id(mut row: serde_json::Value, table: &str) -> serde_json::Value {
    if let Some(id_val) = row.get("id")
        && let Some(bare) = extract_record_id(id_val, table)
    {
        row["id"] = serde_json::Value::String(bare);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScopeLedgerStore, SurrealStore};

    fn success_record(now: u64) -> ScopeSessionRecord {
        ScopeSessionRecord {
            id: None,
            session_id: "mcp-1".into(),
            provider: "codex".into(),
            agent_name: "codex-agent".into(),
            mode: "project".into(),
            requested_path: "/tmp/project/src".into(),
            scope_root: Some("/tmp/project".into()),
            transcript_path: None,
            project_name: Some("project".into()),
            source_count: Some(2),
            connected_at: now,
            last_seen_at: now,
        }
    }

    fn failure_record(now: u64) -> ScopeConnectFailureRecord {
        ScopeConnectFailureRecord {
            id: None,
            session_id: "mcp-1".into(),
            provider: "codex".into(),
            agent_name: None,
            requested_path: "/tmp/project".into(),
            scope_root: Some("/tmp/project".into()),
            transcript_path: None,
            mode: "project".into(),
            status: "setup_required".into(),
            code: "missing_config".into(),
            message: "project config is missing".into(),
            why: Some("daemon8 project mode requires .daemon8/config.md".into()),
            attempt_count: 1,
            first_seen_at: now,
            last_seen_at: now,
        }
    }

    #[tokio::test]
    async fn success_records_session_and_recent_scope() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        ledger
            .record_connect_success(success_record(100))
            .await
            .unwrap();

        let summary = ledger.scope_ledger_summary(5).await.unwrap();
        assert_eq!(summary.recent_scopes.len(), 1);
        assert_eq!(summary.recent_scopes[0].scope_root, "/tmp/project");
        assert_eq!(summary.recent_scopes[0].provider.as_deref(), Some("codex"));
        assert!(summary.recent_failures.is_empty());
    }

    #[tokio::test]
    async fn repeated_failure_coalesces() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        ledger
            .record_connect_failure(failure_record(100))
            .await
            .unwrap();
        ledger
            .record_connect_failure(failure_record(200))
            .await
            .unwrap();

        let summary = ledger.scope_ledger_summary(5).await.unwrap();
        assert_eq!(summary.recent_failures.len(), 1);
        assert_eq!(summary.recent_failures[0].attempt_count, 2);
        assert_eq!(summary.recent_failures[0].first_seen_at, 100);
        assert_eq!(summary.recent_failures[0].last_seen_at, 200);
        assert!(summary.recent_scopes.is_empty());
    }

    #[tokio::test]
    async fn repeated_failure_coalesces_across_sessions() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        ledger
            .record_connect_failure(failure_record(100))
            .await
            .unwrap();

        let mut second = failure_record(200);
        second.session_id = "cli-2".into();
        ledger.record_connect_failure(second).await.unwrap();

        let summary = ledger.scope_ledger_summary(5).await.unwrap();
        assert_eq!(summary.recent_failures.len(), 1);
        assert_eq!(summary.recent_failures[0].session_id, "cli-2");
        assert_eq!(summary.recent_failures[0].attempt_count, 2);
        assert_eq!(summary.recent_failures[0].first_seen_at, 100);
        assert_eq!(summary.recent_failures[0].last_seen_at, 200);
    }

    #[tokio::test]
    async fn recent_scope_partial_update_preserves_connect_metadata() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();
        ledger
            .record_connect_success(success_record(100))
            .await
            .unwrap();

        ledger
            .record_recent_scope(RecentScopeRecord {
                id: None,
                mode: "project".into(),
                requested_path: "/tmp/project".into(),
                scope_root: "/tmp/project".into(),
                provider: None,
                agent_name: None,
                session_id: None,
                project_name: None,
                source_count: None,
                first_seen_at: 200,
                last_seen_at: 200,
            })
            .await
            .unwrap();

        let summary = ledger.scope_ledger_summary(5).await.unwrap();
        assert_eq!(summary.recent_scopes.len(), 1);
        assert_eq!(summary.recent_scopes[0].first_seen_at, 100);
        assert_eq!(summary.recent_scopes[0].last_seen_at, 200);
        assert_eq!(summary.recent_scopes[0].provider.as_deref(), Some("codex"));
        assert_eq!(
            summary.recent_scopes[0].agent_name.as_deref(),
            Some("codex-agent")
        );
        assert_eq!(
            summary.recent_scopes[0].session_id.as_deref(),
            Some("mcp-1")
        );
        assert_eq!(
            summary.recent_scopes[0].project_name.as_deref(),
            Some("project")
        );
        assert_eq!(summary.recent_scopes[0].source_count, Some(2));
    }

    #[tokio::test]
    async fn failure_pruning_keeps_bounded_recent_set() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        for idx in 0..205 {
            let mut record = failure_record(idx);
            record.requested_path = format!("/tmp/project/{idx}");
            record.first_seen_at = idx;
            record.last_seen_at = idx;
            ledger.record_connect_failure(record).await.unwrap();
        }

        let mut result = ledger
            .db
            .query("SELECT count() AS total FROM scope_connect_failure GROUP ALL")
            .await
            .unwrap();
        let row: Option<serde_json::Value> = result.take(0).unwrap();
        assert_eq!(
            row.and_then(|value| value.get("total").and_then(|total| total.as_u64())),
            Some(MAX_FAILURE_RECORDS as u64)
        );

        let summary = ledger.scope_ledger_summary(5).await.unwrap();
        assert_eq!(summary.recent_failures.len(), 5);
        assert_eq!(summary.recent_failures[0].last_seen_at, 204);
        assert_eq!(summary.recent_failures[4].last_seen_at, 200);
    }

    #[tokio::test]
    async fn reset_clears_scope_ledger() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();
        ledger
            .record_connect_success(success_record(100))
            .await
            .unwrap();
        ledger.set_scope_ignored("/tmp/ignored").await.unwrap();

        let report = store.reset().await.unwrap();
        let summary = ledger.scope_ledger_summary(5).await.unwrap();

        assert_eq!(report.scope_ledger_records_dropped, 3);
        assert!(summary.recent_scopes.is_empty());
        assert!(summary.recent_failures.is_empty());
        assert!(!ledger.is_scope_ignored("/tmp/ignored").await.unwrap());
    }

    #[tokio::test]
    async fn scope_ignore_roundtrip() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        assert!(!ledger.is_scope_ignored("/tmp/project").await.unwrap());

        ledger.set_scope_ignored("/tmp/project").await.unwrap();
        assert!(ledger.is_scope_ignored("/tmp/project").await.unwrap());
        assert!(!ledger.is_scope_ignored("/tmp/other").await.unwrap());

        ledger.remove_scope_ignored("/tmp/project").await.unwrap();
        assert!(!ledger.is_scope_ignored("/tmp/project").await.unwrap());
    }

    #[tokio::test]
    async fn set_scope_ignored_is_idempotent() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        ledger.set_scope_ignored("/tmp/project").await.unwrap();
        ledger.set_scope_ignored("/tmp/project").await.unwrap();
        assert!(ledger.is_scope_ignored("/tmp/project").await.unwrap());
    }

    #[tokio::test]
    async fn remove_nonexistent_ignore_is_noop() {
        let store = SurrealStore::memory().await.unwrap();
        let ledger = store.scope_ledger_store();

        ledger.remove_scope_ignored("/tmp/nowhere").await.unwrap();
        assert!(!ledger.is_scope_ignored("/tmp/nowhere").await.unwrap());
    }
}
