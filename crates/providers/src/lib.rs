// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod claude;
pub mod codex;
pub mod gemini;
pub(crate) mod helpers;
pub mod hook_management;
pub mod opencode;
pub mod traits;

pub use traits::{
    AiProvider, HookEvent, HookEventEntry, HookProvider, HookScope, InstalledHookEntry, LogLevel,
    ProjectEntry,
};

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Result;

use self::claude::ClaudeCodeProvider;
use self::codex::CodexProvider;
use self::gemini::GeminiProvider;
use self::opencode::OpenCodeProvider;

#[derive(Debug, Clone)]
pub struct ServiceIdentity {
    pub name: &'static str,
    pub channel_name: Option<&'static str>,
    pub display_name: &'static str,
    pub hook_marker: &'static str,
    pub status_message: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    ClaudeCode,
    Gemini,
    Codex,
    OpenCode,
}

pub const ALL_PROVIDERS: &[Provider] = &[
    Provider::ClaudeCode,
    Provider::Gemini,
    Provider::Codex,
    Provider::OpenCode,
];

pub const HOOK_PROVIDERS: &[Provider] = &[Provider::ClaudeCode, Provider::Gemini, Provider::Codex];

impl Provider {
    pub fn as_provider(self) -> &'static dyn AiProvider {
        match self {
            Self::ClaudeCode => &ClaudeCodeProvider,
            Self::Gemini => &GeminiProvider,
            Self::Codex => &CodexProvider,
            Self::OpenCode => &OpenCodeProvider,
        }
    }

    pub fn as_hook_provider(self) -> Option<&'static dyn HookProvider> {
        match self {
            Self::ClaudeCode => Some(&ClaudeCodeProvider),
            Self::Gemini => Some(&GeminiProvider),
            Self::Codex => Some(&CodexProvider),
            Self::OpenCode => None,
        }
    }

    pub fn label(self) -> &'static str {
        self.as_provider().label()
    }

    pub fn restart_label(self) -> &'static str {
        self.as_provider().restart_label()
    }

    pub fn detect_dir(self) -> &'static str {
        self.as_provider().detect_dir()
    }

    pub fn config_path(self, home: &Path) -> PathBuf {
        self.as_provider().config_path(home)
    }
}

#[derive(Debug, Clone)]
pub struct DetectedProvider {
    pub provider: Provider,
    pub config_path: PathBuf,
    pub already_configured: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderWriteSummary {
    pub provider_files: Vec<PathBuf>,
    pub hook_files: Vec<PathBuf>,
    pub restart_labels: Vec<&'static str>,
}

impl ProviderWriteSummary {
    pub fn note_restart(&mut self, provider: Provider) {
        let label = provider.restart_label();
        if !self.restart_labels.contains(&label) {
            self.restart_labels.push(label);
        }
    }
}

pub fn list_all_projects() -> Vec<ProjectEntry> {
    let home = dirs_home();
    ALL_PROVIDERS
        .iter()
        .flat_map(|p| p.as_provider().list_projects(&home))
        .collect()
}

pub fn is_non_interactive() -> bool {
    std::env::var_os("CI").is_some() || !std::io::stdin().is_terminal()
}

pub fn parse_provider_list(raw: &str) -> Result<Vec<Provider>> {
    let mut parsed = Vec::new();
    for item in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let provider = ALL_PROVIDERS
            .iter()
            .find(|p| {
                p.as_provider()
                    .aliases()
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(item))
            })
            .copied()
            .ok_or_else(|| anyhow::anyhow!("unknown provider '{item}'"))?;
        if !parsed.contains(&provider) {
            parsed.push(provider);
        }
    }
    Ok(parsed)
}

