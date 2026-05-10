// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_types::{Filter, Observation, OriginPattern, SourceActivator};

use crate::config::{FileSourceConfig, SourceConfig};

const REAPER_INTERVAL: Duration = Duration::from_secs(5 * 60);
#[cfg(test)]
const DEFAULT_IDLE_TTL_SECS: u64 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceState {
    Dormant,
    Active,
}

struct SourceEntry {
    config: FileSourceConfig,
    state: SourceState,
    last_accessed_ns: Arc<AtomicU64>,
    cancel: Option<CancellationToken>,
}

#[derive(Clone)]
pub struct SourceManager {
    sources: Arc<std::sync::Mutex<BTreeMap<String, SourceEntry>>>,
    obs_tx: mpsc::UnboundedSender<Observation>,
    tasks: Arc<tokio::sync::Mutex<JoinSet<()>>>,
    idle_ttl_ns: u64,
}

impl SourceManager {
    pub fn new(
        sources: BTreeMap<String, SourceConfig>,
        obs_tx: mpsc::UnboundedSender<Observation>,
        idle_ttl_secs: u64,
    ) -> Self {
        let mut entries = BTreeMap::new();
        for (name, source) in sources {
            match source {
                SourceConfig::File(cfg) => {
                    entries.insert(
                        name.clone(),
                        SourceEntry {
                            config: cfg,
                            state: SourceState::Dormant,
                            last_accessed_ns: Arc::new(AtomicU64::new(0)),
                            cancel: None,
                        },
                    );
                }
            }
        }

        Self {
            sources: Arc::new(std::sync::Mutex::new(entries)),
            obs_tx,
            tasks: Arc::new(tokio::sync::Mutex::new(JoinSet::new())),
            idle_ttl_ns: idle_ttl_secs * 1_000_000_000,
        }
    }

    pub async fn activate(&self, name: &str) -> bool {
        let (cfg, cancel_child) = {
            let guard = self.sources.lock().unwrap();
            let Some(entry) = guard.get(name) else {
                return false;
            };
            if entry.state == SourceState::Active {
                entry
                    .last_accessed_ns
                    .store(current_ns(), Ordering::Relaxed);
                return false;
            }
            (entry.config.clone(), CancellationToken::new())
        };

        {
            let mut tasks = self.tasks.lock().await;
            super::file_watcher::spawn_file_source(
                &mut tasks,
                name.to_owned(),
                cfg,
                self.obs_tx.clone(),
                cancel_child.clone(),
            );
        }

        {
            let mut guard = self.sources.lock().unwrap();
            if let Some(entry) = guard.get_mut(name) {
                entry.state = SourceState::Active;
                entry.cancel = Some(cancel_child);
                entry
                    .last_accessed_ns
                    .store(current_ns(), Ordering::Relaxed);
            }
        }

        tracing::info!(source = %name, "file source activated");
        true
    }

    fn deactivate(&self, name: &str) -> bool {
        let mut guard = self.sources.lock().unwrap();
        let Some(entry) = guard.get_mut(name) else {
            return false;
        };
        if entry.state != SourceState::Active {
            return false;
        }
        if let Some(cancel) = entry.cancel.take() {
            cancel.cancel();
        }
        entry.state = SourceState::Dormant;
        tracing::info!(source = %name, "file source deactivated");
        true
    }

    fn resolve_matching(&self, filter: &Filter) -> Vec<String> {
        let guard = self.sources.lock().unwrap();

        let mut matched: BTreeSet<&str> = BTreeSet::new();

        if let Some(ref origins) = filter.origins {
            for pattern in origins {
                match pattern {
                    OriginPattern::AnyApplication => {
                        for name in guard.keys() {
                            matched.insert(name);
                        }
                    }
                    OriginPattern::ApplicationNamed(app_name) => {
                        let key: &str = app_name.as_ref();
                        if guard.contains_key(key) {
                            matched.insert(key);
                        }
                    }
                    OriginPattern::AnyBrowser
                    | OriginPattern::BrowserTab(_)
                    | OriginPattern::AnyDevice
                    | OriginPattern::DeviceSerial(_) => {}
                }
            }
        }

        if let Some(ref tags) = filter.tags {
            for (name, entry) in guard.iter() {
                if entry.config.tags.iter().any(|t| tags.contains(t)) {
                    matched.insert(name);
                }
            }
        }

        matched.into_iter().map(String::from).collect()
    }

