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
    let command = format!("{} cli-hook", quote_command_path(&current_exe_string()));
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
                event: "PermissionRequest",
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
    let command = format!(
        "{} cli-hook --tool codex-cli",
        quote_command_path(&current_exe_string())
    );
    install_json_hooks(
        &settings_path,
        &command,
        &[
            HookSpec {
                event: "SessionStart",
                matcher: Some("startup|resume|clear"),
                status_message: Some("daemon8 session telemetry"),
            },
            HookSpec {
                event: "UserPromptSubmit",
                matcher: None,
                status_message: Some("daemon8 turn telemetry"),
            },
            HookSpec {
                event: "PreToolUse",
                matcher: Some("Bash|apply_patch|Edit|Write"),
                status_message: Some("daemon8 tool telemetry"),
            },
            HookSpec {
                event: "PermissionRequest",
                matcher: Some("Bash|apply_patch|Edit|Write"),
                status_message: Some("daemon8 permission telemetry"),
            },
            HookSpec {
                event: "PostToolUse",
                matcher: Some("Bash|apply_patch|Edit|Write"),
                status_message: Some("daemon8 tool result telemetry"),
            },
            HookSpec {
                event: "Stop",
                matcher: None,
                status_message: Some("daemon8 lifecycle telemetry"),
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

        let daemon_group = build_hook_group(command, *spec);

        if force {
            groups.retain(|group| !group_contains_daemon_hook(group));
            groups.push(daemon_group);
        } else if !groups.iter().any(group_contains_daemon_hook) {
            groups.push(daemon_group);
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

fn quote_command_path(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

/// Remove all daemon8 hook entries from a provider's settings.json. Returns
/// the path written if any entries were removed, or None if the file didn't
/// exist or had no daemon8 entries.
pub fn remove_json_hooks(settings_path: &Path) -> Result<Option<PathBuf>> {
    if !settings_path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(settings_path)
        .with_context(|| format!("failed to read {}", settings_path.display()))?;
    let mut root: Value = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", settings_path.display()))?;

    let Some(root_obj) = root.as_object_mut() else {
        return Ok(None);
    };
    let Some(hooks) = root_obj.get_mut("hooks") else {
        return Ok(None);
    };
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Ok(None);
    };

    let mut removed = false;
    let mut empty_events = Vec::new();
    for (event, entry) in hooks_obj.iter_mut() {
        let Some(groups) = entry.as_array_mut() else {
            continue;
        };
        let before = groups.len();
        groups.retain(|group| !group_contains_daemon_hook(group));
        if groups.len() != before {
            removed = true;
        }
        if groups.is_empty() {
            empty_events.push(event.clone());
        }
    }
    for event in empty_events {
        hooks_obj.remove(&event);
    }
    if hooks_obj.is_empty() {
        root_obj.remove("hooks");
    }

    if !removed {
        return Ok(None);
    }

    let tmp = settings_path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, settings_path)?;
    Ok(Some(settings_path.to_path_buf()))
}

/// Inspect a provider settings.json for daemon8 hook entries. Returns the
/// list of event names that currently contain a daemon8 entry, plus the
/// command string used (for drift detection vs current binary).
pub fn list_json_hooks(settings_path: &Path) -> Result<Vec<InstalledHookEntry>> {
    if !settings_path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(settings_path)
        .with_context(|| format!("failed to read {}", settings_path.display()))?;
    let root: Value = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", settings_path.display()))?;

    let mut entries = Vec::new();
    let Some(hooks_obj) = root.get("hooks").and_then(Value::as_object) else {
        return Ok(entries);
    };
    for (event, value) in hooks_obj {
        let Some(groups) = value.as_array() else {
            continue;
        };
        for group in groups {
            let Some(items) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for item in items {
                let Some(cmd) = item.get("command").and_then(Value::as_str) else {
                    continue;
                };
                if is_daemon_hook_command(cmd) {
                    entries.push(InstalledHookEntry {
                        event: event.clone(),
                        command: cmd.to_string(),
                    });
                }
            }
        }
    }
    Ok(entries)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledHookEntry {
    pub event: String,
    pub command: String,
}

/// Rewrite Claude hooks for a given scope (force=true). Mirrors install with
/// drift correction — useful when the daemon binary path has changed.
pub fn update_claude_hooks(scope: HookScope, cwd: &Path, home: &Path) -> Result<PathBuf> {
    install_claude_hooks(scope, cwd, home, true)
}

/// Rewrite Codex hooks (force=true).
pub fn update_codex_hooks(home: &Path) -> Result<PathBuf> {
    install_codex_hooks(home, true)
}

pub fn remove_claude_hooks(scope: HookScope, cwd: &Path, home: &Path) -> Result<Option<PathBuf>> {
    let path = scope.settings_path(cwd, home);
    remove_json_hooks(&path)
}

pub fn remove_codex_hooks(home: &Path) -> Result<Option<PathBuf>> {
    let path = home.join(".codex/hooks.json");
    remove_json_hooks(&path)
}

pub fn list_claude_hooks(
    scope: HookScope,
    cwd: &Path,
    home: &Path,
) -> Result<Vec<InstalledHookEntry>> {
    let path = scope.settings_path(cwd, home);
    list_json_hooks(&path)
}

pub fn list_codex_hooks(home: &Path) -> Result<Vec<InstalledHookEntry>> {
    let path = home.join(".codex/hooks.json");
    list_json_hooks(&path)
}

#[cfg(test)]
mod hook_management_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_then_remove_round_trip_claude() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let home = tmp.path().to_path_buf();

        let path = install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();
        let entries = list_json_hooks(&path).unwrap();
        assert!(
            !entries.is_empty(),
            "install should have written hook entries"
        );
        assert!(entries.iter().any(|e| e.event == "PreToolUse"));

        let removed = remove_json_hooks(&path).unwrap();
        assert!(removed.is_some());
        let after = list_json_hooks(&path).unwrap();
        assert!(after.is_empty(), "all daemon8 entries should be gone");
    }

    #[test]
    fn remove_is_noop_when_not_installed() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(remove_json_hooks(&path).unwrap().is_none());
    }

    #[test]
    fn update_strips_stale_and_replaces() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let home = tmp.path().to_path_buf();
        let path = install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();

        // Inject a stale-looking daemon8 entry at PreToolUse.
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let pre = root["hooks"]["PreToolUse"].as_array_mut().unwrap();
        pre.push(json!({"hooks": [{"type": "command", "command": "/old/path/daemon8 cli-hook"}]}));
        std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap()).unwrap();

        let before = list_json_hooks(&path).unwrap();
        let pre_count_before = before.iter().filter(|e| e.event == "PreToolUse").count();
        assert!(pre_count_before >= 2);

        let _ = update_claude_hooks(HookScope::Local, &cwd, &home).unwrap();
        let after = list_json_hooks(&path).unwrap();
        let pre_count_after = after.iter().filter(|e| e.event == "PreToolUse").count();
        assert_eq!(
            pre_count_after, 1,
            "update should leave exactly one entry per event"
        );
    }
}
