// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml::Table;

use super::ServiceIdentity;
use super::helpers::{
    HookSpec, codex_has_mcp_server, current_exe_string, install_json_hooks, list_json_hooks,
    quote_command_path, remove_json_hooks,
};
use super::traits::{
    AiProvider, HookEvent, HookEventEntry, HookProvider, HookScope, InstalledHookEntry, LogLevel,
};

pub struct CodexProvider;

static HOOK_EVENTS: &[HookEventEntry] = &[
    HookEventEntry {
        event: HookEvent::SessionStart,
        native_name: "SessionStart",
        severity: LogLevel::Info,
        matcher: Some("startup|resume|clear"),
    },
    HookEventEntry {
        event: HookEvent::PromptSubmit,
        native_name: "UserPromptSubmit",
        severity: LogLevel::Info,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::ToolPre,
        native_name: "PreToolUse",
        severity: LogLevel::Debug,
        matcher: Some("Bash|apply_patch|Edit|Write"),
    },
    HookEventEntry {
        event: HookEvent::PermissionRequest,
        native_name: "PermissionRequest",
        severity: LogLevel::Warn,
        matcher: Some("Bash|apply_patch|Edit|Write"),
    },
    HookEventEntry {
        event: HookEvent::ToolPost,
        native_name: "PostToolUse",
        severity: LogLevel::Debug,
        matcher: Some("Bash|apply_patch|Edit|Write"),
    },
    HookEventEntry {
        event: HookEvent::Stop,
        native_name: "Stop",
        severity: LogLevel::Info,
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

    fn is_configured(&self, config_path: &Path, service: &ServiceIdentity) -> bool {
        codex_has_mcp_server(config_path, service.name)
    }

    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        project_dir: Option<&Path>,
        service: &ServiceIdentity,
    ) -> Result<()> {
        write_codex_toml_config(config_path, mcp_url, project_dir, service)
    }

    fn remove_mcp_config(&self, config_path: &Path, service: &ServiceIdentity) -> Result<bool> {
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
            .map(|table| table.remove(service.name).is_some())
            .unwrap_or(false);

        if removed {
            let tmp = config_path.with_extension("tmp");
            std::fs::write(&tmp, toml::to_string_pretty(&root)?)?;
            std::fs::rename(&tmp, config_path)?;
        }
        Ok(removed)
    }

    fn global_config_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".codex"))
    }
    fn project_config_dir(&self) -> Option<&'static str> {
        Some(".codex")
    }
    fn skills_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".codex/skills"))
    }
    fn conversation_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".codex/sessions"))
    }
    fn conversation_file_glob(&self) -> Option<&'static str> {
        Some("**/*.jsonl")
    }
    fn session_id_from_env(&self) -> Option<String> {
        std::env::var("CODEX_SESSION_ID").ok()
    }
    fn memory_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".codex/memories"))
    }
    fn history_file(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".codex/history.jsonl"))
    }
    fn list_projects(&self, home: &Path) -> Vec<super::traits::ProjectEntry> {
        let db_path = home.join(".codex/state_5.sqlite");
        if !db_path.exists() {
            return Vec::new();
        }
        let Ok(output) = std::process::Command::new("sqlite3")
            .arg(&db_path)
            .arg("SELECT cwd, MAX(COALESCE(updated_at_ms, updated_at * 1000)) FROM threads GROUP BY cwd ORDER BY 2 DESC")
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let (cwd, ts) = line.split_once('|')?;
                let path = PathBuf::from(cwd);
                let last_active_ms = ts.parse::<u64>().ok();
                Some(super::traits::ProjectEntry {
                    slug: cwd.to_string(),
                    path,
                    provider: "codex",
                    last_active_ms,
                })
            })
            .collect()
    }
}

impl HookProvider for CodexProvider {
    fn supported_scopes(&self) -> &'static [HookScope] {
        &[HookScope::Shared, HookScope::Global]
    }

    fn hooks_path(&self, scope: HookScope, cwd: &Path, home: &Path) -> PathBuf {
        match scope {
            HookScope::Local | HookScope::Shared => cwd.join(".codex/hooks.json"),
            HookScope::Global => home.join(".codex/hooks.json"),
        }
    }

    fn scope_display_hint(&self, scope: HookScope, _cwd: &Path, _home: &Path) -> String {
        match scope {
            HookScope::Local | HookScope::Shared => ".codex/hooks.json".into(),
            HookScope::Global => "~/.codex/hooks.json".into(),
        }
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
        service: &ServiceIdentity,
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
                status_message: service.status_message,
            })
            .collect();
        install_json_hooks(&settings_path, &command, &specs, force, service.hook_marker)
    }

    fn list_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        service: &ServiceIdentity,
    ) -> Result<Vec<InstalledHookEntry>> {
        let path = self.hooks_path(scope, cwd, home);
        list_json_hooks(&path, service.hook_marker)
    }

    fn remove_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        service: &ServiceIdentity,
    ) -> Result<Option<PathBuf>> {
        let path = self.hooks_path(scope, cwd, home);
        remove_json_hooks(&path, service.hook_marker)
    }
}

