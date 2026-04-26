// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{HookScope, current_exe_string};

#[derive(Debug, Clone, Copy)]
pub(crate) struct HookSpec {
    event: &'static str,
    matcher: Option<&'static str>,
    status_message: Option<&'static str>,
}

pub fn install_claude_hooks(
    scope: HookScope,
    cwd: &Path,
    home: &Path,
    force: bool,
) -> Result<PathBuf> {
    let settings_path = scope.settings_path(cwd, home);
    let command = format!("{} cli-hook", current_exe_string());
    install_json_hooks(
        &settings_path,
        &command,
        &[
            HookSpec {
                event: "SessionStart",
                matcher: None,
                status_message: None,
            },
            HookSpec {
                event: "SessionEnd",
                matcher: None,
                status_message: None,
            },
            HookSpec {
                event: "UserPromptSubmit",
                matcher: None,
                status_message: None,
            },
            HookSpec {
                event: "PreToolUse",
                matcher: None,
                status_message: None,
            },
            HookSpec {
                event: "PostToolUse",
                matcher: None,
                status_message: None,
            },
            HookSpec {
                event: "PreCompact",
                matcher: None,
                status_message: None,
            },
            HookSpec {
                event: "Stop",
                matcher: None,
                status_message: None,
            },
        ],
        force,
    )
}

pub fn install_codex_hooks(home: &Path, force: bool) -> Result<PathBuf> {
    let settings_path = home.join(".codex/hooks.json");
    let command = format!("{} cli-hook --tool codex-cli", current_exe_string());
    install_json_hooks(
        &settings_path,
        &command,
        &[
            HookSpec {
                event: "SessionStart",
                matcher: Some("startup|resume"),
                status_message: Some("daemon8 session hook"),
            },
            HookSpec {
                event: "UserPromptSubmit",
                matcher: None,
                status_message: Some("daemon8 prompt hook"),
            },
            HookSpec {
                event: "PreToolUse",
                matcher: Some("Bash"),
                status_message: Some("daemon8 pre-tool hook"),
            },
            HookSpec {
                event: "PermissionRequest",
                matcher: Some("Bash"),
                status_message: Some("daemon8 permission hook"),
            },
            HookSpec {
                event: "PostToolUse",
                matcher: Some("Bash"),
                status_message: Some("daemon8 post-tool hook"),
            },
            HookSpec {
                event: "Stop",
                matcher: None,
                status_message: Some("daemon8 stop hook"),
            },
        ],
        force,
    )
}

fn install_json_hooks(
    settings_path: &Path,
    command: &str,
    specs: &[HookSpec],
    force: bool,
) -> Result<PathBuf> {
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root = if settings_path.exists() {
        let contents = std::fs::read_to_string(settings_path)
            .with_context(|| format!("failed to read {}", settings_path.display()))?;
        serde_json::from_str::<Value>(&contents)
            .with_context(|| format!("failed to parse {}", settings_path.display()))?
    } else {
        json!({})
    };

    let root_obj = root
        .as_object_mut()
        .context("settings root must be a JSON object")?;
    let hooks = root_obj
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .context("settings.hooks must be a JSON object")?;

    for spec in specs {
        let entry = hooks_obj
            .entry(spec.event.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let groups = entry
            .as_array_mut()
            .context("hook event entry must be an array")?;

        let daemon_index = groups.iter().position(group_contains_daemon_hook);
        let daemon_group = build_hook_group(command, *spec);

        match daemon_index {
            Some(index) if force => groups[index] = daemon_group,
            Some(_) => {}
            None => groups.push(daemon_group),
        }
    }

    let tmp = settings_path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, settings_path)?;

    Ok(settings_path.to_path_buf())
}

fn build_hook_group(command: &str, spec: HookSpec) -> Value {
    let mut hook = json!({
        "type": "command",
        "command": command,
    });

    if let Some(status_message) = spec.status_message {
        hook["statusMessage"] = Value::String(status_message.to_string());
    }

    let mut group = json!({
        "hooks": [hook],
    });

    if let Some(matcher) = spec.matcher {
        group["matcher"] = Value::String(matcher.to_string());
    }

    group
}

fn group_contains_daemon_hook(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("command")
                    .and_then(Value::as_str)
                    .map(is_daemon_hook_command)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn is_daemon_hook_command(command: &str) -> bool {
    command.contains("daemon8") && command.contains("cli-hook")
}
