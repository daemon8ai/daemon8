// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Universal CLI hook telemetry handler.
//!
//! Invoked by Claude Code, Cursor CLI, Gemini CLI, GitHub Copilot CLI,
//! OpenAI Codex CLI, and Continue.dev. Reads a stdin JSON payload, normalizes
//! the event name across the seven case conventions, resolves project-local
//! `.daemon8.toml`, and POSTs provider facts to the daemon's `/ingest`
//! endpoint as `cli.*` observations.
//!
//! Performance budget: <20ms per invocation. Uses blocking `ureq` rather than
//! async, since a hook fires once per event and pays a runtime startup cost
//! if we boot tokio here.

use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use daemon8_types::{SYSTEM_TAG, Severity};

use crate::cli_config::{self, CliConfig};
use crate::config;

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(clap::Args, Default)]
pub struct CliHookArgs {
    /// Explicit tool identifier, e.g. "claude-code" / "cursor" / "gemini-cli".
    /// When omitted, detected from env vars.
    #[arg(long)]
    pub tool: Option<String>,

    /// Override the daemon host:port for POST /ingest. Defaults to config.
    #[arg(long)]
    pub daemon_url: Option<String>,

    /// Emit JSON diagnostic to stderr even when enrollment is disabled.
    #[arg(long)]
    pub verbose: bool,
}

// ---------------------------------------------------------------------------
// Hook payload (common shape across CLIs)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct HookInput {
    #[serde(default, alias = "session_id", alias = "sessionId")]
    session_id: Option<String>,
    #[serde(
        default,
        alias = "hook_event_name",
        alias = "hookEventName",
        alias = "event"
    )]
    hook_event_name: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, alias = "conversation_id", alias = "conversationId")]
    conversation_id: Option<String>,
    #[serde(default, alias = "turn_id", alias = "turnId")]
    turn_id: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default, alias = "tool_name", alias = "toolName")]
    tool_name: Option<String>,
    #[serde(default, alias = "tool_use_id", alias = "toolUseId")]
    tool_use_id: Option<String>,
    #[serde(default, alias = "tool_input", alias = "toolInput")]
    tool_input: HookToolInput,
    #[serde(default, alias = "tool_response", alias = "toolResponse")]
    tool_response: Option<Value>,
    #[allow(dead_code)]
    #[serde(
        default,
        alias = "last_assistant_message",
        alias = "lastAssistantMessage"
    )]
    last_assistant_message: Option<String>,
    #[allow(dead_code)]
    #[serde(default, alias = "stop_hook_active", alias = "stopHookActive")]
    stop_hook_active: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct HookToolInput {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "filePath", alias = "file_path", alias = "path")]
    file_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Normalized CLI hook event
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliHookEvent {
    SessionStarted,
    PromptSubmitted,
    ToolPreUse,
    ToolPostUse,
    PermissionRequested,
    SessionEnded,
    PreCompact,
    PostCompact,
    Other,
}

// ---------------------------------------------------------------------------
// Hook policy
// ---------------------------------------------------------------------------
//
// Hooks are investigation telemetry. They record session, turn, tool, shell
// command, file edit, permission, error, compaction, and lifecycle facts when
// the provider includes them. They do not derive cause/effect, inject prompts,
// or route agent work.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookPolicy {
    /// Drop the event entirely. No observation is emitted.
    Drop,
    /// Emit a `cli.*` observation at this severity.
    Observe { severity: Severity },
}

impl HookPolicy {
    /// Map a normalized hook event to its policy.
    ///
    /// Session boundaries, prompt submissions, and compactions surface at
    /// `info` so they appear naturally in default queries. Permission
    /// requests escalate to `warn` because the user's attention matters.
    /// Tool pre/post events emit at `debug` because they are useful for
    /// targeted diagnosis but flood the stream when surfaced at `info`.
    /// Unknown events drop.
    fn decide(event: CliHookEvent) -> HookPolicy {
        use CliHookEvent::*;
        use Severity::*;
        match event {
            SessionStarted | SessionEnded | PromptSubmitted | PreCompact | PostCompact => {
                HookPolicy::Observe { severity: Info }
            }
            PermissionRequested => HookPolicy::Observe { severity: Warn },
            ToolPreUse | ToolPostUse => HookPolicy::Observe { severity: Debug },
            Other => HookPolicy::Drop,
        }
    }
}

