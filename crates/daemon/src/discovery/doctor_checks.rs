// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Doctor checks for the project-aware onboarding state (D8 + D11).
//!
//! Pure logic split out of `cli/doctor.rs` so it can be unit-tested
//! against an in-memory librarian without going through the doctor CLI
//! entry point. `cli/doctor.rs` renders the results returned here into
//! its private `Check`/`CheckResult` shape.
//!
//! Three checks:
//!
//! 1. [`check_project_node`] — does the librarian already have a
//!    project node for this root? An empty librarian returns
//!    [`ProjectNodeStatus::Absent`], which the doctor renders as an
//!    `OkHint` rather than an error.
//! 2. [`check_source_templates`] — how many `source_template` entries
//!    on this machine match the project's classification tags? Zero
//!    is normal for never-onboarded project types.
//! 3. [`check_source_drift`] — for every `source_instance` linked from
//!    the project node, does the path still resolve? Missing paths
//!    trigger version-aware drift diagnosis: if the project's
//!    `framework_versions` have changed since the project node was
//!    last serialized, the diagnosis points at the version delta as
//!    the likely cause; otherwise it lists generic causes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use daemon8_store::{LibrarianFilter, LibrarianStore, StoreError};
use daemon8_types::{
    LibrarianEdgeKind, LibrarianNodeKind, ProjectClassification, ProjectNodeData,
    SourceInstanceData,
};

/// Files older than this without a write are flagged as stale even
/// when they exist. Chosen to be longer than a typical build cycle so
/// active development doesn't false-positive while still surfacing
/// stale Metro/Kepler logs that mean the dev server is down.
pub const STALE_THRESHOLD_SECS: u64 = 4 * 60 * 60;

#[derive(Debug, Clone)]
pub enum ProjectNodeStatus {
    /// No project node in the librarian for this root. Empty-librarian
    /// safety: doctor surfaces this as `OkHint`, not an error.
    Absent,
    /// Project node present, `skip_discovery` flag set. User opted out.
    SkipDiscovery { slug: String },
    /// Project node present and discoverable.
    Present {
        slug: String,
        classification_tags: Vec<String>,
        framework_versions: BTreeMap<String, String>,
        last_serve_age_secs: Option<u64>,
    },
    /// Project node present but its payload doesn't deserialize.
    Malformed,
}

#[derive(Debug, Clone)]
pub enum SourceTemplatesStatus {
    /// No matching templates yet for this project type on this machine.
    /// Normal state for a never-onboarded framework; not a warning.
    None { matched_tags: Vec<String> },
    Some {
        count: usize,
        matched_tags: Vec<String>,
    },
}

/// One drift report per `source_instance` linked from the project
/// node. The doctor renders each variant into its own `Check`.
#[derive(Debug, Clone)]
pub enum SourceDriftReport {
    Ok {
        description: String,
        path: PathBuf,
        last_write_age_secs: Option<u64>,
    },
    Stale {
        description: String,
        path: PathBuf,
        last_write_age_secs: u64,
    },
    MissingNoVersionChange {
        description: String,
        path: PathBuf,
    },
    /// Path missing AND a framework named by the source's tags upgraded
    /// since the project node was last serialized. Strongest version
    /// hypothesis.
    MissingWithVersionChange {
        description: String,
        path: PathBuf,
        framework: String,
        old_version: String,
        new_version: String,
    },
    /// Path missing AND some framework upgraded, but none that the
    /// source's tags link to. Softer message — version is plausible
    /// but not certain.
    MissingPartialVersionChange {
        description: String,
        path: PathBuf,
        changed_frameworks: Vec<(String, String, String)>,
    },
}

pub async fn check_project_node(
    librarian: &dyn LibrarianStore,
    classification: &ProjectClassification,
) -> Result<ProjectNodeStatus, StoreError> {
    let Some(node) = lookup_project_node(librarian, classification).await? else {
        return Ok(ProjectNodeStatus::Absent);
    };
    let Some(data_val) = node.data.as_ref() else {
        return Ok(ProjectNodeStatus::Malformed);
    };
    let Ok(data) = serde_json::from_value::<ProjectNodeData>(data_val.clone()) else {
        return Ok(ProjectNodeStatus::Malformed);
    };
    if data.skip_discovery {
        return Ok(ProjectNodeStatus::SkipDiscovery { slug: data.slug });
    }
    Ok(ProjectNodeStatus::Present {
        slug: data.slug,
        classification_tags: data.classification_tags,
        framework_versions: data.framework_versions,
        last_serve_age_secs: age_secs(data.last_serve_at_ns),
    })
}

