// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Source registration (D4).
//!
//! Once a caller has explicitly accepted a [`DiscoveryPlan`], this module
//! turns the plan into librarian state: one source_instance node per
//! resolved path, plus the `has_source` and `derived_from` edges, plus a
//! refreshed project node. The hook into [`crate::sources::SourceManager`]
//! also runs here so the daemon starts watching the new paths immediately.
//!
//! [`register_plan`] is the only public entry point. It is idempotent
//! at the librarian level — the underlying `index_node` upserts by
//! locator, so re-running the registration after a restart does not
//! create duplicates.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use daemon8_store::{LibrarianEdge, LibrarianNode, LibrarianStore};
use daemon8_types::{
    LibrarianEdgeKind, LibrarianNodeKind, LocatorKind, ProjectNodeData, SourceInstanceData,
};

use crate::config::{ConversationSourceConfig, FileSourceConfig, SourceConfig};
use crate::discovery::scanner::{DiscoveryPlan, ResolvedSource};
use daemon8_types::SourceKind;

#[derive(Debug, thiserror::Error)]
pub enum RegistrarError {
    #[error("librarian error: {0}")]
    Librarian(#[source] daemon8_store::StoreError),
}

/// Summary of what was actually written. Returned so the serve loop can
/// log a single structured line per registration and so tests have
/// something concrete to assert against.
#[derive(Debug, Clone, Default)]
pub struct RegistrationOutcome {
    pub project_node_id: Option<String>,
    pub instance_ids: Vec<String>,
    pub edges_written: usize,
    pub sources_attached: Vec<(String, SourceConfig)>,
}

/// Register the plan's resolved sources with the librarian and the
/// running source manager.
///
/// `attach` is called once per resolved source with the auto-generated
/// source name and the synthesized [`SourceConfig`]. Production wires
/// this to a closure that hands the config to the [`crate::sources::SourceManager`];
/// tests pass a closure that records the call.
pub async fn register_plan<F>(
    plan: &DiscoveryPlan,
    librarian: &dyn LibrarianStore,
    mut attach: F,
) -> Result<RegistrationOutcome, RegistrarError>
where
    F: FnMut(String, SourceConfig),
{
    let now = now_ns();
    let mut outcome = RegistrationOutcome::default();

    if plan.resolved_sources.is_empty() {
        // Still upsert the project node so future explicit discovery calls
        // can see the empty-but-acknowledged state.
        let project_id = upsert_project_node(plan, librarian, now, false).await?;
        outcome.project_node_id = Some(project_id);
        return Ok(outcome);
    }

    let project_id = upsert_project_node(plan, librarian, now, false).await?;
    outcome.project_node_id = Some(project_id.clone());

    for (idx, source) in plan.resolved_sources.iter().enumerate() {
        let instance_node = build_instance_node(plan, source, now);
        let instance_id = librarian
            .index_node(instance_node)
            .await
            .map_err(RegistrarError::Librarian)?;
        outcome.instance_ids.push(instance_id.clone());

        let has_source = LibrarianEdge {
            id: None,
            kind: LibrarianEdgeKind::HasSource,
            from_node: project_id.clone(),
            to_node: instance_id.clone(),
            created_at: now,
        };
        librarian
            .index_edge(has_source)
            .await
            .map_err(RegistrarError::Librarian)?;
        outcome.edges_written += 1;

        if let Some(template_id) = source.template_id.as_ref() {
            let derived = LibrarianEdge {
                id: None,
                kind: LibrarianEdgeKind::DerivedFrom,
                from_node: instance_id.clone(),
                to_node: template_id.clone(),
                created_at: now,
            };
            librarian
                .index_edge(derived)
                .await
                .map_err(RegistrarError::Librarian)?;
            outcome.edges_written += 1;
        }

        let source_name = synth_source_name(source, idx);
        let cfg = synth_source_config(source);
        attach(source_name.clone(), cfg.clone());
        outcome.sources_attached.push((source_name, cfg));
    }

    Ok(outcome)
}

/// Idempotent project-node upsert. `skip_discovery` is set true only by
/// [`mark_skip`]; confirmed registrations always write false so a later
/// discovery call picks up newly classified tags.
async fn upsert_project_node(
    plan: &DiscoveryPlan,
    librarian: &dyn LibrarianStore,
    now: u64,
    skip_discovery: bool,
) -> Result<String, RegistrarError> {
    let data = build_project_data(plan, now, skip_discovery);
    let slug = data.slug.clone();
    let node = LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::Project,
        label: slug.clone(),
        locator_kind: LocatorKind::File,
        locator: data.root_path.to_string_lossy().to_string(),
        tags: vec![],
        project_slug: slug,
        version: String::new(),
        parent_id: None,
        created_at: now,
        updated_at: now,
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&data).expect("ProjectNodeData serializes")),
    };
    librarian
        .index_node(node)
        .await
        .map_err(RegistrarError::Librarian)
}