fn normalize_event(raw: &str) -> CliHookEvent {
    match raw.to_ascii_lowercase().as_str() {
        "sessionstart" => CliHookEvent::SessionStarted,
        "sessionend" | "stop" => CliHookEvent::SessionEnded,
        "userpromptsubmit" | "userpromptsubmitted" | "beforesubmitprompt" => {
            CliHookEvent::PromptSubmitted
        }
        "pretooluse" | "beforetool" => CliHookEvent::ToolPreUse,
        "posttooluse" | "aftertool" => CliHookEvent::ToolPostUse,
        "permissionrequest" => CliHookEvent::PermissionRequested,
        "beforeagent" | "afteragent" | "afteragentresponse" => CliHookEvent::Other,
        "precompact" | "precompress" | "session.compacting" => CliHookEvent::PreCompact,
        "postcompact" => CliHookEvent::PostCompact,
        _ => CliHookEvent::Other,
    }
}

// ---------------------------------------------------------------------------
// Tool detection
// ---------------------------------------------------------------------------

fn detect_tool(explicit: Option<&str>) -> String {
    if let Some(t) = explicit {
        return t.to_string();
    }
    // Env-var sniffing in order of specificity.
    if env::var_os("CLAUDE_PROJECT_DIR").is_some() {
        return "claude-code".into();
    }
    if env::var_os("GEMINI_SESSION_ID").is_some() || env::var_os("GEMINI_PROJECT_DIR").is_some() {
        return "gemini-cli".into();
    }
    if env::var_os("COPILOT_AGENT_SESSION_ID").is_some() {
        return "copilot-cli".into();
    }
    if env::var_os("CODEX_SESSION_ID").is_some() {
        return "codex-cli".into();
    }
    if env::var_os("CURSOR_SESSION_ID").is_some() {
        return "cursor".into();
    }
    "unknown".into()
}

// ---------------------------------------------------------------------------
// CLI session card
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CliSessionCard<'a> {
    event: &'static str,
    session_ref: String,
    host: String,
    tool: &'a str,
    tool_version: Option<String>,
    model: Option<&'a str>,
    session_id: &'a str,
    project_slug: String,
    cwd: String,
    scope: &'a [String],
    pid: u32,
    parent_pid: Option<u32>,
    started_at: String,
}

fn session_ref(host: &str, tool: &str, session_id: &str) -> String {
    format!("{host}:{tool}:{session_id}")
}

fn hostname() -> String {
    env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .arg("-s")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "localhost".into())
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Minimal ISO 8601 without pulling chrono. Good enough for diagnostic.
    format!("@{secs}")
}

fn parent_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        // SAFETY: getppid is always safe to call.
        unsafe { Some(libc::getppid() as u32) }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

/// Hook handlers must NEVER hard-fail in a way that breaks the host CLI.
/// Any error is absorbed; optional stderr diagnostic when `DAEMON8_HOOK_VERBOSE`
/// is set. Returns `Ok(())` unconditionally.
pub fn cmd_cli_hook(args: CliHookArgs) -> Result<()> {
    if let Err(e) = run(args)
        && env::var_os("DAEMON8_HOOK_VERBOSE").is_some()
    {
        eprintln!("[daemon8 cli-hook] soft-failed: {e:#}");
    }
    Ok(())
}

