// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! End-to-end tests for the discovery scanner (D3 / Commit 3).
//!
//! The scanner orchestrates classification, librarian lookup,
//! filesystem probing, hint emission, and the agent-wait poll loop.
//! These tests exercise it against a real in-process SurrealDB so the
//! librarian read/write path is the same one the daemon uses in
//! production.
//!
//! The scanner module is private to the `daemon8` binary, so we drive
//! it through a small bin-side `pub use` re-export that lives in
//! `crates/daemon/src/discovery/scanner.rs`. The integration target
//! consumes the bin crate as a regular Rust library — tests in
//! `tests/` see all `pub` items in the bin source.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daemon8_store::{LibrarianEdge, LibrarianNode, LibrarianStore, SurrealStore};
use daemon8_types::{
    LibrarianEdgeKind, LibrarianNodeKind, LocatorKind, Observation, Platform, ProjectNodeData,
    SourceKind, SourceTemplateData, TemplateConfidence,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

// We need a fixture project root the classifier accepts. `daemon8_providers::classify`
// requires `.git` (or some manifest) to tag the project; without any
// tags the scanner returns early without emitting a hint. A bare temp
// dir plus an empty `.git` directory produces the universal `git-repo`
// tag, which is enough to exercise the hint path.
fn write_fixture_project(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join(".git")).expect("create .git");
    // package.json with react-native and a known version so the
    // classifier emits the `react-native` tag and frameworks map.
    let pkg = serde_json::json!({
        "name": "scanner-fixture",
        "dependencies": {
            "react-native": "0.74.5"
        }
    });
    std::fs::write(
        dir.join("package.json"),
        serde_json::to_string_pretty(&pkg).unwrap(),
    )
    .expect("write package.json");
}

fn template_for(project_types: &[&str], pattern: &str) -> SourceTemplateData {
    SourceTemplateData {
        project_types: project_types.iter().map(|s| (*s).to_string()).collect(),
        kind: SourceKind::Log,
        locator_pattern: pattern.to_string(),
        platforms: vec![Platform::current()],
        parser_hint: None,
        default_tags: vec!["fixture".into()],
        description: "fixture template".into(),
        version_constraint: None,
        discovered_by_session: None,
        discovered_by_provider: None,
        discovered_at_ns: now_ns(),
        verified_count: 0,
        last_verified_at_ns: now_ns(),
        confidence: TemplateConfidence::AgentDiscovered,
    }
}

fn template_node(data: &SourceTemplateData, label: &str) -> LibrarianNode {
    LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceTemplate,
        label: label.into(),
        locator_kind: LocatorKind::File,
        locator: data.locator_pattern.clone(),
        tags: data.project_types.clone(),
        project_slug: String::new(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(data).unwrap()),
    }
}

