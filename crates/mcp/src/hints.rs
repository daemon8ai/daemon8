// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Path-pattern hints injected into `query_observations` responses.
//!
//! When an observation references a filesystem path that looks like a
//! reusable runtime-data location (a log file, a framework data dir,
//! an editor extension dir) and no librarian `source_template` covers
//! that kind of path for the active project's classification tags,
//! daemon8 emits a hint nudging the agent to call `librarian_index`
//! with a `source_template` entry.
//!
//! The matcher is **conservative on purpose**. False positives train
//! agents to ignore hints. We only fire when:
//!
//! 1. The string is unambiguously a filesystem path (absolute, `~`,
//!    `<root>`, or env-var prefix; long enough to be a real path;
//!    contains a separator).
//! 2. The path matches one of a small, hand-picked category prefix
//!    set (system log, user library, user cache, project-local).
//! 3. No librarian source_template already covers a path with the
//!    same prefix for the project's tags.
//! 4. The path is not an AI-provider conversation file (those are
//!    handled by the D5 first-run flow, not this matcher).
//!
//! We cap output at three paths per response — a longer list
//! overwhelms the response and dulls the signal.
//!
//! The hint text lands in `DaemonMeta::hints` and serializes as
//! `daemon8.hints: ["..."]` on the response envelope.

use std::collections::BTreeSet;
use std::sync::Arc;

use daemon8_store::{LibrarianFilter, LibrarianStore};
use daemon8_types::{LibrarianNodeKind, SourceTemplateData};
use serde_json::Value;

const MAX_HINT_PATHS: usize = 3;
const MIN_PATH_LEN: usize = 8;

/// Pre-rendered hint plus its supporting data — exposed for tests
/// that want to assert structure, not just substring matches.
#[derive(Debug, Clone)]
pub struct PathPatternHint {
    pub paths: Vec<String>,
    pub matched_category: HintCategory,
    pub project_tags: Vec<String>,
    pub hint_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintCategory {
    SystemLog,
    UserLibrary,
    UserCache,
    ProjectLocal,
}

impl HintCategory {
    fn label(self) -> &'static str {
        match self {
            Self::SystemLog => "system log",
            Self::UserLibrary => "user library",
            Self::UserCache => "user cache",
            Self::ProjectLocal => "project-local",
        }
    }
}

/// Inspect the observations a `query_observations` call is about to
/// return. If any contain a path that meets every conservative gate
/// above, return a [`PathPatternHint`] for the response envelope.
///
/// Runs on every successful query. Designed to be cheap: a single
/// librarian lookup (templates only), a single walk of the response
/// observation JSON, no filesystem I/O.
pub async fn maybe_emit_path_hint(
    response_observations: &[Value],
    librarian: Option<&Arc<dyn LibrarianStore>>,
    project_tags: &[String],
    project_root: Option<&str>,
) -> Option<PathPatternHint> {
    if response_observations.is_empty() {
        return None;
    }

    let candidates = collect_candidate_paths(response_observations);
    if candidates.is_empty() {
        return None;
    }

    let covered_prefixes = match librarian {
        Some(store) => covered_prefixes(store.as_ref(), project_tags).await,
        None => Vec::new(),
    };

    // First category to produce uncovered paths wins. A single hint
    // per response stays focused; mixing categories dilutes the nudge.
    let mut by_category: Vec<(HintCategory, Vec<String>)> = Vec::new();
    for category in [
        HintCategory::SystemLog,
        HintCategory::UserLibrary,
        HintCategory::UserCache,
        HintCategory::ProjectLocal,
    ] {
        let mut matches = Vec::new();
        for path in &candidates {
            if classify_path(path, project_root) == Some(category)
                && !is_suppressed_path(path)
                && !template_covers_path(path, &covered_prefixes)
                && !matches.contains(path)
            {
                matches.push(path.clone());
            }
        }
        if !matches.is_empty() {
            by_category.push((category, matches));
        }
    }

    let (category, mut paths) = by_category.into_iter().next()?;
    let overflow = paths.len().saturating_sub(MAX_HINT_PATHS);
    paths.truncate(MAX_HINT_PATHS);

    let hint_text = render_hint(category, &paths, overflow, project_tags);

    Some(PathPatternHint {
        paths,
        matched_category: category,
        project_tags: project_tags.to_vec(),
        hint_text,
    })
}

