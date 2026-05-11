// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::Result;
use daemon8_types::Severity;

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
    pub severity: Severity,
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

    fn is_configured(&self, config_path: &Path) -> bool;
    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        project_dir: Option<&Path>,
    ) -> Result<()>;
    fn remove_mcp_config(&self, config_path: &Path) -> Result<bool>;
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
    ) -> Result<PathBuf>;

    fn list_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
    ) -> Result<Vec<InstalledHookEntry>>;

    fn remove_hooks(&self, scope: HookScope, cwd: &Path, home: &Path) -> Result<Option<PathBuf>>;

    fn update_hooks(&self, scope: HookScope, cwd: &Path, home: &Path) -> Result<PathBuf> {
        self.install_hooks(scope, cwd, home, true)
    }
}