fn write_codex_toml_config(
    config_path: &Path,
    mcp_url: &str,
    project_dir: Option<&Path>,
    service: &ServiceIdentity,
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
    let mut entry_table = Table::new();
    entry_table.insert(
        "name".to_string(),
        toml::Value::String(service.display_name.to_string()),
    );
    entry_table.insert("url".to_string(), toml::Value::String(mcp_url.to_string()));
    mcp_servers.insert(service.name.to_string(), toml::Value::Table(entry_table));

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn supported_scopes_include_shared_and_global() {
        let scopes = CodexProvider.supported_scopes();
        assert!(scopes.contains(&HookScope::Shared));
        assert!(scopes.contains(&HookScope::Global));
    }

    #[test]
    fn hooks_path_routes_by_scope() {
        let cwd = Path::new("/project");
        let home = Path::new("/home/user");
        assert_eq!(
            CodexProvider.hooks_path(HookScope::Shared, cwd, home),
            cwd.join(".codex/hooks.json")
        );
        assert_eq!(
            CodexProvider.hooks_path(HookScope::Global, cwd, home),
            home.join(".codex/hooks.json")
        );
    }

    #[test]
    fn install_list_remove_shared_scope() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let home = tmp.path().join("fakehome");
        std::fs::create_dir_all(cwd.join(".codex")).unwrap();
        let svc = crate::test_service();

        let path = CodexProvider
            .install_hooks(HookScope::Shared, &cwd, &home, false, &svc)
            .unwrap();
        assert_eq!(path, cwd.join(".codex/hooks.json"));
        assert!(path.exists());

        let entries = CodexProvider
            .list_hooks(HookScope::Shared, &cwd, &home, &svc)
            .unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].command.contains("cli-hook --tool codex-cli"));

        let removed = CodexProvider
            .remove_hooks(HookScope::Shared, &cwd, &home, &svc)
            .unwrap();
        assert!(removed.is_some());

        let after = CodexProvider
            .list_hooks(HookScope::Shared, &cwd, &home, &svc)
            .unwrap();
        assert!(after.is_empty());
    }

    #[test]
    fn install_list_remove_global_scope() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let svc = crate::test_service();

        let path = CodexProvider
            .install_hooks(HookScope::Global, &PathBuf::new(), &home, false, &svc)
            .unwrap();
        assert_eq!(path, home.join(".codex/hooks.json"));
        assert!(path.exists());

        let entries = CodexProvider
            .list_hooks(HookScope::Global, &PathBuf::new(), &home, &svc)
            .unwrap();
        assert!(!entries.is_empty());

        let removed = CodexProvider
            .remove_hooks(HookScope::Global, &PathBuf::new(), &home, &svc)
            .unwrap();
        assert!(removed.is_some());
    }

    #[test]
    fn write_mcp_config_creates_entry() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        let svc = crate::test_service();

        CodexProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None, &svc)
            .unwrap();

        let content: toml::Value =
            toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        let entry = content
            .get("mcp_servers")
            .and_then(|m| m.get(svc.name))
            .expect("mcp_servers.test-svc key");
        assert_eq!(entry["url"].as_str().unwrap(), "http://127.0.0.1:8371/mcp");
    }

    #[test]
    fn remove_mcp_config_deletes_entry() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("config.toml");
        let svc = crate::test_service();

        CodexProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None, &svc)
            .unwrap();
        assert!(CodexProvider.is_configured(&config, &svc));

        let removed = CodexProvider.remove_mcp_config(&config, &svc).unwrap();
        assert!(removed);
        assert!(!CodexProvider.is_configured(&config, &svc));
    }

    #[test]
    fn list_projects_no_db() {
        let home = tempfile::tempdir().unwrap();
        let entries = CodexProvider.list_projects(home.path());
        assert!(entries.is_empty());
    }
}