pub fn detect_ai_tools(service: &ServiceIdentity) -> Vec<DetectedProvider> {
    let home = dirs_home();
    let mut tools = Vec::new();

    for &provider in ALL_PROVIDERS {
        if !home.join(provider.detect_dir()).exists() {
            continue;
        }

        let p = provider.as_provider();
        let config_path = p.config_path(&home);
        let already_configured = p.is_configured(&config_path, service);

        tools.push(DetectedProvider {
            provider,
            config_path,
            already_configured,
        });
    }

    tools.sort_by_key(|item| item.provider);
    tools
}

pub fn write_provider_config(
    provider: Provider,
    config_path: &Path,
    mcp_url: &str,
    project_dir: Option<&Path>,
    service: &ServiceIdentity,
) -> Result<()> {
    provider
        .as_provider()
        .write_mcp_config(config_path, mcp_url, project_dir, service)
}

pub fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn summarize_restarts(summary: &ProviderWriteSummary) -> Vec<String> {
    summary
        .restart_labels
        .iter()
        .map(|label| label.to_string())
        .collect()
}

pub fn install_hooks_for_provider(
    provider: Provider,
    scope: HookScope,
    cwd: &Path,
    home: &Path,
    force: bool,
    service: &ServiceIdentity,
) -> Result<PathBuf> {
    let hp = provider
        .as_hook_provider()
        .ok_or_else(|| anyhow::anyhow!("{} does not support hooks", provider.label()))?;
    hp.install_hooks(scope, cwd, home, force, service)
}

pub fn detect_provider_from_env() -> Option<(&'static str, Provider)> {
    for &provider in ALL_PROVIDERS {
        let p = provider.as_provider();
        for &var in p.session_env_vars() {
            if std::env::var_os(var).is_some() {
                let aliases = p.aliases();
                let tool_name = aliases.last().copied().unwrap_or(p.id());
                return Some((tool_name, provider));
            }
        }
    }
    None
}

pub fn resolve_hook_event(raw: &str) -> Option<(HookEvent, LogLevel)> {
    for &provider in HOOK_PROVIDERS {
        let hp = provider.as_hook_provider().unwrap();
        for entry in hp.hook_events() {
            if entry.native_name.eq_ignore_ascii_case(raw) {
                return Some((entry.event, entry.severity));
            }
        }
    }

    static EXTRA_ALIASES: &[(&str, HookEvent)] = &[
        ("userpromptsubmitted", HookEvent::PromptSubmit),
        ("beforesubmitprompt", HookEvent::PromptSubmit),
        ("session.compacting", HookEvent::PreCompact),
        ("postcompact", HookEvent::PostCompact),
    ];

    for &(alias, hook_event) in EXTRA_ALIASES {
        if alias.eq_ignore_ascii_case(raw) {
            let level = match hook_event {
                HookEvent::ToolPre | HookEvent::ToolPost => LogLevel::Debug,
                HookEvent::PermissionRequest => LogLevel::Warn,
                _ => LogLevel::Info,
            };
            return Some((hook_event, level));
        }
    }

    None
}

impl Provider {
    pub fn from_label(label: &str) -> Option<Provider> {
        ALL_PROVIDERS
            .iter()
            .find(|p| p.as_provider().label() == label)
            .copied()
    }
}

#[cfg(test)]
pub(crate) fn test_service() -> ServiceIdentity {
    ServiceIdentity {
        name: "test-svc",
        channel_name: Some("test-channel"),
        display_name: "Test Service",
        hook_marker: "daemon8",
        status_message: Some("test telemetry"),
    }
}

#[cfg(test)]
mod filesystem_layout_tests {
    use std::path::Path;

    use super::*;

