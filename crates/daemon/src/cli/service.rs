// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
#[cfg(target_os = "macos")]
use std::process::Output;
use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[cfg(unix)]
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader},
};

use daemon8_providers::Provider;

#[cfg(target_os = "macos")]
const LABEL: &str = "dev.daemon8.daemon";
const SETUP_PROVIDERS: &[Provider] = &[Provider::ClaudeCode, Provider::Gemini, Provider::Codex];
#[cfg(test)]
const INSTRUCTION_HEADING: &str = "## Daemon8 -- Runtime Observation Layer (ALWAYS ON)";
const INSTRUCTION_VERSION_MARKER: &str = "<!-- daemon8-instructions:v2026-05-demand-recall -->";
const INSTRUCTION_BLOCK: &str = r#"## Daemon8 -- Runtime Observation Layer (ALWAYS ON)
<!-- daemon8-instructions:v2026-05-demand-recall -->

Daemon8 is the runtime awareness layer for this agent. Use it for debugging, app logs, browser control, device logs, and demand-driven cross-provider conversation recovery. Never guess console output, network activity, DOM state, application logs, or what another agent already tried -- query daemon8.

Call `daemon8_connect` once at session start. If it returns `setup_required`, call `daemon8_init`, complete `.daemon8/config.md`, and then reconnect. Treat daemon8 response `requirements` and `next_actions` as control flow, not optional advice.

Do not run conversation recall automatically. After the project is connected and any required setup is complete, ask once whether the user wants to recover recent conversation history across AI providers. If the user asks for or accepts history, review, catch-up, continuity, prior work, or another provider's activity, run recent recall with `build_context_snapshot` and omit the `facets` filter so all standard facets are built across available Claude, Codex, and Gemini transcripts from the default last 24 hours. Use `since: "conversation_start"` only when the user explicitly asks for full history. Use `link_conversation` only when daemon8 reports missing/unlinked transcript sources or the user points to a specific provider/session. Do not repeatedly ask once the user has answered in the current session.

Do not rely on the visible chat as the whole project history. daemon8 may have linked conversations, provider transcripts, debug sessions, live observations, browser state, and source logs that are not visible in this chat.

For real bugs, use the checkpointed loop:
`daemon8_connect`
  -> `start_debug_session`
  -> `create_checkpoint`
  -> [reproduce/change/test]
  -> `read_live_feed` with `since_checkpoint`
  -> repeat until the evidence explains the result
  -> `resolve_debug_session` with the root cause, fix, and commands that mattered

**Primary tools:**
- `read_live_feed` -- console, network, errors, app telemetry (use `since_checkpoint` for incremental reads)
- `build_context_snapshot` -- recent cross-provider project recall when the user asks for or accepts conversation/history recovery
- `link_conversation` -- attach missing or explicit provider transcripts before recall
- `issue_command` -- browser control (eval_js, screenshot, navigate, viewport, storage, network throttle)
- `list_connections` -- see active input sources (browsers, devices, apps)
- `write_to_live_feed` -- emit notes, metrics, or agent-to-agent messages
- `set_lens` / `clear_lens` -- persistent filters that surface matching observations automatically
- `start_debug_session` / `create_checkpoint` / `resolve_debug_session` -- durable investigations with before/after evidence
- `daemon8_help` -- guidance on any daemon8 topic
"#;

fn binary_path() -> Result<PathBuf> {
    std::env::current_exe()
        .context("failed to determine binary path")?
        .canonicalize()
        .context("failed to resolve binary path")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cargo_bin_dir() -> String {
    home_dir().join(".cargo/bin").display().to_string()
}

#[cfg(target_os = "linux")]
fn local_bin_dir() -> String {
    home_dir().join(".local/bin").display().to_string()
}

#[cfg(target_os = "macos")]
fn macos_permission_preflight(vvd_enabled: bool) {
    println!();
    println!("  macOS may ask once to allow daemon8 as a background item.");
    if vvd_enabled {
        println!("  VVD screenshots require Screen Recording permission for daemon8.");
    }
    println!("  If Chromium control needs App Management later, daemon8 will guide that setup.");
    println!("  A browser extension is not needed.");
    println!();
}

// Best-effort jump to the Privacy & Security Screen Recording pane. Fails
// silently if the URL scheme is unavailable or `open` returns non-zero --
// install has already succeeded at this point.
#[cfg(target_os = "macos")]
pub(crate) fn macos_open_privacy_pane() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .output();
}

#[derive(clap::Args, Default)]
pub struct InstallArgs {
    /// Accept default yes answers for provider MCP and instruction-file setup prompts.
    #[arg(long)]
    pub yes: bool,

    /// Skip provider MCP configuration during service install.
    #[arg(long)]
    pub no_provider_setup: bool,

    /// Skip instruction-file guidance/write prompts during service install.
    #[arg(long)]
    pub no_instruction_setup: bool,
}