#[tokio::test]
async fn scan_emits_hint_then_resolves_when_template_written_mid_wait() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());

    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    let (obs_tx, mut obs_rx) = mpsc::unbounded_channel::<Observation>();
    let cancel = CancellationToken::new();

    // The scanner runs in a task so we can mutate the librarian while
    // it's in the wait loop. Short wait_timeout so the test isn't slow
    // if the resolution doesn't happen.
    let cfg = daemon8::discovery::scanner::ScannerConfig {
        wait_timeout: Duration::from_secs(6),
        poll_interval: Duration::from_millis(250),
        cache_max_age: Duration::from_secs(7 * 24 * 60 * 60),
    };

    let root_for_scan = tmp.path().to_path_buf();
    let lib_for_scan = lib.clone();
    let cancel_for_scan = cancel.clone();
    let scan_task = tokio::spawn(async move {
        daemon8::discovery::scanner::scan(
            &root_for_scan,
            &*lib_for_scan,
            &obs_tx,
            Vec::new(),
            cfg,
            cancel_for_scan,
            None,
        )
        .await
    });

    // The hint should appear on the observation channel within a
    // moment of scan() being called.
    let hint = tokio::time::timeout(Duration::from_secs(2), obs_rx.recv())
        .await
        .expect("hint observation should arrive")
        .expect("channel open");
    match &hint.kind {
        daemon8_types::ObservationKind::Custom { channel } => {
            assert_eq!(channel, "discovery_hint");
        }
        other => panic!("expected Custom kind, got {other:?}"),
    }

    // Write a template that resolves to a real path inside the fixture
    // project. The scanner's poll loop should pick it up on the next
    // tick and consider the react-native tag covered.
    let log_path = tmp.path().join("runtime.log");
    std::fs::write(&log_path, "ok").unwrap();
    let pattern = format!("{}/runtime.log", tmp.path().display());
    // git-repo template too so every classification tag is covered.
    let t1 = template_for(&["react-native"], &pattern);
    let t2 = template_for(&["git-repo"], &pattern);
    lib.index_node(template_node(&t1, "react-native log"))
        .await
        .unwrap();
    lib.index_node(template_node(&t2, "git-repo log"))
        .await
        .unwrap();

    let plan = tokio::time::timeout(Duration::from_secs(8), scan_task)
        .await
        .expect("scan task should finish")
        .expect("join")
        .expect("scan ok");

    assert!(
        !plan.awaiting_agent,
        "plan should not be awaiting agent once tags are covered; plan={plan:?}"
    );
    assert!(
        !plan.resolved_sources.is_empty(),
        "expected at least one resolved source; plan={plan:?}"
    );
    assert!(
        plan.resolved_sources
            .iter()
            .any(|s| s.resolved_path == log_path),
        "resolved source list should contain the fixture log path"
    );
}

#[tokio::test]
async fn scan_cancellation_returns_promptly_mid_wait() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    let (obs_tx, _obs_rx) = mpsc::unbounded_channel::<Observation>();
    let cancel = CancellationToken::new();

    // 60s timeout so the only way the scanner returns within the test
    // budget is via the cancellation token.
    let cfg = daemon8::discovery::scanner::ScannerConfig {
        wait_timeout: Duration::from_secs(60),
        poll_interval: Duration::from_secs(5),
        cache_max_age: Duration::from_secs(7 * 24 * 60 * 60),
    };
    let lib_for_scan = lib.clone();
    let cancel_for_scan = cancel.clone();
    let root_for_scan = tmp.path().to_path_buf();
    let scan_task = tokio::spawn(async move {
        daemon8::discovery::scanner::scan(
            &root_for_scan,
            &*lib_for_scan,
            &obs_tx,
            Vec::new(),
            cfg,
            cancel_for_scan,
            None,
        )
        .await
    });

    // Give the scanner a moment to enter the wait loop, then cancel.
    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel.cancel();

    let plan = tokio::time::timeout(Duration::from_secs(2), scan_task)
        .await
        .expect("cancel should free the task within 2s")
        .expect("join")
        .expect("scan ok");
    assert!(!plan.awaiting_agent);
}

