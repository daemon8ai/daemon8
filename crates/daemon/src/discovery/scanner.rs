// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Discovery scanner (D3).
//!
//! Orchestrates the project-aware onboarding flow. Given a project root
//! the scanner:
//!
//! 1. Classifies the project (D1) and canonicalizes the root path.
//! 2. Asks the librarian whether it already knows this project (cache
//!    hit path) or knows source_templates for the project's tags
//!    (template-match path).
//! 3. Expands each matching template's `locator_pattern` against the
//!    filesystem, separating templates that resolve from those that
//!    miss.
//! 4. Emits a `DiscoveryHint` observation (via [`crate::discovery::hint`])
//!    if any classification tag is uncovered, then enters a bounded
//!    poll loop waiting for the agent to write new source_templates.
//! 5. Returns a [`DiscoveryPlan`] — the input to D4 presentation.
//!
//! The scanner never registers sources or writes to the SourceManager.
//! Plan -> register is a D4 concern, gated on explicit user confirmation.
//!
//! The wait loop is cancellable from three directions:
//!   - the per-scan timeout (configurable, default 60s);
//!   - the daemon's shutdown `CancellationToken`;
//!   - an out-of-band signal flipped by `daemon8 discover --complete`
//!     (see [`DiscoverySignals`]).
//!
//! `expand_locator_pattern` and `template_matches_versions` are pure
//! helpers extracted so they can be exhaustively unit-tested without
//! standing up a daemon.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daemon8_store::{LibrarianFilter, LibrarianNode, LibrarianStore};
use daemon8_types::{
    LibrarianNodeKind, Observation, ProjectClassification, SourceKind, SourceTemplateData,
};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::config::SourceConfig;
use crate::discovery::{conversation, hint};

/// Skip-marker path relative to the project root. Future `daemon8 serve`
/// invocations honor this file and bypass the scanner entirely.
pub const SKIP_MARKER_REL_PATH: &str = ".daemon8/skip-discovery";

/// Environment variable that overrides the wait-loop timeout.
pub const DISCOVERY_TIMEOUT_ENV: &str = "DAEMON8_DISCOVERY_TIMEOUT_SECS";

/// Out-of-band signal slots flipped by `daemon8 discover --complete` /
/// `--skip`. The scanner polls these between intervals; the HTTP /
/// admin layer in a later commit flips the bools and pulses [`notify`]
/// to wake the wait loop promptly.
#[derive(Debug, Default)]
pub struct DiscoverySignals {
    pub complete: AtomicBool,
    pub skip: AtomicBool,
    pub notify: Notify,
}

impl DiscoverySignals {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::SeqCst)
    }

    pub fn is_skip(&self) -> bool {
        self.skip.load(Ordering::SeqCst)
    }
}

