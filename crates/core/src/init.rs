// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::control::{
    AlphaEnvelope, AlphaStatus, NextAction, ScopeCandidate, ScopeMode, classify_scope,
};
use crate::project_config::{
    PROJECT_CONFIG_SCHEMA, parse_project_config_str, slugify, split_project_config,
};

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
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigBodyStatus {
    GeneratedSetupInstructionsPresent,
    ProjectNotes,
}

impl ConfigBodyStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GeneratedSetupInstructionsPresent => "generated_setup_instructions_present",
            Self::ProjectNotes => "project_notes",
        }
    }

    pub fn action(self) -> Option<&'static str> {
        match self {
            Self::GeneratedSetupInstructionsPresent => Some("replace_with_project_notes"),
            Self::ProjectNotes => None,
        }
    }
}

pub fn config_body_status(input: &str) -> ConfigBodyStatus {
    let Ok((_, body)) = split_project_config(input) else {
        return ConfigBodyStatus::ProjectNotes;
    };
    if contains_generated_setup_instructions(body) {
        ConfigBodyStatus::GeneratedSetupInstructionsPresent
    } else {
        ConfigBodyStatus::ProjectNotes
    }
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
    let ecosystems = crate::detect::detect_ecosystems(&scope_root);
    let stack = if ecosystems.is_empty() {
        default_stack()
    } else {
        crate::detect::ecosystems_to_stack(&ecosystems)
    };
    let sources = crate::detect::ecosystems_to_sources(&ecosystems);
    let contents = render_project_config(&name, &scope_root, &stack, &sources);
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
    let requirement = if config.sources.is_empty() {
        "REQUIRED: open .daemon8/config.md and complete ALL steps in the markdown body before proceeding. The config is not usable until sources are populated."
    } else {
        "REQUIRED: open .daemon8/config.md, verify the generated frontmatter, confirm the source list with the user, and replace the generated markdown body with concise project-specific notes before proceeding."
    };

    InitOutcome {
        envelope: AlphaEnvelope::success("initialized", "project config written", data)
            .with_next_action(NextAction::new(
                "daemon8_connect",
                "connect this MCP session to the initialized project",
                json!({"project_path": scope_root.display().to_string()}),
            ))
            .with_requirement(requirement),
        config_path: Some(config_path),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveRequest {
    pub project_path: PathBuf,
}

pub fn remove_project_config(request: RemoveRequest, confirmed: bool) -> InitOutcome {
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
                    "requested_path": requested_path,
                })),
                config_path: None,
            };
        }
    };

    let config_dir = canonical.join(PROJECT_CONFIG_DIR);
    let config_path = config_dir.join(PROJECT_CONFIG_FILENAME);
    let common_data = json!({
        "requested_path": requested_path,
        "scope_root": canonical.display().to_string(),
    });

    if !config_dir.exists() {
        return InitOutcome {
            envelope: AlphaEnvelope::success(
                "already_removed",
                "no .daemon8/ directory",
                common_data,
            ),
            config_path: None,
        };
    }

    match std::fs::symlink_metadata(&config_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "unsafe_config_dir",
                    "project config directory is a symlink",
                    ".daemon8 must be a real project-local directory before removal",
                )
                .with_data(common_data),
                config_path: Some(config_path),
            };
        }
        Ok(_) => {}
        Err(err) => {
            return InitOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Error,
                    "remove_failed",
                    "failed to inspect .daemon8",
                    format!("{}: {err}", config_dir.display()),
                )
                .with_data(common_data),
                config_path: None,
            };
        }
    }

    if !config_path.exists() {
        return InitOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::Blocked,
                "not_initialized",
                ".daemon8/ exists but has no config.md",
                "manual cleanup required -- daemon8 will not delete a .daemon8 directory it did not create",
            )
            .with_data(common_data),
            config_path: None,
        };
    }

    if !confirmed {
        return InitOutcome {
            envelope: AlphaEnvelope::success(
                "remove_pending",
                "ready to remove .daemon8/",
                common_data,
            ),
            config_path: Some(config_path),
        };
    }

    if let Err(err) = std::fs::remove_dir_all(&config_dir) {
        return InitOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::Error,
                "remove_failed",
                "failed to delete .daemon8/",
                format!("{}: {err}", config_dir.display()),
            )
            .with_data(common_data),
            config_path: None,
        };
    }

    InitOutcome {
        envelope: AlphaEnvelope::success("removed", ".daemon8/ deleted", common_data),
        config_path: None,
    }
}

