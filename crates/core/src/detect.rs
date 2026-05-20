// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const ECOSYSTEMS_JSONL: &str = include_str!("../data/ecosystems.jsonl");
const FILE_SIZE_CAP: u64 = 1_048_576; // 1 MB

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EcosystemCategory {
    Language,
    Framework,
    Tool,
    PackageManager,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MarkerRule {
    FileExists { path: String },
    FileContains { path: String, pattern: String },
    DirExists { path: String },
    FileGlob { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LogPathEntry {
    pub path: String,
    pub service: String,
    pub parser: String,
    pub id_suffix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EcosystemEntry {
    pub id: String,
    pub category: EcosystemCategory,
    pub language: String,
    pub markers: Vec<MarkerRule>,
    #[serde(default)]
    pub log_paths: Vec<LogPathEntry>,
    #[serde(default)]
    pub supersedes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedEcosystem {
    pub id: String,
    pub category: EcosystemCategory,
    pub language: String,
    pub log_paths: Vec<LogPathEntry>,
    pub superseded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSuggestion {
    pub id: String,
    pub service: String,
    pub path: String,
    pub parser: String,
}

pub fn load_ecosystems() -> Vec<EcosystemEntry> {
    ECOSYSTEMS_JSONL
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn match_glob(root: &Path, pattern: &str) -> bool {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| glob_matches(pattern, name) && entry.path().is_file())
        {
            return true;
        }
    }
    false
}

fn glob_matches(pattern: &str, name: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

fn check_marker(root: &Path, rule: &MarkerRule) -> bool {
    match rule {
        MarkerRule::FileExists { path } => root.join(path).is_file(),
        MarkerRule::DirExists { path } => root.join(path).is_dir(),
        MarkerRule::FileGlob { path } => match_glob(root, path),
        MarkerRule::FileContains { path, pattern } => {
            let file_path = root.join(path);
            if !file_path.is_file() {
                return false;
            }
            let meta = match file_path.metadata() {
                Ok(m) => m,
                Err(_) => return false,
            };
            if meta.len() > FILE_SIZE_CAP {
                return false;
            }
            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(_) => return false,
            };
            match regex::Regex::new(pattern) {
                Ok(re) => re.is_match(&content),
                Err(_) => false,
            }
        }
    }
}

fn matches_ecosystem(root: &Path, entry: &EcosystemEntry) -> bool {
    entry.markers.iter().any(|rule| check_marker(root, rule))
}

pub fn detect_ecosystems(root: &Path) -> Vec<DetectedEcosystem> {
    let entries = load_ecosystems();

    let (matched, supersedes): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .filter(|entry| matches_ecosystem(root, entry))
        .map(|entry| {
            let sup = entry.supersedes.clone();
            let eco = DetectedEcosystem {
                id: entry.id,
                category: entry.category,
                language: entry.language,
                log_paths: entry.log_paths,
                superseded: false,
            };
            (eco, sup)
        })
        .unzip();

    let matched_ids: HashSet<&str> = matched.iter().map(|m| m.id.as_str()).collect();
    let superseded_ids: HashSet<&str> = supersedes
        .iter()
        .zip(&matched)
        .filter(|(_, m)| matched_ids.contains(m.id.as_str()))
        .flat_map(|(targets, _)| targets.iter().map(String::as_str))
        .collect();

    matched
        .into_iter()
        .map(|mut m| {
            m.superseded = superseded_ids.contains(m.id.as_str());
            m
        })
        .collect()
}

pub fn detect_workspace_children(root: &Path) -> Vec<(PathBuf, Vec<DetectedEcosystem>)> {
    let dir_entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let ecosystems = load_ecosystems();
    let mut results = Vec::new();

    for entry in dir_entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }

        let child_matches: Vec<DetectedEcosystem> = ecosystems
            .iter()
            .filter(|eco| matches_ecosystem(&path, eco))
            .map(|eco| DetectedEcosystem {
                id: eco.id.clone(),
                category: eco.category.clone(),
                language: eco.language.clone(),
                log_paths: eco.log_paths.clone(),
                superseded: false,
            })
            .collect();

        if !child_matches.is_empty() {
            results.push((path, child_matches));
        }
    }

    results
}

pub fn ecosystems_to_stack(ecosystems: &[DetectedEcosystem]) -> crate::init::DetectedStack {
    let mut languages = HashSet::new();
    let mut frameworks = HashSet::new();
    let mut tools = HashSet::new();

    for eco in ecosystems {
        languages.insert(eco.language.clone());

        if eco.superseded {
            continue;
        }

        match eco.category {
            EcosystemCategory::Framework => {
                frameworks.insert(eco.id.clone());
            }
            EcosystemCategory::Tool | EcosystemCategory::PackageManager => {
                tools.insert(eco.id.clone());
            }
            EcosystemCategory::Language => {}
        }
    }

    let mut languages: Vec<_> = languages.into_iter().collect();
    let mut frameworks: Vec<_> = frameworks.into_iter().collect();
    let mut tools: Vec<_> = tools.into_iter().collect();
    languages.sort();
    frameworks.sort();
    tools.sort();

    crate::init::DetectedStack {
        languages,
        frameworks,
        tools,
    }
}

pub fn ecosystems_to_sources(ecosystems: &[DetectedEcosystem]) -> Vec<SourceSuggestion> {
    let mut sources = Vec::new();
    let mut seen_ids = HashSet::new();

    for eco in ecosystems {
        if eco.superseded {
            continue;
        }
        for log in &eco.log_paths {
            let id = format!("{}.{}", eco.id, log.id_suffix);
            if !seen_ids.insert(id.clone()) {
                continue;
            }
            sources.push(SourceSuggestion {
                id,
                service: log.service.clone(),
                path: log.path.clone(),
                parser: log.parser.clone(),
            });
        }
    }

    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_parses_all_entries() {
        let entries = load_ecosystems();
        assert!(
            entries.len() >= 10,
            "expected at least 10 entries, got {}",
            entries.len()
        );
        for entry in &entries {
            assert!(!entry.id.is_empty(), "entry has empty id");
            assert!(
                !entry.markers.is_empty(),
                "entry {} has no markers",
                entry.id
            );
        }
    }

    #[test]
    fn jsonl_entries_have_valid_parsers() {
        let valid = [
            "line", "json", "syslog", "logfmt", "clf", "monolog", "auto", "grok",
        ];
        for entry in load_ecosystems() {
            for log in &entry.log_paths {
                assert!(
                    valid.contains(&log.parser.as_str()),
                    "entry {} has invalid parser: {}",
                    entry.id,
                    log.parser
                );
            }
        }
    }

    #[test]
    fn jsonl_supersedes_reference_valid_ids() {
        let entries = load_ecosystems();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        for entry in &entries {
            for target in &entry.supersedes {
                assert!(
                    ids.contains(&target.as_str()),
                    "entry {} supersedes non-existent id: {}",
                    entry.id,
                    target
                );
            }
        }
    }

    #[test]
    fn jsonl_all_regex_patterns_compile() {
        for entry in load_ecosystems() {
            for marker in &entry.markers {
                if let MarkerRule::FileContains { pattern, .. } = marker {
                    assert!(
                        regex::Regex::new(pattern).is_ok(),
                        "entry {} has invalid regex: {}",
                        entry.id,
                        pattern
                    );
                }
            }
        }
    }

    #[test]
    fn detect_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        let detected = detect_ecosystems(dir.path());
        let ids: Vec<&str> = detected.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"rust"), "expected rust, got {:?}", ids);
        assert!(ids.contains(&"cargo"), "expected cargo, got {:?}", ids);
    }

    #[test]
    fn detect_laravel_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(
            dir.path().join("composer.json"),
            r#"{"require":{"laravel/framework":"^11.0"}}"#,
        )
        .unwrap();

        let detected = detect_ecosystems(dir.path());
        let ids: Vec<&str> = detected.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"laravel"), "expected laravel, got {:?}", ids);
    }

    #[test]
    fn detect_nextjs_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("next.config.js"), "module.exports = {}").unwrap();

        let detected = detect_ecosystems(dir.path());
        let ids: Vec<&str> = detected.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"nextjs"), "expected nextjs, got {:?}", ids);
    }

    #[test]
    fn supersedes_chain_marks_language() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(dir.path().join("composer.json"), "{}").unwrap();

        let detected = detect_ecosystems(dir.path());
        let php = detected.iter().find(|d| d.id == "php");
        let laravel = detected.iter().find(|d| d.id == "laravel");
        assert!(laravel.is_some(), "laravel should be detected");
        if let Some(php_entry) = php {
            assert!(php_entry.superseded, "php should be superseded by laravel");
        }
    }

    #[test]
    fn ecosystems_to_stack_populates_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(dir.path().join("composer.json"), "{}").unwrap();

        let detected = detect_ecosystems(dir.path());
        let stack = ecosystems_to_stack(&detected);
        assert!(stack.languages.contains(&"php".into()));
        assert!(stack.frameworks.contains(&"laravel".into()));
    }

    #[test]
    fn ecosystems_to_sources_generates_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(dir.path().join("composer.json"), "{}").unwrap();

        let detected = detect_ecosystems(dir.path());
        let sources = ecosystems_to_sources(&detected);
        let laravel_source = sources.iter().find(|s| s.id.starts_with("laravel."));
        assert!(
            laravel_source.is_some(),
            "expected laravel source suggestion"
        );
        let src = laravel_source.unwrap();
        assert_eq!(src.parser, "monolog");
    }

    #[test]
    fn workspace_children_detects_monorepo() {
        let dir = tempfile::tempdir().unwrap();
        let child_a = dir.path().join("backend");
        let child_b = dir.path().join("frontend");
        std::fs::create_dir_all(&child_a).unwrap();
        std::fs::create_dir_all(&child_b).unwrap();
        std::fs::write(child_a.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(child_b.join("package.json"), "{}").unwrap();

        let results = detect_workspace_children(dir.path());
        assert!(
            results.len() >= 2,
            "expected 2+ children, got {}",
            results.len()
        );
    }

    #[test]
    fn workspace_children_skips_hidden_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let hidden = dir.path().join(".git");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("Cargo.toml"), "[package]").unwrap();

        let results = detect_workspace_children(dir.path());
        assert!(results.is_empty(), "hidden dirs should be skipped");
    }

    #[test]
    fn workspace_children_empty_for_flat_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let results = detect_workspace_children(dir.path());
        assert!(
            results.is_empty(),
            "single project should have no workspace children"
        );
    }

    #[test]
    fn glob_matches_suffix_pattern() {
        assert!(glob_matches("*.sln", "MyApp.sln"));
        assert!(glob_matches("*.csproj", "Web.csproj"));
        assert!(!glob_matches("*.sln", "MyApp.txt"));
        assert!(!glob_matches("*.sln", "sln"));
    }

    #[test]
    fn glob_matches_prefix_pattern() {
        assert!(glob_matches("lib*", "libfoo"));
        assert!(glob_matches("lib*", "lib"));
        assert!(!glob_matches("lib*", "notlib"));
    }

    #[test]
    fn glob_matches_exact() {
        assert!(glob_matches("Makefile", "Makefile"));
        assert!(!glob_matches("Makefile", "makefile"));
    }

    #[test]
    fn match_glob_finds_file_in_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("App.sln"), "").unwrap();
        assert!(match_glob(dir.path(), "*.sln"));
        assert!(!match_glob(dir.path(), "*.csproj"));
    }

    #[test]
    fn file_contains_skips_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let big_file = dir.path().join("big.json");
        let content = "x".repeat(FILE_SIZE_CAP as usize + 1);
        std::fs::write(&big_file, content).unwrap();

        let rule = MarkerRule::FileContains {
            path: "big.json".into(),
            pattern: "x".into(),
        };
        assert!(!check_marker(dir.path(), &rule));
    }
}