    fn home() -> &'static Path {
        Path::new("/tmp/daemon8-test-home")
    }

    #[test]
    fn all_providers_have_global_config_dir() {
        for p in ALL_PROVIDERS {
            let dir = p.as_provider().global_config_dir(home());
            assert!(
                dir.is_some(),
                "{} should have a global_config_dir",
                p.label()
            );
        }
    }

    #[test]
    fn claude_filesystem_layout() {
        let p = Provider::ClaudeCode.as_provider();
        assert_eq!(p.global_config_dir(home()).unwrap(), home().join(".claude"));
        assert_eq!(p.project_config_dir(), Some(".claude"));
        assert_eq!(
            p.skills_dir(home()).unwrap(),
            home().join(".claude/commands")
        );
        assert_eq!(p.project_skills_dir(), Some(".claude/commands"));
        assert_eq!(p.rules_dir(home()).unwrap(), home().join(".claude/rules"));
        assert_eq!(p.project_rules_dir(), Some(".claude/rules"));
        assert!(p.agents_dir(home()).is_none());
        assert_eq!(
            p.conversation_dir(home()).unwrap(),
            home().join(".claude/projects")
        );
        assert_eq!(p.conversation_file_glob(), Some("**/*.jsonl"));
        assert_eq!(
            p.memory_dir(home()).unwrap(),
            home().join(".claude/projects")
        );
        assert!(p.history_file(home()).is_none());
    }

    #[test]
    fn codex_filesystem_layout() {
        let p = Provider::Codex.as_provider();
        assert_eq!(p.global_config_dir(home()).unwrap(), home().join(".codex"));
        assert_eq!(p.project_config_dir(), Some(".codex"));
        assert_eq!(p.skills_dir(home()).unwrap(), home().join(".codex/skills"));
        assert!(p.project_skills_dir().is_none());
        assert!(p.rules_dir(home()).is_none());
        assert!(p.agents_dir(home()).is_none());
        assert_eq!(
            p.conversation_dir(home()).unwrap(),
            home().join(".codex/sessions")
        );
        assert_eq!(p.conversation_file_glob(), Some("**/*.jsonl"));
        assert_eq!(
            p.memory_dir(home()).unwrap(),
            home().join(".codex/memories")
        );
        assert_eq!(
            p.history_file(home()).unwrap(),
            home().join(".codex/history.jsonl")
        );
    }

    #[test]
    fn gemini_filesystem_layout() {
        let p = Provider::Gemini.as_provider();
        assert_eq!(p.global_config_dir(home()).unwrap(), home().join(".gemini"));
        assert_eq!(p.project_config_dir(), Some(".gemini"));
        assert_eq!(p.skills_dir(home()).unwrap(), home().join(".gemini/skills"));
        assert_eq!(p.project_skills_dir(), Some(".gemini/skills"));
        assert!(p.rules_dir(home()).is_none());
        assert_eq!(p.agents_dir(home()).unwrap(), home().join(".gemini/agents"));
        assert_eq!(p.project_agents_dir(), Some(".gemini/agents"));
        assert_eq!(
            p.conversation_dir(home()).unwrap(),
            home().join(".gemini/tmp")
        );
        assert_eq!(p.conversation_file_glob(), Some("**/chats/session-*.jsonl"));
        assert!(p.memory_dir(home()).is_none());
        assert!(p.history_file(home()).is_none());
    }

    #[test]
    fn opencode_filesystem_layout() {
        let p = Provider::OpenCode.as_provider();
        assert_eq!(
            p.global_config_dir(home()).unwrap(),
            home().join(".config/opencode")
        );
        assert!(p.project_config_dir().is_none());
        assert!(p.skills_dir(home()).is_none());
        assert_eq!(
            p.rules_dir(home()).unwrap(),
            home().join(".config/opencode/rules")
        );
        assert!(p.agents_dir(home()).is_none());
        assert!(p.conversation_dir(home()).is_none());
        assert!(p.memory_dir(home()).is_none());
    }

    #[test]
    fn hook_providers_have_conversation_dirs() {
        for p in HOOK_PROVIDERS {
            let dir = p.as_provider().conversation_dir(home());
            assert!(dir.is_some(), "{} should have conversation_dir", p.label());
            let glob = p.as_provider().conversation_file_glob();
            assert!(
                glob.is_some(),
                "{} should have conversation_file_glob",
                p.label()
            );
        }
    }
}
