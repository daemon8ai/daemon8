// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, params};

use daemon8_types::{
    Checkpoint, ConnectionInfo, Filter, HealthStatus, Observation, ObservationKind, Origin,
    OriginPattern, RuntimeSummary, Severity, SliceSummary, SourceLocation, StateSlice,
};

use crate::{StateModel, StoreError};

struct Inner {
    conn: Connection,
    connections: Vec<ConnectionInfo>,
}

pub struct SqliteStore {
    inner: Mutex<Inner>,
}

// rusqlite::Connection is !Send, but we never move it across threads --
// all access goes through the Mutex. This is safe because only one thread
// touches the Connection at a time.
unsafe impl Send for SqliteStore {}
unsafe impl Sync for SqliteStore {}

impl SqliteStore {
    /// Open (or create) a SQLite database at `path`, configure WAL mode,
    /// and create the schema.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 268435456;
             PRAGMA cache_size = -64000;
             PRAGMA auto_vacuum = INCREMENTAL;",
        )?;

        // Migrate legacy databases from auto_vacuum=OFF to INCREMENTAL.
        // This requires a full VACUUM (one-time, rewrites the entire file).
        let auto_vacuum: i64 = conn
            .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
            .unwrap_or(0);
        if auto_vacuum == 0 {
            tracing::info!("migrating database to incremental auto-vacuum (one-time VACUUM)");
            conn.execute_batch("VACUUM;")?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ns INTEGER NOT NULL,
                severity TEXT NOT NULL,
                kind_tag TEXT NOT NULL,
                origin_json TEXT NOT NULL,
                kind_json TEXT NOT NULL,
                data TEXT NOT NULL,
                source_file TEXT,
                source_line INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_obs_ts ON observations(timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_obs_severity_ts ON observations(severity, timestamp_ns);
            CREATE INDEX IF NOT EXISTS idx_obs_kind_ts ON observations(kind_tag, timestamp_ns);",
        )?;

        Ok(Self {
            inner: Mutex::new(Inner {
                conn,
                connections: Vec::new(),
            }),
        })
    }
}

fn severity_from_str(s: &str) -> Severity {
    s.parse().unwrap_or(Severity::Trace)
}

/// All severity values whose level >= min, as SQL-safe lowercase strings.
fn severities_gte(min: Severity) -> Vec<&'static str> {
    const ALL: &[(&str, Severity)] = &[
        ("trace", Severity::Trace),
        ("debug", Severity::Debug),
        ("info", Severity::Info),
        ("warn", Severity::Warn),
        ("error", Severity::Error),
    ];
    ALL.iter()
        .filter(|(_, s)| s.level() >= min.level())
        .map(|(name, _)| *name)
        .collect()
}

impl StateModel for SqliteStore {
    fn insert(&self, obs: Observation) -> Result<u64, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;

        let origin_json = serde_json::to_string(&obs.origin)?;
        let kind_json = serde_json::to_string(&obs.kind)?;
        let data_str = serde_json::to_string(&obs.data)?;
        let severity_str = obs.severity.to_string();
        let kind_tag_str = obs.kind.tag().to_string();

        let (source_file, source_line): (Option<&str>, Option<i32>) = match &obs.source_location {
            Some(loc) => (Some(loc.file.as_str()), Some(loc.line as i32)),
            None => (None, None),
        };

