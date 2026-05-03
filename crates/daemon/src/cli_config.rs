// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Project-local CLI hook configuration.
//!
//! `.daemon8.toml` lives at the project root (next to `.editorconfig` /
//! `.gitignore`). It abstracts per-CLI enrollment behavior so the same config
//! drives Claude Code, Cursor, Gemini, Codex, Copilot, Continue, and opencode.
//!
//! Resolution order (merged last-wins):
//!  1. System defaults
//!  2. User file:    `{config_dir}/cli.toml`
//!  3. Project file: nearest `.daemon8.toml` walking up from `cwd` to
//!     the first `.git` directory or filesystem root.
//!
//! Schema reference: https://daemon8.ai/docs/cli-hook-config

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const PROJECT_CONFIG_FILENAME: &str = ".daemon8.toml";
pub const USER_CONFIG_FILENAME: &str = "cli.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(default)]
    pub project: ProjectSection,
    #[serde(default)]
    pub enrollment: EnrollmentSection,
    #[serde(default)]
    pub features: FeaturesSection,
    #[serde(default)]
    pub intents: IntentsSection,
    #[serde(default)]
    pub distillation: DistillationSection,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
    #[serde(default)]
    pub sources: BTreeMap<String, crate::config::SourceConfig>,

    /// Runtime-only: absolute path of the project `.daemon8.toml` that
    /// provided the merge, or `None` if none was found.
    #[serde(skip)]
    pub project_config_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSection {
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnrollmentSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub banner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesSection {
    #[serde(default = "default_true")]
    pub intents: bool,
    #[serde(default = "default_true")]
    pub inbox: bool,
    #[serde(default = "default_true")]
    pub state_tracking: bool,
    #[serde(default = "default_true")]
    pub compaction_recovery: bool,
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntentsSection {
    #[serde(default)]
    pub auto_declare: AutoDeclareSection,
    #[serde(default)]
    pub interests: Vec<InterestEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoDeclareSection {
    #[serde(default)]
    pub expertise: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestEntry {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub push_on_match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistillationSection {
    #[serde(default = "default_true")]
    pub track_todowrite: bool,
    #[serde(default = "default_true")]
    pub track_git_commits: bool,
    #[serde(default = "default_true")]
    pub track_file_writes: bool,
    #[serde(default = "default_coarsen_threshold")]
    pub coarsen_file_writes_below_threshold: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

fn default_heartbeat() -> String {
    "30s".into()
}

fn default_priority() -> String {
    "normal".into()
}

fn default_coarsen_threshold() -> u32 {
    5
}

impl Default for FeaturesSection {
    fn default() -> Self {
        Self {
            intents: true,
            inbox: true,
            state_tracking: true,
            compaction_recovery: true,
            heartbeat_interval: default_heartbeat(),
        }
    }
}

impl Default for DistillationSection {
    fn default() -> Self {
        Self {
            track_todowrite: true,
            track_git_commits: true,
            track_file_writes: true,
            coarsen_file_writes_below_threshold: default_coarsen_threshold(),
        }
    }
}

impl Default for ProviderEntry {
    fn default() -> Self {
        Self {
            enabled: true,
            reason: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Walk-up discovery
// ---------------------------------------------------------------------------

/// Walk up from `start` looking for `.daemon8.toml`. Stop at the first
/// `.git` directory encountered (project boundary) or the filesystem root.
pub fn find_project_config(start: &Path) -> Option<PathBuf> {
    let mut cursor = Some(start);
    while let Some(dir) = cursor {
        let candidate = dir.join(PROJECT_CONFIG_FILENAME);
        if candidate.exists() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        cursor = dir.parent();
    }
    None
}

/// Platform-standard user config path: `{config_dir}/cli.toml`.
///
/// Mirrors the `project_dirs()` choice in `config.rs` so debug builds isolate
/// to `daemon8-dev` and never cross the production vs. dev boundary.
pub fn user_config_path() -> Option<PathBuf> {
    let app = if cfg!(debug_assertions) {
        "daemon8-dev"
    } else {
        "daemon8"
    };
    directories::ProjectDirs::from("dev", "daemon8", app)
        .map(|d| d.config_dir().join(USER_CONFIG_FILENAME))
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load a `CliConfig` by merging system defaults, the user config (if any),
/// and the nearest project config reachable by walking up from `cwd`.
///
/// Never hard-fails: parse errors fall back to the next lower layer and are
/// reported in the returned `LoadReport`.
pub fn load(cwd: &Path) -> (CliConfig, LoadReport) {
    let mut report = LoadReport::default();

    // 1. Defaults
    let mut merged = CliConfig::default();

    // 2. User file
    if let Some(user_path) = user_config_path()
        && user_path.exists()
    {
        match load_toml(&user_path) {
            Ok(cfg) => merge_into(&mut merged, cfg),
            Err(e) => report.user_error = Some(format!("{}: {e}", user_path.display())),
        }
    }

    // 3. Project file (walk-up)
    if let Some(project_path) = find_project_config(cwd) {
        match load_toml(&project_path) {
            Ok(cfg) => {
                merge_into(&mut merged, cfg);
                merged.project_config_path = Some(project_path);
            }
            Err(e) => {
                report.project_error = Some(format!("{}: {e}", project_path.display()));
            }
        }
    }

    (merged, report)
}

#[derive(Debug, Default)]
pub struct LoadReport {
    pub user_error: Option<String>,
    pub project_error: Option<String>,
}

impl LoadReport {
    pub fn has_errors(&self) -> bool {
        self.user_error.is_some() || self.project_error.is_some()
    }
}

fn load_toml(path: &Path) -> Result<CliConfig, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str::<CliConfig>(&text).map_err(|e| e.to_string())
}

/// Merge `src` into `dst` with last-wins semantics at the section level.
/// Maps (providers, sources) merge key-by-key.
fn merge_into(dst: &mut CliConfig, src: CliConfig) {
    if src.project.slug.is_some() {
        dst.project.slug = src.project.slug;
    }

    dst.enrollment = src.enrollment;
    dst.features = src.features;
    dst.distillation = src.distillation;

    if !src.intents.auto_declare.expertise.is_empty() {
        dst.intents.auto_declare.expertise = src.intents.auto_declare.expertise;
    }
    if !src.intents.interests.is_empty() {
        dst.intents.interests = src.intents.interests;
    }

    for (k, v) in src.providers {
        dst.providers.insert(k, v);
    }
    for (k, v) in src.sources {
        dst.sources.insert(k, v);
    }
}

// ---------------------------------------------------------------------------
// Behavior helpers
// ---------------------------------------------------------------------------

impl CliConfig {
    /// Returns true if enrollment should run under the given CLI tool. A
    /// provider-level `enabled = false` overrides a project-level
    /// `enrollment.enabled = true` (defensive).
    pub fn enrollment_enabled_for(&self, tool: &str) -> bool {
        if !self.enrollment.enabled {
            return false;
        }
        match self.providers.get(tool) {
            Some(p) => p.enabled,
            None => true,
        }
    }

    /// Resolve a project slug. Prefers explicit `[project].slug`, else derives
    /// from the project config's parent directory name, else from `cwd`.
    pub fn resolved_slug(&self, cwd: &Path) -> String {
        if let Some(ref s) = self.project.slug {
            return s.clone();
        }
        let base = self
            .project_config_path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(cwd);
        base.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn default_is_disabled() {
        let cfg = CliConfig::default();
        assert!(!cfg.enrollment_enabled_for("claude-code"));
        assert!(cfg.features.intents);
    }

    #[test]
    fn provider_disabled_overrides_project_enabled() {
        let mut cfg = CliConfig::default();
        cfg.enrollment.enabled = true;
        cfg.providers.insert(
            "copilot".into(),
            ProviderEntry {
                enabled: false,
                reason: Some("license".into()),
            },
        );
        assert!(cfg.enrollment_enabled_for("claude-code"));
        assert!(!cfg.enrollment_enabled_for("copilot"));
    }

    #[test]
    fn walk_up_stops_at_git() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let nested = project.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(project.join(".git")).unwrap();
        fs::write(project.join(PROJECT_CONFIG_FILENAME), "").unwrap();

        let found = find_project_config(&nested).unwrap();
        assert_eq!(found, project.join(PROJECT_CONFIG_FILENAME));
    }

    #[test]
    fn walk_up_does_not_cross_git_boundary() {
        // Writing the config file ABOVE a .git directory must not be picked up
        // from a cwd inside that git repo.
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path();
        let inner = outer.join("inner");
        fs::create_dir_all(&inner).unwrap();
        fs::create_dir(inner.join(".git")).unwrap();
        fs::write(outer.join(PROJECT_CONFIG_FILENAME), "").unwrap();

        let found = find_project_config(&inner);
        assert!(found.is_none(), "walk must stop at .git boundary");
    }

    #[test]
    fn parse_roundtrip() {
        let toml_text = r#"
[project]
slug = "daemonai"

[enrollment]
enabled = true
scope = ["crates/mcp/**"]
banner = "testing"

[features]
intents = true
inbox = true
state_tracking = false
compaction_recovery = true
heartbeat_interval = "60s"

[intents.auto_declare]
expertise = ["rust", "php"]

[[intents.interests]]
tags = ["file-write"]
pattern = '.data.tool == "Write"'
priority = "normal"
persistent = false
push_on_match = false

[providers.claude-code]
enabled = true
"#;

        let cfg: CliConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.project.slug.as_deref(), Some("daemonai"));
        assert_eq!(cfg.enrollment.scope, vec!["crates/mcp/**"]);
        assert!(!cfg.features.state_tracking);
        assert_eq!(cfg.features.heartbeat_interval, "60s");
        assert_eq!(cfg.intents.auto_declare.expertise, vec!["rust", "php"]);
        assert_eq!(cfg.intents.interests.len(), 1);
        assert_eq!(cfg.intents.interests[0].tags, vec!["file-write"]);
        assert!(cfg.providers.contains_key("claude-code"));
    }

    #[test]
    fn graceful_on_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join(PROJECT_CONFIG_FILENAME);
        fs::write(&bad, "this is [not valid toml").unwrap();

        let (_cfg, report) = load(tmp.path());
        assert!(report.project_error.is_some());
    }
}
