// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! End-to-end tests for the D4 first-run presentation flow.
//!
//! Covers the post-scan path: render the plan, call the registrar
//! (auto-confirmed because the test harness is non-TTY), and assert
//! librarian state. The skip path also runs through `mark_skip` and
//! checks that the on-disk marker is written.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use daemon8_store::{LibrarianFilter, LibrarianStore, SurrealStore};
use daemon8_types::{
    LibrarianEdgeKind, LibrarianNodeKind, Platform, ProjectClassification, ProjectNodeData,
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

fn write_fixture_project(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join(".git")).expect("create .git");
    let pkg = serde_json::json!({
        "name": "presentation-fixture",
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
        parser_hint: Some("line".into()),
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

fn template_node(data: &SourceTemplateData, label: &str) -> daemon8_store::LibrarianNode {
    daemon8_store::LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceTemplate,
        label: label.into(),
        locator_kind: daemon8_types::LocatorKind::File,
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
async fn registrar_persists_source_instance_and_edges_when_confirmed() {
    use daemon8::discovery::{registrar, scanner};

    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());

    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    // Seed templates that resolve to fixture files so the scanner can
    // produce a non-empty DiscoveryPlan without going through the
    // agent-wait loop.
    let log_path = tmp.path().join("runtime.log");
    std::fs::write(&log_path, "ok").unwrap();
    let pattern = format!("{}/runtime.log", tmp.path().display());

    let t_rn = template_for(&["react-native"], &pattern);
    let t_git = template_for(&["git-repo"], &pattern);
    lib.index_node(template_node(&t_rn, "rn"))
        .await
        .expect("seed rn template");
    lib.index_node(template_node(&t_git, "git"))
        .await
        .expect("seed git template");

    let (obs_tx, _obs_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let plan = scanner::scan(
        tmp.path(),
        &*lib,
        &obs_tx,
        Vec::new(),
        scanner::ScannerConfig {
            wait_timeout: std::time::Duration::from_millis(500),
            poll_interval: std::time::Duration::from_millis(100),
            cache_max_age: std::time::Duration::from_secs(7 * 24 * 60 * 60),
        },
        cancel,
        None,
    )
    .await
    .expect("scan ok");

    assert!(
        !plan.resolved_sources.is_empty(),
        "expected resolved sources, got plan={plan:?}"
    );

    let mut attach_log = Vec::new();
    let outcome = registrar::register_plan(&plan, &*lib, |name, cfg| {
        attach_log.push((name, cfg));
    })
    .await
    .expect("register ok");

    assert!(outcome.project_node_id.is_some(), "project node persisted");
    assert!(
        !outcome.instance_ids.is_empty(),
        "at least one source_instance"
    );
    assert!(!attach_log.is_empty(), "attach hook called");

    // The librarian should now contain a project node, the two source
    // templates, and at least one source_instance with a has_source
    // edge back to the project.
    let instances = lib
        .lookup(&LibrarianFilter {
            kinds: Some(vec![LibrarianNodeKind::SourceInstance]),
            limit: Some(16),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !instances.is_empty(),
        "source_instance nodes should be queryable"
    );

    let project_id = outcome.project_node_id.unwrap();
    let edges = lib.get_edges(&project_id).await.unwrap();
    assert!(
        edges.iter().any(|e| e.kind == LibrarianEdgeKind::HasSource),
        "has_source edge from project to source_instance"
    );

    // Verify project node payload landed with the right tags.
    let project_node = lib.get_node(&project_id).await.unwrap().expect("project");
    let data: ProjectNodeData =
        serde_json::from_value(project_node.data.expect("payload")).unwrap();
    assert!(data.classification_tags.contains(&"react-native".into()));
    assert!(!data.skip_discovery);
}

#[tokio::test]
async fn mark_skip_writes_marker_and_skip_flag() {
    use daemon8::discovery::{registrar, scanner};

    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());

    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    // Build a minimal plan via the scanner (no templates -> empty plan).
    let (obs_tx, _obs_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let plan = scanner::scan(
        tmp.path(),
        &*lib,
        &obs_tx,
        Vec::new(),
        scanner::ScannerConfig {
            wait_timeout: std::time::Duration::from_millis(300),
            poll_interval: std::time::Duration::from_millis(100),
            cache_max_age: std::time::Duration::from_secs(7 * 24 * 60 * 60),
        },
        cancel,
        None,
    )
    .await
    .expect("scan ok");

    let project_id = registrar::mark_skip(&plan, &*lib).await.expect("mark skip");
    let node = lib.get_node(&project_id).await.unwrap().expect("project");
    let data: ProjectNodeData = serde_json::from_value(node.data.expect("payload")).unwrap();
    assert!(data.skip_discovery, "skip flag persisted on project node");

    let marker = plan.classification.root.join(scanner::SKIP_MARKER_REL_PATH);
    assert!(marker.exists(), "on-disk skip marker present");
}

#[tokio::test]
async fn render_then_register_produces_consistent_outcome() {
    use daemon8::discovery::{
        presentation::{PresentationMode, PromptOutcome, prompt_confirm, render_plan},
        registrar,
        scanner::{DiscoveryPlan, LibrarianStatus, ResolvedSource},
    };

    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());

    // Hand-build a plan rather than scan, so the test pins the
    // presentation pipeline without depending on classification.
    let plan = DiscoveryPlan {
        classification: ProjectClassification {
            tags: vec!["react-native".into(), "git-repo".into()],
            framework_versions: BTreeMap::new(),
            root: PathBuf::from("/tmp/presentation-flow"),
            manifests: BTreeMap::new(),
            platform: Platform::current(),
        },
        librarian_status: LibrarianStatus::TemplatesPartial,
        resolved_sources: vec![ResolvedSource {
            template_id: None,
            kind: SourceKind::Log,
            resolved_path: PathBuf::from("/tmp/presentation-flow/runtime.log"),
            parser: Some("line".into()),
            tags: vec!["fixture".into()],
            version_constraint: None,
            provider: None,
        }],
        template_misses: Vec::new(),
        user_overrides: Vec::new(),
        awaiting_agent: false,
        cache_used: false,
        cache_age_secs: None,
    };

    let mut buf = Vec::new();
    render_plan(&plan, &mut buf).expect("render ok");
    let rendered = String::from_utf8(buf).unwrap();
    assert!(rendered.contains("Agent discovered 1 source instance"));
    assert!(rendered.contains("/tmp/presentation-flow/runtime.log"));

    let outcome = prompt_confirm(&plan, || PresentationMode::NonInteractive).unwrap();
    assert_eq!(outcome, PromptOutcome::NonInteractiveAutoConfirm);

    // Auto-confirm path persists.
    let result = registrar::register_plan(&plan, &*lib, |_n, _c| {})
        .await
        .expect("register ok");
    assert!(result.project_node_id.is_some());
    assert_eq!(result.instance_ids.len(), 1);
}
