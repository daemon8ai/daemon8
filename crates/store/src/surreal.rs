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
    Checkpoint, ConnectionInfo, Filter, HealthStatus, Observation, Origin, OriginPattern,
    RuntimeSummary, Severity, SliceSummary, StateSlice,
};

use crate::{
    StateModel, StoreError, bookkeeper::SurrealBookkeeperStore, card::SurrealCardStore,
    embedding_profile::SurrealEmbeddingProfileStore, envelope::SurrealEnvelopeStore,
    memory::SurrealMemoryStore, memory_long::SurrealMemoryLongStore,
    memory_reference::SurrealMemoryReferenceStore, memory_short::SurrealMemoryShortStore,
};

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
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Db(format!("creating database directory: {e}")))?;
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

    /// Create a `SurrealMemoryStore` sharing this database handle.
    /// `Surreal<Db>` is internally Arc'd, so cloning is cheap.
    pub fn memory_store(&self) -> SurrealMemoryStore {
        SurrealMemoryStore::new(self.db.clone())
    }

    /// Create a `SurrealCardStore` sharing this database handle.
    /// `Surreal<Db>` is internally Arc'd, so cloning is cheap.
    pub fn card_store(&self) -> SurrealCardStore {
        SurrealCardStore::new(self.db.clone())
    }

    /// Create a `SurrealEnvelopeStore` sharing this database handle.
    /// `Surreal<Db>` is internally Arc'd, so cloning is cheap.
    pub fn envelope_store(&self) -> SurrealEnvelopeStore {
        SurrealEnvelopeStore::new(self.db.clone())
    }

    /// Create a `SurrealMemoryShortStore` sharing this database handle.
    pub fn memory_short_store(&self) -> SurrealMemoryShortStore {
        SurrealMemoryShortStore::new(self.db.clone())
    }

    /// Create a `SurrealMemoryReferenceStore` sharing this database handle.
    pub fn memory_reference_store(&self) -> SurrealMemoryReferenceStore {
        SurrealMemoryReferenceStore::new(self.db.clone())
    }

    /// Create a `SurrealMemoryLongStore` sharing this database handle.
    pub fn memory_long_store(&self) -> SurrealMemoryLongStore {
        SurrealMemoryLongStore::new(self.db.clone())
    }

    /// Create a `SurrealBookkeeperStore` sharing this database handle.
    /// The bookkeeper composes the three tier stores; this accessor returns
    /// a thin handle that runs the dedup / sweep operations directly via
    /// SurrealQL rather than going through the per-tier stores.
    pub fn bookkeeper_store(&self) -> SurrealBookkeeperStore {
        SurrealBookkeeperStore::new(self.db.clone())
    }

    /// Create a `SurrealEmbeddingProfileStore` sharing this database handle.
    pub fn embedding_profile_store(&self) -> SurrealEmbeddingProfileStore {
        SurrealEmbeddingProfileStore::new(self.db.clone())
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

        self.db
            .query(crate::envelope::ENVELOPE_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("envelope schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("envelope schema init check: {e}")))?;

        self.db
            .query(crate::card::CARD_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("card schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("card schema init check: {e}")))?;

        self.db
            .query(crate::memory_short::MEMORY_SHORT_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("memory_short schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_short schema init check: {e}")))?;

        self.db
            .query(crate::memory_reference::MEMORY_REFERENCE_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("memory_reference schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_reference schema init check: {e}")))?;

        self.db
            .query(crate::memory_long::MEMORY_LONG_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("memory_long schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("memory_long schema init check: {e}")))?;

        self.db
            .query(crate::embedding_profile::EMBEDDING_PROFILE_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("embedding_profile schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("embedding_profile schema init check: {e}")))?;

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

    fn build_query_sql(filter: &Filter) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if !filter.include_system.unwrap_or(false) {
            conditions.push("(tags IS NONE OR tags CONTAINSNOT '_system')".to_string());
        }

        if let Some(ref cp) = filter.since {
            conditions.push("seq > $since_seq".to_string());
            binds.push(("since_seq".into(), serde_json::json!(cp.0)));
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
            conditions.push("severity IN $allowed_severities".to_string());
            binds.push(("allowed_severities".into(), serde_json::json!(allowed)));
        }

        if let Some(ref kinds) = filter.kinds
            && !kinds.is_empty()
        {
            let tags: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
            conditions.push("kind_tag IN $allowed_kinds".to_string());
            binds.push(("allowed_kinds".into(), serde_json::json!(tags)));
        }

        if let Some(ref cid) = filter.correlation_id {
            conditions.push("correlation_id = $corr_id".to_string());
            binds.push(("corr_id".into(), serde_json::json!(cid)));
        }

        if let Some(ref required_tags) = filter.tags
            && !required_tags.is_empty()
        {
            conditions.push("tags CONTAINSALL $required_tags".to_string());
            binds.push(("required_tags".into(), serde_json::json!(required_tags)));
        }

        if let Some(ref origins) = filter.origins
            && !origins.is_empty()
        {
            let origin_conds: Vec<String> = origins
                .iter()
                .enumerate()
                .map(|(i, pat)| Self::origin_pattern_sql(pat, i, &mut binds))
                .collect();
            conditions.push(format!("({})", origin_conds.join(" OR ")));
        }

        if let Some(ref text) = filter.text_match {
            conditions.push(
                "string::contains(string::lowercase(<string> data), $text_lower)".to_string(),
            );
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
            format!("SELECT * FROM observation{where_clause} ORDER BY seq DESC{limit_clause}");

        (sql, binds)
    }

    fn origin_pattern_sql(
        pat: &OriginPattern,
        idx: usize,
        binds: &mut Vec<(String, serde_json::Value)>,
    ) -> String {
        match pat {
            OriginPattern::AnyApplication => "origin.type = 'application'".to_string(),
            OriginPattern::ApplicationNamed(name) => {
                let bind_name = format!("origin_name_{idx}");
                binds.push((bind_name.clone(), serde_json::json!(name)));
                format!("(origin.type = 'application' AND origin.name = ${bind_name})")
            }
            OriginPattern::AnyBrowser => "origin.type = 'browser'".to_string(),
            OriginPattern::BrowserTab(tab_id) => {
                let bind_name = format!("origin_tab_{idx}");
                binds.push((bind_name.clone(), serde_json::json!(tab_id)));
                format!("(origin.type = 'browser' AND origin.tab_id = ${bind_name})")
            }
            OriginPattern::AnyDevice => "origin.type = 'device'".to_string(),
            OriginPattern::DeviceSerial(serial) => {
                let bind_name = format!("origin_serial_{idx}");
                binds.push((bind_name.clone(), serde_json::json!(serial)));
                format!("(origin.type = 'device' AND origin.serial = ${bind_name})")
            }
        }
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
            let mut conns = self
                .connections
                .lock()
                .map_err(|_| StoreError::LockPoisoned)?;
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
        for (name, value) in binds {
            query = query.bind((name, value));
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
        observations.reverse();

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
            .query(
                "SELECT count() AS total FROM observation WHERE timestamp_ns < $cutoff GROUP ALL",
            )
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

    async fn health_check(&self) -> Result<(), StoreError> {
        self.db
            .query("RETURN 1")
            .await
            .map_err(|e| StoreError::Db(format!("health check: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("health check: {e}")))?;
        Ok(())
    }

    async fn memory_export_select_page(
        &self,
        query: &str,
        limit: u64,
        start: u64,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        let paged_query = format!("{} LIMIT $limit START $start", query.trim());
        let mut result = self
            .db
            .query(&paged_query)
            .bind(("limit", serde_json::json!(limit)))
            .bind(("start", serde_json::json!(start)))
            .await
            .map_err(|e| StoreError::Db(format!("memory export select page: {e}")))?;

        result
            .take(0)
            .map_err(|e| StoreError::Db(format!("memory export select page read: {e}")))
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
    use crate::{CardStore, Memory, MemoryStore};
    use daemon8_types::{AgentCard, AgentKind, AgentStatus, MemoryKind, ObservationKind};

    fn make_agent(id: &str, slug: &str) -> AgentCard {
        AgentCard {
            id: id.into(),
            actor_ref: format!("agent.{slug}"),
            address: format!("agent.{slug}"),
            slug: slug.into(),
            display_name: Some(slug.into()),
            agent_kind: AgentKind::Specialist,
            status: AgentStatus::Alive,
            persona: serde_json::json!({}),
            model: serde_json::json!({}),
            capabilities: Vec::new(),
            subjects_handled: Vec::new(),
            project_refs: vec!["project:daemon8".into()],
            team_refs: vec!["team:core".into()],
            primary_team_ref: Some("team:core".into()),
            spawned_by_actor_ref: None,
            spawned_from_cwd: None,
            spawned_from_project_ref: None,
            host_id: None,
            pid: None,
            parent_pid: None,
            process_group_id: None,
            executable_path: None,
            argv_hash: None,
            runtime_kind: Some("daemon8.deliber8".into()),
            runtime_version: None,
            launch_nonce: None,
            started_at: None,
            last_seen_at: None,
            heartbeat_interval_ms: None,
            stop_state: serde_json::json!({}),
            last_stop_request_at: None,
            last_exit_code: None,
            last_signal: None,
            cost_window_usd: 0.0,
            cost_total_usd: 0.0,
            budget_daily_usd: None,
            created_at: 1,
            updated_at: 1,
        }
    }

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
    async fn global_schema_bootstrap_creates_deliber8_card_tables() {
        let store = SurrealStore::memory().await.unwrap();
        let schema_probe = store
            .db
            .query(
                "CREATE type::record('agent_card', 'schema-probe') CONTENT {
                    slug: 'schema-probe',
                    unexpected_schema_probe_field: true
                }",
            )
            .await
            .unwrap()
            .check();
        assert!(
            schema_probe.is_err(),
            "agent_card must be SCHEMAFULL from global SurrealStore bootstrap"
        );

        let cards = store.card_store();

        cards
            .upsert_agent(make_agent("agent-bootstrap", "bootstrap"))
            .await
            .unwrap();

        let agent = cards.get_agent_by_slug("bootstrap").await.unwrap().unwrap();
        assert_eq!(agent.id, "agent-bootstrap");
    }

    #[tokio::test]
    async fn checkpoint_advances() {
        let store = SurrealStore::memory().await.unwrap();
        assert_eq!(store.checkpoint().await.0, 0);

        let id1 = store
            .insert(make_obs(Severity::Debug, 1_000))
            .await
            .unwrap();
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
    async fn memory_export_select_page_returns_bounded_page() {
        let store = SurrealStore::memory().await.unwrap();
        let memory_store = store.memory_store();
        for (created_at, content) in [(1, "one"), (2, "two"), (3, "three")] {
            memory_store
                .save_memory(Memory {
                    id: None,
                    created_at,
                    updated_at: created_at,
                    kind: MemoryKind::Pattern,
                    content: content.into(),
                    embedding: None,
                    source_observations: Vec::new(),
                    tags: vec!["project:daemon8".into()],
                    project_slug: "daemon8".into(),
                    session_id: None,
                    confidence: 1.0,
                })
                .await
                .unwrap();
        }

        let first_page = store
            .memory_export_select_page("SELECT * FROM memory ORDER BY created_at ASC", 2, 0)
            .await
            .unwrap();
        assert_eq!(first_page.len(), 2);
        assert_eq!(first_page[0]["content"], "one");
        assert_eq!(first_page[1]["content"], "two");

        let second_page = store
            .memory_export_select_page("SELECT * FROM memory ORDER BY created_at ASC", 2, 2)
            .await
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0]["content"], "three");
    }

    #[tokio::test]
    async fn severity_filter() {
        let store = SurrealStore::memory().await.unwrap();
        store
            .insert(make_obs(Severity::Debug, 1_000))
            .await
            .unwrap();
        store
            .insert(make_obs(Severity::Error, 2_000))
            .await
            .unwrap();

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
        store
            .insert(make_obs(Severity::Error, 2_000))
            .await
            .unwrap();

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

    #[tokio::test]
    async fn origin_filter_by_type() {
        let store = SurrealStore::memory().await.unwrap();

        store.insert(make_obs(Severity::Info, 1_000)).await.unwrap();

        let mut browser_obs = make_obs(Severity::Info, 2_000);
        browser_obs.origin = Origin::Browser {
            tab_id: "tab-1".into(),
            url: "https://example.com".into(),
        };
        store.insert(browser_obs).await.unwrap();

        let filter = Filter {
            origins: Some(vec![OriginPattern::AnyBrowser]),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert!(matches!(
            slice.observations[0].origin,
            Origin::Browser { .. }
        ));
    }

    #[tokio::test]
    async fn origin_filter_by_name() {
        let store = SurrealStore::memory().await.unwrap();

        store.insert(make_obs(Severity::Info, 1_000)).await.unwrap();

        let mut obs2 = make_obs(Severity::Info, 2_000);
        obs2.origin = Origin::Application {
            name: "other-app".into(),
        };
        store.insert(obs2).await.unwrap();

        let filter = Filter {
            origins: Some(vec![OriginPattern::ApplicationNamed("test-app".into())]),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert!(matches!(
            &slice.observations[0].origin,
            Origin::Application { name } if name.as_ref() == "test-app"
        ));
    }

    #[tokio::test]
    async fn origin_filter_multiple_patterns() {
        let store = SurrealStore::memory().await.unwrap();

        store.insert(make_obs(Severity::Info, 1_000)).await.unwrap();

        let mut browser_obs = make_obs(Severity::Info, 2_000);
        browser_obs.origin = Origin::Browser {
            tab_id: "tab-1".into(),
            url: "https://example.com".into(),
        };
        store.insert(browser_obs).await.unwrap();

        let mut device_obs = make_obs(Severity::Info, 3_000);
        device_obs.origin = Origin::Device {
            serial: "ABC123".into(),
            platform: daemon8_types::DevicePlatform::Android,
        };
        store.insert(device_obs).await.unwrap();

        let filter = Filter {
            origins: Some(vec![
                OriginPattern::AnyBrowser,
                OriginPattern::DeviceSerial("ABC123".into()),
            ]),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 2);
    }

    #[tokio::test]
    async fn text_match_filter() {
        let store = SurrealStore::memory().await.unwrap();

        let mut obs1 = make_obs(Severity::Info, 1_000);
        obs1.data = serde_json::json!({"msg": "connection timeout on port 443"});
        store.insert(obs1).await.unwrap();

        let mut obs2 = make_obs(Severity::Info, 2_000);
        obs2.data = serde_json::json!({"msg": "request completed successfully"});
        store.insert(obs2).await.unwrap();

        let filter = Filter {
            text_match: Some("TIMEOUT".to_string()),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert!(slice.observations[0].data.to_string().contains("timeout"));
    }

    #[tokio::test]
    async fn origin_filter_respects_limit() {
        let store = SurrealStore::memory().await.unwrap();

        for i in 0..10 {
            store
                .insert(make_obs(Severity::Info, i * 1000))
                .await
                .unwrap();
        }

        let mut browser_obs = make_obs(Severity::Info, 100_000);
        browser_obs.origin = Origin::Browser {
            tab_id: "tab-1".into(),
            url: "https://example.com".into(),
        };
        store.insert(browser_obs).await.unwrap();

        let filter = Filter {
            origins: Some(vec![OriginPattern::AnyBrowser]),
            limit: Some(5),
            ..Default::default()
        };
        let slice = store.query(&filter).await.unwrap();
        assert_eq!(slice.observations.len(), 1);
    }

    #[tokio::test]
    async fn system_tag_excluded_by_default() {
        let store = SurrealStore::memory().await.unwrap();

        let mut system_obs = make_obs(Severity::Info, 1_000);
        system_obs.tags = Some(vec![daemon8_types::SYSTEM_TAG.to_string()]);
        store.insert(system_obs).await.unwrap();

        store.insert(make_obs(Severity::Info, 2_000)).await.unwrap();

        let default_slice = store.query(&Filter::default()).await.unwrap();
        assert_eq!(default_slice.observations.len(), 1);
        assert_eq!(default_slice.observations[0].timestamp_ns, 2_000);

        let include_slice = store
            .query(&Filter {
                include_system: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(include_slice.observations.len(), 2);
    }
}
