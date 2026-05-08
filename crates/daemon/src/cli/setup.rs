// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use daemon8_mcp::SetupToolAction;
use serde::Serialize;

use crate::cli::init::HookInstallScope;
use crate::cli_config::{self, PROJECT_CONFIG_FILENAME};
use crate::config::{self, FileSourceConfig, ProjectSetupState, SourceConfig};
use crate::providers::{
    ProviderWriteSummary, dirs_home, install_claude_hooks, install_codex_hooks,
    parse_provider_list, summarize_restarts, write_provider_config,
};

#[derive(Args, Default)]
pub struct SetupArgs {
    #[command(subcommand)]
    pub command: Option<SetupCommand>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum SetupCommand {
    /// Show current setup state without writing files.
    Status,
    /// Show proposed setup changes without writing files.
    Plan,
    /// Apply setup changes explicitly.
    Apply(SetupApplyArgs),
}

#[derive(Args, Default, Clone)]
pub struct SetupApplyArgs {
    /// Confirm noninteractive setup changes.
    #[arg(short = 'y', long, visible_alias = "no-interaction")]
    pub yes: bool,

    /// Comma-separated providers to configure.
    #[arg(long)]
    pub providers: Option<String>,

    /// Register Claude Code CLI hooks at the given scope.
    #[arg(long, value_enum)]
    pub install_hooks: Option<HookInstallScope>,

