// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Universal CLI hook handler.
//!
//! Invoked by Claude Code, Cursor CLI, Gemini CLI, GitHub Copilot CLI,
//! OpenAI Codex CLI, and Continue.dev. Reads a stdin JSON payload, normalizes
//! the event name across the seven case conventions, resolves project-local
//! `.daemon8-cli.toml`, and POSTs the corresponding `agent.*` observation to
//! the daemon's `/ingest` endpoint.
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
}

// ---------------------------------------------------------------------------
// Normalized agent event
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentEvent {
    Joined,
    Heartbeat,
    Departed,
    PreCompact,
    PostCompact,
    Other,
}

fn normalize_event(raw: &str) -> AgentEvent {
    match raw.to_ascii_lowercase().as_str() {
        "sessionstart" => AgentEvent::Joined,
        "sessionend" | "stop" => AgentEvent::Departed,
        "pretooluse"
        | "beforetool"
        | "userpromptsubmit"
        | "userpromptsubmitted"
        | "beforesubmitprompt"
        | "beforeagent" => AgentEvent::Heartbeat,
        "posttooluse" | "aftertool" | "afteragent" | "afteragentresponse" => AgentEvent::Heartbeat,
        "precompact" | "precompress" | "session.compacting" => AgentEvent::PreCompact,
        "postcompact" => AgentEvent::PostCompact,
        _ => AgentEvent::Other,
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
// Worker card
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct WorkerCard<'a> {
    event: &'static str,
    agent_id: String,
    host: String,
    tool: &'a str,
    tool_version: Option<String>,
    model: Option<&'a str>,
    session_id: &'a str,
    project_slug: String,
    cwd: String,
    role: String,
    scope: &'a [String],
    pid: u32,
    parent_pid: Option<u32>,
    started_at: String,
    capabilities: Vec<&'static str>,
    pairs_with: Option<String>,
}

fn agent_id(host: &str, tool: &str, session_id: &str) -> String {
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
    let input = read_input()?;
    let cwd = resolve_cwd(&input);
    let (cli_cfg, report) = cli_config::load(&cwd);

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
    if matches!(event, AgentEvent::Other) {
        return Ok(());
    }

    let Some(session_id) = effective_session_id(&input) else {
        return Ok(());
    };

    let host = hostname();
    let agent_id = agent_id(&host, &tool, &session_id);
    let slug = cli_cfg.resolved_slug(&cwd);
    let daemon_url = resolve_daemon_url(args.daemon_url.as_deref())?;

    match event {
        AgentEvent::Joined => {
            post_joined(JoinContext {
                daemon_url: &daemon_url,
                cli_cfg: &cli_cfg,
                tool: &tool,
                agent_id: &agent_id,
                host: &host,
                session_id: &session_id,
                slug: &slug,
                cwd: &cwd,
                input: &input,
            })?;
            if let Some(banner) = cli_cfg.enrollment.banner.as_deref() {
                post_banner(&daemon_url, &agent_id, banner)?;
            }
        }
        AgentEvent::Heartbeat => {
            post_minimal(&daemon_url, "agent.heartbeat", &agent_id)?;
        }
        AgentEvent::Departed => {
            post_minimal(&daemon_url, "agent.departed", &agent_id)?;
        }
        AgentEvent::PreCompact => {
            // Phase 1: record that a compaction is about to happen. The
            // recovery-note injection path lands in Phase 2c.
            post_minimal(&daemon_url, "agent.precompact", &agent_id)?;
        }
        AgentEvent::PostCompact => {
            post_minimal(&daemon_url, "agent.postcompact", &agent_id)?;
        }
        AgentEvent::Other => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_input() -> Result<HookInput> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read stdin")?;
    if buf.trim().is_empty() {
        return Ok(HookInput::default());
    }
    serde_json::from_str(&buf).context("parse stdin JSON")
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
    daemon_url: &'a str,
    cli_cfg: &'a CliConfig,
    tool: &'a str,
    agent_id: &'a str,
    host: &'a str,
    session_id: &'a str,
    slug: &'a str,
    cwd: &'a Path,
    input: &'a HookInput,
}

fn post_joined(ctx: JoinContext<'_>) -> Result<()> {
    let card = WorkerCard {
        event: "agent.joined",
        agent_id: ctx.agent_id.to_string(),
        host: ctx.host.to_string(),
        tool: ctx.tool,
        tool_version: env::var("DAEMON8_TOOL_VERSION").ok(),
        model: ctx.input.model.as_deref(),
        session_id: ctx.session_id,
        project_slug: ctx.slug.to_string(),
        cwd: ctx.cwd.display().to_string(),
        role: ctx.cli_cfg.project.role_default.clone(),
        scope: &ctx.cli_cfg.enrollment.scope,
        pid: std::process::id(),
        parent_pid: parent_pid(),
        started_at: now_iso(),
        capabilities: default_capabilities(&ctx.cli_cfg.project.role_default),
        pairs_with: None,
    };

    let observation = json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "agent",
        "severity": "info",
        "data": card,
    });

    post_json(ctx.daemon_url, &observation)
}

