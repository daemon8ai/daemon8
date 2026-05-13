// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! First-run presentation (D4).
//!
//! The scanner (D3) produces a [`DiscoveryPlan`]. This module turns that
//! plan into either an interactive Y/n prompt or a structured log entry,
//! depending on whether stdout/stdin look like a real terminal.
//!
//! Public surface intentionally narrow:
//!
//! - [`detect_mode`] returns [`PresentationMode::Interactive`] only when
//!   both stdin and stdout are TTYs. CI runners, launchd, systemd, and
//!   nohup'd processes all fall to [`PresentationMode::NonInteractive`].
//! - [`render_plan`] writes the human-facing report to any
//!   [`std::io::Write`]. Tests assert against the rendered string; the
//!   serve loop writes to stdout.
//! - [`prompt_confirm`] reads a single line from stdin in interactive
//!   mode and returns a [`PromptOutcome`]. Non-interactive mode returns
//!   [`PromptOutcome::NonInteractiveAutoConfirm`] after logging a warn
//!   so silent CI auto-learning surfaces in logs.
//!
//! The `tty_check` parameter on [`prompt_confirm`] keeps the function
//! testable: production passes [`detect_mode`], tests pass a closure
//! returning the desired mode.

use std::io::{BufRead, Write};
use std::time::Duration;

use crate::discovery::scanner::{DiscoveryPlan, LibrarianStatus, ResolvedSource, TemplateMiss};

/// Inactivity timeout on the interactive Y/n prompt. After this elapses
/// without input, the caller treats the prompt as Declined and writes
/// the skip marker — daemon8 will not hang forever waiting for
/// keystrokes from a forgotten foreground daemon.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationMode {
    Interactive,
    NonInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptOutcome {
    Confirmed,
    Declined,
    NonInteractiveAutoConfirm,
}

#[derive(Debug, thiserror::Error)]
pub enum PresentationError {
    #[error("failed to read stdin: {0}")]
    Io(#[from] std::io::Error),
}

/// Returns [`PresentationMode::Interactive`] only when both stdin and
/// stdout are attached to a terminal. Anything else (`/dev/null` from
/// launchd, a redirected pipe, a CI runner) returns
/// [`PresentationMode::NonInteractive`].
pub fn detect_mode() -> PresentationMode {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        PresentationMode::Interactive
    } else {
        PresentationMode::NonInteractive
    }
}

/// Render the plan to the provided writer. The format follows the
/// canonical Case A / B / C layout from the project-aware onboarding
/// spec, branching on [`DiscoveryPlan::librarian_status`].
pub fn render_plan(plan: &DiscoveryPlan, writer: &mut dyn Write) -> std::io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    writeln!(writer, "daemon8 {version}")?;
    writeln!(writer)?;

    let tags = if plan.classification.tags.is_empty() {
        "(none)".to_string()
    } else {
        plan.classification.tags.join(", ")
    };
    writeln!(writer, "Detected: {tags}")?;
    writeln!(writer, "Root: {}", plan.classification.root.display())?;
    writeln!(writer)?;

    match plan.librarian_status {
        LibrarianStatus::CacheHit | LibrarianStatus::CacheStale => {
            render_case_a(plan, writer)?;
        }
        LibrarianStatus::TemplatesPartial | LibrarianStatus::TemplatesMissing => {
            render_case_b(plan, writer)?;
        }
    }

    if !plan.user_overrides.is_empty() {
        writeln!(writer)?;
        writeln!(
            writer,
            "User overrides from .daemon8.toml ({}) are honored separately and not part of this confirmation.",
            plan.user_overrides.len()
        )?;
    }

    Ok(())
}