impl daemon8_types::DiscoveryControl for DiscoverySignals {
    fn signal_complete(&self) {
        self.complete.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn signal_skip(&self) {
        self.skip.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

/// What the librarian had to say about the project on this scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarianStatus {
    /// Project node found, fresh, all has_source paths still resolve.
    CacheHit,
    /// Project node found but stale or with at least one drifted path.
    CacheStale,
    /// No project node yet; some classification tags have templates,
    /// others do not.
    TemplatesPartial,
    /// No project node and no templates cover any classification tag.
    TemplatesMissing,
}

/// A template that successfully expanded to a concrete path on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResolvedSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    pub kind: SourceKind,
    pub resolved_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    /// AI provider id for conversation-kind sources. Derived from the
    /// underlying source_template's `default_tags` — agents are
    /// instructed to include the provider id when writing a conversation
    /// template, and the first-run check keys on that same tag.
    /// `None` for non-conversation sources or when no recognized
    /// provider tag was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateMissReason {
    PathNotFound,
    InvalidPattern(String),
    VersionMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TemplateMiss {
    pub template_id: String,
    pub locator_pattern: String,
    pub reason: TemplateMissReason,
}

/// Output of [`scan`]. D4 reads this, renders it for the user, and
/// (post-confirmation) feeds it to the registrar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiscoveryPlan {
    pub classification: ProjectClassification,
    pub librarian_status: LibrarianStatus,
    pub resolved_sources: Vec<ResolvedSource>,
    pub template_misses: Vec<TemplateMiss>,
    /// User-provided `[sources.*]` entries from `.daemon8.toml`. The
    /// scanner does not touch these — they pass through unchanged so D4
    /// can show "user override" rows alongside discovered ones.
    pub user_overrides: Vec<SourceConfig>,
    pub awaiting_agent: bool,
    pub cache_used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_age_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ScannerConfig {
    pub wait_timeout: Duration,
    pub poll_interval: Duration,
    pub cache_max_age: Duration,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        let wait_secs = std::env::var(DISCOVERY_TIMEOUT_ENV)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        Self {
            wait_timeout: Duration::from_secs(wait_secs),
            poll_interval: Duration::from_secs(5),
            cache_max_age: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScannerError {
    #[error("classify failed: {0}")]
    Classify(#[source] anyhow::Error),

    #[error("librarian error: {0}")]
    Librarian(#[source] daemon8_store::StoreError),
}

/// Scan a project root and return a [`DiscoveryPlan`].
///
/// The scanner reads the librarian read-only; any new source_template
/// entries written during the wait loop must be written by the agent
/// via `librarian_index`. The scanner picks them up on the next poll
/// tick and short-circuits when every classification tag is covered.
pub async fn scan(
    root: &Path,
    librarian: &dyn LibrarianStore,
    obs_tx: &UnboundedSender<Observation>,
    user_overrides: Vec<SourceConfig>,
    config: ScannerConfig,
    cancel: CancellationToken,
    signals: Option<Arc<DiscoverySignals>>,
) -> Result<DiscoveryPlan, ScannerError> {
    let classification = daemon8_providers::classify(root).map_err(ScannerError::Classify)?;
    let canonical_root = canonicalize_root(&classification.root);
    let mut classification = classification;
    classification.root = canonical_root.clone();

    // Honor the skip marker before any librarian work — agents and
    // users have already opted out.
    if has_skip_marker(&canonical_root) {
        tracing::info!(
            root = %canonical_root.display(),
            "discovery skip marker present; producing empty plan"
        );
        return Ok(empty_plan(classification, user_overrides));
    }

    // Cache path: project node by canonical root path.
    let project_node = lookup_project_node(librarian, &canonical_root).await?;
    if let Some(node) = project_node.as_ref() {
        let (status, cache_age) = classify_cache_freshness(node, config.cache_max_age);
        if status == LibrarianStatus::CacheHit {
            let plan = build_cached_plan(
                librarian,
                node,
                &classification,
                user_overrides.clone(),
                cache_age,
            )
            .await?;
            // Cache hit implies template coverage for the user's tags;
            // no hint is emitted on this path.
            return Ok(plan);
        }
    }

    let outcome = probe_templates_inner(librarian, &classification).await?;
    let uncovered =
        classification_tags_uncovered(&classification.tags, &outcome.tags_with_resolved_template);
    let mut plan = plan_from_outcome(outcome, &classification, user_overrides.clone());

    // D5: every serve checks whether each AI provider on this machine
    // already has a conversation source_template. A first-run provider
    // adds its bootstrap payload to the hint even when classification
    // coverage is otherwise complete.
    let first_run_providers = collect_first_run_providers(librarian).await?;

    if uncovered.is_empty() && first_run_providers.is_empty() {
        // Coverage complete and every provider already has a template:
        // no agent involvement needed.
        return Ok(plan);
    }

    // Emit the hint and enter the wait loop. The agent reads the hint
    // via query_observations, writes source_template entries via
    // librarian_index, and (optionally) signals --complete.
    let payload = hint::build_payload(&classification, &[], &uncovered, first_run_providers);
    if let Err(e) = hint::emit_discovery_hint(obs_tx, payload) {
        // Channel closure during emission means the daemon is shutting
        // down. Return what we have rather than continuing to wait.
        tracing::warn!("failed to emit discovery hint: {e}");
        plan.awaiting_agent = false;
        return Ok(plan);
    }
    plan.awaiting_agent = true;

    let final_plan = wait_for_agent(
        librarian,
        plan,
        &classification,
        user_overrides,
        &config,
        cancel,
        signals,
    )
    .await?;

    Ok(final_plan)
}

fn plan_from_outcome(
    outcome: ProbeOutcome,
    classification: &ProjectClassification,
    user_overrides: Vec<SourceConfig>,
) -> DiscoveryPlan {
    let status = if outcome.tags_with_resolved_template.is_empty() && outcome.misses.is_empty() {
        LibrarianStatus::TemplatesMissing
    } else {
        LibrarianStatus::TemplatesPartial
    };
    DiscoveryPlan {
        classification: classification.clone(),
        librarian_status: status,
        resolved_sources: outcome.resolved,
        template_misses: outcome.misses,
        user_overrides,
        awaiting_agent: false,
        cache_used: false,
        cache_age_secs: None,
    }
}

// ── Pure helpers (heavily tested below) ──────────────────────────────

/// Expand `~`, `<root>`, and `$VAR`/`${VAR}` references in a locator
/// pattern, then run glob expansion if the pattern contains glob
/// metacharacters. Returns `Err(InvalidPattern)` for malformed patterns
/// — callers fold the error into a [`TemplateMiss`] rather than aborting.
pub fn expand_locator_pattern(
    pattern: &str,
    root: &Path,
) -> Result<Vec<PathBuf>, TemplateMissReason> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(TemplateMissReason::InvalidPattern(
            "pattern is empty".into(),
        ));
    }

    let after_home = expand_home(trimmed)?;
    let after_root = after_home.replace("<root>", &root.to_string_lossy());
    let after_env = expand_env_vars(&after_root)?;

    if has_glob_chars(&after_env) {
        let mut paths = Vec::new();
        let entries = glob::glob(&after_env)
            .map_err(|e| TemplateMissReason::InvalidPattern(format!("glob parse error: {e}")))?;
        for entry in entries {
            match entry {
                Ok(p) => paths.push(p),
                Err(e) => {
                    tracing::warn!(
                        pattern = %after_env,
                        "glob entry error: {e}"
                    );
                }
            }
        }
        Ok(paths)
    } else {
        Ok(vec![PathBuf::from(after_env)])
    }
}

fn expand_home(pattern: &str) -> Result<String, TemplateMissReason> {
    if let Some(rest) = pattern.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or_else(|| {
            TemplateMissReason::InvalidPattern("home directory unavailable".into())
        })?;
        Ok(format!("{}/{rest}", home.display()))
    } else if pattern == "~" {
        let home = dirs::home_dir().ok_or_else(|| {
            TemplateMissReason::InvalidPattern("home directory unavailable".into())
        })?;
        Ok(home.display().to_string())
    } else {
        Ok(pattern.to_string())
    }
}

fn expand_env_vars(input: &str) -> Result<String, TemplateMissReason> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // ${VAR} form
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            let mut closed = false;
            for nc in chars.by_ref() {
                if nc == '}' {
                    closed = true;
                    break;
                }
                name.push(nc);
            }
            if !closed {
                return Err(TemplateMissReason::InvalidPattern(format!(
                    "unterminated ${{}} reference near {name}"
                )));
            }
            let value = std::env::var(&name).map_err(|_| {
                TemplateMissReason::InvalidPattern(format!(
                    "environment variable ${name} is not set"
                ))
            })?;
            out.push_str(&value);
            continue;
        }
        // $VAR form (alphanumeric + underscore)
        let mut name = String::new();
        while let Some(&nc) = chars.peek() {
            if nc.is_ascii_alphanumeric() || nc == '_' {
                name.push(nc);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            // Bare `$` with no name — keep literal.
            out.push('$');
            continue;
        }
        let value = std::env::var(&name).map_err(|_| {
            TemplateMissReason::InvalidPattern(format!("environment variable ${name} is not set"))
        })?;
        out.push_str(&value);
    }
    Ok(out)
}

fn has_glob_chars(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '['))
}