pub fn cmd_install(args: InstallArgs) -> Result<()> {
    let binary = binary_path()?;
    let binary_str = binary.display().to_string();
    let cfg = crate::config::load(None).unwrap_or_default();
    let port = cfg.server.port;
    let mcp_url = format!("http://localhost:{port}/mcp");
    let chrome_endpoint = if cfg.browser.auto_connect {
        Some(cfg.browser.endpoint.clone())
    } else {
        None
    };

    println!("Installing daemon8 service...");
    println!("  Binary: {binary_str}");

    #[cfg(target_os = "macos")]
    {
        macos_permission_preflight(cfg.device.vvd.enabled);
        install_launchd(&binary_str, chrome_endpoint.as_deref(), port)?;

        if should_request_screen_recording_on_install(&cfg) {
            let _ = request_macos_screen_recording_permission();
            macos_open_privacy_pane();
        }
    }

    #[cfg(target_os = "linux")]
    install_systemd(&binary_str, chrome_endpoint.as_deref(), port)?;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        println!();
        println!("Daemon8 is running as your machine-wide MCP server.");
        println!("  Local-only endpoint: {mcp_url}");
    }

    #[cfg(windows)]
    {
        install_schtasks(&binary_str, chrome_endpoint.as_deref(), port)?;
        println!();
        println!("Daemon8 is running as your machine-wide MCP server.");
        println!("  Local-only endpoint: {mcp_url}");
    }

    println!();
    let configured_providers = if args.no_provider_setup {
        println!("Provider MCP setup skipped by flag.");
        configured_provider_targets()
    } else {
        setup_provider_mcp(&mcp_url, args.yes)?
    };

    if args.no_instruction_setup {
        print_install_outro(&configured_providers, false);
    } else {
        setup_instruction_files(&configured_providers, args.yes)?;
        print_install_outro(&configured_providers, true);
    }

    Ok(())
}

pub fn cmd_request_screen_recording() -> Result<()> {
    #[cfg(all(target_os = "macos", feature = "xcap"))]
    {
        if request_macos_screen_recording_permission() {
            println!(
                "Screen Recording permission request completed for this daemon8 binary. Verify the launchd daemon with a device screenshot; macOS may still require toggling daemon8 in System Settings after reinstall."
            );
            macos_open_privacy_pane();
        } else {
            println!(
                "Screen Recording permission is still not granted. Opening System Settings so you can approve daemon8."
            );
            macos_open_privacy_pane();
        }
        Ok(())
    }

    #[cfg(all(target_os = "macos", not(feature = "xcap")))]
    {
        anyhow::bail!(
            "screen recording permission requests require daemon8 to be built with --features xcap"
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        println!("Screen Recording permission is macOS-only.");
        Ok(())
    }
}

#[cfg(all(target_os = "macos", feature = "xcap"))]
pub(crate) fn request_macos_screen_recording_permission() -> bool {
    if objc2_core_graphics::CGPreflightScreenCaptureAccess() {
        return true;
    }

    println!("Requesting macOS Screen Recording permission for daemon8...");
    objc2_core_graphics::CGRequestScreenCaptureAccess()
}

#[cfg(all(target_os = "macos", not(feature = "xcap")))]
pub(crate) fn request_macos_screen_recording_permission() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn should_request_screen_recording_on_install(cfg: &crate::config::Config) -> bool {
    cfg.device.vvd.enabled
}

#[derive(Clone)]
struct ProviderSetupTarget {
    provider: Provider,
    config_path: PathBuf,
    detected: bool,
    configured: bool,
    existing_entry: bool,
}

impl ProviderSetupTarget {
    fn instruction_path(&self, home: &Path) -> Option<PathBuf> {
        let provider = self.provider.as_provider();
        Some(
            provider
                .global_config_dir(home)?
                .join(provider.instruction_file_name()),
        )
    }
}

fn provider_setup_targets(home: &Path, mcp_url: Option<&str>) -> Vec<ProviderSetupTarget> {
    SETUP_PROVIDERS
        .iter()
        .map(|&provider| {
            let p = provider.as_provider();
            let config_path = p.config_path(home);
            let detected = home.join(p.detect_dir()).exists() || config_path.exists();
            let existing_entry = p.is_configured(&config_path, &crate::cli_config::SERVICE);
            let configured = mcp_url.map_or(existing_entry, |url| {
                p.mcp_config_matches(&config_path, &crate::cli_config::SERVICE, url)
            });

            ProviderSetupTarget {
                provider,
                config_path,
                detected,
                configured,
                existing_entry,
            }
        })
        .collect()
}

fn configured_provider_targets() -> Vec<ProviderSetupTarget> {
    provider_setup_targets(&daemon8_providers::dirs_home(), None)
        .into_iter()
        .filter(|target| target.configured)
        .collect()
}

fn setup_provider_mcp(mcp_url: &str, yes: bool) -> Result<Vec<ProviderSetupTarget>> {
    let home = daemon8_providers::dirs_home();
    let mut configured = Vec::new();

    println!("Provider MCP setup:");
    for mut target in provider_setup_targets(&home, Some(mcp_url)) {
        let provider = target.provider.as_provider();
        if target.configured {
            println!(
                "  [ok] {} already has daemon8 MCP settings",
                provider.label()
            );
            configured.push(target);
            continue;
        }

        if !target.detected {
            println!(
                "  [--] {} not detected at {}",
                provider.label(),
                home.join(provider.detect_dir()).display()
            );
            continue;
        }

        let question = format!(
            "  {} daemon8 MCP settings for {} at {}? [Y/n]: ",
            if target.existing_entry {
                "Update"
            } else {
                "Add"
            },
            provider.label(),
            target.config_path.display()
        );
        let was_existing = target.existing_entry;
        if yes || prompt_yes_default(&question)? == Some(true) {
            daemon8_providers::write_provider_config(
                target.provider,
                &target.config_path,
                mcp_url,
                None,
                &crate::cli_config::SERVICE,
            )
            .with_context(|| format!("writing {} MCP config", provider.label()))?;
            target.configured = true;
            target.existing_entry = true;
            println!(
                "  [ok] {} MCP settings {}",
                provider.label(),
                if was_existing { "updated" } else { "added" }
            );
            configured.push(target);
        } else {
            println!("  [--] {} MCP setup skipped", provider.label());
        }
    }

    if configured.is_empty() {
        println!("  [--] No provider MCP settings were added.");
    }
    Ok(configured)
}

fn setup_instruction_files(configured_providers: &[ProviderSetupTarget], yes: bool) -> Result<()> {
    let targets = instruction_targets(configured_providers);

    if targets.is_empty() {
        println!();
        println!("No detected provider instruction file paths to update automatically.");
        return Ok(());
    }

    println!();
    println!("Detected instruction file paths:");
    for target in &targets {
        println!("  {}: {}", target.provider.label(), target.path.display());
    }

    if yes {
        for target in &targets {
            print_instruction_write_result(target);
        }
        return Ok(());
    }

    loop {
        let question = "Add daemon8 instructions to those files? [Y]es/[N]o/[P]rint/[C]opy: ";
        let answer = match prompt_raw(question)? {
            Some(s) => s,
            None => break,
        };

        match answer.as_str() {
            "y" | "yes" | "" => {
                for target in &targets {
                    print_instruction_write_result(target);
                }
                break;
            }
            "n" | "no" => {
                println!("  [--] instruction-file write skipped");
                break;
            }
            "p" | "print" => {
                println!();
                print!("{}", strip_markdown(INSTRUCTION_BLOCK.trim_end()));
                println!();
                println!();
            }
            "c" | "copy" => {
                if copy_to_clipboard(INSTRUCTION_BLOCK.trim_end())? {
                    println!("  [ok] copied to clipboard");
                } else {
                    println!("  [!!] clipboard not available");
                }
            }
            _ => {
                println!("  unrecognized input");
            }
        }
    }

    Ok(())
}

fn print_instruction_write_result(target: &InstructionTarget) {
    match prepend_instruction_block(&target.path) {
        Ok(InstructionWrite::Written) => {
            println!("  [ok] updated {}", target.path.display());
        }
        Ok(InstructionWrite::Updated) => {
            println!("  [ok] refreshed {}", target.path.display());
        }
        Ok(InstructionWrite::AlreadyPresent) => {
            println!("  [ok] already present in {}", target.path.display());
        }
        Err(err) => {
            println!("  [!!] {}: {err}", target.path.display());
        }
    }
}

fn strip_markdown(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let clean = line.replace("**", "").replace('`', "");
        out.push_str(&clean);
        out.push('\n');
    }
    out
}