fn run(args: CliHookArgs) -> Result<()> {
    let (input, raw_input) = read_input()?;
    let cwd = resolve_cwd(&input);
    let (cli_cfg, report) = cli_config::load(&cwd);

    if cli_cfg.project_config_path.is_none() {
        return Ok(());
    }

    if report.has_errors() && args.verbose {
        if let Some(ref e) = report.user_error {
            eprintln!("[daemon8 cli-hook] user config: {e}");
        }
        if let Some(ref e) = report.project_error {
            eprintln!("[daemon8 cli-hook] project config: {e}");
        }
    }

    let tool = detect_tool(args.tool.as_deref());
    if !cli_cfg.enrollment_enabled_for(&tool) {
        return Ok(());
    }

    let Some(raw_event) = input.hook_event_name.as_deref() else {
        return Ok(());
    };
    let event = normalize_event(raw_event);
    let HookPolicy::Observe { severity } = HookPolicy::decide(event) else {
        return Ok(());
    };

    let Some(session_id) = effective_session_id(&input) else {
        return Ok(());
    };

    let host = hostname();
    let session_ref = session_ref(&host, &tool, &session_id);
    let slug = cli_cfg.resolved_slug(&cwd);
    let daemon_url = resolve_daemon_url(args.daemon_url.as_deref())?;

    for observation in build_observations(ObservationContext {
        event,
        severity,
        cli_cfg: &cli_cfg,
        tool: &tool,
        session_ref: &session_ref,
        host: &host,
        session_id: &session_id,
        slug: &slug,
        cwd: &cwd,
        input: &input,
        raw_input: &raw_input,
    }) {
        post_json(&daemon_url, &observation)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_input() -> Result<(HookInput, Value)> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read stdin")?;
    if buf.trim().is_empty() {
        return Ok((HookInput::default(), Value::Null));
    }
    let raw: Value = serde_json::from_str(&buf).context("parse stdin JSON")?;
    let parsed = serde_json::from_value(raw.clone()).context("parse hook input")?;
    Ok((parsed, raw))
}

fn resolve_cwd(input: &HookInput) -> PathBuf {
    input
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn effective_session_id(input: &HookInput) -> Option<String> {
    if let Some(ref s) = input.session_id {
        return Some(s.clone());
    }
    if let Some(ref s) = input.conversation_id {
        return Some(s.clone());
    }
    for var in [
        "CLAUDE_SESSION_ID",
        "GEMINI_SESSION_ID",
        "COPILOT_AGENT_SESSION_ID",
        "CODEX_SESSION_ID",
        "CURSOR_SESSION_ID",
    ] {
        if let Ok(v) = env::var(var)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    None
}

fn resolve_daemon_url(explicit: Option<&str>) -> Result<String> {
    if let Some(u) = explicit {
        return Ok(u.to_string());
    }
    // Try to load the daemon config to find host:port. Fall back to default.
    let cfg = config::load(None).unwrap_or_default();
    Ok(format!("http://{}:{}", cfg.server.host, cfg.server.port))
}

// ---------------------------------------------------------------------------
// Ingest posts
// ---------------------------------------------------------------------------

struct JoinContext<'a> {
    cli_cfg: &'a CliConfig,
    tool: &'a str,
    session_ref: &'a str,
    host: &'a str,
    session_id: &'a str,
    slug: &'a str,
    cwd: &'a Path,
    input: &'a HookInput,
}

struct ObservationContext<'a> {
    event: CliHookEvent,
    severity: Severity,
    cli_cfg: &'a CliConfig,
    tool: &'a str,
    session_ref: &'a str,
    host: &'a str,
    session_id: &'a str,
    slug: &'a str,
    cwd: &'a Path,
    input: &'a HookInput,
    raw_input: &'a Value,
}

fn build_observations(ctx: ObservationContext<'_>) -> Vec<Value> {
    let severity = ctx.severity;
    let mut observations = build_observations_inner(ctx);
    let severity_str = severity.to_string();
    for obs in &mut observations {
        if let Some(map) = obs.as_object_mut() {
            map.insert("severity".to_string(), Value::String(severity_str.clone()));
        }
    }
    observations
}

fn build_observations_inner(ctx: ObservationContext<'_>) -> Vec<Value> {
    let mut observations = Vec::new();

    match ctx.event {
        CliHookEvent::SessionStarted => {
            observations.push(build_joined(JoinContext {
                cli_cfg: ctx.cli_cfg,
                tool: ctx.tool,
                session_ref: ctx.session_ref,
                host: ctx.host,
                session_id: ctx.session_id,
                slug: ctx.slug,
                cwd: ctx.cwd,
                input: ctx.input,
            }));
            if let Some(banner) = ctx.cli_cfg.enrollment.banner.as_deref() {
                observations.push(build_banner(ctx.session_ref, banner));
            }
        }
        CliHookEvent::PromptSubmitted => {
            observations.push(build_prompt_submitted(
                ctx.tool,
                ctx.session_ref,
                ctx.session_id,
                ctx.slug,
                ctx.cwd,
                ctx.input,
                ctx.raw_input,
            ));
        }
        CliHookEvent::ToolPreUse => {
            observations.push(build_tool_event(
                "cli.tool_pre_use",
                ctx.tool,
                ctx.session_ref,
                ctx.session_id,
                ctx.slug,
                ctx.cwd,
                ctx.input,
                ctx.raw_input,
            ));
        }
        CliHookEvent::ToolPostUse => {
            observations.push(build_tool_event(
                "cli.tool_post_use",
                ctx.tool,
                ctx.session_ref,
                ctx.session_id,
                ctx.slug,
                ctx.cwd,
                ctx.input,
                ctx.raw_input,
            ));
        }
        CliHookEvent::PermissionRequested => {
            observations.push(build_permission_requested(
                ctx.tool,
                ctx.session_ref,
                ctx.session_id,
                ctx.slug,
                ctx.cwd,
                ctx.input,
                ctx.raw_input,
            ));
        }
        CliHookEvent::SessionEnded => {
            observations.push(build_minimal("cli.session_ended", ctx.session_ref));
        }
        CliHookEvent::PreCompact => {
            observations.push(build_minimal("cli.precompact", ctx.session_ref));
        }
        CliHookEvent::PostCompact => {
            observations.push(build_minimal("cli.postcompact", ctx.session_ref));
        }
        CliHookEvent::Other => {}
    }

    observations
}

fn build_joined(ctx: JoinContext<'_>) -> Value {
    let card = CliSessionCard {
        event: "cli.session_started",
        session_ref: ctx.session_ref.to_string(),
        host: ctx.host.to_string(),
        tool: ctx.tool,
        tool_version: env::var("DAEMON8_TOOL_VERSION").ok(),
        model: ctx.input.model.as_deref(),
        session_id: ctx.session_id,
        project_slug: ctx.slug.to_string(),
        cwd: ctx.cwd.display().to_string(),
        scope: &ctx.cli_cfg.enrollment.scope,
        pid: std::process::id(),
        parent_pid: parent_pid(),
        started_at: now_iso(),
    };

    json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "cli-hook",
        "severity": "info",
        "tags": [SYSTEM_TAG],
        "data": card,
    })
}