fn post_banner(daemon_url: &str, agent_id: &str, banner: &str) -> Result<()> {
    let observation = json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "agent",
        "severity": "info",
        "data": {
            "event": "agent.banner",
            "agent_id": agent_id,
            "banner": banner,
        },
    });
    post_json(daemon_url, &observation)
}

fn post_minimal(daemon_url: &str, event: &str, agent_id: &str) -> Result<()> {
    let observation = json!({
        "app": "daemon8-cli-hook",
        "kind": "custom",
        "channel": "agent",
        "severity": "info",
        "data": {
            "event": event,
            "agent_id": agent_id,
        },
    });
    post_json(daemon_url, &observation)
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

fn default_capabilities(role: &str) -> Vec<&'static str> {
    match role {
        "watchdog" => vec!["reads_only"],
        _ => vec!["writes_code", "runs_tests"],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_claude_events() {
        assert_eq!(normalize_event("SessionStart"), AgentEvent::Joined);
        assert_eq!(normalize_event("SessionEnd"), AgentEvent::Departed);
        assert_eq!(normalize_event("PreToolUse"), AgentEvent::Heartbeat);
        assert_eq!(normalize_event("PostToolUse"), AgentEvent::Heartbeat);
        assert_eq!(normalize_event("PreCompact"), AgentEvent::PreCompact);
        assert_eq!(normalize_event("PostCompact"), AgentEvent::PostCompact);
    }

    #[test]
    fn normalize_cursor_events() {
        assert_eq!(normalize_event("sessionStart"), AgentEvent::Joined);
        assert_eq!(normalize_event("sessionEnd"), AgentEvent::Departed);
        assert_eq!(normalize_event("preToolUse"), AgentEvent::Heartbeat);
        assert_eq!(normalize_event("preCompact"), AgentEvent::PreCompact);
    }

    #[test]
    fn normalize_gemini_events() {
        assert_eq!(normalize_event("BeforeTool"), AgentEvent::Heartbeat);
        assert_eq!(normalize_event("AfterTool"), AgentEvent::Heartbeat);
        assert_eq!(normalize_event("PreCompress"), AgentEvent::PreCompact);
    }

    #[test]
    fn normalize_codex_events() {
        assert_eq!(normalize_event("Stop"), AgentEvent::Departed);
        assert_eq!(normalize_event("UserPromptSubmit"), AgentEvent::Heartbeat);
    }

    #[test]
    fn normalize_opencode_events() {
        assert_eq!(
            normalize_event("session.compacting"),
            AgentEvent::PreCompact
        );
    }

    #[test]
    fn unknown_event_falls_to_other() {
        assert_eq!(normalize_event("RandomEvent"), AgentEvent::Other);
    }

    #[test]
    fn agent_id_format_is_stable() {
        let id = agent_id("darkstar", "claude-code", "a3f1b2");
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
}