struct InstructionTarget {
    provider: Provider,
    path: PathBuf,
}

fn instruction_targets(configured_providers: &[ProviderSetupTarget]) -> Vec<InstructionTarget> {
    let home = daemon8_providers::dirs_home();
    configured_providers
        .iter()
        .filter_map(|target| {
            Some(InstructionTarget {
                provider: target.provider,
                path: target.instruction_path(&home)?,
            })
        })
        .collect()
}

enum InstructionWrite {
    Written,
    Updated,
    AlreadyPresent,
}

fn prepend_instruction_block(path: &Path) -> io::Result<InstructionWrite> {
    let existing = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    if existing.contains(INSTRUCTION_VERSION_MARKER) {
        return Ok(InstructionWrite::AlreadyPresent);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Some(next) = replace_existing_instruction_block(&existing) {
        fs::write(path, next)?;
        return Ok(InstructionWrite::Updated);
    }

    if existing.contains("daemon8_connect") {
        return Ok(InstructionWrite::AlreadyPresent);
    }

    let next = if existing.trim().is_empty() {
        format!("{}\n", INSTRUCTION_BLOCK.trim_end())
    } else {
        format!("{}\n\n{}", INSTRUCTION_BLOCK.trim_end(), existing)
    };
    fs::write(path, next)?;
    Ok(InstructionWrite::Written)
}

fn replace_existing_instruction_block(existing: &str) -> Option<String> {
    let lines = existing.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| is_daemon8_instruction_heading(line))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(idx, line)| line.trim_start().starts_with("## ").then_some(idx))
        .unwrap_or(lines.len());

    let before = lines[..start].join("\n");
    let after = lines[end..].join("\n");
    let block = INSTRUCTION_BLOCK.trim_end();

    let mut next = String::new();
    if !before.trim().is_empty() {
        next.push_str(before.trim_end());
        next.push_str("\n\n");
    }
    next.push_str(block);
    next.push('\n');
    if !after.trim().is_empty() {
        next.push('\n');
        next.push_str(after.trim_start());
        next.push('\n');
    }

    (next.trim_end() != existing.trim_end()).then_some(next)
}

