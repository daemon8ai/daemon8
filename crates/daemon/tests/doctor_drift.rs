// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration test for D8 / D11 doctor drift diagnosis.
//!
//! Drives `discovery::doctor_checks::check_source_drift` against an
//! in-process SurrealDB. The fixture wires a project node with a
//! known `framework_versions` map plus a `source_instance` that
//! points at a non-existent path, then asks the check what it sees
//! when the active classification's framework_versions have moved
//! forward.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use daemon8::discovery::doctor_checks::{
    self, ProjectNodeStatus, SourceDriftReport, SourceTemplatesStatus,
};
use daemon8_store::{LibrarianEdge, LibrarianNode, LibrarianStore, SurrealStore};
use daemon8_types::{
    LibrarianEdgeKind, LibrarianNodeKind, LocatorKind, Platform, ProjectClassification,
    ProjectNodeData, SourceInstanceData, SourceKind,
};

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

async fn fresh_librarian() -> Arc<dyn LibrarianStore> {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    Arc::new(store.librarian_store())
}

fn classification(
    root: std::path::PathBuf,
    tags: &[&str],
    versions: &[(&str, &str)],
) -> ProjectClassification {
    let mut framework_versions = BTreeMap::new();
    for (k, v) in versions {
        framework_versions.insert((*k).into(), (*v).into());
    }
    ProjectClassification {
        tags: tags.iter().map(|s| (*s).to_string()).collect(),
        framework_versions,
        root,
        manifests: BTreeMap::new(),
        platform: Platform::current(),
    }
}

async fn install_project_with_missing_source(
    lib: &dyn LibrarianStore,
    root: &std::path::Path,
    tags: &[&str],
    registered_versions: &[(&str, &str)],
    missing_path: &std::path::Path,
    source_tags: &[&str],
) {
    let mut framework_versions = BTreeMap::new();
    for (k, v) in registered_versions {
        framework_versions.insert((*k).into(), (*v).into());
    }

    let project_data = ProjectNodeData {
        root_path: root.to_path_buf(),
        slug: "fixture-drift".into(),
        classification_tags: tags.iter().map(|s| (*s).to_string()).collect(),
        framework_versions,
        platform: Platform::current(),
        created_at_ns: now_ns(),
        last_serve_at_ns: now_ns(),
        skip_discovery: false,
    };

    let project_node = LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::Project,
        label: "fixture-drift".into(),
        locator_kind: LocatorKind::File,
        locator: root.to_string_lossy().to_string(),
        tags: Vec::new(),
        project_slug: "fixture-drift".into(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(project_data).unwrap()),
    };
    let project_id = lib.index_node(project_node).await.unwrap();

    let instance_data = SourceInstanceData {
        kind: SourceKind::Log,
        resolved_path: missing_path.to_path_buf(),
        parser: None,
        tags: source_tags.iter().map(|s| (*s).to_string()).collect(),
        version_constraint: None,
        registered_at_ns: now_ns(),
        last_verified_at_ns: now_ns(),
    };
    let instance_node = LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceInstance,
        label: "Kepler Studio log".into(),
        locator_kind: LocatorKind::File,
        locator: missing_path.to_string_lossy().to_string(),
        tags: source_tags.iter().map(|s| (*s).to_string()).collect(),
        project_slug: "fixture-drift".into(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(instance_data).unwrap()),
    };
    let instance_id = lib.index_node(instance_node).await.unwrap();

    lib.index_edge(LibrarianEdge {
        id: None,
        kind: LibrarianEdgeKind::HasSource,
        from_node: project_id,
        to_node: instance_id,
        created_at: now_ns(),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn drift_diagnosis_calls_out_version_upgrade_and_rescan() {
    let lib = fresh_librarian().await;
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("kepler-studio.log");

    install_project_with_missing_source(
        lib.as_ref(),
        tmp.path(),
        &["react-native"],
        &[("react-native", "0.72.1")],
        &missing,
        &["react-native", "kepler"],
    )
    .await;

    let cls = classification(
        tmp.path().to_path_buf(),
        &["react-native"],
        &[("react-native", "0.74.5")],
    );
    let reports = doctor_checks::check_source_drift(lib.as_ref(), &cls)
        .await
        .unwrap();
    assert_eq!(reports.len(), 1);

    match &reports[0] {
        SourceDriftReport::MissingWithVersionChange {
            description,
            framework,
            old_version,
            new_version,
            ..
        } => {
            assert_eq!(framework, "react-native");
            assert_eq!(old_version, "0.72.1");
            assert_eq!(new_version, "0.74.5");
            assert!(
                description.contains("Kepler"),
                "description should keep the instance label, got: {description}"
            );
        }
        other => panic!("expected MissingWithVersionChange, got {other:?}"),
    }
}

#[tokio::test]
async fn drift_with_matched_versions_falls_back_to_generic_diagnosis() {
    let lib = fresh_librarian().await;
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("kepler-studio.log");

    install_project_with_missing_source(
        lib.as_ref(),
        tmp.path(),
        &["react-native"],
        &[("react-native", "0.74.5")],
        &missing,
        &["react-native"],
    )
    .await;

    let cls = classification(
        tmp.path().to_path_buf(),
        &["react-native"],
        &[("react-native", "0.74.5")],
    );
    let reports = doctor_checks::check_source_drift(lib.as_ref(), &cls)
        .await
        .unwrap();
    assert_eq!(reports.len(), 1);
    assert!(matches!(
        &reports[0],
        SourceDriftReport::MissingNoVersionChange { .. }
    ));
}

#[tokio::test]
async fn empty_librarian_yields_no_drift_reports_and_absent_project_node() {
    let lib = fresh_librarian().await;
    let tmp = tempfile::tempdir().unwrap();

    let cls = classification(tmp.path().to_path_buf(), &["react-native"], &[]);

    let project_status = doctor_checks::check_project_node(lib.as_ref(), &cls)
        .await
        .unwrap();
    assert!(matches!(project_status, ProjectNodeStatus::Absent));

    let templates_status = doctor_checks::check_source_templates(lib.as_ref(), &cls)
        .await
        .unwrap();
    assert!(matches!(
        templates_status,
        SourceTemplatesStatus::None { .. }
    ));

    let reports = doctor_checks::check_source_drift(lib.as_ref(), &cls)
        .await
        .unwrap();
    assert!(reports.is_empty());
}