#[tokio::test]
async fn scan_uses_cache_when_project_node_is_fresh() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    // Pre-seed: project node + has_source edge to a real file.
    let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
    let project_data = ProjectNodeData {
        root_path: canonical_root.clone(),
        slug: "scanner-fixture".into(),
        classification_tags: vec!["react-native".into(), "git-repo".into()],
        framework_versions: {
            let mut m = BTreeMap::new();
            m.insert("react-native".into(), "0.74.5".into());
            m
        },
        platform: Platform::current(),
        created_at_ns: now_ns(),
        // 1 second ago — well inside the 7-day cache window.
        last_serve_at_ns: now_ns().saturating_sub(1_000_000_000),
        skip_discovery: false,
    };
    let project_node = LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::Project,
        label: "scanner-fixture".into(),
        locator_kind: LocatorKind::File,
        locator: canonical_root.to_string_lossy().to_string(),
        tags: vec!["scanner-fixture".into()],
        project_slug: "scanner-fixture".into(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&project_data).unwrap()),
    };
    let project_id = lib.index_node(project_node).await.unwrap();

    let log_path = tmp.path().join("cached.log");
    std::fs::write(&log_path, "ok").unwrap();
    let instance = LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::Doc, // any non-source_template/non-project kind avoids the validators
        label: "cached log".into(),
        locator_kind: LocatorKind::File,
        locator: log_path.to_string_lossy().to_string(),
        tags: vec!["log".into(), "fixture".into()],
        project_slug: "scanner-fixture".into(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: None,
    };
    let instance_id = lib.index_node(instance).await.unwrap();
    lib.index_edge(LibrarianEdge {
        id: None,
        kind: LibrarianEdgeKind::HasSource,
        from_node: project_id,
        to_node: instance_id,
        created_at: now_ns(),
    })
    .await
    .unwrap();

    let (obs_tx, mut obs_rx) = mpsc::unbounded_channel::<Observation>();
    let cancel = CancellationToken::new();
    let cfg = daemon8::discovery::scanner::ScannerConfig {
        wait_timeout: Duration::from_secs(2),
        poll_interval: Duration::from_millis(250),
        cache_max_age: Duration::from_secs(7 * 24 * 60 * 60),
    };
    let plan = daemon8::discovery::scanner::scan(
        tmp.path(),
        &*lib,
        &obs_tx,
        Vec::new(),
        cfg,
        cancel,
        None,
    )
    .await
    .expect("scan ok");

    assert!(plan.cache_used, "fresh project node should hit cache");
    assert_eq!(
        plan.librarian_status,
        daemon8::discovery::scanner::LibrarianStatus::CacheHit,
    );
    assert!(
        plan.resolved_sources
            .iter()
            .any(|s| s.resolved_path == log_path),
        "cached has_source edge should produce a resolved source"
    );
    assert!(!plan.awaiting_agent);

    // No hint should have been emitted on the cache hit path.
    let no_hint = tokio::time::timeout(Duration::from_millis(200), obs_rx.recv()).await;
    assert!(
        no_hint.is_err(),
        "no hint should be emitted on cache hit; saw {no_hint:?}"
    );
}

#[tokio::test]
async fn scan_marks_stale_when_project_node_last_serve_old() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());
    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    let canonical_root = std::fs::canonicalize(tmp.path()).unwrap();
    // 30 days ago, well past the 7-day cache window.
    let old_ns = now_ns().saturating_sub(30u64 * 24 * 60 * 60 * 1_000_000_000);
    let project_data = ProjectNodeData {
        root_path: canonical_root.clone(),
        slug: "scanner-fixture".into(),
        classification_tags: vec!["react-native".into(), "git-repo".into()],
        framework_versions: BTreeMap::new(),
        platform: Platform::current(),
        created_at_ns: old_ns,
        last_serve_at_ns: old_ns,
        skip_discovery: false,
    };
    let project_node = LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::Project,
        label: "scanner-fixture".into(),
        locator_kind: LocatorKind::File,
        locator: canonical_root.to_string_lossy().to_string(),
        tags: vec!["scanner-fixture".into()],
        project_slug: "scanner-fixture".into(),
        version: String::new(),
        parent_id: None,
        created_at: old_ns,
        updated_at: old_ns,
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&project_data).unwrap()),
    };
    lib.index_node(project_node).await.unwrap();

    let (obs_tx, _obs_rx) = mpsc::unbounded_channel::<Observation>();
    let cancel = CancellationToken::new();
    // Very short wait_timeout because templates_missing path will hit
    // the timeout (no templates exist).
    let cfg = daemon8::discovery::scanner::ScannerConfig {
        wait_timeout: Duration::from_millis(750),
        poll_interval: Duration::from_millis(200),
        cache_max_age: Duration::from_secs(7 * 24 * 60 * 60),
    };
    let plan = daemon8::discovery::scanner::scan(
        tmp.path(),
        &*lib,
        &obs_tx,
        Vec::new(),
        cfg,
        cancel,
        None,
    )
    .await
    .expect("scan ok");

    // Stale cache forces a re-probe; with no templates this lands in
    // TemplatesMissing after the wait window expires.
    assert!(!plan.cache_used);
    assert!(matches!(
        plan.librarian_status,
        daemon8::discovery::scanner::LibrarianStatus::TemplatesMissing
            | daemon8::discovery::scanner::LibrarianStatus::TemplatesPartial,
    ));
}
