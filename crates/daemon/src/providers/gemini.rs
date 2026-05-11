// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::Result;
use daemon8_types::Severity;
use serde_json::json;

use super::helpers::{
    HookSpec, current_exe_string, install_json_hooks, json_has_daemon8, list_json_hooks,
    quote_command_path, remove_json_hooks, shim_command,
};
use super::traits::{
    AiProvider, CanonicalEvent, HookProvider, HookScope, InstalledHookEntry, NormalizedHookEvent,
};

pub struct GeminiProvider;

static HOOK_EVENTS: &[NormalizedHookEvent] = &[
    NormalizedHookEvent {
        canonical: CanonicalEvent::SessionStart,
        native_name: "SessionStart",
        severity: Severity::Info,
        matcher: None,
    },
    NormalizedHookEvent {
        canonical: CanonicalEvent::SessionEnd,
        native_name: "SessionEnd",
        severity: Severity::Info,
        matcher: None,
    },
    NormalizedHookEvent {
        canonical: CanonicalEvent::ToolPre,
        native_name: "BeforeTool",
        severity: Severity::Debug,
        matcher: None,
    },
    NormalizedHookEvent {
        canonical: CanonicalEvent::ToolPost,
        native_name: "AfterTool",
        severity: Severity::Debug,
        matcher: None,
    },
    NormalizedHookEvent {
        canonical: CanonicalEvent::PreCompact,
        native_name: "PreCompress",
        severity: Severity::Info,
        matcher: None,
    },
];

impl AiProvider for GeminiProvider {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn label(&self) -> &'static str {
        "Gemini CLI"
    }

    fn restart_label(&self) -> &'static str {
        "restart Gemini CLI sessions"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["gemini", "gemini-cli"]
    }

    fn detect_dir(&self) -> &'static str {
        ".gemini"
    }

    fn session_env_vars(&self) -> &'static [&'static str] {
        &["GEMINI_SESSION_ID", "GEMINI_PROJECT_DIR"]
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".gemini/settings.json")
    }

    fn instruction_file_name(&self) -> &'static str {
        "GEMINI.md"
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
        let ok = shim_command("gemini")
            .args([
                "mcp",
                "add",
                "daemon8",
                mcp_url,
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
            let entry = json!({ "httpUrl": mcp_url });
            super::helpers::write_json_mcp_entries(config_path, &[("daemon8", entry)])
        }
    }

    fn remove_mcp_config(&self, config_path: &Path) -> Result<bool> {
        let ok = shim_command("gemini")
            .args(["mcp", "remove", "daemon8"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Ok(true);
        }
        super::helpers::remove_json_mcp_entry(config_path, "daemon8")
    }
}

impl HookProvider for GeminiProvider {
    fn supported_scopes(&self) -> &'static [HookScope] {
        &[HookScope::Global]
    }

    fn hooks_path(&self, _scope: HookScope, _cwd: &Path, home: &Path) -> PathBuf {
        home.join(".gemini/settings.json")
    }

    fn hook_events(&self) -> &'static [NormalizedHookEvent] {
        HOOK_EVENTS
    }

    fn install_hooks(
        &self,
        scope: HookScope,
        cwd: &Path,
        home: &Path,
        force: bool,
    ) -> Result<PathBuf> {
        let settings_path = self.hooks_path(scope, cwd, home);
        let command = format!(
            "{} cli-hook --tool gemini-cli",
            quote_command_path(&current_exe_string())
        );
        let specs: Vec<HookSpec> = HOOK_EVENTS
            .iter()
            .map(|e| HookSpec {
                event: e.native_name,
                matcher: e.matcher,
                timeout: Some(10000),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_creates_hooks_with_gemini_events() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();

        let path = GeminiProvider
            .install_hooks(HookScope::Global, &PathBuf::new(), &home, false)
            .unwrap();
        assert!(path.exists());

        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = content.get("hooks").and_then(|h| h.as_object()).unwrap();
        for event in [
            "SessionStart",
            "SessionEnd",
            "BeforeTool",
            "AfterTool",
            "PreCompress",
        ] {
            assert!(hooks.contains_key(event), "missing Gemini event {event}");
        }
    }

    #[test]
    fn install_list_remove_round_trip() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();

        GeminiProvider
            .install_hooks(HookScope::Global, &PathBuf::new(), &home, false)
            .unwrap();

        let entries = GeminiProvider
            .list_hooks(HookScope::Global, &PathBuf::new(), &home)
            .unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].command.contains("cli-hook --tool gemini-cli"));

        let removed = GeminiProvider
            .remove_hooks(HookScope::Global, &PathBuf::new(), &home)
            .unwrap();
        assert!(removed.is_some());

        let after = GeminiProvider
            .list_hooks(HookScope::Global, &PathBuf::new(), &home)
            .unwrap();
        assert!(after.is_empty());
    }
}
