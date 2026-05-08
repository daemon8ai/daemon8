// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration tests for `daemon8 init`.
//!
//! Exercise the compiled binary end-to-end: clap parsing, dispatch,
//! `.daemon8.toml` generation, and hook registration into
//! `.claude/settings.{local.,}json` / `~/.claude/settings.json` (via fake HOME).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_daemon8"))
}

/// Run `daemon8 init ...` with:
///   - cwd set to `dir`
///   - stdin = /dev/null (no TTY, forces non-interactive)
///   - HOME overridden to `fake_home` so Global scope can't touch the real home
fn run_init(dir: &Path, fake_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .arg("init")
        .args(args)
        .current_dir(dir)
        .env("HOME", fake_home)
        .env_remove("CI")
        .stdin(Stdio::null())
        .output()
        .expect("spawn daemon8 init")
}

fn run_init_with_env(
    dir: &Path,
    fake_home: &Path,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(binary());
    cmd.arg("init")
        .args(args)
        .current_dir(dir)
        .env("HOME", fake_home)
        .stdin(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn daemon8 init")
}

fn run_daemon8(args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn daemon8")
}

fn run_setup(
    dir: &Path,
    fake_home: &Path,
    config_path: &Path,
    args: &[&str],
) -> std::process::Output {
    Command::new(binary())
        .arg("--config")
        .arg(config_path)
        .arg("setup")
        .args(args)
        .current_dir(dir)
        .env("HOME", fake_home)
        .env_remove("CI")
        .stdin(Stdio::null())
        .output()
        .expect("spawn daemon8 setup")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).expect("read target json");
    serde_json::from_str(&text).expect("parse target json")
}

fn hooks_for<'a>(root: &'a Value, event: &str) -> &'a Vec<Value> {
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|e| e.as_array())
        .unwrap_or_else(|| panic!("missing hooks.{event}"))
}

fn commands_under(root: &Value, event: &str) -> Vec<String> {
    hooks_for(root, event)
        .iter()
        .filter_map(|group| {
            group
                .get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|arr| arr.first())
                .and_then(|h| h.get("command"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

fn setup_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    let workdir = tmp.path().join("work");
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();
    (tmp, workdir, fake_home)
}

fn codex_hooks_path(home: &Path) -> PathBuf {
    home.join(".codex").join("hooks.json")
}

fn codex_config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

// ---------------------------------------------------------------------------

#[test]
fn cli_yes_writes_toml_only() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(work.join(".daemon8.toml").exists());
    let toml = std::fs::read_to_string(work.join(".daemon8.toml")).unwrap();
    assert!(!toml.contains("role_default"));
    assert!(
        !work.join(".claude").exists(),
        ".claude must NOT be created without --install-hooks"
    );
    assert!(
        !work.join(".codex").exists(),
        ".codex must NOT be created without provider selection"
    );
}

#[test]
fn cli_yes_with_install_hooks_local_writes_both() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes", "--install-hooks", "local"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(work.join(".daemon8.toml").exists());
    let settings = work.join(".claude").join("settings.local.json");
    assert!(settings.exists(), "local settings file missing");

    let parsed = read_json(&settings);
    // All 7 default events registered.
    let events = parsed
        .get("hooks")
        .and_then(|h| h.as_object())
        .expect("hooks obj");
    for expected in [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PreCompact",
        "Stop",
    ] {
        assert!(events.contains_key(expected), "missing event {expected}");
    }

    // Command is absolute and ends with ` cli-hook`.
    let cmds = commands_under(&parsed, "SessionStart");
    assert_eq!(cmds.len(), 1);
    let cmd = &cmds[0];
    assert!(cmd.ends_with(" cli-hook"), "actual: {cmd}");
    assert!(
        Path::new(cmd.split(' ').next().unwrap()).is_absolute(),
        "expected absolute binary path, got {cmd}"
    );
}

#[test]
fn cli_agent_command_is_removed() {
    let out = run_daemon8(&["agent", "--help"]);
    assert!(!out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized subcommand") || stderr.contains("unexpected argument"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_yes_install_hooks_shared_writes_project_settings_json() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes", "--install-hooks", "shared"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let shared = work.join(".claude").join("settings.json");
    let local = work.join(".claude").join("settings.local.json");
    assert!(shared.exists(), "shared settings must exist");
    assert!(
        !local.exists(),
        "local settings must NOT exist under --install-hooks=shared"
    );
}

#[test]
fn cli_yes_install_hooks_global_targets_fake_home() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes", "--install-hooks", "global"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let global = home.join(".claude").join("settings.json");
    assert!(global.exists(), "global settings in fake home must exist");
    // Project .claude must NOT have been touched at all.
    assert!(
        !work.join(".claude").exists(),
        "project .claude must not exist for global scope"
    );
}

#[test]
fn cli_yes_with_codex_provider_writes_codex_config_and_hooks() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes", "--providers", "codex-cli"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let config = codex_config_path(&home);
    let hooks = codex_hooks_path(&home);
    assert!(config.exists(), "codex config missing");
    assert!(hooks.exists(), "codex hooks missing");

    let config_toml = std::fs::read_to_string(&config).unwrap();
    let config_parsed: toml::Value = toml::from_str(&config_toml).unwrap();
    assert_eq!(config_parsed["features"]["hooks"].as_bool(), Some(true));
    assert!(
        config_parsed["features"].get("codex_hooks").is_none(),
        "deprecated codex_hooks flag must not be written"
    );

    let parsed = read_json(&hooks);
    let events = parsed
        .get("hooks")
        .and_then(|h| h.as_object())
        .expect("hooks obj");
    for expected in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "Stop",
    ] {
        assert!(events.contains_key(expected), "missing event {expected}");
    }
    let session = &hooks_for(&parsed, "SessionStart")[0];
    assert_eq!(
        session.get("matcher").and_then(Value::as_str),
        Some("startup|resume")
    );
}

