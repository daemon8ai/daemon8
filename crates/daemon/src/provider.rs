// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use toml::Table;

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

#[derive(Debug, Clone, Copy)]
struct HookSpec {
    event: &'static str,
    matcher: Option<&'static str>,
    status_message: Option<&'static str>,
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

pub fn write_provider_config(
    provider: Provider,
    config_path: &Path,
    project_dir: Option<&Path>,
) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let port = crate::config::load(None).unwrap_or_default().server.port;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");

    match provider {
        Provider::Codex => write_codex_config(config_path, &mcp_url, project_dir),
        Provider::Gemini => {
            let ok = shim_command("gemini")
                .args([
                    "mcp",
                    "add",
                    "daemon8",
                    &mcp_url,
                    "--transport",
                    "http",
                    "--scope",
                    "user",
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                Ok(())
            } else {
                write_json_mcp_config(provider, config_path, &mcp_url)
            }
        }
        Provider::ClaudeCode => {
            let ok = shim_command("claude")
                .args([
                    "mcp",
                    "add",
                    "--scope",
                    "user",
                    "--transport",
                    "http",
                    "daemon8",
                    &mcp_url,
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if ok {
                let _ = shim_command("claude")
                    .args([
                        "mcp",
                        "add",
                        "--scope",
                        "user",
                        "--transport",
                        "stdio",
                        "daemon8-channel",
                        current_exe_string().as_str(),
                        "--",
                        "channel",
                    ])
                    .status();
                Ok(())
            } else {
                write_json_mcp_config(provider, config_path, &mcp_url)
            }
        }
        Provider::Cursor | Provider::Windsurf => {
            write_json_mcp_config(provider, config_path, &mcp_url)
        }
    }
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

fn write_json_mcp_config(provider: Provider, config_path: &Path, mcp_url: &str) -> Result<()> {
    let daemon8_entry = if provider == Provider::Gemini {
        json!({ "httpUrl": mcp_url })
    } else {
        json!({ "type": "http", "url": mcp_url })
    };
    let channel_entry = json!({
        "command": current_exe_string(),
        "args": ["channel"],
    });
    let include_channel = provider == Provider::ClaudeCode;

    let mut root = if config_path.exists() {
        let contents = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        serde_json::from_str::<Value>(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?
    } else {
        json!({})
    };

    let root_obj = root
        .as_object_mut()
        .context("provider config must be a JSON object")?;
    let servers = root_obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    let servers_obj = servers
        .as_object_mut()
        .context("mcpServers must be a JSON object")?;
    servers_obj.insert("daemon8".to_string(), daemon8_entry);
    if include_channel {
        servers_obj.insert("daemon8-channel".to_string(), channel_entry);
    }

    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, config_path)?;
    Ok(())
}

fn write_codex_config(config_path: &Path, mcp_url: &str, project_dir: Option<&Path>) -> Result<()> {
    let mut root = if config_path.exists() {
        let contents = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        toml::from_str::<toml::Value>(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?
    } else {
        toml::Value::Table(Table::new())
    };

    let root_table = root
        .as_table_mut()
        .context("codex config root must be a table")?;

    let mcp_servers = get_or_insert_table(root_table, "mcp_servers")?;
    let daemon8 = mcp_servers
        .entry("daemon8".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    let daemon8_table = daemon8
        .as_table_mut()
        .context("mcp_servers.daemon8 must be a table")?;
    daemon8_table.insert("name".to_string(), toml::Value::String("Daemon8".into()));
    daemon8_table.insert("url".to_string(), toml::Value::String(mcp_url.to_string()));

    if let Some(project_dir) = project_dir {
        let projects = get_or_insert_table(root_table, "projects")?;
        let key = project_dir.to_string_lossy().to_string();
        let project_entry = projects
            .entry(key)
            .or_insert_with(|| toml::Value::Table(Table::new()));
        let project_table = project_entry
            .as_table_mut()
            .context("projects entry must be a table")?;
        project_table.insert(
            "trust_level".to_string(),
            toml::Value::String("trusted".into()),
        );
    }

    let tmp = config_path.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, config_path)?;
    Ok(())
}

fn get_or_insert_table<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    let value = root
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    value
        .as_table_mut()
        .with_context(|| format!("{key} must be a TOML table"))
}

fn current_exe_string() -> String {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("daemon8"))
        .to_string_lossy()
        .to_string()
}

fn json_has_daemon8(config_path: &Path) -> bool {
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

fn codex_has_daemon8(config_path: &Path) -> bool {
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

pub fn provider_map(items: &[DetectedProvider]) -> BTreeMap<Provider, DetectedProvider> {
    let mut map = BTreeMap::new();
    for item in items {
        map.insert(item.provider, item.clone());
    }
    map
}