fn is_daemon8_instruction_heading(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("## Daemon8")
        && line.contains("Runtime Observation Layer")
        && line.contains("(ALWAYS ON)")
}

fn print_install_outro(configured_providers: &[ProviderSetupTarget], instruction_setup: bool) {
    println!();
    println!("Important note:");
    println!("  daemon8 is self-guided from here. If the steps above completed without errors,");
    println!("  your manual setup should be done. Start a fresh AI CLI/REPL session and confirm");
    println!("  daemon8 appears in its MCP list.");
    println!(
        "  The agent should call daemon8_connect first; daemon8 will guide daemon8_init only when a project needs it."
    );
    println!("  No browser extension is needed.");
    println!("  For Claude Code users, be sure to disable Claude for Chrome and explicitly tell");
    println!("  CLAUDE.md to use daemon8 instead of Claude for Chrome.");

    if configured_providers.is_empty() {
        println!("  No provider MCP settings were configured during this install.");
        println!(
            "  Run daemon8 service install --yes after installing Claude Code, Gemini CLI, or Codex."
        );
    }
    if !instruction_setup {
        println!(
            "  Instruction-file setup was skipped; add the daemon8 note before relying on agent guidance."
        );
    }
}

fn prompt_yes_default(question: &str) -> io::Result<Option<bool>> {
    if let Some(answer) = prompt_with_tty(question)? {
        return Ok(Some(answer));
    }
    if io::stdin().is_terminal() {
        return prompt_with_stdio(question).map(Some);
    }
    println!("  [--] non-interactive install; skipped prompt");
    Ok(None)
}

#[cfg(unix)]
fn prompt_with_tty(question: &str) -> io::Result<Option<bool>> {
    let mut tty = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut reader = BufReader::new(tty.try_clone()?);
    loop {
        write!(tty, "{question}")?;
        tty.flush()?;
        let mut input = String::new();
        let n = reader.read_line(&mut input)?;
        if n == 0 {
            return Ok(Some(true));
        }
        if let Some(ans) = parse_yes_no_default(&input) {
            return Ok(Some(ans));
        }
        writeln!(tty, "  unrecognized input; please type 'y' or 'n'")?;
    }
}

#[cfg(not(unix))]
fn prompt_with_tty(_question: &str) -> io::Result<Option<bool>> {
    Ok(None)
}

fn prompt_with_stdio(question: &str) -> io::Result<bool> {
    let stdin = io::stdin();
    loop {
        eprint!("{question}");
        io::stderr().flush()?;
        let mut input = String::new();
        let n = stdin.read_line(&mut input)?;
        if n == 0 {
            return Ok(true);
        }
        if let Some(ans) = parse_yes_no_default(&input) {
            return Ok(ans);
        }
        eprintln!("  unrecognized input; please type 'y' or 'n'");
    }
}

fn parse_yes_no_default(input: &str) -> Option<bool> {
    let answer = input.trim();
    if answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
        Some(true)
    } else if answer.eq_ignore_ascii_case("n") || answer.eq_ignore_ascii_case("no") {
        Some(false)
    } else {
        None
    }
}

fn prompt_raw(question: &str) -> io::Result<Option<String>> {
    if let Some(answer) = prompt_raw_tty(question)? {
        return Ok(Some(answer));
    }
    if io::stdin().is_terminal() {
        eprint!("{question}");
        io::stderr().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        return Ok(Some(input.trim().to_ascii_lowercase()));
    }
    println!("  [--] non-interactive install; skipped prompt");
    Ok(None)
}

#[cfg(unix)]
fn prompt_raw_tty(question: &str) -> io::Result<Option<String>> {
    let mut tty = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    write!(tty, "{question}")?;
    tty.flush()?;
    let mut input = String::new();
    BufReader::new(tty).read_line(&mut input)?;
    Ok(Some(input.trim().to_ascii_lowercase()))
}

#[cfg(not(unix))]
fn prompt_raw_tty(_question: &str) -> io::Result<Option<String>> {
    Ok(None)
}

