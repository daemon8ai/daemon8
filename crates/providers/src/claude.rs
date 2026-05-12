// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::ServiceIdentity;
use super::helpers::{
    HookSpec, current_exe_string, install_json_hooks, json_has_mcp_server, list_json_hooks,
    quote_command_path, remove_json_hooks, shim_command,
};
use super::traits::{
    AiProvider, HookEvent, HookEventEntry, HookProvider, HookScope, InstalledHookEntry, LogLevel,
};

pub struct ClaudeCodeProvider;

static HOOK_EVENTS: &[HookEventEntry] = &[
    HookEventEntry {
        event: HookEvent::SessionStart,
        native_name: "SessionStart",
        severity: LogLevel::Info,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::SessionEnd,
        native_name: "SessionEnd",
        severity: LogLevel::Info,
        matcher: None,
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
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::PermissionRequest,
        native_name: "PermissionRequest",
        severity: LogLevel::Warn,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::ToolPost,
        native_name: "PostToolUse",
        severity: LogLevel::Debug,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::PreCompact,
        native_name: "PreCompact",
        severity: LogLevel::Info,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::Stop,
        native_name: "Stop",
        severity: LogLevel::Info,
        matcher: None,
    },
];

impl AiProvider for ClaudeCodeProvider {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn restart_label(&self) -> &'static str {
        "restart Claude Code sessions"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["claude", "claude-code"]
    }

    fn detect_dir(&self) -> &'static str {
        ".claude"
    }

    fn session_env_vars(&self) -> &'static [&'static str] {
        &["CLAUDE_PROJECT_DIR"]
    }

    fn session_id_env_vars(&self) -> &'static [&'static str] {
        &["CLAUDE_SESSION_ID", "CLAUDE_PROJECT_DIR"]
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".claude.json")
    }

    fn instruction_file_name(&self) -> &'static str {
        "CLAUDE.md"
    }

    fn is_configured(&self, config_path: &Path, service: &ServiceIdentity) -> bool {
        json_has_mcp_server(config_path, service.name)
    }

    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        _project_dir: Option<&Path>,
        service: &ServiceIdentity,
    ) -> Result<()> {
        let ok = shim_command("claude")
            .args([
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "http",
                service.name,
                mcp_url,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok {
            if let Some(channel_name) = service.channel_name {
                let exe = current_exe_string();
                let _ = shim_command("claude")
                    .args([
                        "mcp",
                        "add",
                        "--scope",
                        "user",
                        "--transport",
                        "stdio",
                        channel_name,
                        &exe,
                        "--",
                        "channel",
                    ])
                    .status();
            }
            Ok(())
        } else {
            write_claude_json_config(config_path, mcp_url, service)
        }
    }

    fn remove_mcp_config(&self, config_path: &Path, service: &ServiceIdentity) -> Result<bool> {
        let ok = shim_command("claude")
            .args(["mcp", "remove", "--scope", "user", service.name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            if let Some(channel_name) = service.channel_name {
                let _ = shim_command("claude")
                    .args(["mcp", "remove", "--scope", "user", channel_name])
                    .output();
            }
            return Ok(true);
        }
        super::helpers::remove_json_mcp_entry(config_path, service.name)
    }

    fn global_config_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude"))
    }
    fn project_config_dir(&self) -> Option<&'static str> {
        Some(".claude")
    }
    fn skills_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude/commands"))
    }
    fn project_skills_dir(&self) -> Option<&'static str> {
        Some(".claude/commands")
    }
    fn rules_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude/rules"))
    }
    fn project_rules_dir(&self) -> Option<&'static str> {
        Some(".claude/rules")
    }
    fn conversation_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude/projects"))
    }
    fn conversation_file_glob(&self) -> Option<&'static str> {
        Some("**/*.jsonl")
    }
    fn session_id_from_env(&self) -> Option<String> {
        std::env::var("CLAUDE_CODE_SESSION_ID").ok()
    }
    fn memory_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".claude/projects"))
    }
}

impl HookProvider for ClaudeCodeProvider {
    fn supported_scopes(&self) -> &'static [HookScope] {
        &[HookScope::Local, HookScope::Shared, HookScope::Global]
    }

    fn hooks_path(&self, scope: HookScope, cwd: &Path, home: &Path) -> PathBuf {
        match scope {
            HookScope::Local => cwd.join(".claude/settings.local.json"),
            HookScope::Shared => cwd.join(".claude/settings.json"),
            HookScope::Global => home.join(".claude/settings.json"),
        }
    }

    fn hook_events(&self) -> &'static [HookEventEntry] {
        HOOK_EVENTS
    }

    fn scope_display_hint(&self, scope: HookScope, _cwd: &Path, _home: &Path) -> String {
        match scope {
            HookScope::Local => ".claude/settings.local.json".into(),
            HookScope::Shared => ".claude/settings.json".into(),
            HookScope::Global => "~/.claude/settings.json".into(),
        }
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
        let command = format!("{} cli-hook", quote_command_path(&current_exe_string()));
        let specs: Vec<HookSpec> = HOOK_EVENTS
            .iter()
            .map(|e| HookSpec {
                event: e.native_name,
                matcher: e.matcher,
                timeout: None,
                status_message: None,
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

fn write_claude_json_config(
    config_path: &Path,
    mcp_url: &str,
    service: &ServiceIdentity,
) -> Result<()> {
    use serde_json::json;

    let main_entry = json!({ "type": "http", "url": mcp_url });
    let mut entries: Vec<(&str, serde_json::Value)> = vec![(service.name, main_entry)];

    if let Some(channel_name) = service.channel_name {
        let channel_entry = json!({
            "command": current_exe_string(),
            "args": ["channel"],
        });
        entries.push((channel_name, channel_entry));
    }

    super::helpers::write_json_mcp_entries(config_path, &entries)
}
