// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Cross-provider hook management surface. Iterates over all providers
//! that implement `HookProvider` and delegates install/remove/update/list
//! operations through the trait.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use super::traits::{HookScope, InstalledHookEntry};
use super::{HOOK_PROVIDERS, Provider, dirs_home, helpers};

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

    for &provider in HOOK_PROVIDERS {
        let hp = provider.as_hook_provider().unwrap();
        for &scope in hp.supported_scopes() {
            let entries = hp.list_hooks(scope, cwd, &home)?;
            if entries.is_empty() {
                continue;
            }
            out.push(InstalledHookGroup {
                provider: hp.label(),
                scope: Some(scope_label(scope)),
                settings_path: hp.hooks_path(scope, cwd, &home),
                entries,
            });
        }
    }

    Ok(out)
}

pub fn remove_hooks(
    provider: Provider,
    scope: Option<HookScope>,
    cwd: &Path,
) -> Result<Vec<HookActionReport>> {
    let home = dirs_home();
    let Some(hp) = provider.as_hook_provider() else {
        bail!("{} does not support hooks", provider.label());
    };

    let scopes = match scope {
        Some(s) => vec![s],
        None => hp.supported_scopes().to_vec(),
    };

    let mut reports = Vec::new();
    for s in scopes {
        let path = hp.remove_hooks(s, cwd, &home)?;
        reports.push(HookActionReport {
            provider: hp.label(),
            scope: Some(scope_label(s)),
            action: if path.is_some() { "removed" } else { "noop" },
            settings_path: path,
        });
    }
    Ok(reports)
}

pub fn update_hooks(
    provider: Provider,
    scope: Option<HookScope>,
    cwd: &Path,
) -> Result<Vec<HookActionReport>> {
    let home = dirs_home();
    let Some(hp) = provider.as_hook_provider() else {
        bail!("{} does not support hooks", provider.label());
    };

    let scopes = match scope {
        Some(s) => vec![s],
        None => hp.supported_scopes().to_vec(),
    };

    let mut reports = Vec::new();
    for s in scopes {
        let path = hp.update_hooks(s, cwd, &home)?;
        reports.push(HookActionReport {
            provider: hp.label(),
            scope: Some(scope_label(s)),
            action: "updated",
            settings_path: Some(path),
        });
    }
    Ok(reports)
}

pub fn repair_hooks(cwd: &Path) -> Result<Vec<HookActionReport>> {
    let home = dirs_home();
    let current_cmd_marker = helpers::current_exe_string();
    let mut reports = Vec::new();

    for &provider in HOOK_PROVIDERS {
        let hp = provider.as_hook_provider().unwrap();
        for &scope in hp.supported_scopes() {
            let entries = hp.list_hooks(scope, cwd, &home)?;
            if entries.is_empty() {
                continue;
            }
            let drifted = entries
                .iter()
                .any(|e| !e.command.contains(&current_cmd_marker));
            if drifted {
                let path = hp.install_hooks(scope, cwd, &home, true)?;
                reports.push(HookActionReport {
                    provider: hp.label(),
                    scope: Some(scope_label(scope)),
                    action: "repaired",
                    settings_path: Some(path),
                });
            } else {
                reports.push(HookActionReport {
                    provider: hp.label(),
                    scope: Some(scope_label(scope)),
                    action: "ok",
                    settings_path: Some(hp.hooks_path(scope, cwd, &home)),
                });
            }
        }
    }

    Ok(reports)
}

pub fn parse_hook_provider(raw: &str) -> Result<Provider> {
    match raw.to_ascii_lowercase().as_str() {
        "claude" | "claude-code" | "claude_code" => Ok(Provider::ClaudeCode),
        "codex" | "codex-cli" => Ok(Provider::Codex),
        "gemini" | "gemini-cli" => Ok(Provider::Gemini),
        other => bail!("unknown hook provider '{other}' (valid: claude, codex, gemini)"),
    }
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
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        let home = dirs_home();

        super::super::install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();
        let listed = list_all_hooks(&cwd).unwrap();
        assert!(
            listed
                .iter()
                .any(|g| g.provider == "Claude Code" && g.scope == Some("local"))
        );

        let removed = remove_hooks(Provider::ClaudeCode, Some(HookScope::Local), &cwd).unwrap();
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

        super::super::install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();
        let reports = repair_hooks(&cwd).unwrap();
        let local = reports
            .iter()
            .find(|r| r.scope == Some("local"))
            .expect("local report present");
        assert_eq!(local.action, "ok");
    }

    #[test]
    fn repair_reinstalls_when_command_drifted() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        let home = dirs_home();

        super::super::install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();

        let settings_path = cwd.join(".claude/settings.local.json");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        let tampered = content.replace(&helpers::current_exe_string(), "/old/path/daemon8");
        std::fs::write(&settings_path, tampered).unwrap();

        let reports = repair_hooks(&cwd).unwrap();
        let local = reports
            .iter()
            .find(|r| r.scope == Some("local"))
            .expect("local report present");
        assert_eq!(local.action, "repaired");

        let after = std::fs::read_to_string(&settings_path).unwrap();
        assert!(after.contains(&helpers::current_exe_string()));
    }

    #[test]
    fn update_hooks_reinstalls_existing() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        let home = dirs_home();

        super::super::install_claude_hooks(HookScope::Local, &cwd, &home, false).unwrap();
        let reports = update_hooks(Provider::ClaudeCode, Some(HookScope::Local), &cwd).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].action, "updated");
        assert!(reports[0].settings_path.is_some());
    }
}
