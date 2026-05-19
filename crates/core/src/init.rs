// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::control::{
    AlphaEnvelope, AlphaStatus, NextAction, ScopeCandidate, ScopeMode, classify_scope,
};
use crate::project_config::{PROJECT_CONFIG_SCHEMA, parse_project_config_str, slugify};

pub const PROJECT_CONFIG_DIR: &str = ".daemon8";
pub const PROJECT_CONFIG_FILENAME: &str = "config.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitRequest {
    pub project_path: PathBuf,
    pub name: Option<String>,
    pub overwrite: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitOutcome {
    pub envelope: AlphaEnvelope,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedStack {
    pub languages: Vec<&'static str>,
    pub frameworks: Vec<&'static str>,
    pub tools: Vec<&'static str>,
}

pub fn init_project(request: InitRequest) -> InitOutcome {
    let requested_path = request.project_path.display().to_string();
    let canonical = match canonical_project_dir(&request.project_path) {
        Ok(path) => path,
        Err(reason) => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Error,
                    "invalid_scope",
                    "path cannot be used as a daemon8 project",
                    reason,
                )
                .with_data(json!({
                    "mode": ScopeMode::Invalid,
                    "requested_path": requested_path,
                })),
                config_path: None,
            };
        }
    };
    let scope_root = match classify_scope(&canonical) {
        ScopeCandidate::Project(path) => path,
        ScopeCandidate::General(scope) => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "general_scope",
                    "project marker is missing",
                    "daemon8_init writes project config only; run it from a directory with .git, Cargo.toml, package.json, composer.json, pyproject.toml, go.mod, artisan, or bin/console",
                )
                .with_data(json!({
                    "mode": ScopeMode::General,
                    "requested_path": requested_path,
                    "scope_root": scope.display().to_string(),
                })),
                config_path: None,
            };
        }
    };

    let config_dir = scope_root.join(PROJECT_CONFIG_DIR);
    let config_path = config_dir.join(PROJECT_CONFIG_FILENAME);
    let common_data = json!({
        "mode": ScopeMode::Project,
        "requested_path": requested_path,
        "scope_root": scope_root.display().to_string(),
        "config_path": config_path.display().to_string(),
    });

    if config_path.exists() && !request.overwrite {
        return InitOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::Blocked,
                "config_exists",
                "project config already exists",
                ".daemon8/config.md already exists; daemon8_init will only replace it when overwrite is true",
            )
            .with_data(common_data)
            .with_next_action(NextAction::new(
                "daemon8_init",
                "overwrite the existing project config if replacement is intentional",
                json!({
                    "project_path": scope_root.display().to_string(),
                    "overwrite": true,
                }),
            )),
            config_path: Some(config_path),
        };
    }

    let name = match request.name {
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return InitOutcome {
                    envelope: AlphaEnvelope::non_success(
                        AlphaStatus::Error,
                        "invalid_project_name",
                        "project name is empty",
                        "daemon8_init requires a non-empty project name",
                    )
                    .with_data(common_data),
                    config_path: None,
                };
            }
            trimmed.to_string()
        }
        None => derive_name(&scope_root),
    };
    let stack = detect_stack(&scope_root);
    let contents = render_project_config(&name, &scope_root, &stack);
    let config = match parse_project_config_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Error,
                    "generated_config_invalid",
                    "generated project config did not validate",
                    err.to_string(),
                )
                .with_data(common_data),
                config_path: None,
            };
        }
    };

    match std::fs::symlink_metadata(&config_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "unsafe_config_dir",
                    "project config directory is a symlink",
                    ".daemon8 must be a real project-local directory before daemon8_init can write config.md",
                )
                .with_data(common_data),
                config_path: Some(config_path),
            };
        }
        Ok(_) | Err(_) => {}
    }

    if let Err(err) = std::fs::create_dir_all(&config_dir) {
        return InitOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::Error,
                "write_failed",
                "failed to create project config directory",
                format!("{}: {err}", config_dir.display()),
            )
            .with_data(common_data),
            config_path: Some(config_path),
        };
    }

    match std::fs::symlink_metadata(&config_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "unsafe_config_file",
                    "project config file is a symlink",
                    ".daemon8/config.md must be a real project-local file before daemon8_init can replace it",
                )
                .with_data(common_data),
                config_path: Some(config_path),
            };
        }
        Ok(meta) if !meta.file_type().is_file() => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "unsafe_config_file",
                    "project config path is not a file",
                    ".daemon8/config.md must be a real project-local file before daemon8_init can replace it",
                )
                .with_data(common_data),
                config_path: Some(config_path),
            };
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Error,
                    "write_failed",
                    "failed to inspect project config path",
                    format!("{}: {err}", config_path.display()),
                )
                .with_data(common_data),
                config_path: Some(config_path),
            };
        }
    }

    if let Err(err) = std::fs::write(&config_path, contents) {
        return InitOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::Error,
                "write_failed",
                "failed to write project config",
                format!("{}: {err}", config_path.display()),
            )
            .with_data(common_data),
            config_path: Some(config_path),
        };
    }

    let data = merge_data(
        common_data,
        json!({
            "project_name": config.project.name,
            "source_count": config.sources.len(),
        }),
    );

    InitOutcome {
        envelope: AlphaEnvelope::success("initialized", "project config written", data)
            .with_next_action(NextAction::new(
                "daemon8_connect",
                "connect this MCP session to the initialized project",
                json!({"project_path": scope_root.display().to_string()}),
            ))
            .with_hint("sources is empty -- after connecting, add file and conversation source entries to .daemon8/config.md for this project's logs, build output, and provider transcripts"),
        config_path: Some(config_path),
    }
}

