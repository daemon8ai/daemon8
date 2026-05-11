// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use daemon8_types::Severity;
use toml::Table;

use super::helpers::{
    HookSpec, codex_has_daemon8, current_exe_string, install_json_hooks, list_json_hooks,
    quote_command_path, remove_json_hooks,
};
use super::traits::{
    AiProvider, HookEvent, HookEventEntry, HookProvider, HookScope, InstalledHookEntry,
};

pub struct CodexProvider;

static HOOK_EVENTS: &[HookEventEntry] = &[
    HookEventEntry {
        event: HookEvent::SessionStart,
        native_name: "SessionStart",
        severity: Severity::Info,
        matcher: Some("startup|resume|clear"),
    },
    HookEventEntry {
        event: HookEvent::PromptSubmit,
        native_name: "UserPromptSubmit",
        severity: Severity::Info,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::ToolPre,
        native_name: "PreToolUse",
        severity: Severity::Debug,
        matcher: Some("Bash|apply_patch|Edit|Write"),
    },
    HookEventEntry {
        event: HookEvent::PermissionRequest,
        native_name: "PermissionRequest",
        severity: Severity::Warn,
        matcher: Some("Bash|apply_patch|Edit|Write"),
    },
    HookEventEntry {
        event: HookEvent::ToolPost,
        native_name: "PostToolUse",
        severity: Severity::Debug,
        matcher: Some("Bash|apply_patch|Edit|Write"),
    },
    HookEventEntry {
        event: HookEvent::Stop,
        native_name: "Stop",
        severity: Severity::Info,
        matcher: None,
    },
];

impl AiProvider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn restart_label(&self) -> &'static str {
        "restart Codex sessions"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["codex", "codex-cli"]
    }

    fn detect_dir(&self) -> &'static str {
        ".codex"
    }

    fn session_env_vars(&self) -> &'static [&'static str] {
        &["CODEX_SESSION_ID", "COPILOT_AGENT_SESSION_ID"]
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".codex/config.toml")
    }

    fn instruction_file_name(&self) -> &'static str {
        "AGENTS.md"
    }

    fn init_hint(&self) -> &'static str {
        "MCP config + trust project"
    }

    fn is_configured(&self, config_path: &Path) -> bool {
        codex_has_daemon8(config_path)
    }

    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        project_dir: Option<&Path>,
    ) -> Result<()> {
        write_codex_toml_config(config_path, mcp_url, project_dir)
    }

    fn remove_mcp_config(&self, config_path: &Path) -> Result<bool> {
        if !config_path.exists() {
            return Ok(false);
        }
        let contents = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let mut root: toml::Value = toml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?;

        let removed = root
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
            .map(|table| table.remove("daemon8").is_some())
            .unwrap_or(false);

        if removed {
            let tmp = config_path.with_extension("tmp");
            std::fs::write(&tmp, toml::to_string_pretty(&root)?)?;
            std::fs::rename(&tmp, config_path)?;
        }
        Ok(removed)
    }
}

impl HookProvider for CodexProvider {
    fn supported_scopes(&self) -> &'static [HookScope] {
        &[HookScope::Global]
    }

    fn hooks_path(&self, _scope: HookScope, _cwd: &Path, home: &Path) -> PathBuf {
        home.join(".codex/hooks.json")
    }

    fn hook_events(&self) -> &'static [HookEventEntry] {
        HOOK_EVENTS
    }

    fn install_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        force: bool,
    ) -> Result<PathBuf> {
        let settings_path = self.hooks_path(scope, cwd, home);
        let command = format!(
            "{} cli-hook --tool codex-cli",
            quote_command_path(&current_exe_string())
        );
        let specs: Vec<HookSpec> = HOOK_EVENTS
            .iter()
            .map(|e| HookSpec {
                event: e.native_name,
                matcher: e.matcher,
                timeout: None,
                status_message: Some("daemon8 telemetry"),
            })
            .collect();
        install_json_hooks(&settings_path, &command, &specs, force)
    }

    fn list_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
    ) -> Result<Vec<InstalledHookEntry>> {
        let path = self.hooks_path(scope, cwd, home);
        list_json_hooks(&path)
    }

    fn remove_hooks(&self, scope: HookScope, cwd: &Path, home: &Path) -> Result<Option<PathBuf>> {
        let path = self.hooks_path(scope, cwd, home);
        remove_json_hooks(&path)
    }
}

fn write_codex_toml_config(
    config_path: &Path,
    mcp_url: &str,
    project_dir: Option<&Path>,
) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root = if config_path.exists() {
        let contents = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        toml::from_str::<toml::Value>(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?
    } else {
        toml::Value::Table(Table::new())
    };

    let root_table = root
        .as_table_mut()
        .context("codex config root must be a table")?;

    let mcp_servers = get_or_insert_table(root_table, "mcp_servers")?;
    let mut daemon8_table = Table::new();
    daemon8_table.insert("name".to_string(), toml::Value::String("Daemon8".into()));
    daemon8_table.insert("url".to_string(), toml::Value::String(mcp_url.to_string()));
    mcp_servers.insert("daemon8".to_string(), toml::Value::Table(daemon8_table));

    let features = get_or_insert_table(root_table, "features")?;
    features.insert("codex_hooks".to_string(), toml::Value::Boolean(true));

    if let Some(project_dir) = project_dir {
        let projects = get_or_insert_table(root_table, "projects")?;
        let key = project_dir.to_string_lossy().to_string();
        let project_entry = projects
            .entry(key)
            .or_insert_with(|| toml::Value::Table(Table::new()));
        let project_table = project_entry
            .as_table_mut()
            .context("projects entry must be a table")?;
        project_table.insert(
            "trust_level".to_string(),
            toml::Value::String("trusted".into()),
        );
    }

    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, config_path)?;
    Ok(())
}

fn get_or_insert_table<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    let value = root
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    value
        .as_table_mut()
        .with_context(|| format!("{key} must be a TOML table"))
}
