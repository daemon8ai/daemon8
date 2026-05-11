// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use super::ServiceIdentity;
use super::traits::AiProvider;

pub struct OpenCodeProvider;

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

    fn aliases(&self) -> &'static [&'static str] {
        &["opencode"]
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

    fn instruction_file_name(&self) -> &'static str {
        "AGENTS.md"
    }

    fn is_configured(&self, config_path: &Path, service: &ServiceIdentity) -> bool {
        opencode_has_mcp_server(config_path, service.name)
    }

    fn write_mcp_config(
        &self,
        config_path: &Path,
        mcp_url: &str,
        _project_dir: Option<&Path>,
        service: &ServiceIdentity,
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
            service.name.to_string(),
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

    fn remove_mcp_config(&self, config_path: &Path, service: &ServiceIdentity) -> Result<bool> {
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
            .map(|servers| servers.remove(service.name).is_some())
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
}

fn opencode_has_mcp_server(config_path: &Path, name: &str) -> bool {
    config_path.exists()
        && std::fs::read_to_string(config_path)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|v| v.get("mcp")?.as_object().map(|m| m.contains_key(name)))
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
        let svc = crate::test_service();

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None, &svc)
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        let entry = content
            .get("mcp")
            .and_then(|m| m.get(svc.name))
            .expect("mcp.test-svc key");
        assert_eq!(entry["type"], "remote");
        assert_eq!(entry["url"], "http://127.0.0.1:8371/mcp");
    }

    #[test]
    fn write_preserves_existing_entries() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        let svc = crate::test_service();
        std::fs::write(
            &config,
            r#"{"mcp":{"other-server":{"type":"stdio","command":"foo"}}}"#,
        )
        .unwrap();

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None, &svc)
            .unwrap();

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert!(content["mcp"]["other-server"].is_object());
        assert!(content["mcp"][svc.name].is_object());
    }

    #[test]
    fn remove_mcp_config_deletes_entry() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        let svc = crate::test_service();

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None, &svc)
            .unwrap();
        assert!(opencode_has_mcp_server(&config, svc.name));

        let removed = OpenCodeProvider.remove_mcp_config(&config, &svc).unwrap();
        assert!(removed);
        assert!(!opencode_has_mcp_server(&config, svc.name));
    }

    #[test]
    fn remove_returns_false_when_not_present() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        let svc = crate::test_service();
        std::fs::write(&config, r#"{"mcp":{}}"#).unwrap();

        let removed = OpenCodeProvider.remove_mcp_config(&config, &svc).unwrap();
        assert!(!removed);
    }

    #[test]
    fn is_configured_detects_service() {
        let tmp = tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        let svc = crate::test_service();

        assert!(!OpenCodeProvider.is_configured(&config, &svc));

        OpenCodeProvider
            .write_mcp_config(&config, "http://127.0.0.1:8371/mcp", None, &svc)
            .unwrap();
        assert!(OpenCodeProvider.is_configured(&config, &svc));
    }
}
