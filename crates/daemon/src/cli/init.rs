// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8/config.md` at cwd.

use std::env;
use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use daemon8_core::project_config::PROJECT_CONFIG_SCHEMA;

use crate::cli_config::{PROJECT_CONFIG_DIR, PROJECT_CONFIG_FILENAME};

#[derive(clap::Args, Default)]
pub struct InitArgs {
    /// Overwrite an existing `.daemon8/config.md` at this location
    #[arg(long)]
    pub force: bool,

    /// Explicit project name. Defaults to the cwd basename
    #[arg(long)]
    pub name: Option<String>,

    /// Accept defaults without prompting.
    #[arg(short = 'y', long, visible_alias = "no-interaction")]
    pub yes: bool,
}

pub fn cmd_init(args: InitArgs) -> Result<()> {
    let cwd = env::current_dir().context("cannot read current working directory")?;
    let config_dir = cwd.join(PROJECT_CONFIG_DIR);
    let target = config_dir.join(PROJECT_CONFIG_FILENAME);

    if target.exists() && !args.force {
        println!(
            "{} already exists. Use --force to overwrite.",
            target.display()
        );
        return Ok(());
    }

    let name = args.name.clone().unwrap_or_else(|| derive_name(&cwd));
    let stack = detect_stack(&cwd);
    let root = cwd
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", cwd.display()))?;
    let contents = render_template(&name, &root, &stack);

    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    std::fs::write(&target, contents)
        .with_context(|| format!("failed to write {}", target.display()))?;

    println!("wrote {}", target.display());
    println!("name: {name}");

    Ok(())
}

fn derive_name(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedStack {
    languages: Vec<&'static str>,
    frameworks: Vec<&'static str>,
    tools: Vec<&'static str>,
}

fn detect_stack(cwd: &Path) -> DetectedStack {
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

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

fn yaml_array(values: &[&str]) -> String {
    serde_json::to_string(values).expect("JSON array serialization cannot fail")
}

fn render_template(name: &str, root: &Path, stack: &DetectedStack) -> String {
    let root = root.display().to_string();
    let now = humantime::format_rfc3339(SystemTime::now()).to_string();
    format!(
        r##"---
daemon8_schema: {schema}
created_at: {created_at}
updated_at: {updated_at}
project:
  name: {name}
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
        root = yaml_string(&root),
        languages = yaml_array(&stack.languages),
        frameworks = yaml_array(&stack.frameworks),
        tools = yaml_array(&stack.tools),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_includes_project_name_and_schema() {
        let root = std::path::Path::new("/tmp/my-proj");
        let stack = DetectedStack {
            languages: vec!["generic"],
            frameworks: Vec::new(),
            tools: Vec::new(),
        };
        let out = render_template("my-proj", root, &stack);
        assert!(out.contains("daemon8_schema: 1"));
        assert!(out.contains(r#"name: "my-proj""#));
        assert!(out.contains(r#"PRJ_ROOT: "/tmp/my-proj""#));
        assert!(!out.contains("root:"));
    }

    #[test]
    fn template_uses_empty_alpha_sources_array() {
        let root = std::path::Path::new("/tmp/my-app");
        let stack = DetectedStack {
            languages: vec!["rust"],
            frameworks: Vec::new(),
            tools: vec!["cargo"],
        };
        let out = render_template("my-app", root, &stack);
        assert!(out.contains("sources: []"));
        assert!(!out.contains("kind: sqlite"));
        assert!(!out.contains("kind: log"));
        assert!(!out.contains("type ="));
    }

    #[test]
    fn detect_stack_defaults_to_generic() {
        let tmp = std::env::temp_dir().join("daemon8-test-empty");
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(
            detect_stack(&tmp),
            DetectedStack {
                languages: vec!["generic"],
                frameworks: Vec::new(),
                tools: Vec::new(),
            }
        );
        let _ = std::fs::remove_dir_all(&tmp);
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
        let p = std::path::PathBuf::from("/tmp/foo/bar");
        assert_eq!(derive_name(&p), "bar");
    }
}