    async fn drain_completed_tasks(&self) {
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            if let Err(e) = result {
                tracing::warn!("file source task panicked: {e}");
            }
        }
    }

    async fn reap_idle(&self) {
        self.drain_completed_tasks().await;

        let now = current_ns();
        let stale: Vec<String> = {
            let guard = self.sources.lock().unwrap();
            guard
                .iter()
                .filter(|(_, entry)| {
                    entry.state == SourceState::Active
                        && now.saturating_sub(entry.last_accessed_ns.load(Ordering::Relaxed))
                            > self.idle_ttl_ns
                })
                .map(|(name, _)| name.clone())
                .collect()
        };

        if stale.is_empty() {
            return;
        }

        let mut count = 0u32;
        for name in &stale {
            if self.deactivate(name) {
                count += 1;
            }
        }

        tracing::info!(count, "reaped idle file sources");
    }

    async fn deactivate_all(&self) {
        let names: Vec<String> = {
            let guard = self.sources.lock().unwrap();
            guard
                .iter()
                .filter(|(_, e)| e.state == SourceState::Active)
                .map(|(n, _)| n.clone())
                .collect()
        };
        for name in &names {
            self.deactivate(name);
        }
    }

    pub fn spawn_reaper_task(self, tasks: &mut JoinSet<()>, cancel: CancellationToken) {
        tasks.spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(REAPER_INTERVAL) => {}
                    () = cancel.cancelled() => break,
                }
                self.reap_idle().await;
            }
            self.deactivate_all().await;
            tracing::debug!("source reaper task stopped");
        });
    }
}

impl SourceActivator for SourceManager {
    fn touch_matching(&self, filter: &Filter) {
        let now_ns = current_ns();
        let matching = self.resolve_matching(filter);
        if matching.is_empty() {
            return;
        }

        let mut to_activate = Vec::new();
        {
            let guard = self.sources.lock().unwrap();
            for name in &matching {
                if let Some(entry) = guard.get(name.as_str()) {
                    entry.last_accessed_ns.store(now_ns, Ordering::Relaxed);
                    if entry.state == SourceState::Dormant {
                        to_activate.push(name.clone());
                    }
                }
            }
        }

        if !to_activate.is_empty() {
            let mgr = self.clone();
            tokio::spawn(async move {
                for name in to_activate {
                    mgr.activate(&name).await;
                }
            });
        }
    }
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_types::{AppName, OriginPattern};

    fn test_file_config(tags: Vec<&str>) -> FileSourceConfig {
        FileSourceConfig {
            path: "/tmp/test.log".into(),
            parser: "line".into(),
            parser_pattern: None,
            tags: tags.into_iter().map(String::from).collect(),
        }
    }

    fn test_file_config_at(path: &str, tags: Vec<&str>) -> FileSourceConfig {
        FileSourceConfig {
            path: path.into(),
            parser: "line".into(),
            parser_pattern: None,
            tags: tags.into_iter().map(String::from).collect(),
        }
    }

    fn manager_with(entries: Vec<(&str, Vec<&str>)>) -> SourceManager {
        let mut sources = BTreeMap::new();
        for (name, tags) in entries {
            sources.insert(name.to_string(), SourceConfig::File(test_file_config(tags)));
        }
        let (tx, _rx) = mpsc::unbounded_channel();
        SourceManager::new(sources, tx, DEFAULT_IDLE_TTL_SECS)
    }

    fn manager_with_path(entries: Vec<(&str, &str, Vec<&str>)>) -> SourceManager {
        let mut sources = BTreeMap::new();
        for (name, path, tags) in entries {
            sources.insert(
                name.to_string(),
                SourceConfig::File(test_file_config_at(path, tags)),
            );
        }
        let (tx, _rx) = mpsc::unbounded_channel();
        SourceManager::new(sources, tx, DEFAULT_IDLE_TTL_SECS)
    }

    fn force_active(mgr: &SourceManager, name: &str, last_accessed: u64) {
        let mut guard = mgr.sources.lock().unwrap();
        if let Some(entry) = guard.get_mut(name) {
            entry.state = SourceState::Active;
            entry
                .last_accessed_ns
                .store(last_accessed, Ordering::Relaxed);
            entry.cancel = Some(CancellationToken::new());
        }
    }

    #[test]
    fn resolve_matching_by_origin_name() {
        let mgr = manager_with(vec![("laravel", vec![])]);
        let filter = Filter {
            origins: Some(vec![OriginPattern::ApplicationNamed(AppName::from(
                "laravel",
            ))]),
            ..Default::default()
        };
        assert_eq!(mgr.resolve_matching(&filter), vec!["laravel"]);
    }