pub fn derive_name(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string()
}

pub fn detect_stack(cwd: &Path) -> DetectedStack {
    let mut languages = Vec::new();
    let mut frameworks = Vec::new();
    let mut tools = Vec::new();

    if cwd.join("artisan").exists() {
        languages.push("php");
        frameworks.push("laravel");
    }
    if cwd.join("bin/console").exists() {
        languages.push("php");
        frameworks.push("symfony");
    }
    if cwd.join("package.json").exists() {
        languages.push("javascript");
        tools.push("node");
    }
    if cwd.join("Cargo.toml").exists() {
        languages.push("rust");
        tools.push("cargo");
    }
    if languages.is_empty() && frameworks.is_empty() && tools.is_empty() {
        languages.push("generic");
    }

    languages.sort_unstable();
    languages.dedup();
    frameworks.sort_unstable();
    frameworks.dedup();
    tools.sort_unstable();
    tools.dedup();

    DetectedStack {
        languages,
        frameworks,
        tools,
    }
}

pub fn render_project_config(name: &str, root: &Path, stack: &DetectedStack) -> String {
    let root = root.display().to_string();
    let id = slugify(name);
    let now = humantime::format_rfc3339(SystemTime::now()).to_string();
    format!(
        r##"---
daemon8_schema: {schema}
created_at: {created_at}
updated_at: {updated_at}
project:
  name: {name}
  id: {id}
  stack:
    languages: {languages}
    frameworks: {frameworks}
    tools: {tools}
vars:
  PRJ_ROOT: {root}
sources: []
---
# daemon8 project config

daemon8 and LLM sessions read the YAML frontmatter above. Keep runtime behavior in frontmatter.
"##,
        schema = PROJECT_CONFIG_SCHEMA,
        created_at = yaml_string(&now),
        updated_at = yaml_string(&now),
        name = yaml_string(name),
        id = yaml_string(&id),
        root = yaml_string(&root),
        languages = yaml_array(&stack.languages),
        frameworks = yaml_array(&stack.frameworks),
        tools = yaml_array(&stack.tools),
    )
}

fn canonical_project_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    path.canonicalize()
        .map_err(|err| format!("failed to canonicalize {}: {err}", path.display()))
}