/// Decide whether a template's `version_constraint` is compatible with
/// the project's `framework_versions`. Returns true if:
///
/// - the template is version-agnostic (`None`); or
/// - no framework named in the template's `project_types` also appears
///   in `framework_versions` (template can't be filtered by version); or
/// - any matching framework's version satisfies the constraint.
///
/// Unparseable project versions (`workspace:*`, `latest`, ...) log a
/// warning and are treated as version-agnostic for that framework — the
/// template is still considered a match, which is the right
/// fail-permissive default for a discovery hint.
pub fn template_matches_versions(
    template: &SourceTemplateData,
    project_versions: &BTreeMap<String, String>,
) -> bool {
    let Some(ref constraint_str) = template.version_constraint else {
        return true;
    };
    let req = match VersionReq::parse(constraint_str) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                constraint = %constraint_str,
                "template version_constraint is not a valid SemVer range: {e}; treating as agnostic"
            );
            return true;
        }
    };

    let mut had_overlap = false;
    for framework in &template.project_types {
        let Some(raw) = project_versions.get(framework) else {
            continue;
        };
        had_overlap = true;
        let cleaned = raw.trim_start_matches(['^', '~', '=', 'v', 'V']);
        match Version::parse(cleaned) {
            Ok(v) => {
                if req.matches(&v) {
                    return true;
                }
            }
            Err(_) => {
                tracing::warn!(
                    framework = %framework,
                    version = %raw,
                    "project framework version is not parseable as SemVer; treating template as agnostic for this framework"
                );
                return true;
            }
        }
    }

    // No version overlap — template applies on tag basis alone.
    !had_overlap
}

