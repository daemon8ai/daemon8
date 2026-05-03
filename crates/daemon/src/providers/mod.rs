// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod config;
pub mod hooks;

pub use config::write_provider_config;
pub use hooks::{install_claude_hooks, install_codex_hooks};

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

#[cfg(windows)]
pub(crate) fn shim_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/C").arg(program);
    cmd
}

#[cfg(not(windows))]
pub(crate) fn shim_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    ClaudeCode,
    Cursor,
    Windsurf,
    Gemini,
    Codex,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::Gemini => "Gemini",
            Self::Codex => "Codex",
        }
    }

    pub fn restart_label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "restart Claude Code sessions",
            Self::Cursor => "restart Cursor",
            Self::Windsurf => "restart Windsurf",
            Self::Gemini => "restart Gemini CLI sessions",
            Self::Codex => "restart Codex sessions",
        }
    }

    pub fn detect_dir(self) -> &'static str {
        match self {
            Self::ClaudeCode => ".claude",
            Self::Cursor => ".cursor",
            Self::Windsurf => ".codeium/windsurf",
            Self::Gemini => ".gemini",
            Self::Codex => ".codex",
        }
    }

    pub fn config_path(self, home: &Path) -> PathBuf {
        match self {
            Self::ClaudeCode => home.join(".claude.json"),
            Self::Cursor => home.join(".cursor/mcp.json"),
            Self::Windsurf => home.join(".codeium/windsurf/mcp_config.json"),
            Self::Gemini => home.join(".gemini/settings.json"),
            Self::Codex => home.join(".codex/config.toml"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookScope {
    Local,
    Shared,
    Global,
}

impl HookScope {
    pub fn settings_path(self, cwd: &Path, home: &Path) -> PathBuf {
        match self {
            Self::Local => cwd.join(".claude/settings.local.json"),
            Self::Shared => cwd.join(".claude/settings.json"),
            Self::Global => home.join(".claude/settings.json"),
        }
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
        if !self.restart_labels.contains(&provider.restart_label()) {
            self.restart_labels.push(provider.restart_label());
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
            "cursor" => Provider::Cursor,
            "windsurf" => Provider::Windsurf,
            "gemini" | "gemini-cli" => Provider::Gemini,
            "codex" | "codex-cli" => Provider::Codex,
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

    for provider in [
        Provider::ClaudeCode,
        Provider::Cursor,
        Provider::Windsurf,
        Provider::Gemini,
        Provider::Codex,
    ] {
        if !home_dir_exists(provider.detect_dir()) {
            continue;
        }

        let config_path = provider.config_path(&home);
        let already_configured = match provider {
            Provider::Codex => codex_has_daemon8(&config_path),
            _ => json_has_daemon8(&config_path),
        };

        tools.push(DetectedProvider {
            provider,
            config_path,
            already_configured,
        });
    }

    tools.sort_by_key(|item| item.provider);
    tools
}

pub(crate) fn current_exe_string() -> String {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("daemon8"))
        .to_string_lossy()
        .to_string()
}

pub(crate) fn json_has_daemon8(config_path: &Path) -> bool {
    config_path.exists()
        && std::fs::read_to_string(config_path)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|v| {
                v.get("mcpServers")?
                    .as_object()
                    .map(|m| m.contains_key("daemon8"))
            })
            .unwrap_or(false)
}

pub(crate) fn codex_has_daemon8(config_path: &Path) -> bool {
    config_path.exists()
        && std::fs::read_to_string(config_path)
            .ok()
            .and_then(|c| toml::from_str::<toml::Value>(&c).ok())
            .and_then(|v| {
                v.get("mcp_servers")?
                    .as_table()
                    .map(|table| table.contains_key("daemon8"))
            })
            .unwrap_or(false)
}

fn home_dir_exists(rel: &str) -> bool {
    dirs_home().join(rel).exists()
}

pub fn dirs_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn summarize_restarts(summary: &ProviderWriteSummary) -> Vec<String> {
    let mut messages = Vec::new();
    for label in &summary.restart_labels {
        messages.push((*label).to_string());
    }
    messages
}
