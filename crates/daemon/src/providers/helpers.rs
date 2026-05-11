// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::traits::InstalledHookEntry;

#[derive(Debug, Clone, Copy)]
pub struct HookSpec {
    pub event: &'static str,
    pub matcher: Option<&'static str>,
    pub timeout: Option<u64>,
    pub status_message: Option<&'static str>,
}

pub fn current_exe_string() -> String {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("daemon8"))
        .to_string_lossy()
        .to_string()
}

pub fn quote_command_path(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

pub fn is_daemon_hook_command(command: &str) -> bool {
    command.contains("daemon8") && command.contains("cli-hook")
}

pub fn install_json_hooks(
    settings_path: &Path,
    command: &str,
    specs: &[HookSpec],
    force: bool,
) -> Result<PathBuf> {
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root = read_or_empty_json(settings_path)?;

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

        let daemon_group = build_hook_group(command, spec);

        if force {
            groups.retain(|group| !group_contains_daemon_hook(group));
            groups.push(daemon_group);
        } else if !groups.iter().any(group_contains_daemon_hook) {
            groups.push(daemon_group);
        }
    }

    atomic_write_json(settings_path, &root)?;
    Ok(settings_path.to_path_buf())
}

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
        for group in groups.iter_mut() {
            if group
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_daemon_hook_command)
            {
                *group = Value::Null;
                removed = true;
                continue;
            }
            if let Some(items) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = items.len();
                items.retain(|item| {
                    !item
                        .get("command")
                        .and_then(Value::as_str)
                        .map(is_daemon_hook_command)
                        .unwrap_or(false)
                });
                if items.len() != before {
                    removed = true;
                }
            }
        }
        groups.retain(|group| {
            if group.is_null() {
                return false;
            }
            group
                .get("hooks")
                .and_then(Value::as_array)
                .map(|items| !items.is_empty())
                .unwrap_or(true)
        });
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

    atomic_write_json(settings_path, &root)?;
    Ok(Some(settings_path.to_path_buf()))
}

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

pub fn json_has_daemon8(config_path: &Path) -> bool {
    config_path.exists()
        && std::fs::read_to_string(config_path)
            .ok()
            .and_then(|c| serde_json::from_str::<Value>(&c).ok())
            .and_then(|v| {
                v.get("mcpServers")?
                    .as_object()
                    .map(|m| m.contains_key("daemon8"))
            })
            .unwrap_or(false)
}

pub fn codex_has_daemon8(config_path: &Path) -> bool {
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

fn read_or_empty_json(path: &Path) -> Result<Value> {
    if path.exists() {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))
    } else {
        Ok(json!({}))
    }
}

fn atomic_write_json(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let content = serde_json::to_string_pretty(value)?;
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn build_hook_group(command: &str, spec: &HookSpec) -> Value {
    let mut hook = json!({
        "type": "command",
        "command": command,
    });

    if let Some(timeout) = spec.timeout {
        hook["timeout"] = Value::Number(timeout.into());
    }

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

pub fn write_json_mcp_entries(config_path: &Path, entries: &[(&str, Value)]) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root = read_or_empty_json(config_path)?;

    let root_obj = root
        .as_object_mut()
        .context("provider config must be a JSON object")?;
    let servers = root_obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    let servers_obj = servers
        .as_object_mut()
        .context("mcpServers must be a JSON object")?;

    for (name, value) in entries {
        servers_obj.insert(name.to_string(), value.clone());
    }

    atomic_write_json(config_path, &root)
}

pub fn remove_json_mcp_entry(config_path: &Path, name: &str) -> Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }
    let contents = std::fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut root: Value = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;

    let removed = root
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .map(|servers| servers.remove(name).is_some())
        .unwrap_or(false);

    if removed {
        atomic_write_json(config_path, &root)?;
    }
    Ok(removed)
}

#[cfg(windows)]
pub fn shim_command(program: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/C").arg(program);
    cmd
}

#[cfg(not(windows))]
pub fn shim_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}