fn render_hint(
    category: HintCategory,
    paths: &[String],
    overflow: usize,
    project_tags: &[String],
) -> String {
    let count = paths.len() + overflow;
    let joined = if overflow > 0 {
        format!("{}, ... and {overflow} more", paths.join(", "))
    } else {
        paths.join(", ")
    };
    let tag_str = if project_tags.is_empty() {
        "(no active project classified)".to_string()
    } else {
        project_tags.join(", ")
    };
    format!(
        "daemon8 hint: {count} observation path(s) look like a reusable {label} reference ({joined}). \
         No librarian source_template covers this kind of path for the active project's tags ({tag_str}). \
         Consider librarian_index with a source_template entry to capture this for future sessions.",
        label = category.label(),
    )
}

// ── Path extraction ────────────────────────────────────────────────

fn collect_candidate_paths(observations: &[Value]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for obs in observations {
        walk_strings(obs, &mut |s| {
            if !looks_like_path(s) {
                return;
            }
            if seen.insert(s.to_string()) {
                out.push(s.to_string());
            }
        });
    }
    out
}

fn walk_strings(v: &Value, on_string: &mut impl FnMut(&str)) {
    match v {
        Value::String(s) => on_string(s),
        Value::Array(arr) => {
            for item in arr {
                walk_strings(item, on_string);
            }
        }
        Value::Object(map) => {
            for (_, val) in map {
                walk_strings(val, on_string);
            }
        }
        _ => {}
    }
}

fn looks_like_path(s: &str) -> bool {
    if s.len() < MIN_PATH_LEN {
        return false;
    }
    let has_separator = s.contains('/') || s.contains('\\');
    if !has_separator {
        return false;
    }
    starts_with_path_prefix(s)
}

fn starts_with_path_prefix(s: &str) -> bool {
    s.starts_with('/')
        || s.starts_with("~/")
        || s.starts_with("~\\")
        || s.starts_with("<root>")
        || s.starts_with('$')
        || windows_drive_prefix(s)
}

fn windows_drive_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

// ── Category classification ────────────────────────────────────────

fn classify_path(path: &str, project_root: Option<&str>) -> Option<HintCategory> {
    if is_system_log(path) {
        return Some(HintCategory::SystemLog);
    }
    if is_user_library(path) {
        return Some(HintCategory::UserLibrary);
    }
    if is_user_cache(path) {
        return Some(HintCategory::UserCache);
    }
    if is_project_local(path, project_root) {
        return Some(HintCategory::ProjectLocal);
    }
    None
}

fn is_system_log(path: &str) -> bool {
    if path.starts_with("/var/log/") {
        return true;
    }
    if let Some(rest) = path.strip_prefix("/tmp/") {
        return rest.ends_with(".log") || rest.contains("/logs/");
    }
    false
}

fn is_user_library(path: &str) -> bool {
    if path.starts_with("~/Library/Logs/") {
        return true;
    }
    if let Some(rest) = path.strip_prefix("~/Library/Application Support/") {
        return rest.contains("/logs/") || rest.ends_with(".log");
    }
    false
}

fn is_user_cache(path: &str) -> bool {
    path.starts_with("~/.cache/") || path.starts_with("~/Library/Caches/")
}

fn is_project_local(path: &str, project_root: Option<&str>) -> bool {
    let stripped = if let Some(rest) = path.strip_prefix("<root>/") {
        rest
    } else if let Some(root) = project_root {
        // Only treat absolute paths as project-local when they start
        // with the active project root. Without a root we cannot tell.
        match path.strip_prefix(root) {
            Some(rest) => rest.trim_start_matches('/'),
            None => return false,
        }
    } else {
        return false;
    };

    stripped.starts_with("logs/")
        || stripped.starts_with("storage/logs/")
        || stripped.starts_with(".next/")
        || stripped.starts_with("var/log/")
        || stripped.ends_with(".log")
        || stripped.contains("/logs/")
}

// ── Suppression rules ──────────────────────────────────────────────

