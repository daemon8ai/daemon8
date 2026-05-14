// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8.toml` at cwd and optionally
//! bootstrap provider configs.

use std::env;
use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli_config::PROJECT_CONFIG_FILENAME;
use daemon8_providers::{
    Provider, ProviderWriteSummary, dirs_home, is_non_interactive, parse_provider_list,
    summarize_restarts, write_provider_config,
};

#[derive(clap::Args, Default)]
pub struct InitArgs {
    /// Overwrite an existing `.daemon8.toml` at this location
    #[arg(long)]
    pub force: bool,

    /// Explicit project slug. Defaults to the cwd basename
    #[arg(long)]
    pub slug: Option<String>,

    /// Accept defaults without prompting. Auto-enabled when stdin is not a TTY
    /// or when the `CI` env var is set.
    #[arg(short = 'y', long, visible_alias = "no-interaction")]
    pub yes: bool,

    /// Comma-separated providers to configure alongside project bootstrap.
    /// Example: `claude-code,codex-cli`.
    #[arg(long)]
    pub providers: Option<String>,
}

pub fn cmd_init(args: InitArgs) -> Result<()> {
    let cwd = env::current_dir().context("cannot read current working directory")?;
    let target = cwd.join(PROJECT_CONFIG_FILENAME);

    if target.exists() && !args.force {
        println!(
            "{} already exists. Use --force to overwrite.",
            target.display()
        );
        return Ok(());
    }

    let non_interactive = args.yes || is_non_interactive() || !std::io::stdin().is_terminal();
    let slug = args.slug.clone().unwrap_or_else(|| derive_slug(&cwd));
    let project_type = detect_project_type(&cwd);
    let contents = render_template(&slug, project_type);

    std::fs::write(&target, contents)
        .with_context(|| format!("failed to write {}", target.display()))?;

    let mut summary = ProviderWriteSummary::default();

    let port = crate::config::load(None).unwrap_or_default().server.port;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");

    for provider in resolve_providers(&args, non_interactive)? {
        let config_path = provider.config_path(&dirs_home());
        write_provider_config(
            provider,
            &config_path,
            &mcp_url,
            Some(&cwd),
            &crate::cli_config::SERVICE,
        )?;
        summary.provider_files.push(config_path);
        summary.note_restart(provider);
    }

    println!("wrote {}", target.display());
    println!("slug: {slug}");
    if !summary.provider_files.is_empty() {
        println!();
        println!("provider configs:");
        for path in &summary.provider_files {
            println!("  {}", path.display());
        }
    }
    let restart_messages = summarize_restarts(&summary);
    if !restart_messages.is_empty() {
        println!();
        println!("restart required:");
        for message in restart_messages {
            println!("  {message}");
        }
    }

    Ok(())
}

fn resolve_providers(args: &InitArgs, non_interactive: bool) -> Result<Vec<Provider>> {
    if let Some(raw) = args.providers.as_deref() {
        return parse_provider_list(raw);
    }
    if non_interactive {
        return Ok(Vec::new());
    }

    let items: Vec<(Provider, &str, &str)> = daemon8_providers::ALL_PROVIDERS
        .iter()
        .map(|&p| (p, p.label(), p.as_provider().init_hint()))
        .collect();

    Ok(cliclack::multiselect("Select provider configs to write")
        .required(false)
        .items(&items)
        .interact()?)
}

fn derive_slug(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectType {
    Laravel,
    Symfony,
    Node,
    Rust,
    Generic,
}

fn detect_project_type(cwd: &Path) -> ProjectType {
    if cwd.join("artisan").exists() {
        ProjectType::Laravel
    } else if cwd.join("bin/console").exists() {
        ProjectType::Symfony
    } else if cwd.join("package.json").exists() {
        ProjectType::Node
    } else if cwd.join("Cargo.toml").exists() {
        ProjectType::Rust
    } else {
        ProjectType::Generic
    }
}

fn sources_example(project_type: ProjectType) -> &'static str {
    match project_type {
        ProjectType::Laravel => {
            r#"
# [sources.app-logs]
# type = "file"
# path = "storage/logs/laravel.log"
# parser = "monolog"
# tags = ["app"]
"#
        }
        ProjectType::Symfony => {
            r#"
# [sources.app-logs]
# type = "file"
# path = "var/log/*.log"
# parser = "monolog"
# tags = ["app"]
"#
        }
        ProjectType::Node => {
            r#"
# [sources.app-logs]
# type = "file"
# path = "logs/app.log"
# parser = "json"
# tags = ["app"]
"#
        }
        ProjectType::Rust => {
            r#"
# [sources.app-logs]
# type = "file"
# path = "logs/app.log"
# parser = "line"
# tags = ["app"]
"#
        }
        ProjectType::Generic => {
            r#"
# [sources.app-logs]
# type = "file"
# path = "logs/app.log"
# parser = "line"
# tags = ["app"]
"#
        }
    }
}

fn render_template(slug: &str, project_type: ProjectType) -> String {
    let sources = sources_example(project_type);
    format!(
        r##"# Daemon8 project configuration.
# Runtime sources can be registered explicitly here or discovered with the librarian.

[project]
slug = "{slug}"
{sources}"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_includes_slug_without_role() {
        let out = render_template("my-proj", ProjectType::Generic);
        assert!(out.contains(r#"slug = "my-proj""#));
        assert!(!out.contains("role_default"));
    }

    #[test]
    fn template_laravel_sources_example() {
        let out = render_template("my-app", ProjectType::Laravel);
        assert!(out.contains("storage/logs/laravel.log"));
        assert!(out.contains(r#"# parser = "monolog""#));
    }

    #[test]
    fn template_symfony_sources_example() {
        let out = render_template("my-app", ProjectType::Symfony);
        assert!(out.contains("var/log/*.log"));
        assert!(out.contains(r#"# parser = "monolog""#));
    }

    #[test]
    fn template_node_sources_example() {
        let out = render_template("my-app", ProjectType::Node);
        assert!(out.contains("logs/app.log"));
        assert!(out.contains(r#"# parser = "json""#));
    }

    #[test]
    fn template_rust_sources_example() {
        let out = render_template("my-app", ProjectType::Rust);
        assert!(out.contains("logs/app.log"));
        assert!(out.contains(r#"# parser = "line""#));
    }

    #[test]
    fn detect_project_type_defaults_to_generic() {
        let tmp = std::env::temp_dir().join("daemon8-test-empty");
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(detect_project_type(&tmp), ProjectType::Generic);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn derive_slug_uses_basename() {
        let p = std::path::PathBuf::from("/tmp/foo/bar");
        assert_eq!(derive_slug(&p), "bar");
    }

    #[test]
    fn provider_list_parser_accepts_codex() {
        let providers = parse_provider_list("claude-code,codex-cli").unwrap();
        assert_eq!(providers, vec![Provider::ClaudeCode, Provider::Codex]);
    }
}
