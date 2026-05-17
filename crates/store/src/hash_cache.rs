// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Observation hash cache for burst deduplication.
//!
//! Structurally identical observations (same origin, kind, severity,
//! kind-specific content fields, and data payload) are skipped rather
//! than inserted into the store. Volatile metadata (session IDs, tags,
//! node_id, correlation_id, etc.) is excluded from the fingerprint so
//! the same logical event dedupes across MCP sessions. Bounded to
//! 10 000 entries with LRU eviction. Lives entirely in memory and does
//! not survive a daemon restart.

use std::hash::{Hash, Hasher};

use daemon8_types::Observation;

/// Bounded LRU cache (10 000 entries) for burst deduplication.
pub struct ObservationHashCache {
    cache: quick_cache::sync::Cache<u64, ()>,
}

impl ObservationHashCache {
    pub fn new() -> Self {
        Self {
            cache: quick_cache::sync::Cache::new(10_000),
        }
    }

    pub fn dedup_fingerprint(obs: &Observation) -> u64 {
        use daemon8_types::ObservationKind;

        let (_, origin_key) = daemon8_types::observation_origin_fields(&obs.origin);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        origin_key.hash(&mut hasher);
        obs.kind.tag().to_string().hash(&mut hasher);
        obs.severity.to_string().hash(&mut hasher);

        match &obs.kind {
            ObservationKind::Log => {}
            ObservationKind::Query { sql, .. } => sql.hash(&mut hasher),
            ObservationKind::HttpExchange {
                method,
                url,
                status,
                ..
            } => {
                method.hash(&mut hasher);
                url.hash(&mut hasher);
                status.hash(&mut hasher);
            }
            ObservationKind::Exception { message, trace } => {
                message.hash(&mut hasher);
                trace.hash(&mut hasher);
            }
            ObservationKind::JsException {
                message,
                line,
                column,
            } => {
                message.hash(&mut hasher);
                line.hash(&mut hasher);
                column.hash(&mut hasher);
            }
            ObservationKind::StateSnapshot { label } => label.hash(&mut hasher),
            ObservationKind::Metric { name, value } => {
                name.hash(&mut hasher);
                value.to_bits().hash(&mut hasher);
            }
            ObservationKind::Custom { channel } => channel.hash(&mut hasher),
            ObservationKind::Lifecycle {
                event_name,
                frame_id,
            } => {
                event_name.hash(&mut hasher);
                frame_id.hash(&mut hasher);
            }
            ObservationKind::ToolCall { tool, input, .. } => {
                tool.hash(&mut hasher);
                input.to_string().hash(&mut hasher);
            }
        }

        obs.data.to_string().hash(&mut hasher);
        hasher.finish()
    }

    /// Returns `true` if the hash was already present (duplicate).
    /// Inserts it on first sight.
    pub fn contains_or_insert(&self, hash: u64) -> bool {
        if self.cache.get(&hash).is_some() {
            return true;
        }
        self.cache.insert(hash, ());
        false
    }
}

impl Default for ObservationHashCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use daemon8_types::{AppName, Observation, ObservationKind, Origin, Severity};

    fn make_obs(severity: Severity, kind: ObservationKind) -> Observation {
        Observation {
            id: 0,
            origin: Origin::Application {
                name: AppName::from("test"),
            },
            kind,
            data: serde_json::json!({"msg": "test"}),
            severity,
            source_location: None,
            service: None,
            source: None,
            source_instance: None,
            timestamp_ns: 1_000,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            debug_session_id: None,
            checkpoint_id: None,
            error_hash: None,
        }
    }

    #[test]
    fn identical_observations_produce_same_hash() {
        let a = make_obs(Severity::Error, ObservationKind::Log);
        let b = make_obs(Severity::Error, ObservationKind::Log);
        assert_eq!(
            ObservationHashCache::dedup_fingerprint(&a),
            ObservationHashCache::dedup_fingerprint(&b)
        );
    }

    #[test]
    fn different_severity_produces_different_hash() {
        let a = make_obs(Severity::Error, ObservationKind::Log);
        let b = make_obs(Severity::Info, ObservationKind::Log);
        assert_ne!(
            ObservationHashCache::dedup_fingerprint(&a),
            ObservationHashCache::dedup_fingerprint(&b)
        );
    }

    #[test]
    fn cache_hit_on_duplicate() {
        let cache = ObservationHashCache::new();
        let obs = make_obs(Severity::Error, ObservationKind::Log);
        let hash = ObservationHashCache::dedup_fingerprint(&obs);

        assert!(!cache.contains_or_insert(hash));
        assert!(cache.contains_or_insert(hash));
    }

    #[test]
    fn volatile_metadata_excluded_from_fingerprint() {
        let mut a = make_obs(Severity::Info, ObservationKind::Log);
        let mut b = make_obs(Severity::Info, ObservationKind::Log);

        a.debug_session_id = Some(Arc::from("ds_aaa"));
        a.node_id = Some(Arc::from("node-1"));
        a.tags = Some(vec!["project:x".into()]);
        a.session_id = Some(Arc::from("mcp-1"));
        a.checkpoint_id = Some(Arc::from("cp-1"));
        a.correlation_id = Some(Arc::from("corr-1"));

        b.debug_session_id = Some(Arc::from("ds_zzz"));
        b.node_id = Some(Arc::from("node-2"));
        b.tags = Some(vec!["project:y".into()]);
        b.session_id = Some(Arc::from("mcp-2"));
        b.checkpoint_id = Some(Arc::from("cp-2"));
        b.correlation_id = Some(Arc::from("corr-2"));

        assert_eq!(
            ObservationHashCache::dedup_fingerprint(&a),
            ObservationHashCache::dedup_fingerprint(&b),
        );
    }

    #[test]
    fn different_data_produces_different_fingerprint() {
        let mut a = make_obs(Severity::Info, ObservationKind::Log);
        let mut b = make_obs(Severity::Info, ObservationKind::Log);
        a.data = serde_json::json!({"msg": "hello"});
        b.data = serde_json::json!({"msg": "world"});
        assert_ne!(
            ObservationHashCache::dedup_fingerprint(&a),
            ObservationHashCache::dedup_fingerprint(&b),
        );
    }

    #[test]
    fn query_same_sql_different_duration_same_fingerprint() {
        let a = make_obs(
            Severity::Info,
            ObservationKind::Query {
                sql: "SELECT 1".into(),
                duration_ms: 5.0,
            },
        );
        let b = make_obs(
            Severity::Info,
            ObservationKind::Query {
                sql: "SELECT 1".into(),
                duration_ms: 500.0,
            },
        );
        assert_eq!(
            ObservationHashCache::dedup_fingerprint(&a),
            ObservationHashCache::dedup_fingerprint(&b),
        );
    }
}