    /// Replace an existing daemon8 hook entry without prompting.
    #[arg(long)]
    pub force_hooks: bool,
}

#[derive(Debug, Serialize)]
struct SetupStatus {
    project: ProjectStatus,
    global_config_path: String,
    global_setup_applied: bool,
    runtime_sources: Vec<RuntimeSourceStatus>,
    providers: Vec<ProviderStatus>,
    service_installed: bool,
    issues: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProjectStatus {
    cwd: String,
    root: String,
    config_path: Option<String>,
    config_present: bool,
    slug: String,
    source_intents: Vec<ProjectSourceIntent>,
}

#[derive(Debug, Clone, Serialize)]
struct ProjectSourceIntent {
    name: String,
    runtime_name: String,
    source_type: String,
    original_path: String,
    resolved_path: String,
    parser: String,
    tags: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RuntimeSourceStatus {
    name: String,
    registered: bool,
}

#[derive(Debug, Serialize)]
struct ProviderStatus {
    name: String,
    config_path: String,
    already_configured: bool,
}

#[derive(Debug, Serialize)]
struct SetupPlan {
    status: SetupStatus,
    actions: Vec<SetupAction>,
}

#[derive(Debug, Serialize)]
struct SetupAction {
    kind: String,
    target: String,
    detail: String,
    mutating: bool,
}

#[derive(Debug)]
struct SetupContext {
    cwd: PathBuf,
    project_root: PathBuf,
    project_config_path: Option<PathBuf>,
    cli_cfg: cli_config::CliConfig,
    load_report: cli_config::LoadReport,
    global_cfg: config::Config,
    global_config_error: Option<String>,
    global_config_path: PathBuf,
}

pub async fn cmd_setup(config_path: Option<String>, args: SetupArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("cannot read current working directory")?;

    match args.command.unwrap_or(SetupCommand::Status) {
        SetupCommand::Status => {
            let status = setup_status(config_path.as_deref(), &cwd)?;
            print_json_or_human(&status, args.json, print_status)
        }
        SetupCommand::Plan => {
            let plan = setup_plan(config_path.as_deref(), &cwd)?;
            print_json_or_human(&plan, args.json, print_plan)
        }
        SetupCommand::Apply(apply_args) => {
            let plan = apply_setup(config_path.as_deref(), &cwd, &apply_args)?;
            print_json_or_human(&plan, args.json, print_plan)
        }
    }
}

pub async fn cmd_setup_mcp(action: SetupToolAction, config_path: Option<&str>) -> String {
    let cwd = action
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let result = match action.action.as_str() {
        "status" => setup_status(config_path, &cwd).and_then(json_pretty),
        "plan" => setup_plan(config_path, &cwd).and_then(json_pretty),
        "apply" => {
            if action.yes != Some(true) {
                return serde_json::to_string_pretty(
                    &serde_json::json!({ "error": "setup_apply requires yes=true" }),
                )
                .unwrap_or_default();
            }
            match action
                .install_hooks
                .as_deref()
                .map(parse_hook_scope)
                .transpose()
            {
                Ok(install_hooks) => {
                    let apply_args = SetupApplyArgs {
                        yes: true,
                        providers: action.providers,
                        install_hooks,
                        force_hooks: action.force_hooks.unwrap_or(false),
                    };
                    apply_setup(config_path, &cwd, &apply_args).and_then(json_pretty)
                }
                Err(e) => Err(e),
            }
        }
        other => Err(anyhow::anyhow!("unknown setup action '{other}'")),
    };

    match result {
        Ok(json) => json,
        Err(e) => serde_json::to_string_pretty(&serde_json::json!({ "error": e.to_string() }))
            .unwrap_or_default(),
    }
}

fn setup_status(config_path: Option<&str>, cwd: &Path) -> Result<SetupStatus> {
    let ctx = setup_context(config_path, cwd)?;
    Ok(build_status(&ctx))
}

fn setup_plan(config_path: Option<&str>, cwd: &Path) -> Result<SetupPlan> {
    let ctx = setup_context(config_path, cwd)?;
    let status = build_status(&ctx);
    let actions = planned_actions(&ctx, &status, None)?;
    Ok(SetupPlan { status, actions })
}

fn apply_setup(
    config_path: Option<&str>,
    cwd: &Path,
    apply_args: &SetupApplyArgs,
) -> Result<SetupPlan> {
    if is_non_interactive() && !apply_args.yes {
        bail!("setup apply requires --yes when stdin is not interactive or CI is set");
    }

    let ctx = setup_context(config_path, cwd)?;
    let status = build_status(&ctx);
    if !status.project.config_present {
        bail!("setup apply requires a project {PROJECT_CONFIG_FILENAME}; run daemon8 init first");
    }
    if ctx.load_report.has_errors() {
        bail!("setup apply requires valid setup config; run daemon8 setup status for details");
    }
    if ctx.global_config_error.is_some() {
        bail!("setup apply requires valid global config; run daemon8 setup status for details");
    }

    let actions = planned_actions(&ctx, &status, Some(apply_args))?;

    if !ctx.global_config_path.exists()
        && let Some(parent) = ctx.global_config_path.parent()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut cfg = ctx.global_cfg.clone();
    for source in &status.project.source_intents {
        cfg.sources.insert(
            source.runtime_name.clone(),
            SourceConfig::File(FileSourceConfig {
                path: source.resolved_path.clone(),
                parser: source.parser.clone(),
                tags: source.tags.clone(),
            }),
        );
    }

    let slug = status.project.slug.clone();
    cfg.setup.projects.insert(
        slug.clone(),
        ProjectSetupState {
            slug,
            root_path: status.project.root.clone(),
            config_path: status.project.config_path.clone().unwrap_or_else(|| {
                ctx.project_root
                    .join(PROJECT_CONFIG_FILENAME)
                    .display()
                    .to_string()
            }),
            applied_at_ns: now_ns(),
            desired_scope: vec!["file-sources".into()],
            hook_policy: hook_policy(apply_args),
            sources: status
                .project
                .source_intents
                .iter()
                .map(|source| source.runtime_name.clone())
                .collect(),
            source_audit: status
                .project
                .source_intents
                .iter()
                .map(|source| {
                    if source.warnings.is_empty() {
                        format!("{}: registered", source.runtime_name)
                    } else {
                        format!(
                            "{}: registered with warnings ({})",
                            source.runtime_name,
                            source.warnings.join("; ")
                        )
                    }
                })
                .collect(),
        },
    );

    write_global_config(&ctx.global_config_path, &cfg)?;
    apply_provider_and_hooks(&ctx.cwd, apply_args)?;

    // The on-disk apply has already succeeded by this point. If we cannot
    // re-read the post-apply state, surface that distinction explicitly so
    // operators know the config WAS written even though we can't reflect
    // the new state in the response.
    let post_ctx = setup_context(config_path, cwd)
        .with_context(|| "setup apply succeeded; rebuilding status from disk failed")?;
    let post_status = build_status(&post_ctx);
    Ok(SetupPlan {
        status: post_status,
        actions,
    })
}

fn setup_context(config_path: Option<&str>, cwd: &Path) -> Result<SetupContext> {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let (cli_cfg, load_report) = cli_config::load(&cwd);
    let project_config_path = cli_config::find_project_config(&cwd);
    let project_root = project_config_path
        .as_ref()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.clone());
    let (global_cfg, global_config_error) = match config::load(config_path) {
        Ok(cfg) => (cfg, None),
        Err(e) => (config::Config::default(), Some(e.to_string())),
    };
    let global_config_path = global_config_path(config_path, &global_cfg);

