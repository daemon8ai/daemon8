// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::Result;
use daemon8_types::Severity;

use super::helpers::{
    HookSpec, current_exe_string, install_json_hooks, json_has_daemon8, list_json_hooks,
    quote_command_path, remove_json_hooks, shim_command,
};
use super::traits::{
    AiProvider, HookEvent, HookEventEntry, HookProvider, HookScope, InstalledHookEntry,
};

pub struct ClaudeCodeProvider;

static HOOK_EVENTS: &[HookEventEntry] = &[
    HookEventEntry {
        event: HookEvent::SessionStart,
        native_name: "SessionStart",
        severity: Severity::Info,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::SessionEnd,
        native_name: "SessionEnd",
        severity: Severity::Info,
        matcher: None,
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
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::PermissionRequest,
        native_name: "PermissionRequest",
        severity: Severity::Warn,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::ToolPost,
        native_name: "PostToolUse",
        severity: Severity::Debug,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::PreCompact,
        native_name: "PreCompact",
        severity: Severity::Info,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::Stop,
        native_name: "Stop",
        severity: Severity::Info,
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

    fn is_configured(&self, config_path: &Path) -> bool {
        json_has_daemon8(config_path)
    }

    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        _project_dir: Option<&Path>,
    ) -> Result<()> {
        let ok = shim_command("claude")
            .args([
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "http",
                "daemon8",
                mcp_url,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok {
            let exe = current_exe_string();
            let _ = shim_command("claude")
                .args([
                    "mcp",
                    "add",
                    "--scope",
                    "user",
                    "--transport",
                    "stdio",
                    "daemon8-channel",
                    &exe,
                    "--",
                    "channel",
                ])
                .status();
            Ok(())
        } else {
            write_claude_json_config(config_path, mcp_url)
        }
    }

    fn remove_mcp_config(&self, config_path: &Path) -> Result<bool> {
        let ok = shim_command("claude")
            .args(["mcp", "remove", "--scope", "user", "daemon8"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            let _ = shim_command("claude")
                .args(["mcp", "remove", "--scope", "user", "daemon8-channel"])
                .output();
            return Ok(true);
        }
        super::helpers::remove_json_mcp_entry(config_path, "daemon8")
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

fn write_claude_json_config(config_path: &Path, mcp_url: &str) -> Result<()> {
    use serde_json::json;

    let daemon8_entry = json!({ "type": "http", "url": mcp_url });
    let channel_entry = json!({
        "command": current_exe_string(),
        "args": ["channel"],
    });

    super::helpers::write_json_mcp_entries(
        config_path,
        &[
            ("daemon8", daemon8_entry),
            ("daemon8-channel", channel_entry),
        ],
    )
}