fn merge_data(mut base: Value, extra: Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

fn yaml_array(values: &[&str]) -> String {
    serde_json::to_string(values).expect("JSON array serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_config::parse_project_config_str;

    fn mark_project(root: &Path) {
        std::fs::create_dir(root.join(".git")).unwrap();
    }

    #[test]
    fn template_includes_project_name_and_schema() {
        let root = Path::new("/tmp/my-proj");
        let stack = DetectedStack {
            languages: vec!["generic"],
            frameworks: Vec::new(),
            tools: Vec::new(),
        };
        let out = render_project_config("my-proj", root, &stack);
        assert!(out.contains("daemon8_schema: 1"));
        assert!(out.contains(r#"name: "my-proj""#));
        assert!(out.contains(r#"PRJ_ROOT: "/tmp/my-proj""#));
        assert!(!out.contains("root:"));
    }

    #[test]
    fn template_uses_empty_alpha_sources_array() {
        let root = Path::new("/tmp/my-app");
        let stack = DetectedStack {
            languages: vec!["rust"],
            frameworks: Vec::new(),
            tools: vec!["cargo"],
        };
        let out = render_project_config("my-app", root, &stack);
        assert!(out.contains("sources: []"));
        assert!(!out.contains("kind: sqlite"));
        assert!(!out.contains("kind: log"));
        assert!(!out.contains("type ="));
    }

    #[test]
    fn detect_stack_defaults_to_generic() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            detect_stack(tmp.path()),
            DetectedStack {
                languages: vec!["generic"],
                frameworks: Vec::new(),
                tools: Vec::new(),
            }
        );
    }

    #[test]
    fn detect_stack_identifies_rust_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert_eq!(
            detect_stack(tmp.path()),
            DetectedStack {
                languages: vec!["rust"],
                frameworks: Vec::new(),
                tools: vec!["cargo"],
            }
        );
    }

    #[test]
    fn derive_name_uses_basename() {
        let path = PathBuf::from("/tmp/foo/bar");
        assert_eq!(derive_name(&path), "bar");
    }

    #[test]
    fn init_writes_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("alpha".into()),
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        assert_eq!(outcome.envelope.next_actions[0].tool, "daemon8_connect");

        let config_path = tmp.path().join(".daemon8").join("config.md");
        let contents = std::fs::read_to_string(config_path).unwrap();
        let parsed = parse_project_config_str(&contents).unwrap();
        assert_eq!(parsed.project.name, "alpha");
        assert!(parsed.sources.is_empty());
    }

    #[test]
    fn init_refuses_existing_config_without_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".daemon8").join("config.md");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        std::fs::write(&config_path, "# existing\n").unwrap();

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: None,
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(
            std::fs::read_to_string(config_path).unwrap(),
            "# existing\n"
        );
    }

    #[test]
    fn init_overwrites_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".daemon8").join("config.md");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        std::fs::write(&config_path, "# existing\n").unwrap();

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("replacement".into()),
            overwrite: true,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);

        let contents = std::fs::read_to_string(config_path).unwrap();
        let parsed = parse_project_config_str(&contents).unwrap();
        assert_eq!(parsed.project.name, "replacement");
    }

    #[test]
    fn init_refuses_general_scope_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("alpha".into()),
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "general_scope");
        assert!(!tmp.path().join(".daemon8").exists());
    }

    #[test]
    fn init_rejects_empty_project_name_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("   ".into()),
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Error);
        assert_eq!(outcome.envelope.code, "invalid_project_name");
        assert!(!tmp.path().join(".daemon8").exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_rejects_symlinked_config_directory() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        let outside = tmp.path().join("outside-config");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join(".daemon8")).unwrap();

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("alpha".into()),
            overwrite: false,
        });

        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "unsafe_config_dir");
        assert!(!outside.join("config.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn init_rejects_symlinked_config_file_on_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        let config_dir = tmp.path().join(".daemon8");
        std::fs::create_dir(&config_dir).unwrap();
        let outside = tmp.path().join("outside-config.md");
        std::fs::write(&outside, "do not replace").unwrap();
        std::os::unix::fs::symlink(&outside, config_dir.join("config.md")).unwrap();

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("alpha".into()),
            overwrite: true,
        });

        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "unsafe_config_file");
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "do not replace");
    }
}
