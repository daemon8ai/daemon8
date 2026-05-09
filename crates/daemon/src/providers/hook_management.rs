// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Cross-provider hook management surface used by both the CLI subcommand
//! and the MCP tools. Wraps the per-provider install/remove/update/list
//! primitives in `providers::hooks` and ranges over Claude Code (Local /
//! Shared / Global scopes) and Codex (single global scope). Cursor /
//! Windsurf / Gemini do not currently expose a hook callback surface this
//! daemon enrolls in; their MCP server registration is handled separately
//! via `providers::config::write_provider_config`.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use super::{
    HookScope, InstalledHookEntry, dirs_home, install_claude_hooks, install_codex_hooks,
    list_claude_hooks, list_codex_hooks, remove_claude_hooks, remove_codex_hooks,
    update_claude_hooks, update_codex_hooks,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSurface {
    ClaudeCode,
    Codex,
}

impl HookSurface {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claude_code" => Ok(Self::ClaudeCode),
            "codex" | "codex-cli" => Ok(Self::Codex),
            other => bail!("unknown hook surface '{other}' (valid: claude, codex)"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledHookGroup {
    pub provider: &'static str,
    pub scope: Option<&'static str>,
    pub settings_path: PathBuf,
    pub entries: Vec<InstalledHookEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookActionReport {
    pub provider: &'static str,
    pub scope: Option<&'static str>,
    pub action: &'static str,
    pub settings_path: Option<PathBuf>,
}

pub fn list_all_hooks(cwd: &Path) -> Result<Vec<InstalledHookGroup>> {
    let home = dirs_home();
    let mut out = Vec::new();
    for scope in [HookScope::Local, HookScope::Shared, HookScope::Global] {
        let entries = list_claude_hooks(scope, cwd, &home)?;
        if entries.is_empty() {
            continue;
        }
        out.push(InstalledHookGroup {
            provider: HookSurface::ClaudeCode.label(),
            scope: Some(scope_label(scope)),
            settings_path: scope.settings_path(cwd, &home),
            entries,
        });
    }
    let codex = list_codex_hooks(&home)?;
    if !codex.is_empty() {
        out.push(InstalledHookGroup {
            provider: HookSurface::Codex.label(),
            scope: None,
            settings_path: home.join(".codex/hooks.json"),
            entries: codex,
        });
    }
    Ok(out)
}

pub fn remove_hooks(
    surface: HookSurface,
    scope: Option<HookScope>,
    cwd: &Path,
) -> Result<Vec<HookActionReport>> {
    let home = dirs_home();
    match surface {
        HookSurface::ClaudeCode => {
            let scopes = match scope {
                Some(s) => vec![s],
                None => vec![HookScope::Local, HookScope::Shared, HookScope::Global],
            };
            let mut reports = Vec::new();
            for s in scopes {
                let path = remove_claude_hooks(s, cwd, &home)?;
                reports.push(HookActionReport {
                    provider: HookSurface::ClaudeCode.label(),
                    scope: Some(scope_label(s)),
                    action: if path.is_some() { "removed" } else { "noop" },
                    settings_path: path,
                });
            }
            Ok(reports)
        }
        HookSurface::Codex => {
            let path = remove_codex_hooks(&home)?;
            Ok(vec![HookActionReport {
                provider: HookSurface::Codex.label(),
                scope: None,
                action: if path.is_some() { "removed" } else { "noop" },
                settings_path: path,
            }])
        }
    }
}

pub fn update_hooks(
    surface: HookSurface,
    scope: Option<HookScope>,
    cwd: &Path,
) -> Result<Vec<HookActionReport>> {
    let home = dirs_home();
    match surface {
        HookSurface::ClaudeCode => {
            let scopes = match scope {
                Some(s) => vec![s],
                None => vec![HookScope::Local, HookScope::Shared, HookScope::Global],
            };
            let mut reports = Vec::new();
            for s in scopes {
                let path = update_claude_hooks(s, cwd, &home)?;
                reports.push(HookActionReport {
                    provider: HookSurface::ClaudeCode.label(),
                    scope: Some(scope_label(s)),
                    action: "updated",
                    settings_path: Some(path),
                });
            }
            Ok(reports)
        }
        HookSurface::Codex => {
            let path = update_codex_hooks(&home)?;
            Ok(vec![HookActionReport {
                provider: HookSurface::Codex.label(),
                scope: None,
                action: "updated",
                settings_path: Some(path),
            }])
        }
    }
}

/// Scan installed hooks for drift (entries whose command doesn't match the
/// running daemon binary path) and reinstall those scopes. Pure no-op when
/// no drift is detected.
pub fn repair_hooks(cwd: &Path) -> Result<Vec<HookActionReport>> {
    let home = dirs_home();
    let current_cmd_marker = super::current_exe_string();
    let mut reports = Vec::new();

    for scope in [HookScope::Local, HookScope::Shared, HookScope::Global] {
        let entries = list_claude_hooks(scope, cwd, &home)?;
        if entries.is_empty() {
            continue;
        }
        let drifted = entries
            .iter()
            .any(|e| !e.command.contains(&current_cmd_marker));
        if drifted {
            let path = install_claude_hooks(scope, cwd, &home, true)?;
            reports.push(HookActionReport {
                provider: HookSurface::ClaudeCode.label(),
                scope: Some(scope_label(scope)),
                action: "repaired",
                settings_path: Some(path),
            });
        } else {
            reports.push(HookActionReport {
                provider: HookSurface::ClaudeCode.label(),
                scope: Some(scope_label(scope)),
                action: "ok",
                settings_path: Some(scope.settings_path(cwd, &home)),
            });
        }
    }

    let codex = list_codex_hooks(&home)?;
    if !codex.is_empty() {
        let drifted = codex
            .iter()
            .any(|e| !e.command.contains(&current_cmd_marker));
        if drifted {
            let path = install_codex_hooks(&home, true)?;
            reports.push(HookActionReport {
                provider: HookSurface::Codex.label(),
                scope: None,
                action: "repaired",
                settings_path: Some(path),
            });
        } else {
            reports.push(HookActionReport {
                provider: HookSurface::Codex.label(),
                scope: None,
                action: "ok",
                settings_path: Some(home.join(".codex/hooks.json")),
            });
        }
    }

    Ok(reports)
}

pub fn parse_scope(raw: &str) -> Result<HookScope> {
    match raw.to_ascii_lowercase().as_str() {
        "local" => Ok(HookScope::Local),
        "shared" | "project" => Ok(HookScope::Shared),
        "global" | "user" => Ok(HookScope::Global),
        other => bail!("unknown scope '{other}' (valid: local, shared, global)"),
    }
}

pub fn scope_label(scope: HookScope) -> &'static str {
    match scope {
        HookScope::Local => "local",
        HookScope::Shared => "shared",
        HookScope::Global => "global",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_then_remove_round_trip() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        // Use a dedicated home so we don't touch user config.
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        let home = dirs_home();

        install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();
        let listed = list_all_hooks(&cwd).unwrap();
        assert!(
            listed
                .iter()
                .any(|g| g.provider == "claude-code" && g.scope == Some("local"))
        );

        let removed = remove_hooks(HookSurface::ClaudeCode, Some(HookScope::Local), &cwd).unwrap();
        assert_eq!(removed[0].action, "removed");

        let after = list_all_hooks(&cwd).unwrap();
        assert!(after.iter().all(|g| g.scope != Some("local")));
    }

    #[test]
    fn repair_reports_ok_when_command_matches() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        let home = dirs_home();

        install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();
        let reports = repair_hooks(&cwd).unwrap();
        let local = reports
            .iter()
            .find(|r| r.scope == Some("local"))
            .expect("local report present");
        assert_eq!(local.action, "ok");
    }
}