    #[test]
    fn resolve_matching_by_tag() {
        let mgr = manager_with(vec![("app", vec!["php", "laravel"])]);
        let filter = Filter {
            tags: Some(vec!["php".into()]),
            ..Default::default()
        };
        assert_eq!(mgr.resolve_matching(&filter), vec!["app"]);
    }

    #[test]
    fn resolve_matching_empty_filter_activates_nothing() {
        let mgr = manager_with(vec![("app", vec!["php"])]);
        let filter = Filter::default();
        assert!(
            mgr.resolve_matching(&filter).is_empty(),
            "wildcard filter should not activate anything"
        );
    }

    #[test]
    fn resolve_matching_any_app_all() {
        let mgr = manager_with(vec![("a", vec![]), ("b", vec![])]);
        let filter = Filter {
            origins: Some(vec![OriginPattern::AnyApplication]),
            ..Default::default()
        };
        let mut result = mgr.resolve_matching(&filter);
        result.sort();
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn resolve_matching_browser_ignored() {
        let mgr = manager_with(vec![("app", vec![])]);
        let filter = Filter {
            origins: Some(vec![OriginPattern::AnyBrowser]),
            ..Default::default()
        };
        assert!(
            mgr.resolve_matching(&filter).is_empty(),
            "browser patterns should not match file sources"
        );
    }

    #[test]
    fn touch_updates_atomic() {
        let mgr = manager_with(vec![("app", vec!["php"])]);
        force_active(&mgr, "app", 0);

        let filter = Filter {
            tags: Some(vec!["php".into()]),
            ..Default::default()
        };
        mgr.touch_matching(&filter);

        let guard = mgr.sources.lock().unwrap();
        let accessed = guard["app"].last_accessed_ns.load(Ordering::Relaxed);
        assert!(accessed > 0, "last_accessed_ns should have been updated");
    }

    #[tokio::test]
    async fn reap_idle_deactivates() {
        let mgr = manager_with(vec![("stale", vec![])]);
        force_active(&mgr, "stale", 0);

        mgr.reap_idle().await;

        let guard = mgr.sources.lock().unwrap();
        assert_eq!(
            guard["stale"].state,
            SourceState::Dormant,
            "stale source should have been reaped"
        );
    }

    #[tokio::test]
    async fn reap_preserves_recent() {
        let mgr = manager_with(vec![("fresh", vec![])]);
        force_active(&mgr, "fresh", current_ns());

        mgr.reap_idle().await;

        let guard = mgr.sources.lock().unwrap();
        assert_eq!(
            guard["fresh"].state,
            SourceState::Active,
            "recently accessed source should survive reaping"
        );
    }

    #[tokio::test]
    async fn activate_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        std::fs::write(&log, "").unwrap();

        let mgr = manager_with_path(vec![("app", log.to_str().unwrap(), vec!["php"])]);

        assert!(mgr.activate("app").await);
        assert!(
            !mgr.activate("app").await,
            "second activate must be a no-op"
        );

        let guard = mgr.sources.lock().unwrap();
        assert_eq!(guard["app"].state, SourceState::Active);
        assert!(
            guard["app"].last_accessed_ns.load(Ordering::Relaxed) > 0,
            "timestamp should be refreshed on idempotent activate"
        );
    }

    #[tokio::test]
    async fn deactivate_and_reactivate() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        std::fs::write(&log, "").unwrap();

        let mgr = manager_with_path(vec![("app", log.to_str().unwrap(), vec![])]);

        assert!(mgr.activate("app").await);
        assert!(mgr.deactivate("app"));

        {
            let guard = mgr.sources.lock().unwrap();
            assert_eq!(guard["app"].state, SourceState::Dormant);
            assert!(guard["app"].cancel.is_none());
        }

        assert!(
            mgr.activate("app").await,
            "re-activation after deactivate must succeed"
        );

        let guard = mgr.sources.lock().unwrap();
        assert_eq!(guard["app"].state, SourceState::Active);
        assert!(guard["app"].cancel.is_some());
    }

    #[tokio::test]
    async fn touch_matching_activates_dormant() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        std::fs::write(&log, "").unwrap();

        let mgr = manager_with_path(vec![("app", log.to_str().unwrap(), vec!["php"])]);

        let filter = Filter {
            tags: Some(vec!["php".into()]),
            ..Default::default()
        };
        mgr.touch_matching(&filter);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let guard = mgr.sources.lock().unwrap();
        assert_eq!(
            guard["app"].state,
            SourceState::Active,
            "dormant source matching filter should be activated by touch_matching"
        );
    }
}