pub async fn check_source_templates(
    librarian: &dyn LibrarianStore,
    classification: &ProjectClassification,
) -> Result<SourceTemplatesStatus, StoreError> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::SourceTemplate]),
        limit: Some(512),
        ..Default::default()
    };
    let nodes = librarian.lookup(&filter).await?;
    let mut matched = 0;
    for node in &nodes {
        // `tags` on a source_template carries its project_types — the
        // scanner already relies on this invariant when writing
        // templates via the registrar / hint payload.
        let any_overlap = node
            .tags
            .iter()
            .any(|t| classification.tags.contains(t) || t == "any");
        if any_overlap && template_platform_matches(node, classification) {
            matched += 1;
        }
    }
    let matched_tags = classification.tags.clone();
    if matched == 0 {
        Ok(SourceTemplatesStatus::None { matched_tags })
    } else {
        Ok(SourceTemplatesStatus::Some {
            count: matched,
            matched_tags,
        })
    }
}

pub async fn check_source_drift(
    librarian: &dyn LibrarianStore,
    classification: &ProjectClassification,
) -> Result<Vec<SourceDriftReport>, StoreError> {
    let Some(project_node) = lookup_project_node(librarian, classification).await? else {
        return Ok(Vec::new());
    };
    let Some(project_id) = project_node.id.as_deref() else {
        return Ok(Vec::new());
    };
    let registered_versions: BTreeMap<String, String> = project_node
        .data
        .as_ref()
        .and_then(|v| serde_json::from_value::<ProjectNodeData>(v.clone()).ok())
        .map(|d| d.framework_versions)
        .unwrap_or_default();

    let edges = librarian.get_edges(project_id).await?;
    let mut reports = Vec::new();
    for edge in edges {
        if edge.kind != LibrarianEdgeKind::HasSource {
            continue;
        }
        let Some(instance_node) = librarian.get_node(&edge.to_node).await? else {
            continue;
        };
        let description = describe_instance(&instance_node.label, &instance_node.tags);
        let path = instance_path(&instance_node);

        if path.exists() {
            let age = file_last_write_age_secs(&path);
            match age {
                Some(secs) if secs > STALE_THRESHOLD_SECS => {
                    reports.push(SourceDriftReport::Stale {
                        description,
                        path,
                        last_write_age_secs: secs,
                    });
                }
                _ => {
                    reports.push(SourceDriftReport::Ok {
                        description,
                        path,
                        last_write_age_secs: age,
                    });
                }
            }
            continue;
        }

        // Missing path: run version-aware drift diagnosis.
        let source_frameworks = source_framework_candidates(&instance_node);
        let report = diagnose_missing(
            description,
            path,
            &source_frameworks,
            &registered_versions,
            &classification.framework_versions,
        );
        reports.push(report);
    }
    Ok(reports)
}

// ── helpers ───────────────────────────────────────────────────────────

async fn lookup_project_node(
    librarian: &dyn LibrarianStore,
    classification: &ProjectClassification,
) -> Result<Option<daemon8_store::LibrarianNode>, StoreError> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::Project]),
        limit: Some(128),
        ..Default::default()
    };
    let nodes = librarian.lookup(&filter).await?;
    let canonical = classification.root.to_string_lossy().to_string();
    Ok(nodes.into_iter().find(|n| n.locator == canonical))
}

fn template_platform_matches(
    node: &daemon8_store::LibrarianNode,
    classification: &ProjectClassification,
) -> bool {
    // The platform filter is encoded in the template's `data` payload.
    // Templates without a parseable payload predate D6 — accept them
    // permissively because rejecting would mask the underlying issue.
    let Some(data) = node.data.as_ref() else {
        return true;
    };
    let Ok(template) = serde_json::from_value::<daemon8_types::SourceTemplateData>(data.clone())
    else {
        return true;
    };
    template.platforms.contains(&classification.platform)
}

