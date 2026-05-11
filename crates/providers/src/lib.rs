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
