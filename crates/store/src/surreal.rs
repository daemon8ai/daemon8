// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use daemon8_types::{
    Checkpoint, ConnectionInfo, Filter, HealthStatus, Observation, Origin, RuntimeSummary,
    Severity, SliceSummary, StateSlice,
};

use crate::{StateModel, StoreError};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

#[derive(Serialize, Deserialize)]
struct ObsRecord {
    seq: u64,
    timestamp_ns: u64,
    severity: String,
    kind_tag: String,
    origin: serde_json::Value,
    kind: serde_json::Value,
    data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_line: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
}

impl ObsRecord {
    fn from_observation(obs: &Observation, seq: u64) -> Result<Self, StoreError> {
        Ok(Self {
            seq,
            timestamp_ns: obs.timestamp_ns,
            severity: obs.severity.to_string(),
            kind_tag: obs.kind.tag().to_string(),
            origin: serde_json::to_value(&obs.origin)?,
            kind: serde_json::to_value(&obs.kind)?,
            data: obs.data.clone(),
            source_file: obs.source_location.as_ref().map(|l| l.file.clone()),
            source_line: obs.source_location.as_ref().map(|l| l.line as i32),
            correlation_id: obs.correlation_id.as_deref().map(String::from),
            parent_id: obs.parent_id,
            tags: obs.tags.clone(),
            session_id: obs.session_id.as_deref().map(String::from),
            node_id: obs.node_id.as_deref().map(String::from),
        })
    }

    fn into_observation(self) -> Result<Observation, StoreError> {
        let severity: Severity = self
            .severity
            .parse()
            .map_err(|_| StoreError::Db(format!("invalid severity: {}", self.severity)))?;
        let origin: Origin = serde_json::from_value(self.origin)?;
        let kind = serde_json::from_value(self.kind)?;
        let source_location = match (self.source_file, self.source_line) {
            (Some(file), Some(line)) => Some(daemon8_types::SourceLocation {
                file,
                line: line as u32,
                function: None,
            }),
            _ => None,
        };

        Ok(Observation {
            id: self.seq,
            origin,
            kind,
            data: self.data,
            severity,
            source_location,
            timestamp_ns: self.timestamp_ns,
            correlation_id: self.correlation_id.map(Into::into),
            parent_id: self.parent_id,
            tags: self.tags,
            session_id: self.session_id.map(Into::into),
            node_id: self.node_id.map(Into::into),
        })
    }
}

pub struct SurrealStore {
    db: Surreal<Db>,
    next_id: AtomicU64,
    connections: Mutex<Vec<ConnectionInfo>>,
}