fn copy_to_clipboard(content: &str) -> Result<bool> {
    let commands: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(windows) {
        &[("clip", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    };

    for &(program, args) in commands {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => continue,
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(content.as_bytes())?;
        }
        if child.wait()?.success() {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn cmd_uninstall() -> Result<()> {
    println!("Removing daemon8...");
    println!();

    // 1. System service
    #[cfg(target_os = "macos")]
    match uninstall_launchd() {
        Ok(()) => println!("  [ok] launchd service removed"),
        Err(e) => println!("  [--] launchd service: {e}"),
    }

    #[cfg(target_os = "linux")]
    match uninstall_systemd() {
        Ok(()) => println!("  [ok] systemd service removed"),
        Err(e) => println!("  [--] systemd service: {e}"),
    }

    #[cfg(windows)]
    match uninstall_schtasks() {
        Ok(()) => println!("  [ok] scheduled task removed"),
        Err(e) => println!("  [--] scheduled task: {e}"),
    }

    let cfg = crate::config::load(None).unwrap_or_default();
    for target in daemon_owned_removal_targets(&cfg) {
        remove_path(&target.path, target.label);
    }
    println!("  [--] project configs untouched (.daemon8/config.md is project-owned)");

    let home = daemon8_providers::dirs_home();
    for &provider in SETUP_PROVIDERS {
        remove_provider_entry(provider, &home);
    }

    println!();
    println!("Daemon8 fully uninstalled.");
    Ok(())
}

struct RemovalTarget {
    path: PathBuf,
    label: &'static str,
}

fn daemon_owned_removal_targets(cfg: &crate::config::Config) -> Vec<RemovalTarget> {
    let mut targets = vec![RemovalTarget {
        path: cfg.config_dir.clone(),
        label: "config dir",
    }];

    let db_path = crate::config::resolve_db_path(cfg.storage.path.as_deref());
    if let Some(data_dir) = db_path.parent() {
        targets.push(RemovalTarget {
            path: data_dir.to_path_buf(),
            label: "data dir",
        });
    }

    targets
}

fn remove_path(path: &std::path::Path, label: &str) {
    if !path.exists() {
        return;
    }
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => println!("  [ok] {label}: {}", path.display()),
        Err(e) => println!("  [!!] {label}: {} ({e})", path.display()),
    }
}

fn remove_provider_entry(provider: Provider, home: &std::path::Path) {
    let config_path = provider.config_path(home);
    if !config_path.exists() {
        return;
    }
    match provider
        .as_provider()
        .remove_mcp_config(&config_path, &crate::cli_config::SERVICE)
    {
        Ok(true) => println!("  [ok] removed daemon8 from {}", config_path.display()),
        Ok(false) => {}
        Err(e) => println!("  [!!] {}: {e}", config_path.display()),
    }
}

// ---------------------------------------------------------------------------
// macOS: launchd user agent
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn launchd_domain() -> String {
    // User LaunchAgents live in the per-login GUI domain.
    format!("gui/{}", unsafe { libc::geteuid() })
}

#[cfg(target_os = "macos")]
fn launchctl_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    format!("exit status {}", output.status)
}

#[cfg(target_os = "macos")]
fn install_launchd(binary: &str, chrome_endpoint: Option<&str>, port: u16) -> Result<()> {
    let path = plist_path();
    let path_str = path.display().to_string();
    let cargo_bin = cargo_bin_dir();
    let domain = launchd_domain();
    let service_target = format!("{domain}/{LABEL}");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let chrome_args = chrome_endpoint
        .map(|ep| format!("\n        <string>--browser</string>\n        <string>{ep}</string>"))
        .unwrap_or_default();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>serve</string>{chrome_args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/dev/null</string>
    <key>StandardErrorPath</key>
    <string>/dev/null</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:{cargo_bin}</string>
    </dict>
</dict>
</plist>"#
    );

    std::fs::write(&path, &plist).with_context(|| format!("writing {}", path.display()))?;
    println!("  Service: {}", path.display());

    // If already installed, boot it out first (upgrade/reinstall path).
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &domain, &path_str])
        .output();

    let output = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &path_str])
        .output()
        .context("failed to run launchctl bootstrap")?;

    if output.status.success() {
        println!("  Loaded: launchctl bootstrap");
    } else {
        anyhow::bail!("launchctl bootstrap failed: {}", launchctl_message(&output));
    }

    let enable = std::process::Command::new("launchctl")
        .args(["enable", &service_target])
        .output()
        .context("failed to run launchctl enable")?;
    if !enable.status.success() {
        anyhow::bail!("launchctl enable failed: {}", launchctl_message(&enable));
    }

    let kickstart = std::process::Command::new("launchctl")
        .args(["kickstart", "-k", &service_target])
        .output()
        .context("failed to run launchctl kickstart")?;
    if !kickstart.status.success() {
        anyhow::bail!(
            "launchctl kickstart failed: {}",
            launchctl_message(&kickstart)
        );
    }

    // Wait for it to start
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            println!("  Status: running");
            return Ok(());
        }
    }

    println!("  Status: started (verifying health timed out, check 'daemon8 logs')");
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<()> {
    let path = plist_path();
    let path_str = path.display().to_string();
    let domain = launchd_domain();

    if !path.exists() {
        println!("  Not installed (no plist found)");
        return Ok(());
    }

    let output = std::process::Command::new("launchctl")
        .args(["bootout", &domain, &path_str])
        .output()
        .context("failed to run launchctl bootout")?;

    if output.status.success() {
        println!("  Unloaded: launchctl bootout");
    } else {
        println!(
            "  Warning: launchctl bootout: {}",
            launchctl_message(&output)
        );
    }

    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    println!("  Removed: {}", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux: systemd user unit
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn unit_path() -> PathBuf {
    home_dir()
        .join(".config/systemd/user")
        .join("daemon8.service")
}

#[cfg(target_os = "linux")]
fn install_systemd(binary: &str, chrome_endpoint: Option<&str>, port: u16) -> Result<()> {
    let path = unit_path();
    let cargo_bin = cargo_bin_dir();
    let local_bin = local_bin_dir();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let chrome_args = chrome_endpoint
        .map(|ep| format!(" --browser {ep}"))
        .unwrap_or_default();

    let unit = format!(
        r#"[Unit]
Description=Daemon8 — the admin layer for AI agents
After=network.target

[Service]
Type=simple
ExecStart={binary} serve{chrome_args}
Restart=always
RestartSec=5
Environment=PATH=/usr/local/bin:/usr/bin:/bin:{cargo_bin}:{local_bin}

[Install]
WantedBy=default.target
"#
    );

    std::fs::write(&path, &unit).with_context(|| format!("writing {}", path.display()))?;
    println!("  Service: {}", path.display());

    let reload = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output()
        .context("failed to run systemctl daemon-reload")?;

    if !reload.status.success() {
        let stderr = String::from_utf8_lossy(&reload.stderr);
        anyhow::bail!("systemctl daemon-reload failed: {stderr}");
    }

    let enable = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "daemon8"])
        .output()
        .context("failed to run systemctl enable")?;

    if enable.status.success() {
        println!("  Enabled: systemctl --user enable --now daemon8");
    } else {
        let stderr = String::from_utf8_lossy(&enable.stderr);
        anyhow::bail!("systemctl enable failed: {stderr}");
    }

    // Wait for it to start
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            println!("  Status: running");
            return Ok(());
        }
    }

    println!("  Status: started (verifying health timed out, check 'daemon8 logs')");
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> Result<()> {
    let path = unit_path();

    if !path.exists() {
        println!("  Not installed (no unit file found)");
        return Ok(());
    }

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "daemon8"])
        .output();
    println!("  Disabled: systemctl --user disable --now daemon8");

    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    println!("  Removed: {}", path.display());

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();

    Ok(())
}