    Ok(SetupContext {
        cwd,
        project_root,
        project_config_path,
        cli_cfg,
        load_report,
        global_cfg,
        global_config_error,
        global_config_path,
    })
}

fn build_status(ctx: &SetupContext) -> SetupStatus {
    let slug = ctx.cli_cfg.resolved_slug(&ctx.cwd);
    let source_intents = source_intents(&slug, &ctx.project_root, &ctx.cli_cfg.sources);
    let runtime_sources = source_intents
        .iter()
        .map(|source| RuntimeSourceStatus {
            name: source.runtime_name.clone(),
            registered: ctx.global_cfg.sources.contains_key(&source.runtime_name),
        })
        .collect();
    let providers = crate::providers::detect_ai_tools()
        .into_iter()
        .map(|provider| ProviderStatus {
            name: provider.provider.label().to_string(),
            config_path: provider.config_path.display().to_string(),
            already_configured: provider.already_configured,
        })
        .collect();

    let mut issues = Vec::new();
    if ctx.project_config_path.is_none() {
        issues.push(format!("missing {PROJECT_CONFIG_FILENAME}"));
    }
    if let Some(error) = &ctx.load_report.user_error {
        issues.push(format!("user config: {error}"));
    }
    if let Some(error) = &ctx.load_report.project_error {
        issues.push(format!("project config: {error}"));
    }
    if let Some(error) = &ctx.global_config_error {
        issues.push(format!("global config: {error}"));
    }
    for source in &source_intents {
        for warning in &source.warnings {
            issues.push(format!("{}: {warning}", source.name));
        }
    }

    SetupStatus {
        project: ProjectStatus {
            cwd: ctx.cwd.display().to_string(),
            root: ctx.project_root.display().to_string(),
            config_path: ctx
                .project_config_path
                .as_ref()
                .map(|path| path.display().to_string()),
            config_present: ctx.project_config_path.is_some(),
            slug: slug.clone(),
            source_intents,
        },
        global_config_path: ctx.global_config_path.display().to_string(),
        global_setup_applied: ctx.global_cfg.setup.projects.contains_key(&slug),
        runtime_sources,
        providers,
        service_installed: service_installed(),
        issues,
    }
}

fn planned_actions(
    ctx: &SetupContext,
    status: &SetupStatus,
    apply_args: Option<&SetupApplyArgs>,
) -> Result<Vec<SetupAction>> {
    let mut actions = Vec::new();

    if status.project.config_present {
        for source in &status.project.source_intents {
            let registered = ctx.global_cfg.sources.contains_key(&source.runtime_name);
            actions.push(SetupAction {
                kind: if registered {
                    "source-refresh".into()
                } else {
                    "source-register".into()
                },
                target: source.runtime_name.clone(),
                detail: format!(
                    "daemon8 sees file source {} using parser {}; setup apply will {} the runtime source",
                    source.resolved_path,
                    source.parser,
                    if registered { "refresh" } else { "register" }
                ),
                mutating: true,
            });
        }
    } else {
        actions.push(SetupAction {
            kind: "project-config-missing".into(),
            target: ctx
                .project_root
                .join(PROJECT_CONFIG_FILENAME)
                .display()
                .to_string(),
            detail: format!("missing {PROJECT_CONFIG_FILENAME}; next run daemon8 init so setup can inspect source intents"),
            mutating: false,
        });
    }

    if let Some(apply_args) = apply_args {
        for provider in resolve_apply_providers(apply_args)? {
            actions.push(SetupAction {
                kind: "provider-config".into(),
                target: provider.config_path(&dirs_home()).display().to_string(),
                detail: format!("write {} MCP config", provider.label()),
                mutating: true,
            });
        }

        if let Some(scope) = &apply_args.install_hooks {
            actions.push(SetupAction {
                kind: "hook-install".into(),
                target: format!("{scope:?}"),
                detail: "install daemon8 CLI telemetry hooks".into(),
                mutating: true,
            });
        }
    }

    if status.project.config_present && !ctx.load_report.has_errors() {
        actions.push(SetupAction {
            kind: "setup-state".into(),
            target: status.project.slug.clone(),
            detail: "record what daemon8 can see and which source intents were applied".into(),
            mutating: true,
        });
    }

    Ok(actions)
}

fn source_intents(
    slug: &str,
    project_root: &Path,
    sources: &BTreeMap<String, SourceConfig>,
) -> Vec<ProjectSourceIntent> {
    sources
        .iter()
        .map(|(name, source)| match source {
            SourceConfig::File(file) => file_source_intent(slug, project_root, name, file),
        })
        .collect()
}

fn file_source_intent(
    slug: &str,
    project_root: &Path,
    name: &str,
    file: &FileSourceConfig,
) -> ProjectSourceIntent {
    let raw_path = Path::new(&file.path);
    let resolved = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        project_root.join(raw_path)
    };