fn canonicalize_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

fn has_skip_marker(root: &Path) -> bool {
    root.join(SKIP_MARKER_REL_PATH).exists()
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}

fn empty_plan(
    classification: ProjectClassification,
    user_overrides: Vec<SourceConfig>,
) -> DiscoveryPlan {
    DiscoveryPlan {
        classification,
        librarian_status: LibrarianStatus::CacheHit,
        resolved_sources: Vec::new(),
        template_misses: Vec::new(),
        user_overrides,
        awaiting_agent: false,
        cache_used: true,
        cache_age_secs: None,
    }
}

async fn lookup_project_node(
    librarian: &dyn LibrarianStore,
    canonical_root: &Path,
) -> Result<Option<LibrarianNode>, ScannerError> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::Project]),
        limit: Some(64),
        ..Default::default()
    };
    let nodes = librarian
        .lookup(&filter)
        .await
        .map_err(ScannerError::Librarian)?;
    let canonical_str = canonical_root.to_string_lossy().to_string();
    Ok(nodes.into_iter().find(|n| n.locator == canonical_str))
}

fn classify_cache_freshness(
    node: &LibrarianNode,
    max_age: Duration,
) -> (LibrarianStatus, Option<u64>) {
    let Some(ref data_val) = node.data else {
        // Project node without payload predates D6. Treat as stale.
        return (LibrarianStatus::CacheStale, None);
    };
    let parsed: Result<daemon8_types::ProjectNodeData, _> =
        serde_json::from_value(data_val.clone());
    let Ok(project) = parsed else {
        return (LibrarianStatus::CacheStale, None);
    };
    let now = now_ns();
    let last = project.last_serve_at_ns;
    if last == 0 || now < last {
        return (LibrarianStatus::CacheStale, None);
    }
    let age_ns = now - last;
    let age_secs = age_ns / 1_000_000_000;
    if Duration::from_secs(age_secs) > max_age {
        (LibrarianStatus::CacheStale, Some(age_secs))
    } else {
        (LibrarianStatus::CacheHit, Some(age_secs))
    }
}

async fn build_cached_plan(
    librarian: &dyn LibrarianStore,
    project_node: &LibrarianNode,
    classification: &ProjectClassification,
    user_overrides: Vec<SourceConfig>,
    cache_age: Option<u64>,
) -> Result<DiscoveryPlan, ScannerError> {
    let project_id = match project_node.id.as_deref() {
        Some(id) => id,
        None => {
            return Ok(DiscoveryPlan {
                classification: classification.clone(),
                librarian_status: LibrarianStatus::CacheStale,
                resolved_sources: Vec::new(),
                template_misses: Vec::new(),
                user_overrides,
                awaiting_agent: false,
                cache_used: false,
                cache_age_secs: cache_age,
            });
        }
    };
    let edges = librarian
        .get_edges(project_id)
        .await
        .map_err(ScannerError::Librarian)?;

    let mut resolved = Vec::new();
    let mut drift = false;
    for edge in edges {
        if edge.kind != daemon8_types::LibrarianEdgeKind::HasSource {
            continue;
        }
        let instance = librarian
            .get_node(&edge.to_node)
            .await
            .map_err(ScannerError::Librarian)?;
        let Some(node) = instance else { continue };
        let path = PathBuf::from(&node.locator);
        if !path.exists() {
            drift = true;
            continue;
        }
        let kind = source_kind_from_tag_or_default(&node.tags);
        let provider = if kind == SourceKind::Conversation {
            provider_from_tags(&node.tags)
        } else {
            None
        };
        resolved.push(ResolvedSource {
            template_id: None,
            kind,
            resolved_path: path,
            parser: extract_parser_from_data(&node.data),
            tags: node.tags.clone(),
            version_constraint: None,
            provider,
        });
    }

    let status = if drift {
        LibrarianStatus::CacheStale
    } else {
        LibrarianStatus::CacheHit
    };

    Ok(DiscoveryPlan {
        classification: classification.clone(),
        librarian_status: status,
        resolved_sources: resolved,
        template_misses: Vec::new(),
        user_overrides,
        awaiting_agent: false,
        cache_used: !drift,
        cache_age_secs: cache_age,
    })
}