// ---------------------------------------------------------------------------
// Windows: Task Scheduler user task
// ---------------------------------------------------------------------------

// schtasks consumes the XML literally -- any `&` in a path or URL is interpreted
// as an entity reference and aborts parsing. Order matters: `&` must be escaped
// first so the replacement entities aren't double-escaped.
#[cfg(windows)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
const WINDOWS_TASK_NAME: &str = "daemon8-service";

#[cfg(windows)]
const WINDOWS_LEGACY_TASK_NAMES: &[&str] = &["Daemon8", "daemon8-user"];

#[cfg(any(windows, test))]
fn windows_powershell_path() -> String {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    format!("{system_root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

#[cfg(any(windows, test))]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(any(windows, test))]
fn powershell_encoded_command(command: &str) -> String {
    use base64::Engine as _;

    let bytes = command
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect::<Vec<_>>();

    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(any(windows, test))]
fn windows_task_action(binary: &str, chrome_endpoint: Option<&str>) -> (String, String) {
    let mut command = format!("& {} serve", powershell_quote(binary));
    if let Some(endpoint) = chrome_endpoint {
        command.push_str(" --browser ");
        command.push_str(&powershell_quote(endpoint));
    }
    command.push_str("; exit $LASTEXITCODE");

    let powershell = windows_powershell_path();
    let encoded = powershell_encoded_command(&command);
    let args = format!(
        "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand {encoded}"
    );

    (powershell, args)
}

#[cfg(windows)]
fn powershell_script_line(name: &str, value: &str) -> String {
    format!("${name} = {}\n", powershell_quote(value))
}

#[cfg(windows)]
fn install_scheduled_task_with_powershell(
    task_name: &str,
    powershell: &str,
    action_args: &str,
) -> Result<()> {
    let mut script = String::from("$ErrorActionPreference = 'Stop'\n");
    script.push_str(&powershell_script_line("TaskName", task_name));
    script.push_str(&powershell_script_line("PowerShellExe", powershell));
    script.push_str(&powershell_script_line("ActionArgs", action_args));
    script.push_str("$UserId = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\n");
    script.push_str(
        "$Action = New-ScheduledTaskAction -Execute $PowerShellExe -Argument $ActionArgs\n",
    );
    script.push_str("$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId\n");
    script.push_str("$Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 10 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew\n");
    script.push_str("$Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Limited\n");
    script
        .push_str("foreach ($ExistingTask in @('Daemon8', 'daemon8-user', 'daemon8-service')) {\n");
    script.push_str(
        "  try { Stop-ScheduledTask -TaskName $ExistingTask -ErrorAction SilentlyContinue } catch {}\n",
    );
    script.push_str("  try { Get-ScheduledTask -TaskName $ExistingTask -ErrorAction SilentlyContinue | Unregister-ScheduledTask -Confirm:$false -ErrorAction Stop } catch {}\n");
    script.push_str("}\n");
    script.push_str("Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null\n");

    let encoded = powershell_encoded_command(&script);
    let output = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .output()
        .context("failed to run PowerShell ScheduledTasks fallback")?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("PowerShell ScheduledTasks fallback failed: {stdout}{stderr}");
    }

    Ok(())
}

#[cfg(windows)]
fn install_schtasks(binary: &str, chrome_endpoint: Option<&str>, port: u16) -> Result<()> {
    let (powershell, action_args) = windows_task_action(binary, chrome_endpoint);

    // On domain-joined machines schtasks wants `DOMAIN\User`; standalone boxes
    // accept `.\User`. Bare `USERNAME` silently fails under AD policy.
    let raw_user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let username = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{raw_user}"),
        _ => format!(".\\{raw_user}"),
    };
    let username = xml_escape(&username);
    let xml_powershell = xml_escape(&powershell);
    let xml_action_args = xml_escape(&action_args);

    // Task XML: run at logon for current user, restart on failure up to 10
    // times at 1-minute intervals, no execution time limit.
    // schtasks requires UTF-16 LE with BOM when reading from a file.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals>
    <Principal id="Author">
      <UserId>{username}</UserId>
      <LogonType>InteractiveToken</LogonType>
    </Principal>
  </Principals>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{username}</UserId>
    </LogonTrigger>
  </Triggers>
  <Settings>
    <RestartOnFailure>
      <Count>10</Count>
      <Interval>PT1M</Interval>
    </RestartOnFailure>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{xml_powershell}</Command>
      <Arguments>{xml_action_args}</Arguments>
    </Exec>
  </Actions>
</Task>"#
    );

    // PID-suffixed so concurrent installers don't race on the same temp path.
    let tmp = std::env::temp_dir().join(format!("daemon8-task-{}.xml", std::process::id()));
    {
        let utf16: Vec<u16> = std::iter::once(0xFEFF_u16)
            .chain(xml.encode_utf16())
            .collect();
        let bytes: Vec<u8> = utf16.iter().flat_map(|c| c.to_le_bytes()).collect();
        std::fs::write(&tmp, bytes)
            .with_context(|| format!("writing task XML to {}", tmp.display()))?;
    }

    // Remove any existing task first (idempotent upgrade path).
    // `.output()` already captures both streams; no redirection needed.
    for task_name in
        std::iter::once(WINDOWS_TASK_NAME).chain(WINDOWS_LEGACY_TASK_NAMES.iter().copied())
    {
        let _ = std::process::Command::new("schtasks")
            .args(["/End", "/TN", task_name])
            .output();

        let _ = std::process::Command::new("schtasks")
            .args(["/Delete", "/TN", task_name, "/F"])
            .output();
    }

    let create = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            WINDOWS_TASK_NAME,
            "/XML",
            &tmp.display().to_string(),
            "/F",
        ])
        .output()
        .context("failed to run schtasks /Create")?;

    let _ = std::fs::remove_file(&tmp);

    if !create.status.success() {
        let stderr = String::from_utf8_lossy(&create.stderr);
        let task_run = format!("\"{powershell}\" {action_args}");
        let fallback = std::process::Command::new("schtasks")
            .args([
                "/Create",
                "/TN",
                WINDOWS_TASK_NAME,
                "/TR",
                &task_run,
                "/SC",
                "ONLOGON",
                "/RL",
                "LIMITED",
                "/F",
            ])
            .output()
            .context("failed to run fallback schtasks /Create")?;

        if fallback.status.success() {
            println!("  Service: Task Scheduler task '{WINDOWS_TASK_NAME}' registered");
        } else {
            let fallback_stderr = String::from_utf8_lossy(&fallback.stderr);
            match install_scheduled_task_with_powershell(
                WINDOWS_TASK_NAME,
                &powershell,
                &action_args,
            ) {
                Ok(()) => {
                    println!("  Service: Task Scheduler task '{WINDOWS_TASK_NAME}' registered");
                }
                Err(err) => {
                    anyhow::bail!(
                        "schtasks /Create failed: {stderr}; fallback schtasks /Create failed: {fallback_stderr}; {err}\n\
                         Browser control does not require Administrator, but Windows blocked background startup registration. \
                         Run `daemon8 serve` to start daemon8 now, or rerun `daemon8 service install` from PowerShell as Administrator."
                    );
                }
            }
        }
    } else {
        println!("  Service: Task Scheduler task '{WINDOWS_TASK_NAME}' registered");
    }

    let start = std::process::Command::new("schtasks")
        .args(["/Run", "/TN", WINDOWS_TASK_NAME])
        .output()
        .context("failed to run schtasks /Run")?;

    if !start.status.success() {
        let stderr = String::from_utf8_lossy(&start.stderr);
        anyhow::bail!(
            "schtasks /Run failed: {stderr}\n\
             The task was registered but Windows would not start it. Run `daemon8 serve` to start daemon8 now."
        );
    }

    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            println!("  Status: running");
            return Ok(());
        }
    }
    println!("  Status: started (verifying health timed out, check 'daemon8 logs')");
    Ok(())
}