/// Skip path: persist the project node with `skip_discovery: true`
/// so future explicit discovery calls bypass the scanner.
/// Also writes the on-disk skip marker.
pub async fn mark_skip(
    plan: &DiscoveryPlan,
    librarian: &dyn LibrarianStore,
) -> Result<String, RegistrarError> {
    let now = now_ns();
    let project_id = upsert_project_node(plan, librarian, now, true).await?;
    let _ = write_skip_marker(&plan.classification.root);
    Ok(project_id)
}

pub fn write_skip_marker(root: &Path) -> std::io::Result<()> {
    let marker = root.join(crate::discovery::scanner::SKIP_MARKER_REL_PATH);
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker, b"discovery skipped\n")
}

fn build_project_data(plan: &DiscoveryPlan, now: u64, skip_discovery: bool) -> ProjectNodeData {
    let root = &plan.classification.root;
    let slug = derive_slug(root);
    ProjectNodeData {
        root_path: root.clone(),
        slug,
        classification_tags: plan.classification.tags.clone(),
        framework_versions: plan.classification.framework_versions.clone(),
        platform: plan.classification.platform,
        created_at_ns: now,
        last_serve_at_ns: now,
        skip_discovery,
    }
}

fn build_instance_node(plan: &DiscoveryPlan, source: &ResolvedSource, now: u64) -> LibrarianNode {
    let slug = derive_slug(&plan.classification.root);
    let data = SourceInstanceData {
        kind: source.kind,
        resolved_path: source.resolved_path.clone(),
        parser: source.parser.clone(),
        tags: source.tags.clone(),
        version_constraint: source.version_constraint.clone(),
        registered_at_ns: now,
        last_verified_at_ns: now,
    };
    let label = source
        .resolved_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("source")
        .to_string();
    LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceInstance,
        label,
        locator_kind: LocatorKind::File,
        locator: source.resolved_path.to_string_lossy().to_string(),
        tags: source.tags.clone(),
        project_slug: slug,
        version: String::new(),
        parent_id: None,
        created_at: now,
        updated_at: now,
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&data).expect("SourceInstanceData serializes")),
    }
}

fn synth_source_name(source: &ResolvedSource, idx: usize) -> String {
    source
        .resolved_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| {
            // Source manager keys are TOML-table-ish; keep them
            // hyphen-friendly and unique-per-plan by suffixing the
            // index when multiple sources share a stem.
            let cleaned: String = s
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            format!("discovered-{cleaned}-{idx}")
        })
        .unwrap_or_else(|| format!("discovered-{idx}"))
}

fn synth_source_config(source: &ResolvedSource) -> SourceConfig {
    // Conversation-kind sources route to the conversation watcher,
    // which understands provider-specific transcript formats. The
    // provider id is derived from the template's default_tags during
    // scanning (see `provider_from_tags` in scanner.rs); a conversation
    // source whose provider could not be identified falls back to a
    // plain file watcher rather than failing the registration.
    if source.kind == SourceKind::Conversation
        && let Some(ref provider) = source.provider
    {
        return SourceConfig::Conversation(ConversationSourceConfig {
            provider: provider.clone(),
            tags: source.tags.clone(),
        });
    }
    SourceConfig::File(FileSourceConfig {
        path: source.resolved_path.to_string_lossy().to_string(),
        parser: source.parser.clone().unwrap_or_else(|| "line".into()),
        parser_pattern: None,
        tags: source.tags.clone(),
    })
}

