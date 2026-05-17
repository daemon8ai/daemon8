// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROJECT_CONFIG_SCHEMA: u32 = 1;

#[derive(Debug, Error)]
pub enum ProjectConfigError {
    #[error("missing YAML frontmatter delimited by ---")]
    MissingFrontmatter,
    #[error("unterminated YAML frontmatter")]
    UnterminatedFrontmatter,
    #[error("invalid YAML frontmatter: {0}")]
    InvalidYaml(String),
    #[error("failed to read project config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid project config: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ProjectConfigError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub daemon8_schema: u32,
    pub created_at: String,
    pub updated_at: String,
    pub project: ProjectInfo,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    #[serde(default)]
    pub sources: Vec<ProjectSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    pub name: String,
    pub stack: ProjectStack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectStack {
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ProjectSource {
    File(FileSource),
    Conversation(ConversationSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSource {
    pub id: String,
    pub service: String,
    pub path: String,
    #[serde(default = "default_line_parser")]
    pub parser: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser_pattern: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSource {
    pub id: String,
    pub service: String,
    pub path: String,
    pub provider: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub fn parse_project_config_str(input: &str) -> Result<ProjectConfig> {
    let frontmatter = extract_frontmatter(input)?;
    let config: ProjectConfig = serde_norway::from_str(frontmatter)
        .map_err(|err| ProjectConfigError::InvalidYaml(err.to_string()))?;
    validate_project_config(&config)?;
    Ok(config)
}

pub fn parse_project_config_file(path: &Path) -> Result<ProjectConfig> {
    let input = std::fs::read_to_string(path).map_err(|source| ProjectConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_project_config_str(&input)
}

pub fn validate_project_config(config: &ProjectConfig) -> Result<()> {
    if config.daemon8_schema != PROJECT_CONFIG_SCHEMA {
        return Err(ProjectConfigError::Invalid(format!(
            "daemon8_schema must be {PROJECT_CONFIG_SCHEMA}, got {}",
            config.daemon8_schema
        )));
    }

    require_non_empty("created_at", &config.created_at)?;
    require_non_empty("updated_at", &config.updated_at)?;
    require_non_empty("project.name", &config.project.name)?;
    validate_string_list("project.stack.languages", &config.project.stack.languages)?;
    validate_string_list("project.stack.frameworks", &config.project.stack.frameworks)?;
    validate_string_list("project.stack.tools", &config.project.stack.tools)?;

    for (name, value) in &config.vars {
        validate_var_name(name)?;
        validate_var_path(&format!("vars.{name}"), value)?;
    }

    let mut ids = BTreeSet::new();
    for source in &config.sources {
        let id = source.id();
        validate_source_id(id)?;
        if !ids.insert(id.to_string()) {
            return Err(ProjectConfigError::Invalid(format!(
                "duplicate source id '{id}'"
            )));
        }

        require_non_empty(&format!("sources.{id}.service"), source.service())?;
        validate_declared_path(&format!("sources.{id}.path"), source.path())?;
        validate_path_vars(&format!("sources.{id}.path"), source.path(), &config.vars)?;

        match source {
            ProjectSource::File(file) => {
                require_non_empty(&format!("sources.{}.parser", file.id), &file.parser)?;
                validate_string_list(&format!("sources.{}.tags", file.id), &file.tags)?;
            }
            ProjectSource::Conversation(conversation) => {
                require_non_empty(
                    &format!("sources.{}.provider", conversation.id),
                    &conversation.provider,
                )?;
                validate_string_list(
                    &format!("sources.{}.tags", conversation.id),
                    &conversation.tags,
                )?;
            }
        }
    }

    Ok(())
}

fn extract_frontmatter(input: &str) -> Result<&str> {
    let mut lines = input.lines();
    let Some(first) = lines.next() else {
        return Err(ProjectConfigError::MissingFrontmatter);
    };
    if first.trim() != "---" {
        return Err(ProjectConfigError::MissingFrontmatter);
    }

    let mut offset = first.len();
    if input.as_bytes().get(offset) == Some(&b'\r') {
        offset += 1;
    }
    if input.as_bytes().get(offset) == Some(&b'\n') {
        offset += 1;
    }
    let start = offset;

    for line in lines {
        let line_start = offset;
        offset += line.len();
        if input.as_bytes().get(offset) == Some(&b'\r') {
            offset += 1;
        }
        if input.as_bytes().get(offset) == Some(&b'\n') {
            offset += 1;
        }
        if line.trim() == "---" {
            return Ok(&input[start..line_start]);
        }
    }

    Err(ProjectConfigError::UnterminatedFrontmatter)
}

impl ProjectSource {
    pub fn id(&self) -> &str {
        match self {
            Self::File(source) => &source.id,
            Self::Conversation(source) => &source.id,
        }
    }

    pub fn service(&self) -> &str {
        match self {
            Self::File(source) => &source.service,
            Self::Conversation(source) => &source.service,
        }
    }

    pub fn path(&self) -> &str {
        match self {
            Self::File(source) => &source.path,
            Self::Conversation(source) => &source.path,
        }
    }
}

fn default_line_parser() -> String {
    "line".into()
}

fn require_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ProjectConfigError::Invalid(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn validate_string_list(field: &str, values: &[String]) -> Result<()> {
    for value in values {
        require_non_empty(field, value)?;
    }
    Ok(())
}

fn validate_source_id(id: &str) -> Result<()> {
    require_non_empty("source id", id)?;
    if !id.contains('.') {
        return Err(ProjectConfigError::Invalid(format!(
            "source id '{id}' must use dot-path form"
        )));
    }
    for segment in id.split('.') {
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ProjectConfigError::Invalid(format!(
                "source id '{id}' contains an invalid segment"
            )));
        }
    }
    Ok(())
}

fn validate_var_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Err(ProjectConfigError::Invalid(
            "variable names must not be empty".into(),
        ));
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return Err(ProjectConfigError::Invalid(format!(
            "variable '{name}' must start with a letter or underscore"
        )));
    }
    if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(ProjectConfigError::Invalid(format!(
            "variable '{name}' contains invalid characters"
        )));
    }
    Ok(())
}

fn validate_declared_path(field: &str, value: &str) -> Result<()> {
    require_non_empty(field, value)?;
    if value.starts_with("~/") || value == "~" {
        return Err(ProjectConfigError::Invalid(format!(
            "{field} must not use ~/ shorthand"
        )));
    }
    Ok(())
}

fn validate_var_path(field: &str, value: &str) -> Result<()> {
    validate_declared_path(field, value)?;
    if value.contains('$') {
        return Err(ProjectConfigError::Invalid(format!(
            "{field} must not reference another variable"
        )));
    }
    if !is_absolute_path(value) {
        return Err(ProjectConfigError::Invalid(format!(
            "{field} must be absolute"
        )));
    }
    Ok(())
}

fn validate_path_vars(field: &str, value: &str, vars: &BTreeMap<String, String>) -> Result<()> {
    let references = collect_var_references(field, value)?;
    for name in &references {
        if !vars.contains_key(name) {
            return Err(ProjectConfigError::Invalid(format!(
                "{field} references undeclared variable ${name}"
            )));
        }
    }

    let expanded = expand_vars(value, vars, &references);
    if !is_absolute_path(&expanded) {
        return Err(ProjectConfigError::Invalid(format!(
            "{field} must be absolute or expand to an absolute path"
        )));
    }
    Ok(())
}

fn collect_var_references(field: &str, value: &str) -> Result<Vec<String>> {
    let bytes = value.as_bytes();
    let mut refs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }

        if bytes.get(i + 1) == Some(&b'{') {
            let start = i + 2;
            let Some(end) = bytes[start..].iter().position(|b| *b == b'}') else {
                return Err(ProjectConfigError::Invalid(format!(
                    "{field} contains an unterminated variable reference"
                )));
            };
            let name = &value[start..start + end];
            validate_var_name(name)?;
            refs.push(name.to_string());
            i = start + end + 1;
            continue;
        }

        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end == start {
            return Err(ProjectConfigError::Invalid(format!(
                "{field} contains an invalid variable reference"
            )));
        }
        let name = &value[start..end];
        validate_var_name(name)?;
        refs.push(name.to_string());
        i = end;
    }
    refs.sort();
    refs.dedup();
    Ok(refs)
}

fn expand_vars(value: &str, vars: &BTreeMap<String, String>, references: &[String]) -> String {
    let mut expanded = value.to_string();
    for name in references {
        if let Some(replacement) = vars.get(name) {
            expanded = expanded.replace(&format!("${{{name}}}"), replacement);
            expanded = expanded.replace(&format!("${name}"), replacement);
        }
    }
    expanded
}

fn is_absolute_path(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with("\\\\")
        || value.as_bytes().get(0..3).is_some_and(|bytes| {
            bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> String {
        r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: daemon8
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "/tmp/daemon8"
sources:
  - id: cargo.check
    service: cargo
    kind: file
    parser: line
    path: "$PRJ_ROOT/target/daemon8/cargo-check.log"
  - id: claude.conversations
    service: claude
    kind: conversation
    provider: claude
    path: "/tmp/claude/sessions"
---
# daemon8
"#
        .to_string()
    }

    fn parse(input: String) -> Result<ProjectConfig> {
        parse_project_config_str(&input)
    }

    #[test]
    fn parses_valid_config() {
        let config = parse(valid_config()).unwrap();
        assert_eq!(config.daemon8_schema, PROJECT_CONFIG_SCHEMA);
        assert_eq!(config.project.name, "daemon8");
        assert_eq!(config.sources.len(), 2);
    }

    #[test]
    fn parses_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.md");
        std::fs::write(&path, valid_config()).unwrap();

        let config = parse_project_config_file(&path).unwrap();
        assert_eq!(config.project.name, "daemon8");
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let err = parse_project_config_str("daemon8_schema: 1").unwrap_err();
        assert!(err.to_string().contains("missing YAML frontmatter"));
    }

    #[test]
    fn rejects_invalid_yaml() {
        let err = parse_project_config_str("---\n:\n---\n").unwrap_err();
        assert!(err.to_string().contains("invalid YAML"));
    }

    #[test]
    fn rejects_version_field() {
        let input = valid_config().replace("daemon8_schema: 1", "version: 1");
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_daemon8_version_field() {
        let input = valid_config().replace("daemon8_schema: 1", "daemon8_version: 1");
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_project_slug_and_root() {
        let input = valid_config().replace(
            "  name: daemon8\n  stack:",
            "  name: daemon8\n  slug: daemon8\n  root: /tmp/daemon8\n  stack:",
        );
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_empty_stack_values() {
        let input = valid_config().replace("languages: [rust]", r#"languages: [rust, ""]"#);
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("project.stack.languages"));
    }

    #[test]
    fn rejects_map_style_sources() {
        let input = valid_config().replace(
            r#"sources:
  - id: cargo.check
    service: cargo
    kind: file
    parser: line
    path: "$PRJ_ROOT/target/daemon8/cargo-check.log"
  - id: claude.conversations
    service: claude
    kind: conversation
    provider: claude
    path: "/tmp/claude/sessions""#,
            r#"sources:
  cargo.check:
    service: cargo
    kind: file
    path: "$PRJ_ROOT/target/daemon8/cargo-check.log""#,
        );
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("invalid YAML"));
    }

    #[test]
    fn rejects_source_type_field() {
        let input = valid_config().replace("    kind: file", "    type: file");
        let err = parse(input).unwrap_err();
        assert!(
            err.to_string().contains("unknown variant")
                || err.to_string().contains("missing field `kind`")
        );
    }

    #[test]
    fn rejects_non_alpha_source_kind() {
        let input = valid_config().replace("    kind: file", "    kind: sqlite");
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn rejects_raw_relative_paths() {
        let input = valid_config().replace(
            r#"path: "$PRJ_ROOT/target/daemon8/cargo-check.log""#,
            r#"path: "logs/app.log""#,
        );
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("must be absolute"));
    }

    #[test]
    fn rejects_home_shorthand() {
        let input = valid_config().replace(
            r#"path: "$PRJ_ROOT/target/daemon8/cargo-check.log""#,
            r#"path: "~/logs/app.log""#,
        );
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("~/ shorthand"));
    }

    #[test]
    fn rejects_undeclared_variables() {
        let input = valid_config().replace("$PRJ_ROOT/", "$NOPE/");
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("undeclared variable"));
    }

    #[test]
    fn rejects_relative_var_values() {
        let input = valid_config().replace(r#"PRJ_ROOT: "/tmp/daemon8""#, r#"PRJ_ROOT: "daemon8""#);
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("vars.PRJ_ROOT must be absolute"));
    }

    #[test]
    fn rejects_duplicate_source_ids() {
        let input = valid_config().replace("claude.conversations", "cargo.check");
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("duplicate source id"));
    }

    #[test]
    fn rejects_empty_source_tags() {
        let input = valid_config().replace(
            r#"path: "$PRJ_ROOT/target/daemon8/cargo-check.log""#,
            r#"path: "$PRJ_ROOT/target/daemon8/cargo-check.log"
    tags: ["build", ""]"#,
        );
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("sources.cargo.check.tags"));
    }

    #[test]
    fn defaults_file_parser_to_line() {
        let input = valid_config().replace("    parser: line\n", "");
        let config = parse(input).unwrap();
        let ProjectSource::File(file) = &config.sources[0] else {
            panic!("first source should be file");
        };
        assert_eq!(file.parser, "line");
    }

    #[test]
    fn requires_conversation_provider() {
        let input = valid_config().replace("    provider: claude\n", "");
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("missing field `provider`"));
    }
}