fn is_suppressed_path(path: &str) -> bool {
    // D5 already handles AI conversation files via the first-run hint
    // mechanism. Re-nudging here would be noise.
    const PROVIDER_PREFIXES: [&str; 8] = [
        "~/.claude/",
        "~/.codex/",
        "~/.gemini/",
        "~/.opencode/",
        "/Users/", // never seen via librarian patterns, but a literal absolute home path would have failed validation upstream; defense in depth
        "/.claude/",
        "/.codex/",
        "/.gemini/",
    ];
    PROVIDER_PREFIXES
        .iter()
        .any(|p| path.contains(p) && path.contains("/projects/"))
        || path.contains("/.claude/")
        || path.contains("/.codex/")
        || path.contains("/.gemini/")
        || path.contains("/.opencode/")
}

// ── Template coverage ──────────────────────────────────────────────

async fn covered_prefixes(librarian: &dyn LibrarianStore, project_tags: &[String]) -> Vec<String> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::SourceTemplate]),
        limit: Some(256),
        ..Default::default()
    };
    let nodes = match librarian.lookup(&filter).await {
        Ok(n) => n,
        Err(e) => {
            tracing::debug!("path-hint librarian lookup failed: {e}");
            return Vec::new();
        }
    };

    let mut prefixes = Vec::new();
    for node in nodes {
        let Some(ref data) = node.data else { continue };
        let template: SourceTemplateData = match serde_json::from_value(data.clone()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !template_applies_to_tags(&template, project_tags) {
            continue;
        }
        prefixes.push(template.locator_pattern);
    }
    prefixes
}

fn template_applies_to_tags(template: &SourceTemplateData, project_tags: &[String]) -> bool {
    if template.project_types.iter().any(|t| t.as_str() == "any") {
        return true;
    }
    template
        .project_types
        .iter()
        .any(|t| project_tags.contains(t))
}

// Conservative "does this template's pattern cover this observation
// path?" check. We do not invoke filesystem expansion; we strip the
// portable prefixes (`~`, `<root>`, `$VAR` becomes "*") and check
// whether the observation path starts with the resulting stem up to
// the first glob meta-char. False negatives (failing to detect
// coverage) cause a redundant hint at worst; false positives would
// silently suppress legitimate hints, which is the harmful direction.
fn template_covers_path(observation_path: &str, template_patterns: &[String]) -> bool {
    for pattern in template_patterns {
        if pattern_matches(pattern, observation_path) {
            return true;
        }
    }
    false
}

fn pattern_matches(pattern: &str, observation_path: &str) -> bool {
    let pattern_norm = normalize_pattern(pattern);
    let obs_norm = normalize_pattern(observation_path);
    let stem = stem_before_glob(&pattern_norm);
    if stem.is_empty() {
        return false;
    }
    obs_norm.starts_with(&stem)
}

fn normalize_pattern(s: &str) -> String {
    // Strip `~` and `<root>` to leave the shape comparable. We do not
    // expand them to absolute paths because the observation path is
    // already what the agent recorded — usually absolute, sometimes
    // with `~`. Normalizing both sides to the portable form gives a
    // useful overlap test without touching the filesystem.
    let mut out = s.to_string();
    if let Some(rest) = out.strip_prefix("~/") {
        out = format!("HOME/{rest}");
    } else if out == "~" {
        out = "HOME".into();
    }
    if let Some(rest) = out.strip_prefix("<root>/") {
        out = format!("ROOT/{rest}");
    }
    // Reduce expanded home form to the same prefix so an observation
    // at `/Users/jh/Library/...` and a template `~/Library/...` line up.
    if let Some(home) = dirs::home_dir()
        && let Some(home_str) = home.to_str()
        && let Some(rest) = out.strip_prefix(home_str)
    {
        out = format!("HOME{}", rest);
    }
    out
}