/// Identify the AI provider id encoded in a tag set, if any. Used to
/// classify conversation-kind sources so the registrar can synthesize a
/// `SourceConfig::Conversation` with the correct provider field.
///
/// Match logic: walk [`daemon8_providers::ALL_PROVIDERS`] and return
/// the first provider whose `id()` appears in the tags. Returning
/// `None` when no provider tag is present is a feature — the registrar
/// degrades to a file-watcher in that case rather than failing.
fn provider_from_tags(tags: &[String]) -> Option<String> {
    use daemon8_providers::ALL_PROVIDERS;
    for &p in ALL_PROVIDERS {
        let id = p.as_provider().id();
        if tags.iter().any(|t| t == id) {
            return Some(id.to_string());
        }
    }
    None
}

fn source_kind_from_tag_or_default(tags: &[String]) -> SourceKind {
    for tag in tags {
        if let Ok(k) = tag.parse::<SourceKind>() {
            return k;
        }
    }
    SourceKind::Log
}

fn extract_parser_from_data(data: &Option<serde_json::Value>) -> Option<String> {
    data.as_ref()
        .and_then(|v| v.get("parser"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

struct ProbeOutcome {
    resolved: Vec<ResolvedSource>,
    misses: Vec<TemplateMiss>,
    /// Project_type tags that have at least one template successfully
    /// resolved on this filesystem. Drives hint suppression — if every
    /// classification tag is in this set, no hint is needed.
    tags_with_resolved_template: BTreeSet<String>,
}

async fn probe_templates_inner(
    librarian: &dyn LibrarianStore,
    classification: &ProjectClassification,
) -> Result<ProbeOutcome, ScannerError> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::SourceTemplate]),
        limit: Some(256),
        ..Default::default()
    };
    let nodes = librarian
        .lookup(&filter)
        .await
        .map_err(ScannerError::Librarian)?;

    let mut resolved = Vec::new();
    let mut misses = Vec::new();
    let mut tags_with_resolved_template: BTreeSet<String> = BTreeSet::new();

    for node in nodes {
        let template_id = node.id.clone().unwrap_or_default();
        let Some(ref data) = node.data else { continue };
        let template: SourceTemplateData = match serde_json::from_value(data.clone()) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    id = %template_id,
                    "source_template payload does not deserialize: {e}"
                );
                continue;
            }
        };

        if !template.platforms.contains(&classification.platform) {
            continue;
        }

        let tag_overlap: Vec<String> = template
            .project_types
            .iter()
            .filter(|t| classification.tags.contains(t) || t.as_str() == "any")
            .cloned()
            .collect();
        if tag_overlap.is_empty() {
            continue;
        }

        if !template_matches_versions(&template, &classification.framework_versions) {
            misses.push(TemplateMiss {
                template_id: template_id.clone(),
                locator_pattern: template.locator_pattern.clone(),
                reason: TemplateMissReason::VersionMismatch,
            });
            continue;
        }

        match expand_locator_pattern(&template.locator_pattern, &classification.root) {
            Err(reason) => {
                misses.push(TemplateMiss {
                    template_id: template_id.clone(),
                    locator_pattern: template.locator_pattern.clone(),
                    reason,
                });
            }
            Ok(paths) => {
                let existing: Vec<PathBuf> = paths.into_iter().filter(|p| p.exists()).collect();
                if existing.is_empty() {
                    misses.push(TemplateMiss {
                        template_id: template_id.clone(),
                        locator_pattern: template.locator_pattern.clone(),
                        reason: TemplateMissReason::PathNotFound,
                    });
                    continue;
                }
                for tag in &tag_overlap {
                    tags_with_resolved_template.insert(tag.clone());
                }
                let provider = if template.kind == SourceKind::Conversation {
                    provider_from_tags(&template.default_tags)
                } else {
                    None
                };
                for path in existing {
                    resolved.push(ResolvedSource {
                        template_id: Some(template_id.clone()),
                        kind: template.kind,
                        resolved_path: path,
                        parser: template.parser_hint.clone(),
                        tags: template.default_tags.clone(),
                        version_constraint: template.version_constraint.clone(),
                        provider: provider.clone(),
                    });
                }
            }
        }
    }

    Ok(ProbeOutcome {
        resolved,
        misses,
        tags_with_resolved_template,
    })
}

