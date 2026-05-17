// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration tests for `daemon8 init`.
//!
//! Exercise the compiled binary end-to-end: clap parsing, dispatch,
//! and `.daemon8/config.md` generation.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use daemon8_core::project_config::parse_project_config_str;
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

fn run_connect(dir: &Path, fake_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .arg("connect")
        .args(args)
        .current_dir(dir)
        .env("HOME", fake_home)
        .env_remove("CI")
        .stdin(Stdio::null())
        .output()
        .expect("spawn daemon8 connect")
}

fn setup_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    let workdir = tmp.path().join("work");
    let fake_home = tmp.path().join("home");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();
    (tmp, workdir, fake_home)
}

fn project_config_path(root: &Path) -> PathBuf {
    root.join(".daemon8").join("config.md")
}

fn mark_project(root: &Path) {
    std::fs::create_dir(root.join(".git")).unwrap();
}

// ---------------------------------------------------------------------------

#[test]
fn cli_init_writes_project_config_only() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init(&work, &home, &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let config_path = project_config_path(&work);
    assert!(config_path.exists());
    let config = std::fs::read_to_string(config_path).unwrap();
    let parsed = parse_project_config_str(&config).unwrap();
    assert_eq!(parsed.daemon8_schema, 1);
    assert_eq!(parsed.project.name, "work");
    assert_eq!(parsed.project.stack.languages, vec!["generic"]);
    assert!(parsed.project.stack.frameworks.is_empty());
    assert!(parsed.project.stack.tools.is_empty());
    assert!(parsed.sources.is_empty());
    assert!(config.contains("project:"));
    assert!(config.contains("daemon8_schema: 1"));
    assert!(config.contains(r#"PRJ_ROOT: ""#));
    assert!(config.contains("sources: []"));
    assert!(!config.contains("role_default"));
    assert!(!config.contains("kind: sqlite"));
    assert!(!config.contains("kind: log"));
    assert!(!config.contains("[enrollment]"));
    assert!(!config.contains("cli-hook"));
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
fn cli_status_json_uses_common_envelope() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_daemon8_with_env(&work, &home, &[], &["status", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "status");
    assert_eq!(parsed["message"], "daemon status");
    assert!(parsed["data"]["config_path"].is_string());
    assert!(parsed["data"]["daemon_version"].is_string());
    assert!(parsed["data"]["connection"].is_null());
    assert_eq!(parsed["data"]["scope_authority"], "none");
    assert!(parsed.get("result").is_none());
    assert!(parsed.get("daemon8").is_none());
    assert!(parsed.get("error").is_none());
}

#[test]
fn cli_status_json_uses_global_config_file() {
    let (_tmp, work, home) = setup_dirs();
    let config_path = home.join("daemon8.custom.toml");
    let store_path = home.join("custom-store");
    let screenshot_path = home.join("custom-screens");
    std::fs::write(
        &config_path,
        format!(
            "[server]\nport = 9777\n[storage]\npath = {:?}\nscreenshot_path = {:?}\n",
            store_path.display().to_string(),
            screenshot_path.display().to_string()
        ),
    )
    .unwrap();

    let out = run_daemon8_with_env(
        &work,
        &home,
        &[],
        &[
            "--config",
            config_path.to_str().unwrap(),
            "status",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["data"]["port"], 9777);
    assert_eq!(
        parsed["data"]["config_path"],
        config_path.display().to_string()
    );
    assert_eq!(
        parsed["data"]["screenshot_dir"],
        screenshot_path.display().to_string()
    );
}

#[test]
fn cli_observe_and_lens_help_include_provenance_filters() {
    for args in [
        ["query", "--help"].as_slice(),
        ["tail", "--help"].as_slice(),
        ["lens", "set", "--help"].as_slice(),
    ] {
        let out = run_daemon8(args);
        assert!(out.status.success(), "command failed: {args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("--service"),
            "missing --service in {args:?}"
        );
        assert!(stdout.contains("--source"), "missing --source in {args:?}");
        assert!(
            stdout.contains("--source-instance"),
            "missing --source-instance in {args:?}"
        );
    }

    for args in [
        ["query", "--help"].as_slice(),
        ["tail", "--help"].as_slice(),
    ] {
        let out = run_daemon8(args);
        assert!(out.status.success(), "command failed: {args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("--project-path"),
            "missing --project-path in {args:?}"
        );
    }
}

#[test]
fn cli_install_hooks_flag_is_removed() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--install-hooks", "local"]);
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
fn cli_setup_command_is_removed() {
    let out = run_daemon8(&["setup", "--help"]);
    assert!(!out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized subcommand") || stderr.contains("unexpected argument"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_doctor_command_is_removed() {
    let out = run_daemon8(&["doctor", "--help"]);
    assert!(!out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized subcommand") || stderr.contains("unexpected argument"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_init_yes_flag_is_removed() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init(&work, &home, &["--yes"]);
    assert!(!out.status.success(), "removed --yes flag must stay gone");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("unrecognized"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_hidden_service_aliases_are_removed() {
    let install = run_daemon8(&["install", "--help"]);
    assert!(!install.status.success());

    let uninstall = run_daemon8(&["uninstall", "--help"]);
    assert!(!uninstall.status.success());
}

#[test]
fn cli_init_providers_flag_is_removed() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--providers", "codex-cli"]);
    assert!(!out.status.success(), "provider setup moved out of init");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("unrecognized"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_init_slug_flag_is_removed() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--slug", "old-name"]);
    assert!(!out.status.success(), "removed slug flag must stay gone");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("unrecognized"),
        "stderr: {stderr}"
    );
}

#[test]
fn cli_init_name_sets_project_name() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init(&work, &home, &["--name", "alpha-name"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let config = std::fs::read_to_string(project_config_path(&work)).unwrap();
    let parsed = parse_project_config_str(&config).unwrap();
    assert_eq!(parsed.project.name, "alpha-name");
}

#[test]
fn cli_init_json_uses_common_envelope() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init(&work, &home, &["--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "initialized");
    assert_eq!(parsed["data"]["project_name"], "work");
    assert!(parsed.get("result").is_none());
    assert!(parsed.get("daemon8").is_none());
}

#[test]
fn cli_init_refuses_general_scope_without_writing() {
    let (_tmp, work, home) = setup_dirs();
    let out = run_init(&work, &home, &["--json"]);
    assert!(
        out.status.success(),
        "json blocked responses still exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "general_scope");
    assert!(!project_config_path(&work).exists());
}

#[test]
fn cli_init_rejects_empty_name_without_writing() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init(&work, &home, &["--name", "", "--json"]);
    assert!(
        out.status.success(),
        "json error responses still exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "invalid_project_name");
    assert!(!project_config_path(&work).exists());
}

#[test]
fn cli_noninteractive_when_stdin_not_tty() {
    // Stdin is /dev/null via Stdio::null(); init must not try to prompt.
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init(&work, &home, &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(project_config_path(&work).exists());
}

#[test]
fn cli_ci_env_forces_noninteractive() {
    // stdin is /dev/null already, but belt-and-suspenders: with CI=1 we must also
    // skip prompting (important if someone runs the binary from an interactive
    // terminal inside a CI container image).
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let out = run_init_with_env(&work, &home, &[("CI", "1")], &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(project_config_path(&work).exists());
}

#[test]
fn cli_skips_existing_project_config_without_force() {
    let (_tmp, work, home) = setup_dirs();
    let config_path = project_config_path(&work);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "# pre-existing\n").unwrap();

    let out = run_init(&work, &home, &[]);
    assert!(
        out.status.success(),
        "expected success (graceful skip); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let preserved = std::fs::read_to_string(config_path).unwrap();
    assert_eq!(
        preserved, "# pre-existing\n",
        "file must be untouched when --force is not set"
    );
}

#[test]
fn cli_connect_missing_config_returns_setup_required_json() {
    let (_tmp, work, home) = setup_dirs();
    std::fs::write(work.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    let out = run_connect(
        &work,
        &home,
        &[
            "--path",
            work.to_str().unwrap(),
            "--provider",
            "codex",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "setup_required");
    assert_eq!(parsed["code"], "missing_config");
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_init");

    let out = run_connect(
        &work,
        &home,
        &[
            "--path",
            work.to_str().unwrap(),
            "--provider",
            "codex",
            "--json",
        ],
    );
    assert!(out.status.success());

    let status = run_daemon8_with_env(&work, &home, &[], &["status", "--json"]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let parsed: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"][0]["code"],
        "missing_config"
    );
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"][0]["attempt_count"],
        2
    );
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn cli_connect_after_init_returns_connected_json() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    let init = run_init(&work, &home, &[]);
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let out = run_connect(
        &work,
        &home,
        &[
            "--path",
            work.to_str().unwrap(),
            "--provider",
            "codex",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "connected");
    assert_eq!(parsed["data"]["mode"], "project");
    assert_eq!(parsed["data"]["provider"], "codex");

    let status = run_daemon8_with_env(&work, &home, &[], &["status", "--json"]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let parsed: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(parsed["data"]["connection"].is_null());
    assert_eq!(parsed["data"]["scope_authority"], "none");
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_scopes"][0]["scope_root"],
        work.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_scopes"][0]["provider"],
        "codex"
    );
}

#[test]
fn cli_connect_binds_explicit_transcript_path() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    run_init(&work, &home, &[]);
    let sessions = home.join(".codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join("one.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s1\",\"cwd\":\"{}\"}}}}\n",
            work.display()
        ),
    )
    .unwrap();

    let out = run_connect(
        &work,
        &home,
        &[
            "--path",
            work.to_str().unwrap(),
            "--provider",
            "codex-cli",
            "--transcript-path",
            transcript.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["provider"], "codex");
    assert_eq!(parsed["data"]["transcript"]["status"], "bound");
    assert_eq!(
        parsed["data"]["transcript_path"],
        transcript.canonicalize().unwrap().display().to_string()
    );
}

#[test]
fn cli_connect_blocks_ambiguous_transcripts() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);
    run_init(&work, &home, &[]);
    let sessions = home.join(".codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    for name in ["one.jsonl", "two.jsonl"] {
        std::fs::write(
            sessions.join(name),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{name}\",\"cwd\":\"{}\"}}}}\n",
                work.display()
            ),
        )
        .unwrap();
    }

    let out = run_connect(
        &work,
        &home,
        &[
            "--path",
            work.to_str().unwrap(),
            "--provider",
            "codex",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "transcript_ambiguous");
    assert_eq!(
        parsed["data"]["transcript"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn cli_connect_invalid_provider_records_failure() {
    let (_tmp, work, home) = setup_dirs();
    mark_project(&work);

    let out = run_connect(
        &work,
        &home,
        &[
            "--path",
            work.to_str().unwrap(),
            "--provider",
            "unknown-provider",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "invalid_provider");

    let status = run_daemon8_with_env(&work, &home, &[], &["status", "--json"]);
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let parsed: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"][0]["code"],
        "invalid_provider"
    );
}

#[test]
fn cli_force_overwrites_existing_project_config() {
    let (_tmp, work, home) = setup_dirs();
    let config_path = project_config_path(&work);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "# pre-existing\n").unwrap();

    let out = run_init(&work, &home, &["--force"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let content = std::fs::read_to_string(config_path).unwrap();
    parse_project_config_str(&content).unwrap();
    assert!(
        content.contains("project:"),
        "template should replace the pre-existing stub"
    );
}
