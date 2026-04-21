// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use daemon8_types::{
    Checkpoint, ConnectionInfo, Filter, HealthStatus, Observation, Origin, OriginPattern,
    RuntimeSummary, Severity, SliceSummary, StateSlice,
};

use crate::{StateModel, StoreError};

struct Inner {
    observations: Vec<Observation>,
    connections: Vec<ConnectionInfo>,
}

pub struct MemoryStore {
    inner: Mutex<Inner>,
    next_id: AtomicU64,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                observations: Vec::new(),
                connections: Vec::new(),
            }),
            next_id: AtomicU64::new(1),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StateModel for MemoryStore {
    fn insert(&self, mut obs: Observation) -> Result<u64, StoreError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        obs.id = id;

        let mut guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        guard.observations.push(obs);
        Ok(id)
    }

    fn query(&self, filter: &Filter) -> Result<StateSlice, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;

        let mut results: Vec<&Observation> = guard.observations.iter().collect();

        // severity_min
        if let Some(min) = filter.severity_min {
            results.retain(|o| o.severity.level() >= min.level());
        }

        // kinds
        if let Some(ref kinds) = filter.kinds
            && !kinds.is_empty()
        {
            results.retain(|o| kinds.contains(&o.kind.tag()));
        }

        // since checkpoint
        if let Some(ref cp) = filter.since {
            results.retain(|o| o.id > cp.0);
        }

        // text_match (substring on JSON-serialized data)
        if let Some(ref text) = filter.text_match {
            results.retain(|o| {
                let data_str = serde_json::to_string(&o.data).unwrap_or_default();
                data_str.contains(text.as_str())
            });
        }

        // origins
        if let Some(ref origins) = filter.origins {
            results.retain(|o| {
                origins.iter().any(|pattern| match pattern {
                    OriginPattern::AnyApplication => matches!(o.origin, Origin::Application { .. }),
                    OriginPattern::ApplicationNamed(name) => {
                        matches!(&o.origin, Origin::Application { name: n } if n == name)
                    }
                    OriginPattern::AnyBrowser => matches!(o.origin, Origin::Browser { .. }),
                    OriginPattern::BrowserTab(tab_id) => {
                        matches!(&o.origin, Origin::Browser { tab_id: t, .. } if t == tab_id)
                    }
                    OriginPattern::AnyDevice => matches!(o.origin, Origin::Device { .. }),
                    OriginPattern::DeviceSerial(serial) => {
                        matches!(&o.origin, Origin::Device { serial: s, .. } if s == serial)
                    }
                })
            });
        }

        // limit
        if let Some(n) = filter.limit {
            results.truncate(n);
        }

        // Baseline checkpoint for empty results: use the filter's since value
        // (already caught up to there) or current store sequence (caught up to now).
        let max_id = results.iter().map(|o| o.id).max().unwrap_or(0);
        let result_checkpoint = if max_id > 0 {
            max_id
        } else {
            match filter.since {
                Some(ref cp) => cp.0,
                None => guard.observations.iter().map(|o| o.id).max().unwrap_or(0),
            }
        };

        let owned: Vec<Observation> = results.into_iter().cloned().collect();
        let summary = build_slice_summary(&owned);

        Ok(StateSlice {
            observations: owned,
            checkpoint: Checkpoint(result_checkpoint),
            summary,
        })
    }

    fn summary(&self) -> Result<RuntimeSummary, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;

        let total = guard.observations.len() as u64;

        // Find the max timestamp to define "last 60s" window
        let max_ts = guard
            .observations
            .iter()
            .map(|o| o.timestamp_ns)
            .max()
            .unwrap_or(0);
        let cutoff = max_ts.saturating_sub(60_000_000_000);

        let error_count = guard
            .observations
            .iter()
            .filter(|o| o.severity == Severity::Error && o.timestamp_ns > cutoff)
            .count() as u64;

        let mut channels: Vec<String> = guard
            .observations
            .iter()
            .map(|o| o.kind.tag().to_string())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        channels.sort();

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
        let guard = self.inner.lock().expect("memory store mutex poisoned");
        let max_id = guard.observations.iter().map(|o| o.id).max().unwrap_or(0);
        Checkpoint(max_id)
    }

    fn oldest_id(&self) -> Option<u64> {
        let guard = self.inner.lock().expect("memory store mutex poisoned");
        guard.observations.iter().map(|o| o.id).min()
    }

    fn cleanup_before(&self, timestamp_ns: u64) -> Result<u64, StoreError> {
        let mut guard = self.inner.lock().map_err(|_| StoreError::LockPoisoned)?;
        let before = guard.observations.len();
        guard
            .observations
            .retain(|o| o.timestamp_ns >= timestamp_ns);
        let after = guard.observations.len();
        Ok((before - after) as u64)
    }

    fn vacuum_incremental(&self, _pages: u32) -> Result<(), StoreError> {
        Ok(())
    }

    fn wal_checkpoint(&self) -> Result<(), StoreError> {
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
    use daemon8_types::{ObservationKind, ObservationKindTag};

    fn make_obs(severity: Severity, kind: ObservationKind) -> Observation {
        Observation {
            id: 0,
            origin: Origin::Application {
                name: "test-app".into(),
            },
            kind,
            data: serde_json::json!({"msg": "hello"}),
            severity,
            source_location: None,
            timestamp_ns: 1_700_000_000_000_000_000,
        }
    }

    #[test]
    fn insert_and_query_round_trip() {
        let store = MemoryStore::new();

        let obs = make_obs(Severity::Info, ObservationKind::Log);
        let id = store.insert(obs).unwrap();
        assert!(id > 0);

        let slice = store.query(&Filter::default()).unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].id, id);
        assert_eq!(slice.observations[0].severity, Severity::Info);
    }

    #[test]
    fn checkpoint_advances() {
        let store = MemoryStore::new();

        // Empty store starts at checkpoint 0
        assert_eq!(store.checkpoint().0, 0);

        let id1 = store
            .insert(make_obs(Severity::Debug, ObservationKind::Log))
            .unwrap();
        assert_eq!(store.checkpoint().0, id1);

        let id2 = store
            .insert(make_obs(Severity::Warn, ObservationKind::Log))
            .unwrap();
        assert!(id2 > id1);
        assert_eq!(store.checkpoint().0, id2);

        // Query since first checkpoint should only return the second observation
        let filter = Filter {
            since: Some(Checkpoint(id1)),
            ..Default::default()
        };
        let slice = store.query(&filter).unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].id, id2);
    }

    #[test]
    fn severity_filter() {
        let store = MemoryStore::new();
        store
            .insert(make_obs(Severity::Debug, ObservationKind::Log))
            .unwrap();
        store
            .insert(make_obs(Severity::Error, ObservationKind::Log))
            .unwrap();

        let filter = Filter {
            severity_min: Some(Severity::Warn),
            ..Default::default()
        };
        let slice = store.query(&filter).unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].severity, Severity::Error);
    }

    #[test]
    fn kind_filter() {
        let store = MemoryStore::new();
        store
            .insert(make_obs(Severity::Info, ObservationKind::Log))
            .unwrap();
        store
            .insert(make_obs(
                Severity::Info,
                ObservationKind::Exception {
                    message: "boom".into(),
                    trace: None,
                },
            ))
            .unwrap();

        let filter = Filter {
            kinds: Some(vec![ObservationKindTag::Exception]),
            ..Default::default()
        };
        let slice = store.query(&filter).unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.summary.counts_by_kind.get("exception"), Some(&1));
    }

    #[test]
    fn cleanup_removes_old_observations() {
        let store = MemoryStore::new();

        let mut old = make_obs(Severity::Info, ObservationKind::Log);
        old.timestamp_ns = 1_000;
        store.insert(old).unwrap();

        let mut recent = make_obs(Severity::Info, ObservationKind::Log);
        recent.timestamp_ns = 2_000;
        store.insert(recent).unwrap();

        let deleted = store.cleanup_before(1_500).unwrap();
        assert_eq!(deleted, 1);

        let slice = store.query(&Filter::default()).unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].timestamp_ns, 2_000);
    }
}