fn derive_slug(root: &Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .map(|s| {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect()
        })
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "project".into())
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use daemon8_store::SurrealStore;
    use daemon8_types::{Platform, ProjectClassification, SourceKind};

    use crate::discovery::scanner::{DiscoveryPlan, LibrarianStatus, ResolvedSource};

    use super::*;

    fn plan_with(resolved: Vec<ResolvedSource>) -> DiscoveryPlan {
        DiscoveryPlan {
            classification: ProjectClassification {
                tags: vec!["react-native".into(), "git-repo".into()],
                framework_versions: BTreeMap::new(),
                root: PathBuf::from("/tmp/fixture-proj"),
                manifests: BTreeMap::new(),
                platform: Platform::Macos,
            },
            librarian_status: LibrarianStatus::TemplatesPartial,
            resolved_sources: resolved,
            template_misses: Vec::new(),
            user_overrides: Vec::new(),
            awaiting_agent: false,
            cache_used: false,
            cache_age_secs: None,
        }
    }

    fn resolved(path: &str, template_id: Option<&str>) -> ResolvedSource {
        ResolvedSource {
            template_id: template_id.map(|s| s.to_string()),
            kind: SourceKind::Log,
            resolved_path: PathBuf::from(path),
            parser: Some("line".into()),
            tags: vec!["fixture".into()],
            version_constraint: None,
            provider: None,
        }
    }

    #[tokio::test]
    async fn register_plan_writes_project_instance_and_edges() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());

        let plan = plan_with(vec![
            resolved("/tmp/fixture-proj/a.log", Some("template:1")),
            resolved("/tmp/fixture-proj/b.log", None),
        ]);

        let mut attached: Vec<(String, SourceConfig)> = Vec::new();
        let outcome = register_plan(&plan, &*lib, |name, cfg| {
            attached.push((name, cfg));
        })
        .await
        .unwrap();

        assert!(outcome.project_node_id.is_some());
        assert_eq!(outcome.instance_ids.len(), 2);
        // 2 has_source edges + 1 derived_from edge (only the first
        // resolved source carried a template_id).
        assert_eq!(outcome.edges_written, 3);
        assert_eq!(attached.len(), 2);
        assert!(attached[0].0.starts_with("discovered-a-"));
        assert!(attached[1].0.starts_with("discovered-b-"));

        let project_id = outcome.project_node_id.unwrap();
        let edges = lib.get_edges(&project_id).await.unwrap();
        let has_source = edges
            .iter()
            .filter(|e| e.kind == LibrarianEdgeKind::HasSource)
            .count();
        assert_eq!(has_source, 2);
    }

    #[tokio::test]
    async fn register_plan_with_no_resolved_sources_still_upserts_project() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());
        let plan = plan_with(vec![]);

        let mut calls = 0;
        let outcome = register_plan(&plan, &*lib, |_n, _c| {
            calls += 1;
        })
        .await
        .unwrap();
        assert!(outcome.project_node_id.is_some());
        assert!(outcome.instance_ids.is_empty());
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn mark_skip_persists_skip_flag_on_project_node() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());
        let tmp = tempfile::tempdir().unwrap();
        let mut plan = plan_with(vec![]);
        plan.classification.root = tmp.path().to_path_buf();

        let id = mark_skip(&plan, &*lib).await.unwrap();
        let node = lib.get_node(&id).await.unwrap().expect("project node");
        let data: ProjectNodeData = serde_json::from_value(node.data.unwrap()).unwrap();
        assert!(data.skip_discovery);
        assert!(
            tmp.path()
                .join(crate::discovery::scanner::SKIP_MARKER_REL_PATH)
                .exists()
        );
    }

    #[test]
    fn synth_source_name_is_stable_and_unique_per_index() {
        let s = resolved("/tmp/x/runtime.log", None);
        assert_eq!(synth_source_name(&s, 0), "discovered-runtime-0");
        assert_eq!(synth_source_name(&s, 4), "discovered-runtime-4");
    }

    #[test]
    fn synth_source_name_sanitizes_non_alphanumeric() {
        let s = resolved("/tmp/x/some.weird name.log", None);
        let name = synth_source_name(&s, 0);
        assert!(name.starts_with("discovered-"));
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn synth_source_config_uses_parser_or_defaults_to_line() {
        let with = resolved("/tmp/a.log", None);
        match synth_source_config(&with) {
            SourceConfig::File(cfg) => assert_eq!(cfg.parser, "line"),
            _ => panic!("expected file source"),
        }
    }

    #[test]
    fn synth_source_config_emits_conversation_for_conversation_kind() {
        let mut r = resolved("/tmp/claude/proj/session.jsonl", Some("template:1"));
        r.kind = SourceKind::Conversation;
        r.provider = Some("claude".into());
        r.tags = vec!["claude".into(), "agent".into(), "conversation".into()];
        match synth_source_config(&r) {
            SourceConfig::Conversation(cfg) => {
                assert_eq!(cfg.provider, "claude");
                assert_eq!(cfg.tags, r.tags);
            }
            other => panic!("expected Conversation, got {other:?}"),
        }
    }

    #[test]
    fn synth_source_config_falls_back_to_file_when_provider_unknown() {
        let mut r = resolved("/tmp/some/file.jsonl", None);
        r.kind = SourceKind::Conversation;
        // provider field not set even though kind is conversation —
        // could happen if an agent writes a template missing the
        // provider tag. Degrade rather than fail.
        match synth_source_config(&r) {
            SourceConfig::File(_) => {}
            other => panic!("expected File fallback, got {other:?}"),
        }
    }

    #[test]
    fn derive_slug_lowercases_and_substitutes() {
        assert_eq!(derive_slug(Path::new("/tmp/My Project")), "my-project");
        assert_eq!(derive_slug(Path::new("/tmp/rtntv_vega")), "rtntv-vega");
        assert_eq!(derive_slug(Path::new("/")), "project");
    }
}