    let mut tags = file.tags.clone();
    add_tag(&mut tags, format!("project:{slug}"));
    add_tag(&mut tags, "environment:local".into());
    add_tag(&mut tags, "source:file".into());
    add_tag(&mut tags, format!("parser:{}", file.parser));
    add_tag(&mut tags, "risk:local-file".into());

    let mut warnings = Vec::new();
    if let Err(e) = daemon8_parse::resolve_parser(&file.parser) {
        warnings.push(format!("parser '{}' is unavailable: {e}", file.parser));
    }
    if !path_or_glob_parent_exists(&resolved, &file.path) {
        warnings.push(format!(
            "source path is not currently reachable ({})",
            resolved.display()
        ));
    }

    ProjectSourceIntent {
        name: name.into(),
        runtime_name: format!("{slug}.{name}"),
        source_type: "file".into(),
        original_path: file.path.clone(),
        resolved_path: resolved.display().to_string(),
        parser: file.parser.clone(),
        tags,
        warnings,
    }
}

fn add_tag(tags: &mut Vec<String>, tag: String) {
    if !tags.contains(&tag) {
        tags.push(tag);
    }
}

fn path_or_glob_parent_exists(path: &Path, raw: &str) -> bool {
    let is_glob = raw.contains('*') || raw.contains('?');
    if is_glob {
        return path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .is_none_or(Path::exists);
    }
    path.exists()
}

fn apply_provider_and_hooks(cwd: &Path, args: &SetupApplyArgs) -> Result<ProviderWriteSummary> {
    let home = dirs_home();
    let mut summary = ProviderWriteSummary::default();

    for provider in resolve_apply_providers(args)? {
        let config_path = provider.config_path(&home);
        write_provider_config(provider, &config_path, Some(cwd))?;
        summary.provider_files.push(config_path);
        summary.note_restart(provider);

        if provider == crate::providers::Provider::Codex {
            let hook_path = install_codex_hooks(&home, args.force_hooks)?;
            summary.hook_files.push(hook_path);
        }
    }

    if let Some(scope) = args.install_hooks.clone() {
        let path = install_claude_hooks(scope.into(), cwd, &home, args.force_hooks)?;
        summary.hook_files.push(path);
        summary.note_restart(crate::providers::Provider::ClaudeCode);
    }

    Ok(summary)
}

fn resolve_apply_providers(args: &SetupApplyArgs) -> Result<Vec<crate::providers::Provider>> {
    args.providers
        .as_deref()
        .map(parse_provider_list)
        .transpose()
        .map(Option::unwrap_or_default)
}

fn hook_policy(args: &SetupApplyArgs) -> config::HookPolicy {
    match args.install_hooks.as_ref() {
        Some(_) => config::HookPolicy::Install,
        None => config::HookPolicy::Manual,
    }
}

fn global_config_path(config_path: Option<&str>, cfg: &config::Config) -> PathBuf {
    config_path
        .map(PathBuf::from)
        .unwrap_or_else(|| cfg.config_dir.join("config.toml"))
}

fn write_global_config(path: &Path, cfg: &config::Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string_pretty(cfg)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("moving {}", path.display()))?;
    Ok(())
}

