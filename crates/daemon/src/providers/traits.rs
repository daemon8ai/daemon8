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
pub enum CanonicalEvent {
    SessionStart,
    SessionEnd,
    PromptSubmit,
    ToolPre,
    ToolPost,
    PermissionRequest,
    PreCompact,
    Stop,
}

#[derive(Debug, Clone, Copy)]
pub struct NormalizedHookEvent {
    pub canonical: CanonicalEvent,
    pub native_name: &'static str,
    pub severity: Severity,
    pub matcher: Option<&'static str>,
}

pub struct ProviderDocs {
    pub hooks: Option<&'static str>,
    pub mcp: Option<&'static str>,
    pub config: Option<&'static str>,
    pub instructions: Option<&'static str>,
}

pub trait AiProvider: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn restart_label(&self) -> &'static str;

    fn detect_dir(&self) -> &'static str;
    fn session_env_vars(&self) -> &'static [&'static str];

    fn config_path(&self, home: &Path) -> PathBuf;
    fn project_config_path(&self, project: &Path) -> Option<PathBuf>;
    fn instruction_file_name(&self) -> &'static str;

    fn is_configured(&self, config_path: &Path) -> bool;
    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        project_dir: Option<&Path>,
    ) -> Result<()>;
    fn remove_mcp_config(&self, config_path: &Path) -> Result<bool>;

    fn docs(&self) -> &'static ProviderDocs;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledHookEntry {
    pub event: String,
    pub command: String,
}

pub trait HookProvider: AiProvider {
    fn supported_scopes(&self) -> &'static [HookScope];
    fn hooks_path(&self, scope: HookScope, cwd: &Path, home: &Path) -> PathBuf;
    fn hook_events(&self) -> &'static [NormalizedHookEvent];

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
