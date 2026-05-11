// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod helpers;
pub mod hook_management;
pub mod opencode;
pub mod traits;

pub use helpers::shim_command;
pub use traits::{AiProvider, HookProvider, HookScope};

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use self::claude::ClaudeCodeProvider;
use self::codex::CodexProvider;
use self::gemini::GeminiProvider;
use self::opencode::OpenCodeProvider;

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
        let provider = match item {
            "claude" | "claude-code" => Provider::ClaudeCode,
            "gemini" | "gemini-cli" => Provider::Gemini,
            "codex" | "codex-cli" => Provider::Codex,
            "opencode" => Provider::OpenCode,
            other => bail!("unknown provider '{other}'"),
        };
        if !parsed.contains(&provider) {
            parsed.push(provider);
        }
    }
    Ok(parsed)
}

pub fn detect_ai_tools() -> Vec<DetectedProvider> {
    let home = dirs_home();
    let mut tools = Vec::new();

    for &provider in ALL_PROVIDERS {
        if !home.join(provider.detect_dir()).exists() {
            continue;
        }

        let p = provider.as_provider();
        let config_path = p.config_path(&home);
        let already_configured = p.is_configured(&config_path);

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
    project_dir: Option<&Path>,
) -> Result<()> {
    let port = crate::config::load(None).unwrap_or_default().server.port;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    provider
        .as_provider()
        .write_mcp_config(config_path, &mcp_url, project_dir)
}

pub fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn summarize_restarts(summary: &ProviderWriteSummary) -> Vec<String> {
    summary
        .restart_labels
        .iter()
        .map(|label| (*label).to_string())
        .collect()
}

// Convenience wrappers delegating to trait impls (used by init.rs, setup.rs, service.rs)

pub fn install_claude_hooks(
    scope: HookScope,
    cwd: &Path,
    home: &Path,
    force: bool,
) -> Result<PathBuf> {
    ClaudeCodeProvider.install_hooks(scope, cwd, home, force)
}

pub fn install_codex_hooks(home: &Path, force: bool) -> Result<PathBuf> {
    CodexProvider.install_hooks(HookScope::Global, &PathBuf::new(), home, force)
}

pub fn install_gemini_hooks(home: &Path, force: bool) -> Result<PathBuf> {
    GeminiProvider.install_hooks(HookScope::Global, &PathBuf::new(), home, force)
}