fn build_banner(session_ref: &str, banner: &str) -> Value {
    json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "cli-hook",
        "severity": "info",
        "tags": [SYSTEM_TAG],
        "data": {
            "event": "cli.banner",
            "session_ref": session_ref,
            "banner": banner,
        },
    })
}

fn build_minimal(event: &str, session_ref: &str) -> Value {
    json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "cli-hook",
        "severity": "info",
        "tags": [SYSTEM_TAG],
        "data": {
            "event": event,
            "session_ref": session_ref,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn build_prompt_submitted(
    tool: &str,
    session_ref: &str,
    session_id: &str,
    slug: &str,
    cwd: &Path,
    input: &HookInput,
    raw_input: &Value,
) -> Value {
    let prompt = extract_prompt(input, raw_input);
    json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "cli-hook",
        "severity": "info",
        "tags": [SYSTEM_TAG],
        "data": {
            "event": "cli.prompt_submitted",
            "session_ref": session_ref,
            "tool": tool,
            "model": input.model,
            "cwd": cwd.display().to_string(),
            "session_id": session_id,
            "project_slug": slug,
            "conversation_id": input.conversation_id,
            "prompt_text": prompt,
            "prompt": prompt,
            "raw": raw_input,
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn build_tool_event(
    event: &str,
    tool: &str,
    session_ref: &str,
    session_id: &str,
    slug: &str,
    cwd: &Path,
    input: &HookInput,
    raw_input: &Value,
) -> Value {
    json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "cli-hook",
        "severity": "info",
        "tags": [SYSTEM_TAG],
        "data": {
            "event": event,
            "session_ref": session_ref,
            "tool": tool,
            "model": input.model,
            "cwd": cwd.display().to_string(),
            "session_id": session_id,
            "project_slug": slug,
            "turn_id": input.turn_id,
            "tool_name": input.tool_name,
            "tool_use_id": input.tool_use_id,
            "command": input.tool_input.command,
            "file_path": input.tool_input.file_path,
            "description": input.tool_input.description,
            "tool_status": tool_response_status(input.tool_response.as_ref()),
            "tool_error": tool_response_error(input.tool_response.as_ref()),
            "tool_response": input.tool_response,
            "raw": raw_input,
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn build_permission_requested(
    tool: &str,
    session_ref: &str,
    session_id: &str,
    slug: &str,
    cwd: &Path,
    input: &HookInput,
    raw_input: &Value,
) -> Value {
    json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "cli-hook",
        "severity": "info",
        "tags": [SYSTEM_TAG],
        "data": {
            "event": "cli.permission_requested",
            "session_ref": session_ref,
            "tool": tool,
            "model": input.model,
            "cwd": cwd.display().to_string(),
            "session_id": session_id,
            "project_slug": slug,
            "turn_id": input.turn_id,
            "tool_name": input.tool_name,
            "command": input.tool_input.command,
            "file_path": input.tool_input.file_path,
            "description": input.tool_input.description,
            "raw": raw_input,
        }
    })
}

fn tool_response_status(response: Option<&Value>) -> Option<String> {
    let response = response?;
    for key in ["status", "exit_code", "exitCode"] {
        if let Some(value) = response.get(key) {
            if let Some(text) = value.as_str() {
                return Some(text.to_string());
            }
            if let Some(number) = value.as_i64() {
                return Some(number.to_string());
            }
        }
    }
    None
}

fn tool_response_error(response: Option<&Value>) -> Option<bool> {
    let response = response?;
    if let Some(value) = response.get("is_error").or_else(|| response.get("isError")) {
        return value.as_bool();
    }
    Some(response.get("error").is_some())
}

fn extract_prompt(input: &HookInput, raw: &Value) -> Option<String> {
    if let Some(prompt) = input.prompt.as_deref()
        && !prompt.trim().is_empty()
    {
        return Some(prompt.trim().to_string());
    }

    for path in [
        &["prompt"][..],
        &["message"][..],
        &["input"][..],
        &["text"][..],
        &["data", "prompt"][..],
        &["data", "message"][..],
        &["data", "input"][..],
        &["payload", "prompt"][..],
        &["payload", "message"][..],
    ] {
        if let Some(value) = read_json_path(raw, path)
            && let Some(text) = value.as_str()
            && !text.trim().is_empty()
        {
            return Some(text.trim().to_string());
        }
    }
    None
}

fn read_json_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = value;
    for part in path {
        cursor = cursor.get(*part)?;
    }
    Some(cursor)
}

fn post_json(daemon_url: &str, body: &Value) -> Result<()> {
    let url = format!("{}/ingest", daemon_url.trim_end_matches('/'));
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(2000)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    agent
        .post(&url)
        .content_type("application/json")
        .send_json(body)
        .map_err(|e| anyhow::anyhow!("POST {url}: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_claude_events() {
        assert_eq!(
            normalize_event("SessionStart"),
            CliHookEvent::SessionStarted
        );
        assert_eq!(normalize_event("SessionEnd"), CliHookEvent::SessionEnded);
        assert_eq!(normalize_event("PreToolUse"), CliHookEvent::ToolPreUse);
        assert_eq!(normalize_event("PostToolUse"), CliHookEvent::ToolPostUse);
        assert_eq!(normalize_event("PreCompact"), CliHookEvent::PreCompact);
        assert_eq!(normalize_event("PostCompact"), CliHookEvent::PostCompact);
    }

    #[test]
    fn normalize_cursor_events() {
        assert_eq!(
            normalize_event("sessionStart"),
            CliHookEvent::SessionStarted
        );
        assert_eq!(normalize_event("sessionEnd"), CliHookEvent::SessionEnded);
        assert_eq!(normalize_event("preToolUse"), CliHookEvent::ToolPreUse);
        assert_eq!(normalize_event("preCompact"), CliHookEvent::PreCompact);
    }

    #[test]
    fn normalize_gemini_events() {
        assert_eq!(normalize_event("BeforeTool"), CliHookEvent::ToolPreUse);
        assert_eq!(normalize_event("AfterTool"), CliHookEvent::ToolPostUse);
        assert_eq!(normalize_event("PreCompress"), CliHookEvent::PreCompact);
    }

    #[test]
    fn normalize_codex_events() {
        assert_eq!(normalize_event("Stop"), CliHookEvent::SessionEnded);
        assert_eq!(
            normalize_event("PermissionRequest"),
            CliHookEvent::PermissionRequested
        );
        assert_eq!(
            normalize_event("UserPromptSubmit"),
            CliHookEvent::PromptSubmitted
        );
    }

    #[test]
    fn normalize_future_adapter_compaction_aliases() {
        assert_eq!(
            normalize_event("session.compacting"),
            CliHookEvent::PreCompact
        );
    }

    #[test]
    fn unknown_event_falls_to_other() {
        assert_eq!(normalize_event("RandomEvent"), CliHookEvent::Other);
    }

    #[test]
    fn session_ref_format_is_stable() {
        let id = session_ref("darkstar", "claude-code", "a3f1b2");
        assert_eq!(id, "darkstar:claude-code:a3f1b2");
    }

    #[test]
    fn detect_tool_respects_explicit_flag() {
        assert_eq!(detect_tool(Some("cursor")), "cursor");
    }

    #[test]
    fn hook_input_parses_claude_shape() {
        let json = r#"{"session_id":"abc123","hook_event_name":"SessionStart","cwd":"/tmp"}"#;
        let parsed: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.session_id.as_deref(), Some("abc123"));
        assert_eq!(parsed.hook_event_name.as_deref(), Some("SessionStart"));
    }

    #[test]
    fn hook_input_parses_cursor_shape() {
        // Cursor uses conversation_id + generation_id
        let json = r#"{"conversation_id":"conv-42","hook_event_name":"preToolUse"}"#;
        let parsed: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.conversation_id.as_deref(), Some("conv-42"));
    }

    #[test]
    fn extract_prompt_reads_known_fields() {
        let raw = serde_json::json!({"payload": {"message": "hello daemon8"}});
        assert_eq!(
            extract_prompt(&HookInput::default(), &raw).as_deref(),
            Some("hello daemon8")
        );
    }

    #[test]
    fn extract_prompt_prefers_structured_input_field() {
        let input = HookInput {
            prompt: Some("  hello codex  ".into()),
            ..HookInput::default()
        };
        let raw = serde_json::json!({"prompt": "fallback"});
        assert_eq!(extract_prompt(&input, &raw).as_deref(), Some("hello codex"));
    }

    #[test]
    fn build_permission_requested_keeps_normalized_fields_and_raw_payload() {
        let input = HookInput {
            model: Some("claude-sonnet-4-6".into()),
            turn_id: Some("turn-1".into()),
            tool_name: Some("Bash".into()),
            tool_input: HookToolInput {
                command: Some("cargo test".into()),
                description: Some("Escalated command".into()),
                file_path: None,
            },
            ..HookInput::default()
        };
        let raw = serde_json::json!({
            "hook_event_name": "PermissionRequest",
            "tool_input": {"command": "cargo test"}
        });

        let observation = build_permission_requested(
            "codex-cli",
            "host:codex-cli:session-1",
            "session-1",
            "daemon",
            Path::new("/tmp/project"),
            &input,
            &raw,
        );

        assert_eq!(observation["channel"], "cli-hook");
        assert_eq!(observation["data"]["event"], "cli.permission_requested");
        assert_eq!(
            observation["data"]["session_ref"],
            "host:codex-cli:session-1"
        );
        assert!(observation["data"].get("role").is_none());
        assert_eq!(observation["data"]["tool_name"], "Bash");
        assert_eq!(observation["data"]["command"], "cargo test");
        assert_eq!(observation["data"]["description"], "Escalated command");
        assert_eq!(observation["data"]["turn_id"], "turn-1");
        assert_eq!(
            observation["data"]["raw"]["hook_event_name"],
            "PermissionRequest"
        );
    }

    #[test]
    fn build_observations_for_pre_tool_use_emits_only_tool_event() {
        let cfg = CliConfig::default();
        let input = HookInput {
            model: Some("claude-sonnet-4-6".into()),
            turn_id: Some("turn-tool".into()),
            tool_name: Some("Bash".into()),
            tool_use_id: Some("tool-123".into()),
            tool_input: HookToolInput {
                command: Some("git status".into()),
                file_path: Some("src/main.rs".into()),
                description: None,
            },
            tool_response: Some(serde_json::json!({"status": "ok", "is_error": false})),
            ..HookInput::default()
        };
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_input": {"command": "git status"}
        });

        let observations = build_observations(ObservationContext {
            event: CliHookEvent::ToolPreUse,
            severity: Severity::Debug,
            cli_cfg: &cfg,
            tool: "codex-cli",
            session_ref: "host:codex-cli:session-1",
            host: "host",
            session_id: "session-1",
            slug: "daemon",
            cwd: Path::new("/tmp/project"),
            input: &input,
            raw_input: &raw,
        });

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0]["channel"], "cli-hook");
        assert_eq!(observations[0]["data"]["event"], "cli.tool_pre_use");
        assert_eq!(observations[0]["severity"], "debug");
        assert_eq!(
            observations[0]["data"]["session_ref"],
            "host:codex-cli:session-1"
        );
        assert!(observations[0]["data"].get("role").is_none());
        assert_eq!(observations[0]["data"]["tool_use_id"], "tool-123");
        assert_eq!(observations[0]["data"]["command"], "git status");
        assert_eq!(observations[0]["data"]["file_path"], "src/main.rs");
        assert_eq!(observations[0]["data"]["tool_status"], "ok");
        assert_eq!(observations[0]["data"]["tool_error"], false);
    }

    #[test]
    fn all_observations_carry_system_tag() {
        let mut cfg = CliConfig::default();
        cfg.enrollment.banner = Some("test banner".into());

        let input = HookInput {
            tool_name: Some("Bash".into()),
            tool_input: HookToolInput {
                command: Some("ls".into()),
                file_path: None,
                description: None,
            },
            ..HookInput::default()
        };
        let raw = serde_json::json!({});

        for event in [
            CliHookEvent::SessionStarted,
            CliHookEvent::PromptSubmitted,
            CliHookEvent::ToolPreUse,
            CliHookEvent::ToolPostUse,
            CliHookEvent::PermissionRequested,
            CliHookEvent::SessionEnded,
            CliHookEvent::PreCompact,
            CliHookEvent::PostCompact,
        ] {
            let observations = build_observations(ObservationContext {
                event,
                severity: Severity::Info,
                cli_cfg: &cfg,
                tool: "test",
                session_ref: "host:test:s1",
                host: "host",
                session_id: "s1",
                slug: "proj",
                cwd: Path::new("/tmp"),
                input: &input,
                raw_input: &raw,
            });
            for obs in &observations {
                assert_eq!(
                    obs["tags"],
                    serde_json::json!([daemon8_types::SYSTEM_TAG]),
                    "missing _system tag on {event:?} observation: {obs}"
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // HookPolicy
    // ---------------------------------------------------------------------

    #[test]
    fn policy_drops_unknown_events() {
        assert_eq!(HookPolicy::decide(CliHookEvent::Other), HookPolicy::Drop);
    }

    #[test]
    fn policy_observes_session_boundaries_at_info() {
        for event in [CliHookEvent::SessionStarted, CliHookEvent::SessionEnded] {
            assert_eq!(
                HookPolicy::decide(event),
                HookPolicy::Observe {
                    severity: Severity::Info
                },
                "{event:?} should be info"
            );
        }
    }

    #[test]
    fn policy_observes_prompts_and_compactions_at_info() {
        for event in [
            CliHookEvent::PromptSubmitted,
            CliHookEvent::PreCompact,
            CliHookEvent::PostCompact,
        ] {
            assert_eq!(
                HookPolicy::decide(event),
                HookPolicy::Observe {
                    severity: Severity::Info
                },
                "{event:?} should be info"
            );
        }
    }

    #[test]
    fn policy_elevates_permission_request_to_warn() {
        assert_eq!(
            HookPolicy::decide(CliHookEvent::PermissionRequested),
            HookPolicy::Observe {
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn policy_demotes_tool_events_to_debug() {
        for event in [CliHookEvent::ToolPreUse, CliHookEvent::ToolPostUse] {
            assert_eq!(
                HookPolicy::decide(event),
                HookPolicy::Observe {
                    severity: Severity::Debug
                },
                "{event:?} should be debug"
            );
        }
    }

    #[test]
    fn build_observations_applies_policy_severity_to_every_observation() {
        let mut cfg = CliConfig::default();
        cfg.enrollment.banner = Some("warn-banner".into());

        let input = HookInput::default();
        let raw = serde_json::json!({});

        let observations = build_observations(ObservationContext {
            event: CliHookEvent::SessionStarted,
            severity: Severity::Warn,
            cli_cfg: &cfg,
            tool: "test",
            session_ref: "host:test:s1",
            host: "host",
            session_id: "s1",
            slug: "proj",
            cwd: Path::new("/tmp"),
            input: &input,
            raw_input: &raw,
        });

        // SessionStarted with a banner emits two observations; both must carry
        // the policy-chosen severity, not the legacy hardcoded "info".
        assert_eq!(observations.len(), 2);
        for obs in &observations {
            assert_eq!(obs["severity"], "warn", "observation: {obs}");
        }
    }
}