impl SurrealStore {
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        use surrealdb::engine::local::SurrealKv;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StoreError::Db(format!("creating database directory: {e}"))
            })?;
        }

        let db = Surreal::new::<SurrealKv>(path)
            .await
            .map_err(|e| StoreError::Db(format!("opening database: {e}")))?;

        let store = Self {
            db,
            next_id: AtomicU64::new(1),
            connections: Mutex::new(Vec::new()),
        };
        store.init_schema().await?;
        store.recover_seq().await?;
        Ok(store)
    }

    pub async fn memory() -> Result<Self, StoreError> {
        use surrealdb::engine::local::Mem;

        let db = Surreal::new::<Mem>(())
            .await
            .map_err(|e| StoreError::Db(format!("creating in-memory database: {e}")))?;

        let store = Self {
            db,
            next_id: AtomicU64::new(1),
            connections: Mutex::new(Vec::new()),
        };
        store.init_schema().await?;
        Ok(store)
    }

    async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .use_ns(NAMESPACE)
            .use_db(DATABASE)
            .await
            .map_err(|e| StoreError::Db(format!("selecting namespace/database: {e}")))?;

        self.db
            .query(
                "DEFINE TABLE IF NOT EXISTS observation SCHEMAFULL;

                 DEFINE FIELD IF NOT EXISTS seq            ON observation TYPE int;
                 DEFINE FIELD IF NOT EXISTS timestamp_ns   ON observation TYPE int;
                 DEFINE FIELD IF NOT EXISTS severity       ON observation TYPE string;
                 DEFINE FIELD IF NOT EXISTS kind_tag       ON observation TYPE string;
                 DEFINE FIELD IF NOT EXISTS origin         ON observation TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS kind           ON observation TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS data           ON observation TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS source_file    ON observation TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS source_line    ON observation TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS correlation_id ON observation TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS parent_id      ON observation TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS tags           ON observation TYPE option<array>;
                 DEFINE FIELD IF NOT EXISTS session_id     ON observation TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS node_id        ON observation TYPE option<string>;

                 DEFINE INDEX IF NOT EXISTS idx_seq         ON observation FIELDS seq;
                 DEFINE INDEX IF NOT EXISTS idx_timestamp   ON observation FIELDS timestamp_ns;
                 DEFINE INDEX IF NOT EXISTS idx_severity    ON observation FIELDS severity;
                 DEFINE INDEX IF NOT EXISTS idx_kind        ON observation FIELDS kind_tag;
                 DEFINE INDEX IF NOT EXISTS idx_correlation ON observation FIELDS correlation_id;
                 DEFINE INDEX IF NOT EXISTS idx_session     ON observation FIELDS session_id;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("schema init check: {e}")))?;

        Ok(())
    }

    async fn recover_seq(&self) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query("SELECT math::max(seq) AS max_seq FROM observation GROUP ALL")
            .await
            .map_err(|e| StoreError::Db(format!("recovering sequence: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("reading max seq: {e}")))?;

        if let Some(val) = row
            && let Some(max) = val.get("max_seq").and_then(|v| v.as_u64())
        {
            self.next_id.store(max + 1, Ordering::Relaxed);
        }

        Ok(())
    }

    fn build_query_sql(filter: &Filter) -> (String, Vec<(&'static str, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(&'static str, serde_json::Value)> = Vec::new();

        if let Some(ref cp) = filter.since {
            conditions.push("seq > $since_seq");
            binds.push(("since_seq", serde_json::json!(cp.0)));
        }

        if let Some(min) = filter.severity_min {
            let allowed: Vec<String> = [
                Severity::Trace,
                Severity::Debug,
                Severity::Info,
                Severity::Warn,
                Severity::Error,
            ]
            .into_iter()
            .filter(|s| s.level() >= min.level())
            .map(|s| s.to_string())
            .collect();
            conditions.push("severity IN $allowed_severities");
            binds.push(("allowed_severities", serde_json::json!(allowed)));
        }

        if let Some(ref kinds) = filter.kinds
            && !kinds.is_empty()
        {
            let tags: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
            conditions.push("kind_tag IN $allowed_kinds");
            binds.push(("allowed_kinds", serde_json::json!(tags)));
        }

        if let Some(ref cid) = filter.correlation_id {
            conditions.push("correlation_id = $corr_id");
            binds.push(("corr_id", serde_json::json!(cid)));
        }

        if let Some(ref required_tags) = filter.tags
            && !required_tags.is_empty()
        {
            conditions.push("tags CONTAINSALL $required_tags");
            binds.push(("required_tags", serde_json::json!(required_tags)));
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
            "SELECT * FROM observation{where_clause} ORDER BY seq ASC{limit_clause}"
        );

        (sql, binds)
    }
}

#[async_trait::async_trait]
impl StateModel for SurrealStore {
    async fn insert(&self, obs: Observation) -> Result<u64, StoreError> {
        let seq = self.next_id.fetch_add(1, Ordering::Relaxed);
        let record = ObsRecord::from_observation(&obs, seq)?;
        let json_content = serde_json::to_value(&record)?;

        self.db
            .query("CREATE type::record('observation', $seq) CONTENT $content")
            .bind(("seq", serde_json::json!(seq)))
            .bind(("content", json_content))
            .await
            .map_err(|e| StoreError::Db(format!("insert: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("insert check: {e}")))?;

        if let Some(origin_name) = match &obs.origin {
            Origin::Application { name } => Some((name.as_ref(), "application")),
            Origin::Browser { .. } => Some(("browser", "browser")),
            Origin::Device { serial, .. } => Some((serial.as_ref(), "device")),
        } {
            let mut conns = self.connections.lock().map_err(|_| StoreError::LockPoisoned)?;
            if let Some(conn) = conns.iter_mut().find(|c| c.name == origin_name.0) {
                conn.observation_count += 1;
            } else {
                conns.push(ConnectionInfo {
                    id: origin_name.0.to_string(),
                    kind: match origin_name.1 {
                        "browser" => daemon8_types::ConnectionKind::Browser,
                        "device" => daemon8_types::ConnectionKind::Device,
                        _ => daemon8_types::ConnectionKind::Application,
                    },
                    name: origin_name.0.to_string(),
                    observation_count: 1,
                });
            }
        }

        Ok(seq)
    }

    async fn query(&self, filter: &Filter) -> Result<StateSlice, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);

        let mut query = self.db.query(&sql);
        for (name, value) in &binds {
            query = query.bind((*name, value.clone()));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("query: {e}")))?;

        let raw: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("reading query results: {e}")))?;

        let mut observations: Vec<Observation> = Vec::with_capacity(raw.len());
        for val in raw {
            let rec: ObsRecord = serde_json::from_value(val)?;
            observations.push(rec.into_observation()?);
        }

        // Apply Rust-side filter for complex predicates (origins, text_match, tags)
        // that are harder to express in SurrealQL.
        observations.retain(|obs| filter.matches(obs));

        let checkpoint = observations
            .last()
            .map(|o| Checkpoint(o.id))
            .unwrap_or(Checkpoint(0));

        let summary = build_slice_summary(&observations);

        Ok(StateSlice {
            observations,
            checkpoint,
            summary,
        })
    }

    async fn summary(&self) -> Result<RuntimeSummary, StoreError> {
        let mut result = self
            .db
            .query("SELECT count() AS total FROM observation GROUP ALL")
            .await
            .map_err(|e| StoreError::Db(format!("summary count: {e}")))?;

        let count_row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("reading count: {e}")))?;

        let observation_count = count_row
            .and_then(|v| v.get("total").and_then(|t| t.as_u64()))
            .unwrap_or(0);

        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let sixty_secs_ago = now_ns.saturating_sub(60_000_000_000);

        let mut err_result = self
            .db
            .query("SELECT count() AS total FROM observation WHERE severity = 'error' AND timestamp_ns > $cutoff GROUP ALL")
            .bind(("cutoff", serde_json::json!(sixty_secs_ago)))
            .await
            .map_err(|e| StoreError::Db(format!("error count: {e}")))?;

        let err_row: Option<serde_json::Value> = err_result
            .take(0)
            .map_err(|e| StoreError::Db(format!("reading error count: {e}")))?;

        let error_count_last_60s = err_row
            .and_then(|v| v.get("total").and_then(|t| t.as_u64()))
            .unwrap_or(0);

        let connections = self
            .connections
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?
            .clone();

        let health = if connections.is_empty() {
            HealthStatus::NoSources
        } else if error_count_last_60s > 0 {
            HealthStatus::ErrorsDetected
        } else {
            HealthStatus::Ok
        };

        let active_channels: Vec<String> = connections.iter().map(|c| c.name.clone()).collect();

        Ok(RuntimeSummary {
            observation_count,
            error_count_last_60s,
            active_channels,
            connections,
            health,
        })
    }

    async fn checkpoint(&self) -> Checkpoint {
        let current = self.next_id.load(Ordering::Relaxed);
        if current <= 1 {
            Checkpoint(0)
        } else {
            Checkpoint(current - 1)
        }
    }

    async fn oldest_id(&self) -> Option<u64> {
        let mut result = self
            .db
            .query("SELECT math::min(seq) AS min_seq FROM observation GROUP ALL")
            .await
            .ok()?;

        let row: Option<serde_json::Value> = result.take(0).ok()?;
        row.and_then(|v| v.get("min_seq").and_then(|s| s.as_u64()))
    }

    async fn cleanup_before(&self, timestamp_ns: u64) -> Result<u64, StoreError> {
        let mut result = self
            .db
            .query("SELECT count() AS total FROM observation WHERE timestamp_ns < $cutoff GROUP ALL")
            .bind(("cutoff", serde_json::json!(timestamp_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("cleanup count: {e}")))?;

        let count_row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("reading cleanup count: {e}")))?;

        let count = count_row
            .and_then(|v| v.get("total").and_then(|t| t.as_u64()))
            .unwrap_or(0);

        self.db
            .query("DELETE FROM observation WHERE timestamp_ns < $cutoff")
            .bind(("cutoff", serde_json::json!(timestamp_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("cleanup delete: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("cleanup check: {e}")))?;

        Ok(count)
    }

    async fn vacuum_incremental(&self, _pages: u32) -> Result<(), StoreError> {
        Ok(())
    }

    async fn wal_checkpoint(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

fn build_slice_summary(observations: &[Observation]) -> SliceSummary {
    let mut counts_by_kind: HashMap<String, usize> = HashMap::new();
    let mut counts_by_severity: HashMap<String, usize> = HashMap::new();

    for obs in observations {
        *counts_by_kind
            .entry(obs.kind.tag().to_string())
            .or_default() += 1;
        *counts_by_severity
            .entry(obs.severity.to_string())
            .or_default() += 1;
    }

    SliceSummary {
        total: observations.len(),
        counts_by_kind,
        counts_by_severity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_types::ObservationKind;

    fn make_obs(severity: Severity, ts: u64) -> Observation {
        Observation {
            id: 0,
            origin: Origin::Application {
                name: "test-app".into(),
            },
            kind: ObservationKind::Log,
            data: serde_json::json!({"msg": "hello"}),
            severity,
            source_location: None,
            timestamp_ns: ts,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
        }
    }

    #[tokio::test]
    async fn insert_and_query_round_trip() {
        let store = SurrealStore::memory().await.unwrap();
        let obs = make_obs(Severity::Info, 1_000_000);
        let id = store.insert(obs).await.unwrap();
        assert!(id > 0);

        let slice = store.query(&Filter::default()).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].id, id);
        assert_eq!(slice.observations[0].severity, Severity::Info);
    }

    #[tokio::test]
    async fn checkpoint_advances() {
        let store = SurrealStore::memory().await.unwrap();
        assert_eq!(store.checkpoint().await.0, 0);

        let id1 = store.insert(make_obs(Severity::Debug, 1_000)).await.unwrap();
        assert_eq!(store.checkpoint().await.0, id1);

        let id2 = store.insert(make_obs(Severity::Warn, 2_000)).await.unwrap();
        assert!(id2 > id1);
        assert_eq!(store.checkpoint().await.0, id2);

        let filter = Filter {
            since: Some(Checkpoint(id1)),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].id, id2);
    }

    #[tokio::test]
    async fn severity_filter() {
        let store = SurrealStore::memory().await.unwrap();
        store.insert(make_obs(Severity::Debug, 1_000)).await.unwrap();
        store.insert(make_obs(Severity::Error, 2_000)).await.unwrap();

        let filter = Filter {
            severity_min: Some(Severity::Warn),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].severity, Severity::Error);
    }

    #[tokio::test]
    async fn cleanup_removes_old() {
        let store = SurrealStore::memory().await.unwrap();
        store.insert(make_obs(Severity::Info, 1_000)).await.unwrap();
        store.insert(make_obs(Severity::Info, 2_000)).await.unwrap();
        store.insert(make_obs(Severity::Info, 3_000)).await.unwrap();

        let deleted = store.cleanup_before(2_500).await.unwrap();
        assert_eq!(deleted, 2);

        let slice = store.query(&Filter::default()).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].timestamp_ns, 3_000);
    }

    #[tokio::test]
    async fn oldest_id_tracks_minimum() {
        let store = SurrealStore::memory().await.unwrap();
        assert_eq!(store.oldest_id().await, None);

        let id1 = store.insert(make_obs(Severity::Info, 1_000)).await.unwrap();
        assert_eq!(store.oldest_id().await, Some(id1));

        store.insert(make_obs(Severity::Info, 2_000)).await.unwrap();
        assert_eq!(store.oldest_id().await, Some(id1));
    }

    #[tokio::test]
    async fn summary_counts() {
        let store = SurrealStore::memory().await.unwrap();
        store.insert(make_obs(Severity::Info, 1_000)).await.unwrap();
        store.insert(make_obs(Severity::Error, 2_000)).await.unwrap();

        let summary = store.summary().await.unwrap();
        assert_eq!(summary.observation_count, 2);
    }

    #[tokio::test]
    async fn correlation_id_filter() {
        let store = SurrealStore::memory().await.unwrap();

        let mut obs1 = make_obs(Severity::Info, 1_000);
        obs1.correlation_id = Some("req-123".into());
        store.insert(obs1).await.unwrap();

        let mut obs2 = make_obs(Severity::Info, 2_000);
        obs2.correlation_id = Some("req-456".into());
        store.insert(obs2).await.unwrap();

        let filter = Filter {
            correlation_id: Some("req-123".to_string()),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(
            slice.observations[0].correlation_id.as_deref(),
            Some("req-123")
        );
    }
}