        guard.conn.execute(
            "INSERT INTO observations (timestamp_ns, severity, kind_tag, origin_json, kind_json, data, source_file, source_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                obs.timestamp_ns as i64,
                severity_str,
                kind_tag_str,
                origin_json,
                kind_json,
                data_str,
                source_file,
                source_line,
            ],
        )?;

        Ok(guard.conn.last_insert_rowid() as u64)
    }

    fn query(&self, filter: &Filter) -> Result<StateSlice, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;

        // Baseline checkpoint for empty results: use the filter's since value
        // if provided (agent was already caught up to there), otherwise use the
        // current store max ID (agent is caught up to now).
        let baseline: u64 = match filter.since {
            Some(ref cp) => cp.0,
            None => {
                let max: i64 = guard
                    .conn
                    .query_row("SELECT COALESCE(MAX(id), 0) FROM observations", [], |row| {
                        row.get(0)
                    })
                    .unwrap_or(0);
                max as u64
            }
        };

        let mut clauses: Vec<String> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(min) = filter.severity_min {
            let allowed = severities_gte(min);
            let placeholders: Vec<String> = allowed
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", param_values.len() + i + 1))
                .collect();
            clauses.push(format!("severity IN ({})", placeholders.join(",")));
            for s in allowed {
                param_values.push(Box::new(s.to_string()));
            }
        }

        if let Some(ref kinds) = filter.kinds
            && !kinds.is_empty()
        {
            let placeholders: Vec<String> = kinds
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", param_values.len() + i + 1))
                .collect();
            clauses.push(format!("kind_tag IN ({})", placeholders.join(",")));
            for k in kinds {
                param_values.push(Box::new(k.to_string()));
            }
        }

        if let Some(ref cp) = filter.since {
            clauses.push(format!("id > ?{}", param_values.len() + 1));
            param_values.push(Box::new(cp.0 as i64));
        }

        if let Some(ref text) = filter.text_match {
            clauses.push(format!("data LIKE ?{}", param_values.len() + 1));
            param_values.push(Box::new(format!("%{text}%")));
        }

        // Origin filter uses SQLite LIKE patterns against the origin_json column,
        // e.g. "origin_json LIKE '%\"type\":\"application\"%'".  Each predicate is
        // a full table scan — SQLite cannot use a B-tree index to accelerate
        // substring LIKE matches.  This is acceptable in practice because the
        // observation table is bounded by the TTL retention window (hardcoded
        // constant, currently 24 h).  The cleanup task runs every 8 h and
        // enforces the ceiling via cleanup_before().  If retention is extended to
        // multi-hour or multi-day windows in a high-volume
        // deployment, consider adding a generated column or FTS5 table for origin
        // filtering to avoid O(n) scans on large observation sets.
        if let Some(ref origins) = filter.origins {
            let mut origin_clauses = Vec::new();
            for origin in origins {
                match origin {
                    OriginPattern::AnyApplication => {
                        origin_clauses
                            .push("origin_json LIKE '%\"type\":\"application\"%'".to_string());
                    }
                    OriginPattern::ApplicationNamed(name) => {
                        origin_clauses.push(format!(
                            "origin_json LIKE '%\"name\":\"{}\"%'",
                            name.as_str().replace('"', "")
                        ));
                    }
                    OriginPattern::AnyBrowser => {
                        origin_clauses
                            .push("origin_json LIKE '%\"type\":\"browser\"%'".to_string());
                    }
                    OriginPattern::BrowserTab(tab_id) => {
                        origin_clauses.push(format!(
                            "origin_json LIKE '%\"tab_id\":\"{}\"%'",
                            tab_id.as_str().replace('"', "")
                        ));
                    }
                    OriginPattern::AnyDevice => {
                        origin_clauses.push("origin_json LIKE '%\"type\":\"device\"%'".to_string());
                    }
                    OriginPattern::DeviceSerial(serial) => {
                        origin_clauses.push(format!(
                            "origin_json LIKE '%\"serial\":\"{}\"%'",
                            serial.as_str().replace('"', "")
                        ));
                    }
                }
            }
            if !origin_clauses.is_empty() {
                clauses.push(format!("({})", origin_clauses.join(" OR ")));
            }
        }

        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };

        let limit_clause = match filter.limit {
            Some(n) => format!("LIMIT {n}"),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, timestamp_ns, severity, kind_tag, origin_json, kind_json, data, source_file, source_line
             FROM observations {where_clause} ORDER BY id ASC {limit_clause}"
        );

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = guard.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(RawRow {
                id: row.get::<_, i64>(0)?,
                timestamp_ns: row.get::<_, i64>(1)?,
                severity: row.get::<_, String>(2)?,
                _kind_tag: row.get::<_, String>(3)?,
                origin_json: row.get::<_, String>(4)?,
                kind_json: row.get::<_, String>(5)?,
                data: row.get::<_, String>(6)?,
                source_file: row.get::<_, Option<String>>(7)?,
                source_line: row.get::<_, Option<i32>>(8)?,
            })
        })?;

        let mut observations = Vec::new();
        let mut max_id: u64 = 0;

        for row_result in rows {
            let raw = row_result?;

            let origin: Origin = serde_json::from_str(&raw.origin_json)
                .map_err(|e| StoreError::Other(format!("bad origin json: {e}")))?;
            let kind: ObservationKind = serde_json::from_str(&raw.kind_json)
                .map_err(|e| StoreError::Other(format!("bad kind json: {e}")))?;
            let data: serde_json::Value = serde_json::from_str(&raw.data)
                .map_err(|e| StoreError::Other(format!("bad data json: {e}")))?;
            let severity = severity_from_str(&raw.severity);

            let source_location = raw.source_file.map(|file| SourceLocation {
                file,
                line: raw.source_line.unwrap_or(0) as u32,
                function: None,
            });

            let id = raw.id as u64;
            let timestamp_ns = raw.timestamp_ns as u64;

            if id > max_id {
                max_id = id;
            }

            observations.push(Observation {
                id,
                origin,
                kind,
                data,
                severity,
                source_location,
                timestamp_ns,
            });
        }

        let summary = build_slice_summary(&observations);
        let result_checkpoint = if max_id > 0 { max_id } else { baseline };

        Ok(StateSlice {
            observations,
            checkpoint: Checkpoint(result_checkpoint),
            summary,
        })
    }

    fn summary(&self) -> Result<RuntimeSummary, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;

        let total: i64 = guard
            .conn
            .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))?;
        let total = total as u64;

        // Errors in the last 60 seconds (using nanosecond timestamps).
        // We approximate "now" as the max timestamp in the table.
        let error_count: i64 = guard.conn.query_row(
            "SELECT COUNT(*) FROM observations
             WHERE severity = 'error'
               AND timestamp_ns > (
                   COALESCE((SELECT MAX(timestamp_ns) FROM observations), 0) - 60000000000
               )",
            [],
            |row| row.get(0),
        )?;
        let error_count = error_count as u64;

        let mut stmt = guard
            .conn
            .prepare("SELECT DISTINCT kind_tag FROM observations")?;
        let channels: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        let health = if guard.connections.is_empty() {
            HealthStatus::NoSources
        } else if error_count > 0 {
            HealthStatus::ErrorsDetected
        } else {
            HealthStatus::Ok
        };

        Ok(RuntimeSummary {
            observation_count: total,
            error_count_last_60s: error_count,
            active_channels: channels,
            connections: guard.connections.clone(),
            health,
        })
    }

    fn checkpoint(&self) -> Checkpoint {
        let guard = self.inner.lock().expect("sqlite store mutex poisoned");

        let max_id: i64 = guard
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM observations", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        Checkpoint(max_id as u64)
    }

    fn oldest_id(&self) -> Option<u64> {
        let guard = self.inner.lock().expect("sqlite store mutex poisoned");
        let min_id: Option<i64> = guard
            .conn
            .query_row("SELECT MIN(id) FROM observations", [], |row| row.get(0))
            .ok();
        min_id.map(|id| id as u64)
    }

    fn cleanup_before(&self, timestamp_ns: u64) -> Result<u64, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let deleted = guard.conn.execute(
            "DELETE FROM observations WHERE timestamp_ns < ?1",
            params![timestamp_ns as i64],
        )?;
        Ok(deleted as u64)
    }

    fn vacuum_incremental(&self, pages: u32) -> Result<(), StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        guard
            .conn
            .execute_batch(&format!("PRAGMA incremental_vacuum({pages})"))?;
        Ok(())
    }

    fn wal_checkpoint(&self) -> Result<(), StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        guard
            .conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(())
    }
}

struct RawRow {
    id: i64,
    timestamp_ns: i64,
    severity: String,
    _kind_tag: String,
    origin_json: String,
    kind_json: String,
    data: String,
    source_file: Option<String>,
    source_line: Option<i32>,
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
