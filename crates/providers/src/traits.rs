// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{HookIdentity, ServiceIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookScope {
    Local,
    Shared,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    PromptSubmit,
    ToolPre,
    ToolPost,
    PermissionRequest,
    PreCompact,
    PostCompact,
    Stop,
}

#[derive(Debug, Clone, Copy)]
pub struct HookEventEntry {
    pub event: HookEvent,
    pub native_name: &'static str,
    pub severity: LogLevel,
    pub matcher: Option<&'static str>,
}

pub trait AiProvider: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn restart_label(&self) -> &'static str;
    fn aliases(&self) -> &'static [&'static str];

    fn detect_dir(&self) -> &'static str;
    fn session_env_vars(&self) -> &'static [&'static str];
    fn session_id_env_vars(&self) -> &'static [&'static str] {
        self.session_env_vars()
    }

    fn config_path(&self, home: &Path) -> PathBuf;
    fn instruction_file_name(&self) -> &'static str;
    fn init_hint(&self) -> &'static str {
        "MCP config"
    }

    fn is_configured(&self, config_path: &Path, service: &ServiceIdentity) -> bool;
    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        project_dir: Option<&Path>,
        service: &ServiceIdentity,
    ) -> Result<()>;
    fn remove_mcp_config(&self, config_path: &Path, service: &ServiceIdentity) -> Result<bool>;

    fn global_config_dir(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn project_config_dir(&self) -> Option<&'static str> {
        None
    }
    fn skills_dir(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn project_skills_dir(&self) -> Option<&'static str> {
        None
    }
    fn rules_dir(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn project_rules_dir(&self) -> Option<&'static str> {
        None
    }
    fn agents_dir(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn project_agents_dir(&self) -> Option<&'static str> {
        None
    }
    fn conversation_dir(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn conversation_file_glob(&self) -> Option<&'static str> {
        None
    }
    fn session_id_from_env(&self) -> Option<String> {
        None
    }
    fn memory_dir(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn history_file(&self, home: &Path) -> Option<PathBuf> {
        let _ = home;
        None
    }
    fn list_projects(&self, home: &Path) -> Vec<ProjectEntry> {
        let _ = home;
        Vec::new()
    }
    fn project_conversation_files(
        &self,
        home: &Path,
        scope_root: &Path,
        since_ms: u64,
    ) -> Vec<PathBuf> {
        let _ = (home, scope_root, since_ms);
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub slug: String,
    pub path: PathBuf,
    pub provider: &'static str,
    pub last_active_ms: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledHookEntry {
    pub event: String,
    pub command: String,
}

pub trait HookProvider: AiProvider {
    fn supported_scopes(&self) -> &'static [HookScope];
    fn hooks_path(&self, scope: HookScope, cwd: &Path, home: &Path) -> PathBuf;
    fn hook_events(&self) -> &'static [HookEventEntry];
    fn scope_display_hint(&self, scope: HookScope, cwd: &Path, home: &Path) -> String {
        self.hooks_path(scope, cwd, home).display().to_string()
    }

    fn install_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        force: bool,
        identity: &HookIdentity,
    ) -> Result<PathBuf>;

    fn list_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        identity: &HookIdentity,
    ) -> Result<Vec<InstalledHookEntry>>;

    fn remove_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        identity: &HookIdentity,
    ) -> Result<Option<PathBuf>>;

    fn update_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        identity: &HookIdentity,
    ) -> Result<PathBuf> {
        self.install_hooks(scope, cwd, home, true, identity)
    }
}