fn render_case_a(plan: &DiscoveryPlan, writer: &mut dyn Write) -> std::io::Result<()> {
    let resolved_count = plan.resolved_sources.len();
    if resolved_count == 0 && plan.template_misses.is_empty() {
        writeln!(
            writer,
            "Cached project topology found but no source paths still resolve."
        )?;
        writeln!(
            writer,
            "Re-register sources and persist project topology? [Y/n]"
        )?;
        return Ok(());
    }

    if resolved_count > 0 {
        writeln!(
            writer,
            "Applied {resolved_count} source templates from librarian (learned from prior projects):"
        )?;
        for source in &plan.resolved_sources {
            render_resolved(source, writer)?;
        }
    }

    if !plan.template_misses.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Templates that matched but paths not found:")?;
        for miss in &plan.template_misses {
            render_miss(miss, writer)?;
        }
    }

    writeln!(writer)?;
    writeln!(
        writer,
        "Register these and persist project topology for next time? [Y/n]"
    )?;
    Ok(())
}

fn render_case_b(plan: &DiscoveryPlan, writer: &mut dyn Write) -> std::io::Result<()> {
    let resolved_count = plan.resolved_sources.len();
    if resolved_count == 0 {
        writeln!(
            writer,
            "No source templates matched these tags. daemon8 asked the agent in your"
        )?;
        writeln!(
            writer,
            "session to investigate; no source_template entries were written during the"
        )?;
        writeln!(writer, "wait window.")?;
        writeln!(writer)?;
        writeln!(
            writer,
            "Nothing to register yet. Continue serving without auto-sources? [Y/n]"
        )?;
        return Ok(());
    }

    writeln!(
        writer,
        "No prior source_templates matched these tags. daemon8 asked the agent in your"
    )?;
    writeln!(
        writer,
        "session to investigate. Agent discovered {resolved_count} source instance(s) for this project:"
    )?;
    writeln!(writer)?;
    writeln!(writer, "Discovered:")?;
    for source in &plan.resolved_sources {
        render_resolved(source, writer)?;
    }

    if !plan.template_misses.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Templates that matched but paths not found:")?;
        for miss in &plan.template_misses {
            render_miss(miss, writer)?;
        }
    }

    writeln!(writer)?;
    writeln!(
        writer,
        "Register these and persist learnings to librarian for future projects? [Y/n]"
    )?;
    Ok(())
}

fn render_resolved(source: &ResolvedSource, writer: &mut dyn Write) -> std::io::Result<()> {
    let tag_suffix = if source.tags.is_empty() {
        String::new()
    } else {
        format!(" ({})", source.tags.join(", "))
    };
    writeln!(writer, "  [+] {}{tag_suffix}", source.kind)?;
    writeln!(writer, "      {}", source.resolved_path.display())?;
    Ok(())
}

fn render_miss(miss: &TemplateMiss, writer: &mut dyn Write) -> std::io::Result<()> {
    let reason = match &miss.reason {
        crate::discovery::scanner::TemplateMissReason::PathNotFound => "path not found".to_string(),
        crate::discovery::scanner::TemplateMissReason::InvalidPattern(msg) => {
            format!("invalid pattern: {msg}")
        }
        crate::discovery::scanner::TemplateMissReason::VersionMismatch => {
            "version constraint mismatch".to_string()
        }
    };
    writeln!(writer, "  [-] {} ({reason})", miss.locator_pattern)?;
    Ok(())
}