fn describe_instance(label: &str, tags: &[String]) -> String {
    if !label.is_empty() {
        return label.to_string();
    }
    tags.iter()
        .find_map(|t| t.parse::<daemon8_types::SourceKind>().ok())
        .map(|k| format!("{k:?}").to_lowercase())
        .unwrap_or_else(|| "source".into())
}

fn instance_path(node: &daemon8_store::LibrarianNode) -> PathBuf {
    if let Some(data) = node.data.as_ref()
        && let Ok(parsed) = serde_json::from_value::<SourceInstanceData>(data.clone())
    {
        return parsed.resolved_path;
    }
    PathBuf::from(&node.locator)
}

/// Candidate framework names for this source. We treat any tag that
/// also appears as a key in either the registered or current
/// `framework_versions` map as a framework candidate. This lets
/// version-aware diagnosis fire when a source's tags name a framework
/// directly (e.g. `react-native`, `expo`), and skip the strong
/// hypothesis when none do.
fn source_framework_candidates(node: &daemon8_store::LibrarianNode) -> Vec<String> {
    node.tags.clone()
}

fn diagnose_missing(
    description: String,
    path: PathBuf,
    source_frameworks: &[String],
    registered_versions: &BTreeMap<String, String>,
    current_versions: &BTreeMap<String, String>,
) -> SourceDriftReport {
    let changed = changed_frameworks(registered_versions, current_versions);
    if changed.is_empty() {
        return SourceDriftReport::MissingNoVersionChange { description, path };
    }

    let direct_hit = changed
        .iter()
        .find(|(framework, _, _)| source_frameworks.iter().any(|t| t == framework));

    if let Some((framework, old, new)) = direct_hit {
        return SourceDriftReport::MissingWithVersionChange {
            description,
            path,
            framework: framework.clone(),
            old_version: old.clone(),
            new_version: new.clone(),
        };
    }

    SourceDriftReport::MissingPartialVersionChange {
        description,
        path,
        changed_frameworks: changed,
    }
}

fn changed_frameworks(
    registered: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (framework, old) in registered {
        if let Some(new) = current.get(framework)
            && new != old
        {
            out.push((framework.clone(), old.clone(), new.clone()));
        }
    }
    out
}