fn classification_tags_uncovered(
    classification_tags: &[String],
    covered: &BTreeSet<String>,
) -> Vec<String> {
    classification_tags
        .iter()
        .filter(|t| !covered.contains(*t))
        .cloned()
        .collect()
}

async fn wait_for_agent(
    librarian: &dyn LibrarianStore,
    initial_plan: DiscoveryPlan,
    classification: &ProjectClassification,
    user_overrides: Vec<SourceConfig>,
    config: &ScannerConfig,
    cancel: CancellationToken,
    signals: Option<Arc<DiscoverySignals>>,
) -> Result<DiscoveryPlan, ScannerError> {
    let deadline = tokio::time::Instant::now() + config.wait_timeout;
    let mut interval = tokio::time::interval(config.poll_interval);
    // Skip the immediate tick — the agent has had no time to respond
    // yet. First real check fires after `poll_interval`.
    interval.tick().await;

    let mut current = initial_plan;

    loop {
        let signal_notified = async {
            if let Some(ref s) = signals {
                s.notify.notified().await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                tracing::debug!("scanner cancelled by shutdown token");
                current.awaiting_agent = false;
                return Ok(current);
            }
            _ = signal_notified => {
                // Notification only; re-check flags before deciding.
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!(
                    timeout_secs = config.wait_timeout.as_secs(),
                    "discovery wait window elapsed without full template coverage"
                );
                current.awaiting_agent = false;
                return Ok(current);
            }
            _ = interval.tick() => {}
        }

        if let Some(ref s) = signals {
            if s.is_skip() {
                tracing::info!("discovery short-circuit: --skip signal received");
                let _ = write_skip_marker(&classification.root);
                current.awaiting_agent = false;
                return Ok(current);
            }
            if s.is_complete() {
                tracing::info!("discovery short-circuit: --complete signal received");
                let outcome = probe_templates_inner(librarian, classification).await?;
                let mut plan = plan_from_outcome(outcome, classification, user_overrides.clone());
                plan.awaiting_agent = false;
                return Ok(plan);
            }
        }

        // Re-poll the librarian; the agent may have written templates.
        let outcome = probe_templates_inner(librarian, classification).await?;
        let uncovered = classification_tags_uncovered(
            &classification.tags,
            &outcome.tags_with_resolved_template,
        );
        let pending_first_run = collect_first_run_providers(librarian).await?;
        current = plan_from_outcome(outcome, classification, user_overrides.clone());
        if uncovered.is_empty() && pending_first_run.is_empty() {
            current.awaiting_agent = false;
            return Ok(current);
        }
        current.awaiting_agent = true;
    }
}

/// Collect a [`FirstRunPayload`] for every AI provider that lacks a
/// conversation `source_template` in the librarian. Empty vec means all
/// known providers are already represented — no bootstrap branch fires.
async fn collect_first_run_providers(
    librarian: &dyn LibrarianStore,
) -> Result<Vec<daemon8_types::FirstRunPayload>, ScannerError> {
    use daemon8_providers::{ALL_PROVIDERS, dirs_home};
    let home = dirs_home();
    let mut out = Vec::new();
    for &p in ALL_PROVIDERS {
        let provider = p.as_provider();
        // Skip providers that don't expose a conversation directory at
        // all — a first-run hint would carry an empty locator and would
        // not produce a useful template.
        if provider.conversation_dir(&home).is_none() || provider.conversation_file_glob().is_none()
        {
            continue;
        }
        let first_run = conversation::is_first_run_for_provider(librarian, provider.id())
            .await
            .map_err(ScannerError::Librarian)?;
        if first_run {
            out.push(conversation::build_first_run_payload(provider, &home));
        }
    }
    Ok(out)
}