fn parse_hook_scope(raw: &str) -> Result<HookInstallScope> {
    match raw {
        "local" => Ok(HookInstallScope::Local),
        "shared" => Ok(HookInstallScope::Shared),
        "global" => Ok(HookInstallScope::Global),
        other => bail!("unknown hook scope '{other}'"),
    }
}

fn is_non_interactive() -> bool {
    std::env::var_os("CI").is_some() || !std::io::stdin().is_terminal()
}

fn service_installed() -> bool {
    super::service::service_installed()
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn print_json_or_human<T, F>(value: &T, json: bool, print_human: F) -> Result<()>
where
    T: Serialize,
    F: FnOnce(&T),
{
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        print_human(value);
    }
    Ok(())
}

fn json_pretty<T: Serialize>(value: T) -> Result<String> {
    Ok(serde_json::to_string_pretty(&value)?)
}

fn print_status(status: &SetupStatus) {
    println!("setup status");
    println!("daemon8 sees:");
    println!("  project: {}", status.project.slug);
    println!("  cwd: {}", status.project.cwd);
    println!("  global config: {}", status.global_config_path);
    println!("  setup applied: {}", status.global_setup_applied);
    println!("  source intents: {}", status.project.source_intents.len());
    for source in &status.project.source_intents {
        println!(
            "  source {}: {} via {}",
            source.runtime_name, source.resolved_path, source.parser
        );
    }
    println!(
        "missing: {}",
        if status.issues.is_empty() {
            "none"
        } else {
            "see warnings below"
        }
    );
    for issue in &status.issues {
        println!("warning: {issue}");
    }
    if !status.project.config_present {
        println!("next: run daemon8 init, then daemon8 setup plan");
    } else if status.issues.is_empty() && !status.global_setup_applied {
        println!("next: run daemon8 setup plan, then daemon8 setup apply --yes");
    } else if status.issues.is_empty() {
        println!("next: setup is applied; rerun daemon8 setup plan after source changes");
    } else {
        println!("next: fix warnings, then rerun daemon8 setup status");
    }
}

