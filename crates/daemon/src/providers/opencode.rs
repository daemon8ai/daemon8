// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use super::traits::{AiProvider, ProviderDocs};

pub struct OpenCodeProvider;

static DOCS: ProviderDocs = ProviderDocs {
    hooks: Some("https://opencode.ai/docs/plugins/"),
    mcp: Some("https://opencode.ai/docs/mcp-servers/"),
    config: Some("https://opencode.ai/docs/config/"),
    instructions: Some("https://opencode.ai/docs/rules/"),
};

impl AiProvider for OpenCodeProvider {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn restart_label(&self) -> &'static str {
        "restart OpenCode sessions"
    }

    fn detect_dir(&self) -> &'static str {
        ".config/opencode"
    }

    fn session_env_vars(&self) -> &'static [&'static str] {
        &["OPENCODE"]
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".config/opencode/opencode.json")
    }

    fn project_config_path(&self, project: &Path) -> Option<PathBuf> {
        Some(project.join("opencode.json"))
    }

    fn instruction_file_name(&self) -> &'static str {
        "AGENTS.md"
    }

    fn is_configured(&self, config_path: &Path) -> bool {
        opencode_has_daemon8(config_path)
    }

    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        _project_dir: Option<&Path>,
    ) -> Result<()> {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut root = if config_path.exists() {
            let contents = std::fs::read_to_string(config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            serde_json::from_str::<serde_json::Value>(&contents)
                .with_context(|| format!("failed to parse {}", config_path.display()))?
        } else {
            json!({})
        };

        let root_obj = root
            .as_object_mut()
            .context("opencode config must be a JSON object")?;
        let mcp = root_obj
            .entry("mcp".to_string())
            .or_insert_with(|| json!({}));
        let mcp_obj = mcp.as_object_mut().context("mcp must be a JSON object")?;
        mcp_obj.insert(
            "daemon8".to_string(),
            json!({ "type": "remote", "url": mcp_url }),
        );

        use std::io::Write;
        let tmp = config_path.with_extension("tmp");
        let content = serde_json::to_string_pretty(&root)?;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, config_path)?;
        Ok(())
    }

    fn remove_mcp_config(&self, config_path: &Path) -> Result<bool> {
        if !config_path.exists() {
            return Ok(false);
        }
        let contents = std::fs::read_to_string(config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let mut root: serde_json::Value = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?;

        let removed = root
            .get_mut("mcp")
            .and_then(serde_json::Value::as_object_mut)
            .map(|servers| servers.remove("daemon8").is_some())
            .unwrap_or(false);

        if removed {
            use std::io::Write;
            let tmp = config_path.with_extension("tmp");
            let content = serde_json::to_string_pretty(&root)?;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, config_path)?;
        }
        Ok(removed)
    }

    fn docs(&self) -> &'static ProviderDocs {
        &DOCS
    }
}

fn opencode_has_daemon8(config_path: &Path) -> bool {
    config_path.exists()
        && std::fs::read_to_string(config_path)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|v| v.get("mcp")?.as_object().map(|m| m.contains_key("daemon8")))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_mcp_config_creates_mcp_key() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None)
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        let daemon8 = content
            .get("mcp")
            .and_then(|m| m.get("daemon8"))
            .expect("mcp.daemon8 key");
        assert_eq!(daemon8["type"], "remote");
        assert_eq!(daemon8["url"], "http://127.0.0.1:8371/mcp");
    }

    #[test]
    fn write_preserves_existing_entries() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        std::fs::write(
            &config,
            r#"{"mcp":{"other-server":{"type":"stdio","command":"foo"}}}"#,
        )
        .unwrap();

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None)
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert!(content["mcp"]["other-server"].is_object());
        assert!(content["mcp"]["daemon8"].is_object());
    }

    #[test]
    fn remove_mcp_config_deletes_daemon8_entry() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None)
            .unwrap();
        assert!(opencode_has_daemon8(&config));

        let removed = OpenCodeProvider.remove_mcp_config(&config).unwrap();
        assert!(removed);
        assert!(!opencode_has_daemon8(&config));
    }

    #[test]
    fn remove_returns_false_when_not_present() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        std::fs::write(&config, r#"{"mcp":{}}"#).unwrap();

        let removed = OpenCodeProvider.remove_mcp_config(&config).unwrap();
        assert!(!removed);
    }

    #[test]
    fn is_configured_detects_daemon8() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");

        assert!(!OpenCodeProvider.is_configured(&config));

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None)
            .unwrap();
        assert!(OpenCodeProvider.is_configured(&config));
    }
}