fn write_skip_marker(root: &Path) -> std::io::Result<()> {
    let marker = root.join(SKIP_MARKER_REL_PATH);
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker, b"discovery skipped\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use daemon8_types::{Platform, SourceKind, TemplateConfidence};

    use super::*;

    fn template(version_constraint: Option<&str>, frameworks: &[&str]) -> SourceTemplateData {
        SourceTemplateData {
            project_types: frameworks.iter().map(|s| (*s).to_string()).collect(),
            kind: SourceKind::Log,
            locator_pattern: "~/Library/Logs/example.log".into(),
            platforms: vec![Platform::Macos],
            parser_hint: None,
            default_tags: vec!["example".into()],
            description: "test".into(),
            version_constraint: version_constraint.map(|s| s.to_string()),
            discovered_by_session: None,
            discovered_by_provider: None,
            discovered_at_ns: 0,
            verified_count: 0,
            last_verified_at_ns: 0,
            confidence: TemplateConfidence::AgentDiscovered,
        }
    }

    // ── expand_locator_pattern ────────────────────────────────────────

    #[test]
    fn expand_home_tilde_slash() {
        let expanded = expand_locator_pattern("~/example.log", Path::new("/tmp/root")).unwrap();
        assert_eq!(expanded.len(), 1);
        let s = expanded[0].to_string_lossy();
        assert!(s.ends_with("/example.log"));
        assert!(!s.starts_with('~'));
    }

    #[test]
    fn expand_root_placeholder() {
        let expanded =
            expand_locator_pattern("<root>/logs/runtime.log", Path::new("/tmp/proj")).unwrap();
        assert_eq!(expanded, vec![PathBuf::from("/tmp/proj/logs/runtime.log")]);
    }

    #[test]
    fn expand_env_var_braced() {
        // Safe to set in tests — we set and read in the same process.
        unsafe { std::env::set_var("D8_DISCOVERY_TEST_VAR", "/var/tmp/d8") };
        let expanded =
            expand_locator_pattern("${D8_DISCOVERY_TEST_VAR}/x.log", Path::new("/")).unwrap();
        assert_eq!(expanded, vec![PathBuf::from("/var/tmp/d8/x.log")]);
        unsafe { std::env::remove_var("D8_DISCOVERY_TEST_VAR") };
    }

    #[test]
    fn expand_env_var_bare() {
        unsafe { std::env::set_var("D8_DISCOVERY_TEST_VAR2", "/srv/d8") };
        let expanded =
            expand_locator_pattern("$D8_DISCOVERY_TEST_VAR2/y.log", Path::new("/")).unwrap();
        assert_eq!(expanded, vec![PathBuf::from("/srv/d8/y.log")]);
        unsafe { std::env::remove_var("D8_DISCOVERY_TEST_VAR2") };
    }

    #[test]
    fn expand_missing_env_var_is_invalid_pattern() {
        unsafe { std::env::remove_var("D8_DISCOVERY_MISSING_VAR_XYZ") };
        let err =
            expand_locator_pattern("$D8_DISCOVERY_MISSING_VAR_XYZ/x", Path::new("/")).unwrap_err();
        match err {
            TemplateMissReason::InvalidPattern(msg) => {
                assert!(msg.contains("D8_DISCOVERY_MISSING_VAR_XYZ"));
            }
            other => panic!("expected InvalidPattern, got {other:?}"),
        }
    }

    #[test]
    fn expand_empty_pattern_rejected() {
        let err = expand_locator_pattern("   ", Path::new("/tmp")).unwrap_err();
        match err {
            TemplateMissReason::InvalidPattern(_) => {}
            other => panic!("expected InvalidPattern, got {other:?}"),
        }
    }

    #[test]
    fn expand_glob_expands_against_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.log"), "x").unwrap();
        std::fs::write(tmp.path().join("b.log"), "y").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "z").unwrap();

        let pattern = format!("{}/*.log", tmp.path().display());
        let mut expanded = expand_locator_pattern(&pattern, Path::new("/")).unwrap();
        expanded.sort();
        assert_eq!(expanded.len(), 2);
        assert!(
            expanded
                .iter()
                .all(|p| p.extension().is_some_and(|e| e == "log"))
        );
    }

    #[test]
    fn expand_literal_no_glob_no_filesystem_check() {
        // expand_locator_pattern does NOT verify existence — that's the
        // caller's job. A non-existent literal path round-trips.
        let expanded =
            expand_locator_pattern("/this/path/does/not/exist.log", Path::new("/")).unwrap();
        assert_eq!(
            expanded,
            vec![PathBuf::from("/this/path/does/not/exist.log")]
        );
    }

    // ── template_matches_versions ─────────────────────────────────────

    #[test]
    fn version_agnostic_template_always_matches() {
        let t = template(None, &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "0.74.5".into());
        assert!(template_matches_versions(&t, &versions));
    }

    #[test]
    fn version_constraint_matches_satisfied_project() {
        let t = template(Some(">=0.72, <0.80"), &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "0.74.5".into());
        assert!(template_matches_versions(&t, &versions));
    }

    #[test]
    fn version_constraint_rejects_unsatisfied_project() {
        let t = template(Some(">=0.74, <0.80"), &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "0.71.0".into());
        assert!(!template_matches_versions(&t, &versions));
    }

    #[test]
    fn version_constraint_strips_caret_prefix() {
        let t = template(Some(">=0.74"), &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "^0.74.5".into());
        assert!(template_matches_versions(&t, &versions));
    }

    #[test]
    fn unparseable_project_version_falls_back_permissive() {
        let t = template(Some(">=0.74"), &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "workspace:*".into());
        assert!(template_matches_versions(&t, &versions));
    }

    #[test]
    fn unparseable_constraint_falls_back_permissive() {
        let t = template(Some("not-a-semver-range"), &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "0.74.5".into());
        assert!(template_matches_versions(&t, &versions));
    }

    #[test]
    fn no_version_overlap_means_template_matches_on_tag() {
        // Project has rust but the template constraint targets
        // react-native — no version overlap, so the template matches
        // on tag basis alone (rust template might be version-agnostic
        // for its own framework).
        let t = template(Some(">=0.74"), &["react-native"]);
        let mut versions = BTreeMap::new();
        versions.insert("rust".into(), "1.80.0".into());
        assert!(template_matches_versions(&t, &versions));
    }

    // ── LibrarianStatus from cache freshness ──────────────────────────

    fn project_node(last_serve_at_ns: u64) -> LibrarianNode {
        let data = daemon8_types::ProjectNodeData {
            root_path: PathBuf::from("/tmp/proj"),
            slug: "proj".into(),
            classification_tags: vec!["rust".into()],
            framework_versions: BTreeMap::new(),
            platform: Platform::Macos,
            created_at_ns: 0,
            last_serve_at_ns,
            skip_discovery: false,
        };
        LibrarianNode {
            id: Some("test:1".into()),
            kind: LibrarianNodeKind::Project,
            label: "proj".into(),
            locator_kind: daemon8_types::LocatorKind::File,
            locator: "/tmp/proj".into(),
            tags: vec![],
            project_slug: "proj".into(),
            version: String::new(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            last_read_at: None,
            deprecated_at: None,
            canonicalized_at: None,
            data: Some(serde_json::to_value(&data).unwrap()),
        }
    }

    #[test]
    fn cache_freshness_fresh_within_window() {
        let last = now_ns() - 1_000_000_000; // 1s ago
        let (status, age) =
            classify_cache_freshness(&project_node(last), Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(status, LibrarianStatus::CacheHit);
        assert!(age.is_some());
    }

    #[test]
    fn cache_freshness_stale_past_window() {
        let last = now_ns() - 30u64 * 24 * 60 * 60 * 1_000_000_000;
        let (status, age) =
            classify_cache_freshness(&project_node(last), Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(status, LibrarianStatus::CacheStale);
        assert!(age.is_some());
    }

    #[test]
    fn cache_freshness_missing_payload_is_stale() {
        let mut node = project_node(now_ns());
        node.data = None;
        let (status, age) = classify_cache_freshness(&node, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(status, LibrarianStatus::CacheStale);
        assert!(age.is_none());
    }

    // ── DiscoverySignals ──────────────────────────────────────────────

    #[tokio::test]
    async fn signals_complete_flips_flag_and_notifies() {
        use daemon8_types::DiscoveryControl;

        let signals = DiscoverySignals::new();
        let notified = {
            let s = signals.clone();
            tokio::spawn(async move {
                s.notify.notified().await;
                s.is_complete()
            })
        };
        // Yield so the notified future is registered.
        tokio::task::yield_now().await;
        signals.signal_complete();
        let got = notified.await.unwrap();
        assert!(got);
    }
}