fn print_plan(plan: &SetupPlan) {
    print_status(&plan.status);
    println!("actions:");
    for action in &plan.actions {
        println!("  {} {} - {}", action.kind, action.target, action.detail);
    }
    let restarts = summarize_restarts(&ProviderWriteSummary::default());
    for restart in restarts {
        println!("restart required: {restart}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_project_config(root: &Path, body: &str) {
        std::fs::write(root.join(PROJECT_CONFIG_FILENAME), body).unwrap();
    }

    #[test]
    fn status_reports_missing_project_config_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("config.toml");

        let status = setup_status(Some(global.to_str().unwrap()), tmp.path()).unwrap();

        assert!(!status.project.config_present);
        assert!(!global.exists());
        assert!(status.issues.iter().any(|issue| issue.contains("missing")));
    }

    #[test]
    fn plan_resolves_project_file_sources_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("logs")).unwrap();
        std::fs::write(project.join("logs/app.log"), "").unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        write_project_config(
            &project,
            r#"
[project]
slug = "demo"

[sources.app]
type = "file"
path = "logs/app.log"
parser = "line"
tags = ["app"]
"#,
        );
        let global = tmp.path().join("config.toml");

        let plan = setup_plan(Some(global.to_str().unwrap()), &project).unwrap();

        assert!(!global.exists());
        assert_eq!(plan.status.project.slug, "demo");
        let source = &plan.status.project.source_intents[0];
        assert_eq!(source.runtime_name, "demo.app");
        assert!(source.resolved_path.ends_with("project/logs/app.log"));
        assert!(source.tags.contains(&"project:demo".into()));
        assert!(source.warnings.is_empty());
    }

    #[test]
    fn plan_reports_malformed_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        write_project_config(&project, "not [toml");
        let global = tmp.path().join("config.toml");

        let plan = setup_plan(Some(global.to_str().unwrap()), &project).unwrap();

        assert!(plan.status.project.config_present);
        assert!(
            plan.status
                .issues
                .iter()
                .any(|issue| issue.contains("project config"))
        );
        assert!(!global.exists());
    }

    #[test]
    fn apply_registers_sources_and_setup_state_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("logs")).unwrap();
        std::fs::write(project.join("logs/app.log"), "").unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        write_project_config(
            &project,
            r#"
[project]
slug = "demo"

[sources.app]
type = "file"
path = "logs/app.log"
parser = "line"
"#,
        );
        let global = tmp.path().join("config.toml");
        let args = SetupApplyArgs {
            yes: true,
            ..SetupApplyArgs::default()
        };

        apply_setup(Some(global.to_str().unwrap()), &project, &args).unwrap();
        apply_setup(Some(global.to_str().unwrap()), &project, &args).unwrap();

        let parsed: config::Config =
            toml::from_str(&std::fs::read_to_string(&global).unwrap()).unwrap();
        assert!(parsed.sources.contains_key("demo.app"));
        assert_eq!(parsed.sources.len(), 1);
        assert!(parsed.setup.projects.contains_key("demo"));
    }

    #[test]
    fn apply_reports_post_apply_state() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        write_project_config(
            &project,
            r#"
[project]
slug = "demo"
"#,
        );
        let global = tmp.path().join("config.toml");
        let args = SetupApplyArgs {
            yes: true,
            ..SetupApplyArgs::default()
        };

        let result = apply_setup(Some(global.to_str().unwrap()), &project, &args).unwrap();

        assert!(
            result.status.global_setup_applied,
            "apply output must reflect post-apply state, not the pre-apply snapshot"
        );
        assert!(result.status.project.config_present);
    }

    #[test]
    fn apply_rejects_missing_or_malformed_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("config.toml");
        let args = SetupApplyArgs {
            yes: true,
            ..SetupApplyArgs::default()
        };

        let missing = apply_setup(Some(global.to_str().unwrap()), tmp.path(), &args);
        assert!(missing.is_err());
        assert!(!global.exists());

        std::fs::write(tmp.path().join(PROJECT_CONFIG_FILENAME), "not [toml").unwrap();
        let malformed = apply_setup(Some(global.to_str().unwrap()), tmp.path(), &args);
        assert!(malformed.is_err());
        assert!(!global.exists());
    }

    #[test]
    fn apply_rejects_malformed_global_config() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        write_project_config(
            &project,
            r#"
[project]
slug = "demo"
"#,
        );
        let global = tmp.path().join("config.toml");
        std::fs::write(&global, "not [toml").unwrap();
        let args = SetupApplyArgs {
            yes: true,
            ..SetupApplyArgs::default()
        };

        let status = setup_status(Some(global.to_str().unwrap()), &project).unwrap();
        assert!(
            status
                .issues
                .iter()
                .any(|issue| issue.contains("global config"))
        );

        let result = apply_setup(Some(global.to_str().unwrap()), &project, &args);
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&global).unwrap(), "not [toml");
    }

    #[tokio::test]
    async fn mcp_apply_requires_explicit_yes() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("config.toml");
        let response = cmd_setup_mcp(
            SetupToolAction {
                action: "apply".into(),
                cwd: Some(tmp.path().display().to_string()),
                yes: Some(false),
                providers: None,
                install_hooks: None,
                force_hooks: None,
            },
            Some(global.to_str().unwrap()),
        )
        .await;

        assert!(response.contains("setup_apply requires yes=true"));
        assert!(!global.exists());
    }
}