/// Prompt the user. `tty_check` lets tests inject a deterministic mode.
/// In production, callers pass [`detect_mode`].
///
/// Non-interactive mode is loud: a `tracing::warn!` records the
/// auto-confirm decision and the plan summary so CI logs surface what
/// was just learned by accident.
pub fn prompt_confirm(
    plan: &DiscoveryPlan,
    tty_check: fn() -> PresentationMode,
) -> Result<PromptOutcome, PresentationError> {
    match tty_check() {
        PresentationMode::NonInteractive => {
            tracing::warn!(
                status = ?plan.librarian_status,
                resolved = plan.resolved_sources.len(),
                misses = plan.template_misses.len(),
                root = %plan.classification.root.display(),
                "non-interactive discovery: auto-confirming registration without user prompt"
            );
            Ok(PromptOutcome::NonInteractiveAutoConfirm)
        }
        PresentationMode::Interactive => {
            prompt_confirm_interactive(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
        }
    }
}

/// Read a single Y/n response from the provided reader. Pulled out of
/// [`prompt_confirm`] so tests can drive it with a `Cursor`.
pub fn prompt_confirm_interactive(
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<PromptOutcome, PresentationError> {
    write!(writer, "> ")?;
    writer.flush()?;

    let mut buf = String::new();
    let read = reader.read_line(&mut buf)?;
    if read == 0 {
        // EOF before any input — treat as decline. Caller writes the
        // skip marker so future serves do not re-prompt.
        return Ok(PromptOutcome::Declined);
    }

    let answer = buf.trim().to_ascii_lowercase();
    let outcome = match answer.as_str() {
        // Empty input takes the default (Y).
        "" | "y" | "yes" => PromptOutcome::Confirmed,
        _ => PromptOutcome::Declined,
    };
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::path::PathBuf;

    use daemon8_types::{Platform, ProjectClassification, SourceKind};

    use crate::discovery::scanner::{
        DiscoveryPlan, LibrarianStatus, ResolvedSource, TemplateMiss, TemplateMissReason,
    };

    use super::*;

    fn classification(tags: &[&str]) -> ProjectClassification {
        ProjectClassification {
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
            framework_versions: BTreeMap::new(),
            root: PathBuf::from("/tmp/fixture"),
            manifests: BTreeMap::new(),
            platform: Platform::Macos,
        }
    }

    fn resolved(path: &str, tags: &[&str]) -> ResolvedSource {
        ResolvedSource {
            template_id: Some("t1".into()),
            kind: SourceKind::Log,
            resolved_path: PathBuf::from(path),
            parser: None,
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
            version_constraint: None,
        }
    }

    fn miss(pattern: &str, reason: TemplateMissReason) -> TemplateMiss {
        TemplateMiss {
            template_id: "t-miss".into(),
            locator_pattern: pattern.into(),
            reason,
        }
    }

    fn base_plan(status: LibrarianStatus) -> DiscoveryPlan {
        DiscoveryPlan {
            classification: classification(&["react-native", "git-repo"]),
            librarian_status: status,
            resolved_sources: Vec::new(),
            template_misses: Vec::new(),
            user_overrides: Vec::new(),
            awaiting_agent: false,
            cache_used: false,
            cache_age_secs: None,
        }
    }

    fn render(plan: &DiscoveryPlan) -> String {
        let mut buf = Vec::new();
        render_plan(plan, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn render_plan_case_a_cache_hit() {
        let mut plan = base_plan(LibrarianStatus::CacheHit);
        plan.cache_used = true;
        plan.resolved_sources.push(resolved(
            "/tmp/fixture/runtime.log",
            &["fixture", "rn-bridge"],
        ));
        plan.resolved_sources
            .push(resolved("/tmp/fixture/metro.log", &["metro"]));

        let out = render(&plan);
        assert!(out.contains("Detected: react-native, git-repo"));
        assert!(out.contains("Root: /tmp/fixture"));
        assert!(out.contains("Applied 2 source templates"));
        assert!(out.contains("/tmp/fixture/runtime.log"));
        assert!(out.contains("/tmp/fixture/metro.log"));
        assert!(out.contains("Register these and persist project topology"));
    }

    #[test]
    fn render_plan_case_a_with_template_misses() {
        let mut plan = base_plan(LibrarianStatus::CacheStale);
        plan.resolved_sources
            .push(resolved("/tmp/fixture/runtime.log", &["log"]));
        plan.template_misses.push(miss(
            "~/.expo/logs/runtime.log",
            TemplateMissReason::PathNotFound,
        ));

        let out = render(&plan);
        assert!(out.contains("Templates that matched but paths not found"));
        assert!(out.contains("~/.expo/logs/runtime.log"));
        assert!(out.contains("path not found"));
    }

    #[test]
    fn render_plan_case_b_templates_missing() {
        let plan = base_plan(LibrarianStatus::TemplatesMissing);
        let out = render(&plan);
        assert!(out.contains("No source templates matched these tags"));
        assert!(out.contains("Nothing to register yet"));
    }

    #[test]
    fn render_plan_case_b_templates_partial_with_agent_results() {
        let mut plan = base_plan(LibrarianStatus::TemplatesPartial);
        plan.resolved_sources
            .push(resolved("/tmp/fixture/agent-found.log", &["agent"]));
        let out = render(&plan);
        assert!(out.contains("Agent discovered 1 source instance(s)"));
        assert!(out.contains("/tmp/fixture/agent-found.log"));
        assert!(out.contains("Register these and persist learnings to librarian"));
    }

    #[test]
    fn render_plan_empty_resolved_sources_case_a_handled() {
        let plan = base_plan(LibrarianStatus::CacheHit);
        let out = render(&plan);
        assert!(out.contains("Cached project topology found but no source paths still resolve"));
        assert!(out.contains("Re-register sources"));
    }

    #[test]
    fn render_plan_includes_user_override_note_when_present() {
        let mut plan = base_plan(LibrarianStatus::TemplatesPartial);
        plan.resolved_sources
            .push(resolved("/tmp/fixture/agent-found.log", &["agent"]));
        plan.user_overrides.push(crate::config::SourceConfig::File(
            crate::config::FileSourceConfig {
                path: "/tmp/explicit".into(),
                parser: "line".into(),
                parser_pattern: None,
                tags: vec![],
            },
        ));
        let out = render(&plan);
        assert!(out.contains("User overrides from .daemon8.toml"));
    }

    #[test]
    fn detect_mode_non_tty_returns_non_interactive() {
        // In `cargo test` the stdio is captured and not a TTY, so this
        // assertion is stable across CI and local runs. It also pins
        // the contract: if either handle is non-TTY, mode is
        // NonInteractive — never half-and-half.
        assert_eq!(detect_mode(), PresentationMode::NonInteractive);
    }

    #[test]
    fn prompt_confirm_interactive_accepts_default_y() {
        let mut input = Cursor::new(b"\n".to_vec());
        let mut output = Vec::new();
        let outcome = prompt_confirm_interactive(&mut input, &mut output).unwrap();
        assert_eq!(outcome, PromptOutcome::Confirmed);
    }

    #[test]
    fn prompt_confirm_interactive_accepts_yes_variants() {
        for raw in [b"y\n".as_slice(), b"Y\n", b"yes\n", b"YES\n"] {
            let mut input = Cursor::new(raw.to_vec());
            let mut output = Vec::new();
            let outcome = prompt_confirm_interactive(&mut input, &mut output).unwrap();
            assert_eq!(outcome, PromptOutcome::Confirmed, "input was {raw:?}");
        }
    }

    #[test]
    fn prompt_confirm_interactive_declines_on_no() {
        for raw in [b"n\n".as_slice(), b"N\n", b"no\n", b"q\n"] {
            let mut input = Cursor::new(raw.to_vec());
            let mut output = Vec::new();
            let outcome = prompt_confirm_interactive(&mut input, &mut output).unwrap();
            assert_eq!(outcome, PromptOutcome::Declined, "input was {raw:?}");
        }
    }

    #[test]
    fn prompt_confirm_interactive_treats_eof_as_decline() {
        let mut input = Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let outcome = prompt_confirm_interactive(&mut input, &mut output).unwrap();
        assert_eq!(outcome, PromptOutcome::Declined);
    }

    #[test]
    fn prompt_confirm_non_interactive_auto_confirms() {
        let plan = base_plan(LibrarianStatus::TemplatesPartial);
        let outcome = prompt_confirm(&plan, || PresentationMode::NonInteractive).unwrap();
        assert_eq!(outcome, PromptOutcome::NonInteractiveAutoConfirm);
    }
}