pub fn derive_name(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string()
}

pub fn default_stack() -> DetectedStack {
    DetectedStack {
        languages: vec!["generic".into()],
        frameworks: Vec::new(),
        tools: Vec::new(),
    }
}

pub fn detect_stack(cwd: &Path) -> DetectedStack {
    let ecosystems = crate::detect::detect_ecosystems(cwd);
    if ecosystems.is_empty() {
        default_stack()
    } else {
        crate::detect::ecosystems_to_stack(&ecosystems)
    }
}

pub fn render_project_config(
    name: &str,
    root: &Path,
    stack: &DetectedStack,
    sources: &[crate::detect::SourceSuggestion],
) -> String {
    let root = root.display().to_string();
    let id = slugify(name);
    let now = humantime::format_rfc3339(SystemTime::now()).to_string();
    let sources_yaml = render_sources_yaml(sources);
    let body = if sources.is_empty() {
        CONFIG_BODY_EMPTY_SOURCES
    } else {
        CONFIG_BODY_WITH_SOURCES
    };
    format!(
        "---\ndaemon8_schema: {schema}\ncreated_at: {created_at}\nupdated_at: {updated_at}\nproject:\n  name: {name}\n  id: {id}\n  stack:\n    languages: {languages}\n    frameworks: {frameworks}\n    tools: {tools}\nvars:\n  PRJ_ROOT: {root}\n{sources_yaml}\n---\n{body}",
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

fn render_sources_yaml(sources: &[crate::detect::SourceSuggestion]) -> String {
    if sources.is_empty() {
        return "sources: []".to_string();
    }
    let mut yaml = String::from("sources:");
    for src in sources {
        yaml.push_str(&format!(
            "\n  - id: {}\n    service: {}\n    kind: file\n    path: \"$PRJ_ROOT/{}\"\n    parser: {}",
            src.id, src.service, src.path, src.parser,
        ));
    }
    yaml
}

fn contains_generated_setup_instructions(body: &str) -> bool {
    generated_body_with_sources_present(body) || generated_body_empty_sources_present(body)
}

fn generated_body_with_sources_present(body: &str) -> bool {
    body.contains("daemon8 detected ecosystem markers and pre-populated")
        && body.contains("Complete the steps below to finalize the config.")
        && body.contains("Present the updated config to the user")
}

fn generated_body_empty_sources_present(body: &str) -> bool {
    body.contains("REQUIRED: This config is incomplete. daemon8 cannot observe this project until")
        && body.contains("You MUST build a complete picture of this project before adding sources.")
        && body.contains("After completing Steps 1 and 2, present the updated config to the user")
}

const CONFIG_BODY_WITH_SOURCES: &str = r##"# daemon8 project config

daemon8 detected ecosystem markers and pre-populated the stack and sources
sections above. Complete the steps below to finalize the config.

## STEP 1: Verify auto-detected stack and sources

Review the frontmatter for accuracy:

1. Verify the languages, frameworks, and tools lists are complete.
   The auto-detected values are based on ecosystem markers -- add anything
   daemon8 missed.

2. Verify each source path is correct for your environment. The paths are
   based on framework conventions and may not match your project's layout.

3. Search for additional log files, build outputs, and error streams the
   project produces that are NOT listed above:
   - Application logs (storage/logs/, logs/, log/, *.log, stdout/stderr)
   - Web server access and error logs (nginx, Apache, Caddy)
   - Database query logs (slow query logs, general logs)
   - Queue and worker logs (Redis, RabbitMQ, SQS worker output)
   - Container logs (docker-compose log paths, k8s pod logs)

daemon8 supports these log parsers:

  line     -- plain text, one observation per line (catch-all)
  json     -- structured JSON logs (one JSON object per line)
  syslog   -- RFC 3164/5424 syslog format
  logfmt   -- key=value structured logs (Heroku, Go stdlib)
  clf      -- Common/Combined Log Format (Apache, nginx access logs)
  monolog  -- PHP Monolog format (Laravel, Symfony)
  auto     -- tries all parsers, picks the best match
  grok     -- custom pattern (set parser_pattern field)

Source fields:
  file: id, service, kind (file), path, parser (see list above), tags

# Conversation transcripts are discovered automatically on connect and available via build_context_snapshot.

Use $PRJ_ROOT for project-relative paths.

## STEP 2: Confirm with the user

Present the updated config to the user and ask: "Does this config look
thorough? Are there other log files, build outputs, or services I should
add?" Do NOT proceed without user confirmation.
"##;

const CONFIG_BODY_EMPTY_SOURCES: &str = r##"# daemon8 project config

REQUIRED: This config is incomplete. daemon8 cannot observe this project until
the steps below are finished. Do NOT skip or defer these steps.

## STEP 1: Full project audit

You MUST build a complete picture of this project before adding sources.
Do all of the following IN ORDER:

1. Read the package manager files to identify ALL dependencies:
   - package.json, Cargo.toml, composer.json, pyproject.toml, go.mod, Gemfile
   - Lock files (package-lock.json, yarn.lock, Cargo.lock, composer.lock)

2. Scan the project structure for a full understanding of scope:

       tree -L 3 -I 'node_modules|vendor|target|.git|dist|build|__pycache__' .

   On Windows: tree /F (then filter manually)

3. Review containerization and infrastructure:
   - Dockerfile, docker-compose.yml, .dockerignore
   - Kubernetes manifests, terraform files, serverless configs

4. Review deployment and CI/CD:
   - .github/workflows/, .gitlab-ci.yml, Jenkinsfile
   - Deploy scripts, Procfile, Caddyfile, nginx configs

5. Review runtime configuration:
   - .env.example, config files, environment-specific settings
   - Database configs, queue configs, cache configs

After this audit, update the stack section above with ALL languages,
frameworks, and tools found across the ENTIRE project. The auto-detected
values are a starting point -- they are NOT complete.

## STEP 2: Add sources

Using what you learned in Step 1, add file source entries
to the sources array in the frontmatter above. You MUST investigate every
log path, build output, and error stream the project produces. Do NOT stop
at the obvious ones -- dig through config files, docker entrypoints, and
supervisor configs to find ALL log outputs.

daemon8 supports these log parsers. ANY log file that matches one of these
formats MUST be added as a source:

  line     -- plain text, one observation per line (catch-all)
  json     -- structured JSON logs (one JSON object per line)
  syslog   -- RFC 3164/5424 syslog format
  logfmt   -- key=value structured logs (Heroku, Go stdlib)
  clf      -- Common/Combined Log Format (Apache, nginx access logs)
  monolog  -- PHP Monolog format (Laravel, Symfony)
  auto     -- tries all parsers, picks the best match
  grok     -- custom pattern (set parser_pattern field)

Search for ALL of these across the entire project:
- Application logs (storage/logs/, logs/, log/, *.log, stdout/stderr)
- Build output and compilation logs
- Web server access and error logs (nginx, Apache, Caddy)
- Database query logs (slow query logs, general logs)
- Queue and worker logs (Redis, RabbitMQ, SQS worker output)
- Error tracking output (crash logs, exception dumps)
- Container logs (docker-compose log paths, k8s pod logs)

File source format:

    - id: app.logs
      service: app
      kind: file
      path: "$PRJ_ROOT/logs/app.log"
      parser: line

Source fields:
  file: id, service, kind (file), path, parser (see list above), tags

# Conversation transcripts are discovered automatically on connect and available via build_context_snapshot.

Use $PRJ_ROOT for project-relative paths. It resolves to the vars.PRJ_ROOT
value in the frontmatter.

## STEP 3: Confirm with the user

After completing Steps 1 and 2, present the updated config to the user and
ask: "Does this config look thorough? Are there other log files, build
outputs, or services I should add?" Do NOT proceed without user confirmation.

## STEP 4: Delete this body

After the user confirms, DELETE EVERYTHING below the frontmatter `---` and
replace it with concise project-specific notes: dev commands, service
startup, build outputs, environment assumptions, gotchas. These setup
instructions are one-time scaffolding and must not remain in the file.
"##;

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
    if let (Some(base_map), Value::Object(extra_map)) = (base.as_object_mut(), extra) {
        for (key, value) in extra_map {
            base_map.insert(key, value);
        }
    }
    base
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

fn yaml_array(values: &[String]) -> String {
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
            languages: vec!["generic".into()],
            frameworks: Vec::new(),
            tools: Vec::new(),
        };
        let out = render_project_config("my-proj", root, &stack, &[]);
        assert!(out.contains("daemon8_schema: 1"));
        assert!(out.contains(r#"name: "my-proj""#));
        assert!(out.contains(r#"PRJ_ROOT: "/tmp/my-proj""#));
        assert!(!out.contains("root:"));
    }

    #[test]
    fn template_uses_empty_alpha_sources_array() {
        let root = Path::new("/tmp/my-app");
        let stack = DetectedStack {
            languages: vec!["rust".into()],
            frameworks: Vec::new(),
            tools: vec!["cargo".into()],
        };
        let out = render_project_config("my-app", root, &stack, &[]);
        assert!(out.contains("sources: []"));
        assert!(!out.contains("kind: sqlite"));
        assert!(!out.contains("kind: log"));
        assert!(!out.contains("type ="));
    }

    #[test]
    fn generated_empty_source_body_is_detected() {
        let root = Path::new("/tmp/my-app");
        let stack = DetectedStack {
            languages: vec!["rust".into()],
            frameworks: Vec::new(),
            tools: vec!["cargo".into()],
        };
        let out = render_project_config("my-app", root, &stack, &[]);

        assert_eq!(
            config_body_status(&out),
            ConfigBodyStatus::GeneratedSetupInstructionsPresent
        );
    }

    #[test]
    fn generated_auto_source_body_is_detected() {
        let root = Path::new("/tmp/my-app");
        let stack = DetectedStack {
            languages: vec!["php".into()],
            frameworks: vec!["laravel".into()],
            tools: vec!["composer".into()],
        };
        let sources = vec![crate::detect::SourceSuggestion {
            id: "laravel.logs".into(),
            service: "laravel".into(),
            path: "storage/logs/laravel.log".into(),
            parser: "monolog".into(),
        }];
        let out = render_project_config("my-app", root, &stack, &sources);

        assert_eq!(
            config_body_status(&out),
            ConfigBodyStatus::GeneratedSetupInstructionsPresent
        );
    }

    #[test]
    fn custom_body_is_project_notes() {
        let input = r#"---
daemon8_schema: 1
---
# daemon8
"#;

        assert_eq!(config_body_status(input), ConfigBodyStatus::ProjectNotes);
    }

    #[test]
    fn generated_body_with_appended_notes_still_needs_replacement() {
        let root = Path::new("/tmp/my-app");
        let stack = DetectedStack {
            languages: vec!["rust".into()],
            frameworks: Vec::new(),
            tools: vec!["cargo".into()],
        };
        let mut out = render_project_config("my-app", root, &stack, &[]);
        out.push_str("\n## Local Notes\nRun cargo test before release.\n");

        assert_eq!(
            config_body_status(&out),
            ConfigBodyStatus::GeneratedSetupInstructionsPresent
        );
    }

    #[test]
    fn detect_stack_defaults_to_generic() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_stack(tmp.path()), default_stack());
    }

    #[test]
    fn detect_stack_identifies_rust_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let stack = detect_stack(tmp.path());
        assert!(stack.languages.contains(&"rust".to_string()));
        assert!(stack.tools.contains(&"cargo".to_string()));
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

    #[test]
    fn remove_deletes_daemon8_directory() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("alpha".into()),
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        assert!(tmp.path().join(".daemon8").exists());

        let outcome = remove_project_config(
            RemoveRequest {
                project_path: tmp.path().to_path_buf(),
            },
            true,
        );
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        assert_eq!(outcome.envelope.code, "removed");
        assert!(!tmp.path().join(".daemon8").exists());
    }

    #[test]
    fn remove_idempotent_when_no_directory() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());

        let outcome = remove_project_config(
            RemoveRequest {
                project_path: tmp.path().to_path_buf(),
            },
            true,
        );
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        assert_eq!(outcome.envelope.code, "already_removed");
    }

    #[cfg(unix)]
    #[test]
    fn remove_blocks_on_symlinked_directory() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        let outside = tmp.path().join("outside-daemon8");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("config.md"), "# fake\n").unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join(".daemon8")).unwrap();

        let outcome = remove_project_config(
            RemoveRequest {
                project_path: tmp.path().to_path_buf(),
            },
            true,
        );
        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "unsafe_config_dir");
        assert!(outside.exists());
    }

    #[test]
    fn remove_blocks_when_config_missing() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        std::fs::create_dir(tmp.path().join(".daemon8")).unwrap();

        let outcome = remove_project_config(
            RemoveRequest {
                project_path: tmp.path().to_path_buf(),
            },
            true,
        );
        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "not_initialized");
        assert!(tmp.path().join(".daemon8").exists());
    }

    #[test]
    fn remove_pending_does_not_delete() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());
        init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("alpha".into()),
            overwrite: false,
        });

        let outcome = remove_project_config(
            RemoveRequest {
                project_path: tmp.path().to_path_buf(),
            },
            false,
        );
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        assert_eq!(outcome.envelope.code, "remove_pending");
        assert!(tmp.path().join(".daemon8").join("config.md").exists());
    }

    #[test]
    fn render_config_with_auto_sources() {
        let root = Path::new("/tmp/test-proj");
        let stack = DetectedStack {
            languages: vec!["php".into()],
            frameworks: vec!["laravel".into()],
            tools: vec!["composer".into()],
        };
        let sources = vec![crate::detect::SourceSuggestion {
            id: "laravel.logs".into(),
            service: "laravel".into(),
            path: "storage/logs/laravel.log".into(),
            parser: "monolog".into(),
        }];
        let out = render_project_config("test-proj", root, &stack, &sources);
        assert!(out.contains("laravel.logs"));
        assert!(out.contains("$PRJ_ROOT/storage/logs/laravel.log"));
        assert!(out.contains("parser: monolog"));
        assert!(out.contains("kind: file"));
        assert!(!out.contains("sources: []"));
    }

    #[test]
    fn rendered_config_with_sources_parses() {
        let root = Path::new("/tmp/test-proj");
        let stack = DetectedStack {
            languages: vec!["php".into()],
            frameworks: vec!["laravel".into()],
            tools: vec!["composer".into()],
        };
        let sources = vec![crate::detect::SourceSuggestion {
            id: "laravel.logs".into(),
            service: "laravel".into(),
            path: "storage/logs/laravel.log".into(),
            parser: "monolog".into(),
        }];
        let out = render_project_config("test-proj", root, &stack, &sources);
        let config = parse_project_config_str(&out).unwrap();
        assert_eq!(config.sources.len(), 1);
    }

    #[test]
    fn init_auto_populates_sources_for_laravel() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
        std::fs::write(tmp.path().join("composer.json"), "{}").unwrap();
        mark_project(tmp.path());

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("laravel-app".into()),
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);

        let config_path = tmp.path().join(".daemon8").join("config.md");
        let contents = std::fs::read_to_string(config_path).unwrap();
        let config = parse_project_config_str(&contents).unwrap();
        assert!(
            !config.sources.is_empty(),
            "laravel project should have auto-detected sources"
        );
    }

    #[test]
    fn init_empty_sources_when_no_detection() {
        let tmp = tempfile::tempdir().unwrap();
        mark_project(tmp.path());

        let outcome = init_project(InitRequest {
            project_path: tmp.path().to_path_buf(),
            name: Some("empty-proj".into()),
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);

        let config_path = tmp.path().join(".daemon8").join("config.md");
        let contents = std::fs::read_to_string(config_path).unwrap();
        assert!(contents.contains("sources: []"));
    }
}
