// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8-cli.toml` at cwd and optionally
//! bootstrap provider configs / hook settings.

use std::env;
use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cli_config::PROJECT_CONFIG_FILENAME;
use crate::provider::{
    HookScope, Provider, ProviderWriteSummary, dirs_home, install_claude_hooks,
    install_codex_hooks, is_non_interactive, parse_provider_list, summarize_restarts,
    write_provider_config,
};

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum HookInstallScope {
    Local,
    Shared,
    Global,
}

impl From<HookInstallScope> for HookScope {
    fn from(value: HookInstallScope) -> Self {
        match value {
            HookInstallScope::Local => HookScope::Local,
            HookInstallScope::Shared => HookScope::Shared,
            HookInstallScope::Global => HookScope::Global,
        }
    }
}

#[derive(clap::Args, Default)]
pub struct InitArgs {
    /// Preset role for the agent declared by this config: queen | worker | solo | watchdog
    #[arg(long, default_value = "solo")]
    pub role: String,

    /// Overwrite an existing `.daemon8-cli.toml` at this location
    #[arg(long)]
    pub force: bool,

    /// Explicit project slug. Defaults to the cwd basename
    #[arg(long)]
    pub slug: Option<String>,

    /// Accept defaults without prompting. Auto-enabled when stdin is not a TTY
    /// or when the `CI` env var is set.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Comma-separated providers to configure alongside project bootstrap.
    /// Example: `claude-code,codex-cli`.
    #[arg(long)]
    pub providers: Option<String>,

    /// Register Claude hooks at the given scope.
    #[arg(long, value_enum)]
    pub install_hooks: Option<HookInstallScope>,

    /// Replace an existing daemon8 hook entry without prompting.
    #[arg(long)]
    pub force_hooks: bool,
}

pub fn cmd_init(args: InitArgs) -> Result<()> {
    let cwd = env::current_dir().context("cannot read current working directory")?;
    let target = cwd.join(PROJECT_CONFIG_FILENAME);

    if target.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite",
            target.display()
        );
    }

    let non_interactive = args.yes || is_non_interactive() || !std::io::stdin().is_terminal();
    let role = resolve_role(&args, non_interactive)?;
    let slug = args.slug.clone().unwrap_or_else(|| derive_slug(&cwd));
    let contents = render_template(&slug, &role);

    std::fs::write(&target, contents)
        .with_context(|| format!("failed to write {}", target.display()))?;

    let mut summary = ProviderWriteSummary::default();

    let home = dirs_home();
    for provider in resolve_providers(&args, non_interactive)? {
        let config_path = provider.config_path(&dirs_home());
        write_provider_config(provider, &config_path, Some(&cwd))?;
        summary.provider_files.push(config_path);
        summary.note_restart(provider);

        if provider == Provider::Codex {
            let hook_path = install_codex_hooks(&home, args.force_hooks)?;
            summary.hook_files.push(hook_path);
        }
    }

    if let Some(scope) = resolve_hook_scope(&args, non_interactive)? {
        let path = install_claude_hooks(scope, &cwd, &home, args.force_hooks)?;
        summary.hook_files.push(path);
        summary.note_restart(Provider::ClaudeCode);
    }

    println!("wrote {}", target.display());
    println!("role: {role}");
    println!("slug: {slug}");
    if !summary.provider_files.is_empty() {
        println!();
        println!("provider configs:");
        for path in &summary.provider_files {
            println!("  {}", path.display());
        }
    }
    if !summary.hook_files.is_empty() {
        println!();
        println!("hook settings:");
        for path in &summary.hook_files {
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

fn resolve_role(args: &InitArgs, non_interactive: bool) -> Result<String> {
    if non_interactive {
        return validate_role(&args.role);
    }

    let role = cliclack::select("Choose the default role for this project")
        .initial_value(args.role.clone())
        .item("queen".to_string(), "queen", "orchestrator / synthesis")
        .item(
            "worker".to_string(),
            "worker",
            "specialized implementation agent",
        )
        .item("solo".to_string(), "solo", "single-agent default")
        .item("watchdog".to_string(), "watchdog", "read-only observer")
        .interact()?;
    validate_role(&role)
}

fn resolve_providers(args: &InitArgs, non_interactive: bool) -> Result<Vec<Provider>> {
    if let Some(raw) = args.providers.as_deref() {
        return parse_provider_list(raw);
    }
    if non_interactive {
        return Ok(Vec::new());
    }

    Ok(cliclack::multiselect("Select provider configs to write")
        .required(false)
        .item(Provider::ClaudeCode, "Claude Code", "MCP config")
        .item(Provider::Cursor, "Cursor", "MCP config")
        .item(Provider::Windsurf, "Windsurf", "MCP config")
        .item(Provider::Gemini, "Gemini", "MCP config")
        .item(Provider::Codex, "Codex", "MCP config + trust project")
        .interact()?)
}

fn resolve_hook_scope(args: &InitArgs, non_interactive: bool) -> Result<Option<HookScope>> {
    if let Some(scope) = args.install_hooks.clone() {
        return Ok(Some(scope.into()));
    }
    if non_interactive {
        return Ok(None);
    }

    let should_install = cliclack::confirm("Install Claude hooks for this project?")
        .initial_value(false)
        .interact()?;
    if !should_install {
        return Ok(None);
    }

    let scope = cliclack::select("Choose the Claude hook settings target")
        .item(HookScope::Local, "local", ".claude/settings.local.json")
        .item(HookScope::Shared, "shared", ".claude/settings.json")
        .item(HookScope::Global, "global", "~/.claude/settings.json")
        .interact()?;
    Ok(Some(scope))
}

fn validate_role(raw: &str) -> Result<String> {
    match raw {
        "queen" | "worker" | "solo" | "watchdog" => Ok(raw.to_string()),
        other => bail!("invalid --role '{other}'; expected one of: queen, worker, solo, watchdog"),
    }
}

fn derive_slug(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

fn render_template(slug: &str, role: &str) -> String {
    format!(
        r##"# Daemon8 CLI hook configuration.
# Schema reference: https://daemon8.ai/docs/cli-hook-config

[project]
slug = "{slug}"
role_default = "{role}"

[enrollment]
enabled = true
scope = []

[features]
intents = true
inbox = true
state_tracking = true
compaction_recovery = true
heartbeat_interval = "30s"

[intents.auto_declare]
expertise = []

[distillation]
track_todowrite = true
track_git_commits = true
track_file_writes = true
coarsen_file_writes_below_threshold = 5
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_includes_slug_and_role() {
        let out = render_template("my-proj", "worker");
        assert!(out.contains(r#"slug = "my-proj""#));
        assert!(out.contains(r#"role_default = "worker""#));
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
