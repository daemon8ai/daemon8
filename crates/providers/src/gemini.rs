// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::json;

use super::ServiceIdentity;
use super::helpers::{
    HookSpec, current_exe_string, install_json_hooks, json_has_mcp_server, list_json_hooks,
    quote_command_path, remove_json_hooks, shim_command,
};
use super::traits::{
    AiProvider, HookEvent, HookEventEntry, HookProvider, HookScope, InstalledHookEntry, LogLevel,
};

pub struct GeminiProvider;

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
        event: HookEvent::ToolPre,
        native_name: "BeforeTool",
        severity: LogLevel::Debug,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::ToolPost,
        native_name: "AfterTool",
        severity: LogLevel::Debug,
        matcher: None,
    },
    HookEventEntry {
        event: HookEvent::PreCompact,
        native_name: "PreCompress",
        severity: LogLevel::Info,
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
        let ok = shim_command("gemini")
            .args([
                "mcp",
                "add",
                service.name,
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
            super::helpers::write_json_mcp_entries(config_path, &[(service.name, entry)])
        }
    }

    fn remove_mcp_config(&self, config_path: &Path, service: &ServiceIdentity) -> Result<bool> {
        let ok = shim_command("gemini")
            .args(["mcp", "remove", service.name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Ok(true);
        }
        super::helpers::remove_json_mcp_entry(config_path, service.name)
    }

    fn global_config_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".gemini"))
    }
    fn project_config_dir(&self) -> Option<&'static str> {
        Some(".gemini")
    }
    fn skills_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".gemini/skills"))
    }
    fn project_skills_dir(&self) -> Option<&'static str> {
        Some(".gemini/skills")
    }
    fn agents_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".gemini/agents"))
    }
    fn project_agents_dir(&self) -> Option<&'static str> {
        Some(".gemini/agents")
    }
    fn conversation_dir(&self, home: &Path) -> Option<PathBuf> {
        Some(home.join(".gemini/tmp"))
    }
    fn conversation_file_glob(&self) -> Option<&'static str> {
        Some("**/chats/session-*.jsonl")
    }
    fn session_id_from_env(&self) -> Option<String> {
        std::env::var("GEMINI_SESSION_ID").ok()
    }
    fn list_projects(&self, home: &Path) -> Vec<super::traits::ProjectEntry> {
        let projects_path = home.join(".gemini/projects.json");
        let Ok(content) = std::fs::read_to_string(&projects_path) else {
            return Vec::new();
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Vec::new();
        };
        let Some(projects) = root.get("projects").and_then(|v| v.as_object()) else {
            return Vec::new();
        };
        projects
            .iter()
            .filter_map(|(path_str, slug_val)| {
                let slug = slug_val.as_str().unwrap_or(path_str).to_string();
                let path = PathBuf::from(path_str);
                if !path.is_absolute() {
                    return None;
                }
                Some(super::traits::ProjectEntry {
                    slug,
                    path,
                    provider: "gemini",
                    last_active_ms: None,
                })
            })
            .collect()
    }
}

impl HookProvider for GeminiProvider {
    fn supported_scopes(&self) -> &'static [HookScope] {
        &[HookScope::Shared, HookScope::Global]
    }

    fn hooks_path(&self, scope: HookScope, cwd: &Path, home: &Path) -> PathBuf {
        match scope {
            HookScope::Local | HookScope::Shared => cwd.join(".gemini/settings.json"),
            HookScope::Global => home.join(".gemini/settings.json"),
        }
    }

    fn scope_display_hint(&self, scope: HookScope, _cwd: &Path, _home: &Path) -> String {
        match scope {
            HookScope::Local | HookScope::Shared => ".gemini/settings.json".into(),
            HookScope::Global => "~/.gemini/settings.json".into(),
        }
    }

    fn hook_events(&self) -> &'static [HookEventEntry] {
        HOOK_EVENTS
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_creates_hooks_with_gemini_events() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        let svc = crate::test_service();

        let path = GeminiProvider
            .install_hooks(HookScope::Global, &PathBuf::new(), &home, false, &svc)
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
    fn install_list_remove_global_round_trip() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        let svc = crate::test_service();

        GeminiProvider
            .install_hooks(HookScope::Global, &PathBuf::new(), &home, false, &svc)
            .unwrap();

        let entries = GeminiProvider
            .list_hooks(HookScope::Global, &PathBuf::new(), &home, &svc)
            .unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].command.contains("cli-hook --tool gemini-cli"));

        let removed = GeminiProvider
            .remove_hooks(HookScope::Global, &PathBuf::new(), &home, &svc)
            .unwrap();
        assert!(removed.is_some());

        let after = GeminiProvider
            .list_hooks(HookScope::Global, &PathBuf::new(), &home, &svc)
            .unwrap();
        assert!(after.is_empty());
    }

    #[test]
    fn supported_scopes_include_shared_and_global() {
        let scopes = GeminiProvider.supported_scopes();
        assert!(scopes.contains(&HookScope::Shared));
        assert!(scopes.contains(&HookScope::Global));
    }

    #[test]
    fn hooks_path_routes_by_scope() {
        let cwd = Path::new("/project");
        let home = Path::new("/tmp/daemon8-test-home");
        assert_eq!(
            GeminiProvider.hooks_path(HookScope::Shared, cwd, home),
            cwd.join(".gemini/settings.json")
        );
        assert_eq!(
            GeminiProvider.hooks_path(HookScope::Global, cwd, home),
            home.join(".gemini/settings.json")
        );
    }

    #[test]
    fn install_list_remove_shared_scope() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let home = tmp.path().join("fakehome");
        std::fs::create_dir_all(cwd.join(".gemini")).unwrap();
        let svc = crate::test_service();

        let path = GeminiProvider
            .install_hooks(HookScope::Shared, &cwd, &home, false, &svc)
            .unwrap();
        assert_eq!(path, cwd.join(".gemini/settings.json"));
        assert!(path.exists());

        let entries = GeminiProvider
            .list_hooks(HookScope::Shared, &cwd, &home, &svc)
            .unwrap();
        assert!(!entries.is_empty());

        let removed = GeminiProvider
            .remove_hooks(HookScope::Shared, &cwd, &home, &svc)
            .unwrap();
        assert!(removed.is_some());

        let after = GeminiProvider
            .list_hooks(HookScope::Shared, &cwd, &home, &svc)
            .unwrap();
        assert!(after.is_empty());
    }

    #[test]
    fn list_projects_parses_json() {
        let home = tempdir().unwrap();
        let gemini_dir = home.path().join(".gemini");
        std::fs::create_dir_all(&gemini_dir).unwrap();
        std::fs::write(
            gemini_dir.join("projects.json"),
            r#"{"projects":{"/tmp/myproject":"myproject","/tmp/other":"other"}}"#,
        )
        .unwrap();

        let entries = GeminiProvider.list_projects(home.path());
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.provider == "gemini"));
        assert!(entries.iter().any(|e| e.slug == "myproject"));
    }

    #[test]
    fn list_projects_missing_file() {
        let home = tempdir().unwrap();
        let entries = GeminiProvider.list_projects(home.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn list_projects_malformed_json() {
        let home = tempdir().unwrap();
        let gemini_dir = home.path().join(".gemini");
        std::fs::create_dir_all(&gemini_dir).unwrap();
        std::fs::write(gemini_dir.join("projects.json"), "not json").unwrap();

        let entries = GeminiProvider.list_projects(home.path());
        assert!(entries.is_empty());
    }
}
