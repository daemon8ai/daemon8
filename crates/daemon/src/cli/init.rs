// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8/config.md` at cwd.

use std::env;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli_config::{PROJECT_CONFIG_DIR, PROJECT_CONFIG_FILENAME};

#[derive(clap::Args, Default)]
pub struct InitArgs {
    /// Overwrite an existing `.daemon8/config.md` at this location
    #[arg(long)]
    pub force: bool,

    /// Explicit project slug. Defaults to the cwd basename
    #[arg(long)]
    pub slug: Option<String>,

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

    let slug = args.slug.clone().unwrap_or_else(|| derive_slug(&cwd));
    let stack = detect_stack(&cwd);
    let root = cwd
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", cwd.display()))?;
    let contents = render_template(&slug, &root, &stack);

    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    std::fs::write(&target, contents)
        .with_context(|| format!("failed to write {}", target.display()))?;

    println!("wrote {}", target.display());
    println!("slug: {slug}");

    Ok(())
}

fn derive_slug(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

fn detect_stack(cwd: &Path) -> Vec<&'static str> {
    let mut stack = Vec::new();
    if cwd.join("artisan").exists() {
        stack.push("php");
        stack.push("laravel");
    }
    if cwd.join("bin/console").exists() {
        stack.push("php");
        stack.push("symfony");
    }
    if cwd.join("package.json").exists() {
        stack.push("javascript");
        stack.push("node");
    }
    if cwd.join("Cargo.toml").exists() {
        stack.push("rust");
        stack.push("cargo");
    }
    if stack.is_empty() {
        stack.push("generic");
    }
    stack.sort_unstable();
    stack.dedup();
    stack
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).expect("JSON string serialization cannot fail")
}

fn render_stack(stack: &[&str]) -> String {
    stack
        .iter()
        .map(|item| format!("    - {}", yaml_string(item)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_template(slug: &str, root: &Path, stack: &[&str]) -> String {
    let root = root.display().to_string();
    let stack = render_stack(stack);
    format!(
        r##"---
version: 1
project:
  slug: {slug}
  root: {root}
  stack:
{stack}
vars:
  PRJ_ROOT: {root}
sources: {{}}
---
# daemon8 project config

This is the only project-local daemon8 file. The frontmatter is read by daemon8 and by the LLM in the session.

Source entries are intentionally explicit. Use `kind: file` for logs/files and `kind: conversation` for provider transcripts. Keep `parser` separate from `kind`.

```yaml
# sources:
#   app-logs:
#     kind: file
#     path: "$PRJ_ROOT/logs/app.log"
#     parser: line
#     tags: ["app"]
#
#   claude:
#     kind: conversation
#     provider: claude
#     parser: line
#     tags: ["conversation"]
```
"##,
        slug = yaml_string(slug),
        root = yaml_string(&root),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_includes_slug_without_role() {
        let root = std::path::Path::new("/tmp/my-proj");
        let out = render_template("my-proj", root, &["generic"]);
        assert!(out.contains(r#"slug: "my-proj""#));
        assert!(out.contains(r#"PRJ_ROOT: "/tmp/my-proj""#));
        assert!(!out.contains("role_default"));
    }

    #[test]
    fn template_uses_alpha_source_vocabulary() {
        let root = std::path::Path::new("/tmp/my-app");
        let out = render_template("my-app", root, &["cargo", "rust"]);
        assert!(out.contains("kind: file"));
        assert!(out.contains("kind: conversation"));
        assert!(out.contains("logs/app.log"));
        assert!(out.contains("parser: line"));
        assert!(!out.contains("type ="));
    }

    #[test]
    fn detect_stack_defaults_to_generic() {
        let tmp = std::env::temp_dir().join("daemon8-test-empty");
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(detect_stack(&tmp), vec!["generic"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_stack_identifies_rust_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert_eq!(detect_stack(tmp.path()), vec!["cargo", "rust"]);
    }

    #[test]
    fn derive_slug_uses_basename() {
        let p = std::path::PathBuf::from("/tmp/foo/bar");
        assert_eq!(derive_slug(&p), "bar");
    }
}