#[test]
fn cli_yes_with_codex_provider_migrates_deprecated_hook_feature() {
    let (_tmp, work, home) = setup_dirs();
    let config = codex_config_path(&home);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        r#"
[features]
codex_hooks = true
"#,
    )
    .unwrap();

    let out = run_init(&work, &home, &["--yes", "--providers", "codex-cli"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let config_toml = std::fs::read_to_string(&config).unwrap();
    let parsed: toml::Value = toml::from_str(&config_toml).unwrap();
    assert_eq!(parsed["features"]["hooks"].as_bool(), Some(true));
    assert!(
        parsed["features"].get("codex_hooks").is_none(),
        "deprecated codex_hooks flag must be removed"
    );
}

#[test]
fn cli_noninteractive_when_stdin_not_tty() {
    // No --yes, no CI var, but stdin is /dev/null via Stdio::null() → must not hang
    // and must write defaults. (If it tried to prompt, it would either hang or fail.)
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(work.join(".daemon8.toml").exists());
}

#[test]
fn cli_ci_env_forces_noninteractive() {
    // stdin is /dev/null already, but belt-and-suspenders: with CI=1 we must also
    // skip prompting (important if someone runs the binary from an interactive
    // terminal inside a CI container image).
    let (_tmp, work, home) = setup_dirs();
    let out = run_init_with_env(&work, &home, &[("CI", "1")], &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(work.join(".daemon8.toml").exists());
}

#[test]
fn cli_skips_existing_toml_without_force() {
    let (_tmp, work, home) = setup_dirs();
    std::fs::write(work.join(".daemon8.toml"), "# pre-existing\n").unwrap();

    let out = run_init(&work, &home, &["--yes"]);
    assert!(
        out.status.success(),
        "expected success (graceful skip); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let preserved = std::fs::read_to_string(work.join(".daemon8.toml")).unwrap();
    assert_eq!(
        preserved, "# pre-existing\n",
        "file must be untouched when --force is not set"
    );
}

#[test]
fn setup_status_and_plan_are_read_only() {
    let (_tmp, work, home) = setup_dirs();
    let config_path = work.join("global-config.toml");
    std::fs::write(
        work.join(".daemon8.toml"),
        r#"
[project]
slug = "demo"

[sources.app]
type = "file"
path = "logs/app.log"
parser = "line"
"#,
    )
    .unwrap();

    let status = run_setup(&work, &home, &config_path, &["status", "--json"]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(
        !config_path.exists(),
        "status must not create global config"
    );
    let parsed: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(parsed["project"]["slug"], "demo");

    let plan = run_setup(&work, &home, &config_path, &["plan", "--json"]);
    assert!(
        plan.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    assert!(!config_path.exists(), "plan must not create global config");
    let parsed: Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(parsed["actions"][0]["kind"], "source-register");
}

#[test]
fn setup_apply_registers_runtime_sources_and_state() {
    let (_tmp, work, home) = setup_dirs();
    let config_path = work.join("global-config.toml");
    std::fs::create_dir_all(work.join("logs")).unwrap();
    std::fs::write(work.join("logs/app.log"), "").unwrap();
    std::fs::write(
        work.join(".daemon8.toml"),
        r#"
[project]
slug = "demo"

[sources.app]
type = "file"
path = "logs/app.log"
parser = "line"
tags = ["app"]
"#,
    )
    .unwrap();

    let out = run_setup(&work, &home, &config_path, &["apply", "--yes", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let global = std::fs::read_to_string(&config_path).unwrap();
    let parsed: toml::Value = toml::from_str(&global).unwrap();
    let source = &parsed["sources"]["demo.app"];
    assert_eq!(source["type"].as_str(), Some("file"));
    assert!(source["path"].as_str().unwrap().ends_with("logs/app.log"));
    assert!(
        source["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag.as_str() == Some("project:demo"))
    );
    assert_eq!(
        parsed["setup"]["projects"]["demo"]["slug"].as_str(),
        Some("demo")
    );
}

#[test]
fn cli_force_overwrites_existing_toml() {
    let (_tmp, work, home) = setup_dirs();
    std::fs::write(work.join(".daemon8.toml"), "# pre-existing\n").unwrap();

    let out = run_init(&work, &home, &["--yes", "--force"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let content = std::fs::read_to_string(work.join(".daemon8.toml")).unwrap();
    assert!(
        content.contains("[project]"),
        "template should replace the pre-existing stub"
    );
}

#[test]
fn cli_install_hooks_preserves_existing_user_hook() {
    let (_tmp, work, home) = setup_dirs();
    // Pre-populate .claude/settings.local.json with an unrelated user hook +
    // permissions key we want preserved.
    let claude_dir = work.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let existing = serde_json::json!({
        "permissions": { "allow": ["Bash(ls)"] },
        "hooks": {
            "PreToolUse": [
                { "hooks": [{ "type": "command", "command": "my-formatter" }] }
            ]
        }
    });
    std::fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    let out = run_init(&work, &home, &["--yes", "--install-hooks", "local"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed = read_json(&claude_dir.join("settings.local.json"));
    // permissions untouched
    assert_eq!(
        parsed
            .get("permissions")
            .unwrap()
            .get("allow")
            .unwrap()
            .get(0)
            .unwrap(),
        "Bash(ls)"
    );
    // PreToolUse has both the user's hook AND the daemon8 entry
    let pre = commands_under(&parsed, "PreToolUse");
    assert!(
        pre.contains(&"my-formatter".to_string()),
        "user's formatter hook must survive: {pre:?}"
    );
    assert!(
        pre.iter().any(|c| c.contains("cli-hook")),
        "daemon8 entry must be added: {pre:?}"
    );
    assert_eq!(pre.len(), 2, "exactly 2 PreToolUse entries expected");
}

#[test]
fn cli_install_hooks_rejects_malformed_target_file() {
    let (_tmp, work, home) = setup_dirs();
    let claude_dir = work.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings = claude_dir.join("settings.local.json");
    std::fs::write(&settings, "not valid json{").unwrap();

    let out = run_init(&work, &home, &["--yes", "--install-hooks", "local"]);
    assert!(!out.status.success(), "expected non-zero exit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("settings.local.json"),
        "stderr must name the malformed file; got: {stderr}"
    );
    // File contents unchanged.
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        "not valid json{"
    );
}

#[test]
fn cli_install_codex_hooks_rejects_malformed_target_file() {
    let (_tmp, work, home) = setup_dirs();
    let codex_dir = home.join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let hooks = codex_dir.join("hooks.json");
    std::fs::write(&hooks, "not valid json{").unwrap();

    let out = run_init(&work, &home, &["--yes", "--providers", "codex-cli"]);
    assert!(!out.status.success(), "expected non-zero exit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("hooks.json"),
        "stderr must name the malformed file; got: {stderr}"
    );
    assert_eq!(std::fs::read_to_string(&hooks).unwrap(), "not valid json{");
}

#[test]
fn cli_rerun_with_force_hooks_replaces_stale_entry() {
    let (_tmp, work, home) = setup_dirs();
    let claude_dir = work.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // Stale daemon8 entry pointing at a nonexistent binary path.
    let existing = serde_json::json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [{ "type": "command", "command": "/old/daemon8 cli-hook" }] }
            ]
        }
    });
    std::fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    let out = run_init(
        &work,
        &home,
        &["--yes", "--install-hooks", "local", "--force-hooks"],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed = read_json(&claude_dir.join("settings.local.json"));
    let cmds = commands_under(&parsed, "SessionStart");
    assert_eq!(
        cmds.len(),
        1,
        "expected exactly one entry after replacement"
    );
    assert!(
        !cmds[0].starts_with("/old/daemon8"),
        "stale entry must be replaced; got: {cmds:?}"
    );
    assert!(cmds[0].contains("cli-hook"));
}

#[test]
fn cli_install_codex_hooks_preserves_existing_user_hook() {
    let (_tmp, work, home) = setup_dirs();
    let codex_dir = home.join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": "my-codex-formatter" }]
                }
            ]
        }
    });
    std::fs::write(
        codex_dir.join("hooks.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    let out = run_init(&work, &home, &["--yes", "--providers", "codex-cli"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed = read_json(&codex_dir.join("hooks.json"));
    let pre = commands_under(&parsed, "PreToolUse");
    assert!(
        pre.contains(&"my-codex-formatter".to_string()),
        "user hook must survive: {pre:?}"
    );
    assert!(
        pre.iter()
            .any(|c| c.contains("daemon8") && c.contains("cli-hook --tool codex-cli")),
        "daemon8 codex hook must be added: {pre:?}"
    );
    assert_eq!(pre.len(), 2, "exactly 2 PreToolUse entries expected");
}

#[test]
fn cli_rerun_with_force_hooks_replaces_stale_codex_entry() {
    let (_tmp, work, home) = setup_dirs();
    let codex_dir = home.join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "startup|resume",
                    "hooks": [{ "type": "command", "command": "/old/daemon8 cli-hook --tool codex-cli" }]
                }
            ]
        }
    });
    std::fs::write(
        codex_dir.join("hooks.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    let out = run_init(
        &work,
        &home,
        &["--yes", "--providers", "codex-cli", "--force-hooks"],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed = read_json(&codex_dir.join("hooks.json"));
    let cmds = commands_under(&parsed, "SessionStart");
    assert_eq!(cmds.len(), 1, "expected exactly one replacement entry");
    assert!(
        !cmds[0].starts_with("/old/daemon8"),
        "stale codex entry must be replaced; got: {cmds:?}"
    );
    assert!(cmds[0].contains("cli-hook --tool codex-cli"));
}
