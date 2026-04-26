// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::VecDeque;
use std::sync::Arc;

use daemon8_types::{Filter, Observation};
use serde::Serialize;
use tokio::sync::{Mutex, broadcast};

const DEFAULT_CAPACITY: usize = 200;

#[derive(Debug, Serialize)]
pub struct LensStatus {
    pub active: bool,
    pub filter: Option<Filter>,
    pub buffered: usize,
    pub capacity: usize,
    pub cursor: u64,
}

struct ActiveLens {
    filter: Filter,
    buffer: VecDeque<Observation>,
    capacity: usize,
    cursor: u64,
}

impl ActiveLens {
    fn new(filter: Filter, capacity: usize) -> Self {
        Self {
            filter,
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            cursor: 0,
        }
    }

    fn push(&mut self, obs: Observation) {
        if !self.filter.matches(&obs) {
            return;
        }
        if obs.id > self.cursor {
            self.cursor = obs.id;
        }
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(obs);
    }

    fn drain(&mut self) -> Vec<Observation> {
        self.buffer.drain(..).collect()
    }

    fn status(&self) -> LensStatus {
        LensStatus {
            active: true,
            filter: Some(self.filter.clone()),
            buffered: self.buffer.len(),
            capacity: self.capacity,
            cursor: self.cursor,
        }
    }
}

pub struct LensManager {
    inner: Arc<Mutex<Option<ActiveLens>>>,
}

impl LensManager {
    pub fn new(broadcast_rx: broadcast::Receiver<(Arc<Observation>, Arc<str>)>) -> Self {
        let inner: Arc<Mutex<Option<ActiveLens>>> = Arc::new(Mutex::new(None));

        let pump_inner = inner.clone();
        tokio::spawn(async move {
            Self::pump(broadcast_rx, pump_inner).await;
        });

        Self { inner }
    }

    async fn pump(
        mut rx: broadcast::Receiver<(Arc<Observation>, Arc<str>)>,
        inner: Arc<Mutex<Option<ActiveLens>>>,
    ) {
        loop {
            match rx.recv().await {
                Ok((obs, _json)) => {
                    let mut guard = inner.lock().await;
                    if let Some(lens) = guard.as_mut() {
                        lens.push((*obs).clone());
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "lens broadcast receiver lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    pub async fn set(&self, filter: Filter) {
        self.set_with_capacity(filter, DEFAULT_CAPACITY).await;
    }

    pub async fn set_with_capacity(&self, filter: Filter, capacity: usize) {
        let mut guard = self.inner.lock().await;
        *guard = Some(ActiveLens::new(filter, capacity));
    }

    pub async fn clear(&self) {
        let mut guard = self.inner.lock().await;
        *guard = None;
    }

    pub async fn drain(&self) -> Vec<Observation> {
        let mut guard = self.inner.lock().await;
        match guard.as_mut() {
            Some(lens) => lens.drain(),
            None => Vec::new(),
        }
    }

    pub async fn status(&self) -> LensStatus {
        let guard = self.inner.lock().await;
        match guard.as_ref() {
            Some(lens) => lens.status(),
            None => LensStatus {
                active: false,
                filter: None,
                buffered: 0,
                capacity: 0,
                cursor: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_types::{ObservationKind, Origin, Severity};

    fn make_obs(id: u64, severity: Severity, tag: &str) -> Observation {
        Observation {
            id,
            origin: Origin::Application {
                name: "test".into(),
            },
            kind: ObservationKind::Log,
            data: serde_json::json!({"msg": tag}),
            severity,
            source_location: None,
            timestamp_ns: id * 1000,
            correlation_id: None,
            parent_id: None,
            tags: Some(vec![tag.to_string()]),
            session_id: None,
            node_id: None,
        }
    }

    #[test]
    fn active_lens_filters_and_buffers() {
        let filter = Filter {
            tags: Some(vec!["important".to_string()]),
            ..Default::default()
        };
        let mut lens = ActiveLens::new(filter, 10);

        lens.push(make_obs(1, Severity::Info, "important"));
        lens.push(make_obs(2, Severity::Info, "noise"));
        lens.push(make_obs(3, Severity::Error, "important"));

        assert_eq!(lens.buffer.len(), 2);
        assert_eq!(lens.cursor, 3);
    }

    #[test]
    fn ring_buffer_evicts_oldest() {
        let filter = Filter::default();
        let mut lens = ActiveLens::new(filter, 3);

        for i in 1..=5 {
            lens.push(make_obs(i, Severity::Info, "x"));
        }

        assert_eq!(lens.buffer.len(), 3);
        assert_eq!(lens.buffer[0].id, 3);
        assert_eq!(lens.buffer[2].id, 5);
    }

    #[test]
    fn drain_empties_buffer() {
        let filter = Filter::default();
        let mut lens = ActiveLens::new(filter, 10);
        lens.push(make_obs(1, Severity::Info, "a"));
        lens.push(make_obs(2, Severity::Info, "b"));

        let drained = lens.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(lens.buffer.len(), 0);
        assert_eq!(lens.cursor, 2);
    }

    #[tokio::test]
    async fn lens_manager_lifecycle() {
        let (tx, rx) = broadcast::channel::<(Arc<Observation>, Arc<str>)>(64);
        let manager = LensManager::new(rx);

        let status = manager.status().await;
        assert!(!status.active);

        manager
            .set(Filter {
                severity_min: Some(Severity::Warn),
                ..Default::default()
            })
            .await;

        let obs_warn = Arc::new(make_obs(1, Severity::Warn, "w"));
        let obs_info = Arc::new(make_obs(2, Severity::Info, "i"));
        let obs_error = Arc::new(make_obs(3, Severity::Error, "e"));

        let json_w: Arc<str> = Arc::from(serde_json::to_string(&*obs_warn).unwrap().as_str());
        let json_i: Arc<str> = Arc::from(serde_json::to_string(&*obs_info).unwrap().as_str());
        let json_e: Arc<str> = Arc::from(serde_json::to_string(&*obs_error).unwrap().as_str());

        tx.send((obs_warn, json_w)).unwrap();
        tx.send((obs_info, json_i)).unwrap();
        tx.send((obs_error, json_e)).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let status = manager.status().await;
        assert!(status.active);
        assert_eq!(status.buffered, 2);

        let drained = manager.drain().await;
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].severity, Severity::Warn);
        assert_eq!(drained[1].severity, Severity::Error);

        let status = manager.status().await;
        assert_eq!(status.buffered, 0);

        manager.clear().await;
        assert!(!manager.status().await.active);
    }
}