fn age_secs(at_ns: u64) -> Option<u64> {
    if at_ns == 0 {
        return None;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos() as u64;
    if now <= at_ns {
        return Some(0);
    }
    Some((now - at_ns) / 1_000_000_000)
}

fn file_last_write_age_secs(path: &std::path::Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    Some(age.as_secs())
}

/// Cap age in seconds with the same threshold the stale check uses.
/// Exposed for renderer pretty-printing.
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 60 * 60 {
        return format!("{}m", secs / 60);
    }
    if secs < 24 * 60 * 60 {
        return format!("{}h", secs / (60 * 60));
    }
    format!("{}d", secs / (24 * 60 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_store::{LibrarianEdge, LibrarianNode, SurrealStore};
    use daemon8_types::{
        LibrarianEdgeKind, LibrarianNodeKind, LocatorKind, Platform, ProjectNodeData, SourceKind,
        SourceTemplateData, TemplateConfidence,
    };
    use std::sync::Arc;

    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    }

    fn classification(
        root: PathBuf,
        tags: Vec<&str>,
        versions: &[(&str, &str)],
    ) -> ProjectClassification {
        let mut framework_versions = BTreeMap::new();
        for (k, v) in versions {
            framework_versions.insert((*k).into(), (*v).into());
        }
        ProjectClassification {
            tags: tags.into_iter().map(String::from).collect(),
            framework_versions,
            root,
            manifests: BTreeMap::new(),
            platform: Platform::current(),
        }
    }

    fn project_node(
        root: &std::path::Path,
        tags: Vec<&str>,
        versions: &[(&str, &str)],
        skip: bool,
    ) -> LibrarianNode {
        let mut framework_versions = BTreeMap::new();
        for (k, v) in versions {
            framework_versions.insert((*k).into(), (*v).into());
        }
        let data = ProjectNodeData {
            root_path: root.to_path_buf(),
            slug: "fixture".into(),
            classification_tags: tags.into_iter().map(String::from).collect(),
            framework_versions,
            platform: Platform::current(),
            created_at_ns: now_ns(),
            last_serve_at_ns: now_ns(),
            skip_discovery: skip,
        };
        LibrarianNode {
            id: None,
            kind: LibrarianNodeKind::Project,
            label: "fixture".into(),
            locator_kind: LocatorKind::File,
            locator: root.to_string_lossy().to_string(),
            tags: Vec::new(),
            project_slug: "fixture".into(),
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

    fn template_node(project_types: &[&str], pattern: &str) -> LibrarianNode {
        let data = SourceTemplateData {
            project_types: project_types.iter().map(|s| (*s).to_string()).collect(),
            kind: SourceKind::Log,
            locator_pattern: pattern.into(),
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
        };
        LibrarianNode {
            id: None,
            kind: LibrarianNodeKind::SourceTemplate,
            label: format!("template:{pattern}"),
            locator_kind: LocatorKind::File,
            locator: pattern.into(),
            tags: project_types.iter().map(|s| (*s).to_string()).collect(),
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

    fn instance_node(label: &str, path: &std::path::Path, tags: Vec<&str>) -> LibrarianNode {
        let data = SourceInstanceData {
            kind: SourceKind::Log,
            resolved_path: path.to_path_buf(),
            parser: None,
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
            version_constraint: None,
            registered_at_ns: now_ns(),
            last_verified_at_ns: now_ns(),
        };
        LibrarianNode {
            id: None,
            kind: LibrarianNodeKind::SourceInstance,
            label: label.into(),
            locator_kind: LocatorKind::File,
            locator: path.to_string_lossy().to_string(),
            tags: tags.into_iter().map(String::from).collect(),
            project_slug: "fixture".into(),
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

    async fn setup_librarian() -> Arc<dyn LibrarianStore> {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        Arc::new(store.librarian_store())
    }

    #[tokio::test]
    async fn project_node_empty_librarian_returns_absent() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let cls = classification(tmp.path().to_path_buf(), vec!["react-native"], &[]);
        let status = check_project_node(lib.as_ref(), &cls).await.unwrap();
        assert!(
            matches!(status, ProjectNodeStatus::Absent),
            "got {status:?}"
        );
    }

    #[tokio::test]
    async fn project_node_present_reports_slug_and_tags() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let node = project_node(
            tmp.path(),
            vec!["react-native", "git-repo"],
            &[("react-native", "0.74.5")],
            false,
        );
        lib.index_node(node).await.unwrap();

        let cls = classification(
            tmp.path().to_path_buf(),
            vec!["react-native", "git-repo"],
            &[("react-native", "0.74.5")],
        );
        let status = check_project_node(lib.as_ref(), &cls).await.unwrap();
        match status {
            ProjectNodeStatus::Present {
                slug,
                classification_tags,
                framework_versions,
                ..
            } => {
                assert_eq!(slug, "fixture");
                assert!(classification_tags.iter().any(|t| t == "react-native"));
                assert_eq!(framework_versions.get("react-native").unwrap(), "0.74.5");
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn project_node_with_skip_marker_reports_skip() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let node = project_node(tmp.path(), vec!["react-native"], &[], true);
        lib.index_node(node).await.unwrap();

        let cls = classification(tmp.path().to_path_buf(), vec!["react-native"], &[]);
        let status = check_project_node(lib.as_ref(), &cls).await.unwrap();
        assert!(matches!(status, ProjectNodeStatus::SkipDiscovery { .. }));
    }

    #[tokio::test]
    async fn source_templates_zero_returns_none() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let cls = classification(tmp.path().to_path_buf(), vec!["react-native"], &[]);
        let status = check_source_templates(lib.as_ref(), &cls).await.unwrap();
        assert!(matches!(status, SourceTemplatesStatus::None { .. }));
    }

    #[tokio::test]
    async fn source_templates_nonzero_reports_count() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        lib.index_node(template_node(&["react-native"], "/tmp/metro.log"))
            .await
            .unwrap();
        lib.index_node(template_node(&["react-native"], "/tmp/rn.log"))
            .await
            .unwrap();
        lib.index_node(template_node(&["laravel"], "/tmp/laravel.log"))
            .await
            .unwrap();

        let cls = classification(tmp.path().to_path_buf(), vec!["react-native"], &[]);
        let status = check_source_templates(lib.as_ref(), &cls).await.unwrap();
        match status {
            SourceTemplatesStatus::Some { count, .. } => assert_eq!(count, 2),
            other => panic!("expected Some, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn drift_path_exists_returns_ok() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("active.log");
        std::fs::write(&file, "hello").unwrap();

        let pnode = project_node(tmp.path(), vec!["react-native"], &[], false);
        let project_id = lib.index_node(pnode).await.unwrap();
        let inode = instance_node("active log", &file, vec!["react-native"]);
        let instance_id = lib.index_node(inode).await.unwrap();
        lib.index_edge(LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::HasSource,
            from_node: project_id,
            to_node: instance_id,
            created_at: now_ns(),
        })
        .await
        .unwrap();

        let cls = classification(tmp.path().to_path_buf(), vec!["react-native"], &[]);
        let reports = check_source_drift(lib.as_ref(), &cls).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert!(matches!(reports[0], SourceDriftReport::Ok { .. }));
    }

    #[tokio::test]
    async fn drift_path_missing_no_version_change_reports_other_drift() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.log");

        let pnode = project_node(
            tmp.path(),
            vec!["react-native"],
            &[("react-native", "0.74.5")],
            false,
        );
        let project_id = lib.index_node(pnode).await.unwrap();
        let inode = instance_node("missing log", &missing, vec!["react-native"]);
        let instance_id = lib.index_node(inode).await.unwrap();
        lib.index_edge(LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::HasSource,
            from_node: project_id,
            to_node: instance_id,
            created_at: now_ns(),
        })
        .await
        .unwrap();

        let cls = classification(
            tmp.path().to_path_buf(),
            vec!["react-native"],
            &[("react-native", "0.74.5")],
        );
        let reports = check_source_drift(lib.as_ref(), &cls).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert!(matches!(
            reports[0],
            SourceDriftReport::MissingNoVersionChange { .. }
        ));
    }

    #[tokio::test]
    async fn drift_path_missing_with_version_change_reports_version_diagnosis() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.log");

        let pnode = project_node(
            tmp.path(),
            vec!["react-native"],
            &[("react-native", "0.72.1")],
            false,
        );
        let project_id = lib.index_node(pnode).await.unwrap();
        let inode = instance_node("missing log", &missing, vec!["react-native"]);
        let instance_id = lib.index_node(inode).await.unwrap();
        lib.index_edge(LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::HasSource,
            from_node: project_id,
            to_node: instance_id,
            created_at: now_ns(),
        })
        .await
        .unwrap();

        let cls = classification(
            tmp.path().to_path_buf(),
            vec!["react-native"],
            &[("react-native", "0.74.5")],
        );
        let reports = check_source_drift(lib.as_ref(), &cls).await.unwrap();
        assert_eq!(reports.len(), 1);
        match &reports[0] {
            SourceDriftReport::MissingWithVersionChange {
                framework,
                old_version,
                new_version,
                ..
            } => {
                assert_eq!(framework, "react-native");
                assert_eq!(old_version, "0.72.1");
                assert_eq!(new_version, "0.74.5");
            }
            other => panic!("expected MissingWithVersionChange, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn drift_partial_version_change_reports_softer_message() {
        let lib = setup_librarian().await;
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.log");

        // The project node records two frameworks; only `expo` changed.
        // The source's tag set names `react-native`, which did not
        // change — so we expect the partial / softer report.
        let pnode = project_node(
            tmp.path(),
            vec!["react-native", "expo"],
            &[("react-native", "0.74.5"), ("expo", "~52.0.0")],
            false,
        );
        let project_id = lib.index_node(pnode).await.unwrap();
        let inode = instance_node("missing log", &missing, vec!["react-native"]);
        let instance_id = lib.index_node(inode).await.unwrap();
        lib.index_edge(LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::HasSource,
            from_node: project_id,
            to_node: instance_id,
            created_at: now_ns(),
        })
        .await
        .unwrap();

        let cls = classification(
            tmp.path().to_path_buf(),
            vec!["react-native", "expo"],
            &[("react-native", "0.74.5"), ("expo", "~53.0.0")],
        );
        let reports = check_source_drift(lib.as_ref(), &cls).await.unwrap();
        assert_eq!(reports.len(), 1);
        match &reports[0] {
            SourceDriftReport::MissingPartialVersionChange {
                changed_frameworks, ..
            } => {
                assert_eq!(changed_frameworks.len(), 1);
                assert_eq!(changed_frameworks[0].0, "expo");
            }
            other => panic!("expected MissingPartialVersionChange, got {other:?}"),
        }
    }
}