fn stem_before_glob(pattern: &str) -> String {
    let cut = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    pattern[..cut].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_store::{LibrarianNode, SurrealStore};
    use daemon8_types::{LocatorKind, Platform, SourceKind, TemplateConfidence};
    use serde_json::json;

    fn obs_with_path(path: &str) -> Value {
        json!({
            "id": 1,
            "data": {
                "path": path,
                "message": format!("opened {path}"),
            }
        })
    }

    #[tokio::test]
    async fn empty_observations_returns_none() {
        let hint = maybe_emit_path_hint(&[], None, &["react-native".into()], None).await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn system_log_fires() {
        let obs = vec![obs_with_path("/tmp/metro.log")];
        let hint = maybe_emit_path_hint(&obs, None, &["react-native".into()], None)
            .await
            .expect("hint expected");
        assert_eq!(hint.matched_category, HintCategory::SystemLog);
        assert_eq!(hint.paths, vec!["/tmp/metro.log".to_string()]);
        assert!(hint.hint_text.contains("/tmp/metro.log"));
        assert!(hint.hint_text.contains("react-native"));
    }

    #[tokio::test]
    async fn user_library_fires() {
        let path = "~/Library/Application Support/Code/logs/2026/extension.log";
        let obs = vec![obs_with_path(path)];
        let hint = maybe_emit_path_hint(&obs, None, &["react-native".into()], None)
            .await
            .expect("hint expected");
        assert_eq!(hint.matched_category, HintCategory::UserLibrary);
    }

    #[tokio::test]
    async fn user_cache_fires() {
        let obs = vec![obs_with_path("~/.cache/some-tool/state.bin")];
        let hint = maybe_emit_path_hint(&obs, None, &["rust".into()], None)
            .await
            .expect("hint expected");
        assert_eq!(hint.matched_category, HintCategory::UserCache);
    }

    #[tokio::test]
    async fn project_local_fires_with_root() {
        let obs = vec![obs_with_path("/Users/me/code/app/storage/logs/laravel.log")];
        let hint =
            maybe_emit_path_hint(&obs, None, &["laravel".into()], Some("/Users/me/code/app"))
                .await
                .expect("hint expected");
        assert_eq!(hint.matched_category, HintCategory::ProjectLocal);
    }

    #[tokio::test]
    async fn project_local_root_token() {
        let obs = vec![obs_with_path("<root>/logs/runtime.log")];
        let hint = maybe_emit_path_hint(&obs, None, &["any".into()], None)
            .await
            .expect("hint expected");
        assert_eq!(hint.matched_category, HintCategory::ProjectLocal);
    }

    #[tokio::test]
    async fn non_path_strings_ignored() {
        let obs = vec![json!({
            "data": {
                "message": "this is a regular log message with no path",
                "url": "https://example.com/api",
            }
        })];
        let hint = maybe_emit_path_hint(&obs, None, &["rust".into()], None).await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn similar_but_unqualified_path_ignored() {
        // /etc/hosts looks pathy but isn't a log/data location we care about.
        let obs = vec![obs_with_path("/etc/hosts")];
        let hint = maybe_emit_path_hint(&obs, None, &["rust".into()], None).await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn conversation_path_suppressed() {
        let path = "/Users/jh/.claude/projects/some-slug/abc.jsonl";
        let obs = vec![obs_with_path(path)];
        let hint = maybe_emit_path_hint(&obs, None, &["any".into()], None).await;
        assert!(
            hint.is_none(),
            "conversation paths must be handled by D5 first-run, not the path matcher"
        );
    }

    #[tokio::test]
    async fn codex_conversation_path_suppressed() {
        let obs = vec![obs_with_path("~/.codex/sessions/2026-05-12/x.jsonl")];
        let hint = maybe_emit_path_hint(&obs, None, &["any".into()], None).await;
        assert!(hint.is_none());
    }

    #[tokio::test]
    async fn three_path_cap_with_overflow_suffix() {
        let obs: Vec<Value> = (0..5)
            .map(|i| obs_with_path(&format!("/tmp/svc-{i}.log")))
            .collect();
        let hint = maybe_emit_path_hint(&obs, None, &["any".into()], None)
            .await
            .expect("hint expected");
        assert_eq!(hint.paths.len(), 3);
        assert!(
            hint.hint_text.contains("... and 2 more"),
            "expected overflow suffix in hint, got: {}",
            hint.hint_text
        );
    }

    #[tokio::test]
    async fn template_covered_path_suppressed() {
        let store = SurrealStore::memory().await.unwrap();
        let lib = Arc::new(store.librarian_store()) as Arc<dyn LibrarianStore>;
        let template = SourceTemplateData {
            project_types: vec!["react-native".into()],
            kind: SourceKind::Log,
            locator_pattern: "/tmp/metro.log".into(),
            platforms: vec![Platform::Macos],
            parser_hint: None,
            default_tags: vec![],
            description: "metro".into(),
            version_constraint: None,
            discovered_by_session: None,
            discovered_by_provider: None,
            discovered_at_ns: 0,
            verified_count: 0,
            last_verified_at_ns: 0,
            confidence: TemplateConfidence::AgentDiscovered,
        };
        let node = LibrarianNode {
            id: None,
            kind: LibrarianNodeKind::SourceTemplate,
            label: "metro log".into(),
            locator_kind: LocatorKind::File,
            locator: "/tmp/metro.log".into(),
            tags: vec!["react-native".into()],
            project_slug: "test".into(),
            version: "2026.05.13".into(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            last_read_at: None,
            deprecated_at: None,
            canonicalized_at: None,
            data: Some(serde_json::to_value(&template).unwrap()),
        };
        lib.index_node(node).await.unwrap();

        let obs = vec![obs_with_path("/tmp/metro.log")];
        let hint = maybe_emit_path_hint(&obs, Some(&lib), &["react-native".into()], None).await;
        assert!(
            hint.is_none(),
            "template covers /tmp/metro.log; hint should be suppressed"
        );
    }

    #[tokio::test]
    async fn template_for_different_tag_does_not_suppress() {
        let store = SurrealStore::memory().await.unwrap();
        let lib = Arc::new(store.librarian_store()) as Arc<dyn LibrarianStore>;
        let template = SourceTemplateData {
            project_types: vec!["laravel".into()],
            kind: SourceKind::Log,
            locator_pattern: "/tmp/laravel.log".into(),
            platforms: vec![Platform::Macos],
            parser_hint: None,
            default_tags: vec![],
            description: "laravel".into(),
            version_constraint: None,
            discovered_by_session: None,
            discovered_by_provider: None,
            discovered_at_ns: 0,
            verified_count: 0,
            last_verified_at_ns: 0,
            confidence: TemplateConfidence::AgentDiscovered,
        };
        let node = LibrarianNode {
            id: None,
            kind: LibrarianNodeKind::SourceTemplate,
            label: "laravel log".into(),
            locator_kind: LocatorKind::File,
            locator: "/tmp/laravel.log".into(),
            tags: vec!["laravel".into()],
            project_slug: "test".into(),
            version: "2026.05.13".into(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            last_read_at: None,
            deprecated_at: None,
            canonicalized_at: None,
            data: Some(serde_json::to_value(&template).unwrap()),
        };
        lib.index_node(node).await.unwrap();

        // Active project is react-native, librarian only has a laravel template.
        let obs = vec![obs_with_path("/tmp/metro.log")];
        let hint = maybe_emit_path_hint(&obs, Some(&lib), &["react-native".into()], None).await;
        assert!(
            hint.is_some(),
            "laravel-tagged template should not suppress a react-native hint"
        );
    }

    #[test]
    fn looks_like_path_rejects_short_or_separatorless() {
        assert!(!looks_like_path("hi"));
        assert!(!looks_like_path("no-sep"));
        assert!(!looks_like_path("/x")); // too short
        assert!(looks_like_path("/tmp/foo.log"));
        assert!(looks_like_path("~/Library/Logs/app.log"));
        assert!(looks_like_path("<root>/logs/run.log"));
    }

    #[test]
    fn classify_path_buckets() {
        assert_eq!(
            classify_path("/tmp/metro.log", None),
            Some(HintCategory::SystemLog)
        );
        assert_eq!(
            classify_path("/var/log/app.log", None),
            Some(HintCategory::SystemLog)
        );
        assert_eq!(
            classify_path("~/Library/Logs/app/x.log", None),
            Some(HintCategory::UserLibrary)
        );
        assert_eq!(
            classify_path("~/.cache/foo/bar.bin", None),
            Some(HintCategory::UserCache)
        );
        assert_eq!(classify_path("/etc/hosts", None), None);
        assert_eq!(classify_path("/tmp/x", None), None);
    }
}