#[cfg(windows)]
fn uninstall_schtasks() -> Result<()> {
    let mut found = false;
    for task_name in
        std::iter::once(WINDOWS_TASK_NAME).chain(WINDOWS_LEGACY_TASK_NAMES.iter().copied())
    {
        let query = std::process::Command::new("schtasks")
            .args(["/Query", "/TN", task_name])
            .output()
            .context("failed to run schtasks /Query")?;

        if !query.status.success() {
            continue;
        }

        found = true;
        let _ = std::process::Command::new("schtasks")
            .args(["/End", "/TN", task_name])
            .output();

        let output = std::process::Command::new("schtasks")
            .args(["/Delete", "/TN", task_name, "/F"])
            .output()
            .context("failed to run schtasks /Delete")?;

        if output.status.success() {
            println!("  Removed: Task Scheduler task '{task_name}'");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("  [!!] Task Scheduler task '{task_name}': {stderr}");
        }
    }

    if !found {
        println!("  Service: not installed (nothing to remove)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_targets_only_daemon_owned_paths() {
        let root = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            config_dir: root.path().join("global-config"),
            storage: crate::config::StorageConfig {
                path: Some(root.path().join("data").join("store")),
                ..Default::default()
            },
            ..Default::default()
        };

        let project_config = root
            .path()
            .join(daemon8_core::init::PROJECT_CONFIG_DIR)
            .join(daemon8_core::init::PROJECT_CONFIG_FILENAME);
        let targets = daemon_owned_removal_targets(&cfg);

        assert!(targets.iter().any(|target| target.path == cfg.config_dir));
        assert!(
            targets
                .iter()
                .any(|target| target.path == root.path().join("data"))
        );
        assert!(
            targets.iter().all(|target| target.path != project_config),
            "service uninstall must never delete project-owned .daemon8/config.md"
        );
    }

    #[test]
    fn provider_setup_targets_cover_the_three_public_providers() {
        let root = tempfile::tempdir().unwrap();
        let targets = provider_setup_targets(root.path(), None);
        let providers = targets
            .iter()
            .map(|target| target.provider)
            .collect::<Vec<_>>();

        assert_eq!(
            providers,
            vec![Provider::ClaudeCode, Provider::Gemini, Provider::Codex]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn install_requests_screen_recording_only_when_vvd_is_enabled() {
        let mut cfg = crate::config::Config::default();
        assert!(!should_request_screen_recording_on_install(&cfg));

        cfg.device.vvd.enabled = true;
        assert!(should_request_screen_recording_on_install(&cfg));
    }

    #[test]
    fn parse_yes_no_default_handles_valid_inputs() {
        assert_eq!(parse_yes_no_default(""), Some(true));
        assert_eq!(parse_yes_no_default("y"), Some(true));
        assert_eq!(parse_yes_no_default("YES"), Some(true));
        assert_eq!(parse_yes_no_default("n"), Some(false));
        assert_eq!(parse_yes_no_default("no"), Some(false));
        assert_eq!(parse_yes_no_default("sy"), None);
        assert_eq!(parse_yes_no_default("asdf"), None);
    }

    #[test]
    fn windows_task_action_runs_daemon_hidden_and_synchronously() {
        use base64::Engine as _;

        let (powershell, args) = windows_task_action(
            r"C:\Program Files\daemon8\daemon8's folder\daemon8.exe",
            Some("http://127.0.0.1:9222/devtools/browser/abc"),
        );

        assert!(powershell.ends_with(r"\System32\WindowsPowerShell\v1.0\powershell.exe"));
        assert!(args.contains("-WindowStyle Hidden"));
        assert!(args.contains("-NoLogo"));
        assert!(args.contains("-NoProfile"));
        assert!(args.contains("-NonInteractive"));
        assert!(args.contains("-ExecutionPolicy Bypass"));
        assert!(args.contains("-EncodedCommand "));
        assert!(
            !args.contains("-Command "),
            "Task Scheduler action must use -EncodedCommand; plain -Command can flash a terminal"
        );

        let encoded = args.rsplit(' ').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let units = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let command = String::from_utf16(&units).unwrap();

        assert_eq!(
            command,
            "& 'C:\\Program Files\\daemon8\\daemon8''s folder\\daemon8.exe' serve --browser 'http://127.0.0.1:9222/devtools/browser/abc'; exit $LASTEXITCODE"
        );
    }

    #[test]
    fn prepending_instruction_block_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("CLAUDE.md");

        assert!(matches!(
            prepend_instruction_block(&path).unwrap(),
            InstructionWrite::Written
        ));
        assert!(matches!(
            prepend_instruction_block(&path).unwrap(),
            InstructionWrite::AlreadyPresent
        ));

        let contents = std::fs::read_to_string(path).unwrap();
        assert_eq!(contents.matches(INSTRUCTION_HEADING).count(), 1);
        assert!(contents.contains(INSTRUCTION_VERSION_MARKER));
        assert!(contents.contains("ask once whether the user wants to recover recent conversation history across AI providers"));
        assert!(contents.contains(
            "run recent recall with `build_context_snapshot` and omit the `facets` filter"
        ));
    }

    #[test]
    fn stale_instruction_block_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("AGENTS.md");
        std::fs::write(
            &path,
            "Intro\n\n## Daemon8 — Runtime Observation Layer (ALWAYS ON)\n\nOld daemon8 runtime note.\n\n## Next Section\n\nKeep me.\n",
        )
        .unwrap();

        assert!(matches!(
            prepend_instruction_block(&path).unwrap(),
            InstructionWrite::Updated
        ));

        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.starts_with("Intro\n\n## Daemon8 -- Runtime Observation Layer"));
        assert!(contents.contains(INSTRUCTION_VERSION_MARKER));
        assert!(contents.contains("Do not run conversation recall automatically."));
        assert!(contents.contains("## Next Section\n\nKeep me."));
        assert!(!contents.contains("Old daemon8 runtime note."));
    }
}
