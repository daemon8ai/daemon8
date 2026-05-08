// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use toml::Table;

use super::{current_exe_string, shim_command, Provider};

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

    let features = get_or_insert_table(root_table, "features")?;
    features.remove("codex_hooks");
    features.insert("hooks".to_string(), toml::Value::Boolean(true));

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
