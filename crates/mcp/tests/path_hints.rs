// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration coverage for D7 path-pattern hints. Drives the
//! `query_observations` handler end-to-end against an in-process MCP
//! and asserts that:
//!
//! 1. An observation referencing `/tmp/*.log` produces a hint in the
//!    response envelope when the librarian has no covering template.
//! 2. The same query with a covering template installed suppresses the
//!    hint.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use daemon8_chrome::ConnectionState;
use daemon8_mcp::{ActiveProjectHandle, DaemonMcp, DaemonMcpConfig};
use daemon8_store::{LibrarianNode, LibrarianStore, StateModel, SurrealStore};
use daemon8_types::{
    LibrarianNodeKind, LocatorKind, Observation, ObservationKind, Origin, Platform,
    ProjectClassification, Severity, SourceKind, SourceTemplateData, TemplateConfidence,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

async fn build_mcp(
    librarian: Arc<dyn LibrarianStore>,
    store: Arc<SurrealStore>,
    classification: Option<ProjectClassification>,
) -> DaemonMcp {
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(
        broadcast_tx.subscribe(),
        None,
    ));
    let active_project: ActiveProjectHandle = Arc::new(RwLock::new(classification));
    DaemonMcp::new(DaemonMcpConfig {
        store: store.clone(),
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: None,
        librarian_store: Some(librarian),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        broadcast_tx,
        lens,
        setup_tool_fn: None,
        hooks_tool_fn: None,
        source_activator: None,
        cancel: CancellationToken::new(),
        active_project,
    })
}

fn classification_for_react_native() -> ProjectClassification {
    ProjectClassification {
        tags: vec!["react-native".into(), "git-repo".into()],
        framework_versions: BTreeMap::new(),
        root: PathBuf::from("/tmp/fake-project"),
        manifests: BTreeMap::new(),
        platform: Platform::current(),
    }
}

fn metro_template() -> LibrarianNode {
    let data = SourceTemplateData {
        project_types: vec!["react-native".into()],
        kind: SourceKind::Log,
        locator_pattern: "/tmp/metro.log".into(),
        platforms: vec![Platform::current()],
        parser_hint: None,
        default_tags: vec!["metro".into()],
        description: "Metro bundler log".into(),
        version_constraint: None,
        discovered_by_session: None,
        discovered_by_provider: None,
        discovered_at_ns: 0,
        verified_count: 0,
        last_verified_at_ns: 0,
        confidence: TemplateConfidence::AgentDiscovered,
    };
    LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceTemplate,
        label: "Metro bundler log".into(),
        locator_kind: LocatorKind::File,
        locator: "/tmp/metro.log".into(),
        tags: vec!["react-native".into()],
        project_slug: "fake-project".into(),
        version: "2026.05.13".into(),
        parent_id: None,
        created_at: 0,
        updated_at: 0,
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&data).unwrap()),
    }
}

async fn seed_observation(store: &Arc<SurrealStore>, path: &str) {
    let obs = Observation::new(
        Origin::Application {
            name: "fake-app".into(),
        },
        ObservationKind::Log,
        serde_json::json!({
            "message": format!("found data at {path}"),
            "path": path,
        }),
        Severity::Info,
        None,
    );
    store.insert(obs).await.unwrap();
}

#[tokio::test]
async fn query_observations_emits_hint_when_template_missing() {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());

    seed_observation(&store, "/tmp/metro.log").await;

    let mcp = build_mcp(
        lib.clone(),
        store.clone(),
        Some(classification_for_react_native()),
    )
    .await;

    let rendered = mcp.query_observations_for_tests().await;
    let envelope: serde_json::Value =
        serde_json::from_str(&rendered).expect("envelope JSON parse failed");
    let hints = envelope["daemon8"]["hints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !hints.is_empty(),
        "expected at least one hint, got envelope: {envelope}"
    );
    let joined = hints
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("/tmp/metro.log"),
        "hint should mention path: {joined}"
    );
    assert!(
        joined.contains("source_template"),
        "hint should mention source_template: {joined}"
    );
}

#[tokio::test]
async fn query_observations_suppresses_hint_when_template_covers_path() {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());
    lib.index_node(metro_template()).await.unwrap();

    seed_observation(&store, "/tmp/metro.log").await;

    let mcp = build_mcp(
        lib.clone(),
        store.clone(),
        Some(classification_for_react_native()),
    )
    .await;

    let rendered = mcp.query_observations_for_tests().await;
    let envelope: serde_json::Value =
        serde_json::from_str(&rendered).expect("envelope JSON parse failed");
    let hints = envelope["daemon8"]["hints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        hints.is_empty(),
        "expected zero hints when template covers /tmp/metro.log, got: {envelope}"
    );
}
