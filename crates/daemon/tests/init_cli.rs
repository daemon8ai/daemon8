// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration tests for `daemon8 init`.
//!
//! Exercise the compiled binary end-to-end: clap parsing, dispatch,
//! `.daemon8.toml` generation, and provider MCP registration (via fake HOME).

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

fn run_daemon8_with_env(
    dir: &Path,
    fake_home: &Path,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(binary());
    cmd.args(args)
        .current_dir(dir)
        .env("HOME", fake_home)
        .stdin(Stdio::null());
    for (key, _) in std::env::vars() {
        if key.starts_with("DAEMON8_") {
            cmd.env_remove(key);
        }
    }
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("spawn daemon8")
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
        .arg("apply")
        .args(args)
        .current_dir(dir)
        .env("HOME", fake_home)
        .env_remove("CI")
        .stdin(Stdio::null())
        .output()
        .expect("spawn daemon8 setup apply")
}

fn setup_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    let workdir = tmp.path().join("work");
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();
    (tmp, workdir, fake_home)
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
    assert!(!toml.contains("[enrollment]"));
    assert!(!toml.contains("cli-hook"));
    assert!(
        !work.join(".claude").exists(),
        ".claude must NOT be created by init"
    );
    assert!(
        !work.join(".codex").exists(),
        ".codex must NOT be created without provider selection"
    );
}

#[test]
fn config_env_nested_override_applies() {
    let (_tmp, work, home) = setup_dirs();
    let missing_config = work.join("missing-config.toml");
    let out = run_daemon8_with_env(
        &work,
        &home,
        &[("DAEMON8_SERVER__PORT", "9999")],
        &[
            "--config",
            missing_config.to_str().unwrap(),
            "config",
            "show",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("9999"),
        "nested env override must affect config show output: {stdout}"
    );
}

#[test]
fn cli_install_hooks_flag_is_removed() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes", "--install-hooks", "local"]);
    assert!(!out.status.success(), "hook install flag must be removed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("unrecognized"),
        "stderr: {stderr}"
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
fn cli_yes_with_codex_provider_writes_codex_config_without_hooks() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--yes", "--providers", "codex-cli"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let config = codex_config_path(&home);
    assert!(config.exists(), "codex config missing");
    assert!(!home.join(".codex").join("hooks.json").exists());

    let config_toml = std::fs::read_to_string(&config).unwrap();
    let config_parsed: toml::Value = toml::from_str(&config_toml).unwrap();
    assert_eq!(
        config_parsed["mcp_servers"]["daemon8"]["name"].as_str(),
        Some("Daemon8")
    );
}

#[test]
fn cli_yes_with_codex_provider_preserves_existing_codex_hook_feature() {
    let (_tmp, work, home) = setup_dirs();
    let config = codex_config_path(&home);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        r#"
[features]
codex_hooks = true

[mcp_servers.daemon8]
command = "/old/daemon8"
args = ["mcp"]
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
    assert_eq!(parsed["features"]["codex_hooks"].as_bool(), Some(true));
    assert!(
        parsed["features"].get("hooks").is_none(),
        "daemon8 must not write unsupported codex hooks feature flag"
    );
    assert_eq!(
        parsed["mcp_servers"]["daemon8"]["name"].as_str(),
        Some("Daemon8")
    );
    assert!(
        parsed["mcp_servers"]["daemon8"].get("command").is_none(),
        "stale stdio command must be removed when rewriting codex MCP config"
    );
    assert!(
        parsed["mcp_servers"]["daemon8"].get("args").is_none(),
        "stale stdio args must be removed when rewriting codex MCP config"
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
fn setup_json_reports_providers_and_daemon_state() {
    let (_tmp, work, home) = setup_dirs();
    let config_path = work.join("global-config.toml");

    let out = run_setup(&work, &home, &config_path, &["--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(parsed["providers"].is_array());
    assert!(parsed["daemon_running"].is_boolean());
    assert!(parsed["issues"].is_array());
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
