// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Conversation provider auto-detect (D5).
//!
//! Two responsibilities live here:
//!
//! 1. **Active session resolution** — pure runtime, not a learned
//!    pattern. Reads each provider's `session_id_env_vars()` first,
//!    falls back to mtime-newest file under `conversation_dir()` filtered
//!    by `conversation_file_glob()`. Used by source registration to wire
//!    up a conversation watcher pointing at the right transcript file.
//!
//! 2. **First-run detection** — does the librarian carry a conversation
//!    `source_template` tagged with this provider? If not, the scanner
//!    emits a hint with a per-provider [`FirstRunPayload`] asking the
//!    agent to register one. Once any agent writes the template for a
//!    given provider on this machine, subsequent projects skip the
//!    bootstrap entirely.
//!
//! The `_first_run` branch fires per-provider, not per-machine: if
//! Claude has a template but Codex does not, the hint only carries
//! Codex's bootstrap payload. This matches Open Question #5 in the
//! canonical D-phase plan.

use std::path::{Path, PathBuf};

use daemon8_providers::AiProvider;
use daemon8_store::{LibrarianFilter, LibrarianStore, StoreError};
use daemon8_types::{
    FirstRunPayload, LibrarianNodeKind, SourceInstanceData, SourceKind, SourceTemplateData,
};

/// The AI session currently in scope for the daemon process.
///
/// Production wiring for the conversation watcher uses this resolution
/// in a later commit; for now the resolver is callable from tests and
/// from the registrar's optional active-session correlation path.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ActiveSession {
    pub provider_id: String,
    pub session_file: PathBuf,
    pub session_id: Option<String>,
}

/// Resolve the AI session currently in scope.
///
/// Strategy, per provider in [`daemon8_providers::ALL_PROVIDERS`]:
///   1. Probe each entry in `session_id_env_vars()`. The first
///      non-empty value wins; the file is located by scanning the
///      provider's conversation directory for a filename containing the
///      session id (most providers encode it in the basename).
///   2. Fall back to the most-recently-modified file matching
///      `conversation_file_glob()` under `conversation_dir()`.
///
/// Returns `None` when no provider produced a hit — typical on a clean
/// machine that has never run any of the supported agent CLIs.
#[allow(dead_code)]
pub fn resolve_active_session(home: &Path) -> Option<ActiveSession> {
    use daemon8_providers::ALL_PROVIDERS;
    for &provider in ALL_PROVIDERS {
        let p = provider.as_provider();
        if let Some(session) = resolve_session_for_provider(p, home) {
            return Some(session);
        }
    }
    None
}

#[allow(dead_code)]
fn resolve_session_for_provider(provider: &dyn AiProvider, home: &Path) -> Option<ActiveSession> {
    let convo_dir = provider.conversation_dir(home)?;
    let glob_pattern = provider.conversation_file_glob()?;

    // Env-var path: try each declared session id env var. If set, look
    // for a transcript file whose basename contains the id.
    for var in provider.session_id_env_vars() {
        let Ok(val) = std::env::var(var) else {
            continue;
        };
        if val.is_empty() {
            continue;
        }
        if let Some(path) = find_session_file_by_id(&convo_dir, glob_pattern, &val) {
            return Some(ActiveSession {
                provider_id: provider.id().to_string(),
                session_file: path,
                session_id: Some(val),
            });
        }
    }

    // mtime fallback: newest file under conversation_dir matching the
    // provider's glob.
    let newest = newest_matching_file(&convo_dir, glob_pattern)?;
    Some(ActiveSession {
        provider_id: provider.id().to_string(),
        session_id: derive_session_id_from_filename(&newest),
        session_file: newest,
    })
}

#[allow(dead_code)]
fn find_session_file_by_id(dir: &Path, glob_pattern: &str, session_id: &str) -> Option<PathBuf> {
    let full = dir.join(glob_pattern);
    let pattern = full.to_string_lossy().to_string();
    let entries = glob::glob(&pattern).ok()?;
    for entry in entries.flatten() {
        let stem = entry.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.contains(session_id) {
            return Some(entry);
        }
    }
    None
}

#[allow(dead_code)]
fn newest_matching_file(dir: &Path, glob_pattern: &str) -> Option<PathBuf> {
    let full = dir.join(glob_pattern);
    let pattern = full.to_string_lossy().to_string();
    let entries = glob::glob(&pattern).ok()?;
    entries
        .flatten()
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
}

#[allow(dead_code)]
fn derive_session_id_from_filename(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

/// Returns true if the librarian has no `source_template` node with
/// `kind: conversation` whose `default_tags` reference this provider.
///
/// Provider association comes from `default_tags`: agents writing a
/// conversation template are instructed (via the first-run instruction
/// text) to include the provider id as a tag. This keeps the schema
/// flat — no separate `provider` field on templates — and lets the
/// existing librarian filter do the lookup.
pub async fn is_first_run_for_provider(
    librarian: &dyn LibrarianStore,
    provider_id: &str,
) -> Result<bool, StoreError> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::SourceTemplate]),
        limit: Some(256),
        ..Default::default()
    };
    let nodes = librarian.lookup(&filter).await?;
    for node in nodes {
        let Some(ref data) = node.data else { continue };
        let template: SourceTemplateData = match serde_json::from_value(data.clone()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if template.kind != SourceKind::Conversation {
            continue;
        }
        if template.default_tags.iter().any(|t| t == provider_id) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Returns true if the librarian already has a registered
/// `source_instance` (kind = conversation) for this provider on the
/// machine. Used to suppress the first-run branch even when no template
/// is present — e.g. a hand-authored `[sources.*]` block already wired
/// the watcher and writing a template would be redundant.
#[allow(dead_code)]
pub async fn has_conversation_instance_for_provider(
    librarian: &dyn LibrarianStore,
    provider_id: &str,
) -> Result<bool, StoreError> {
    let filter = LibrarianFilter {
        kinds: Some(vec![LibrarianNodeKind::SourceInstance]),
        limit: Some(256),
        ..Default::default()
    };
    let nodes = librarian.lookup(&filter).await?;
    for node in nodes {
        let Some(ref data) = node.data else { continue };
        let inst: SourceInstanceData = match serde_json::from_value(data.clone()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if inst.kind != SourceKind::Conversation {
            continue;
        }
        if inst.tags.iter().any(|t| t == provider_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Build the per-provider conversation bootstrap payload for the
/// first-run branch. Pure: composes filesystem-layout hints from the
/// provider trait into a structured payload + instruction snippet.
///
/// `home` is taken explicitly rather than hard-coded so tests can point
/// at a temp directory.
pub fn build_first_run_payload(provider: &dyn AiProvider, home: &Path) -> FirstRunPayload {
    let conversation_dir_hint = provider
        .conversation_dir(home)
        .map(|p| display_with_home(&p, home))
        .unwrap_or_default();
    let conversation_file_glob_hint = provider
        .conversation_file_glob()
        .unwrap_or_default()
        .to_string();
    let session_id_env_vars: Vec<String> = provider
        .session_id_env_vars()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let instruction_text = render_provider_instruction(
        provider.id(),
        provider.label(),
        &conversation_dir_hint,
        &conversation_file_glob_hint,
        &session_id_env_vars,
    );
    FirstRunPayload {
        provider_id: provider.id().to_string(),
        provider_label: provider.label().to_string(),
        conversation_dir_hint,
        conversation_file_glob_hint,
        session_id_env_vars,
        instruction_text,
    }
}

/// Replace a leading `<home>` prefix with `~` so the hint surfaces a
/// portable path that conforms to the validator's portability rules.
/// Anything outside the home directory is left untouched.
fn display_with_home(path: &Path, home: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(home) {
        let mut s = String::from("~");
        let r = rel.to_string_lossy();
        if !r.is_empty() {
            s.push('/');
            s.push_str(&r);
        }
        return s;
    }
    path.to_string_lossy().to_string()
}

fn render_provider_instruction(
    provider_id: &str,
    provider_label: &str,
    dir_hint: &str,
    glob_hint: &str,
    env_vars: &[String],
) -> String {
    let env_list = if env_vars.is_empty() {
        "(none declared)".to_string()
    } else {
        env_vars.join(", ")
    };
    let locator = if dir_hint.is_empty() || glob_hint.is_empty() {
        "(provider trait did not expose a directory + glob)".to_string()
    } else {
        format!("{dir_hint}/{glob_hint}")
    };
    let platforms = "[\"macos\", \"linux\", \"windows\"]";
    format!(
        "For provider {provider_label} (id: {provider_id}):\n\
\u{20}\u{20}Conversation directory: {dir_hint}\n\
\u{20}\u{20}Filename glob: {glob_hint}\n\
\u{20}\u{20}Session ID env vars: {env_list}\n\
\n\
Please write a source_template via librarian_index with:\n\
\u{20}\u{20}kind: \"source_template\"\n\
\u{20}\u{20}data: {{\n\
\u{20}\u{20}\u{20}\u{20}project_types: [\"any\"]\n\
\u{20}\u{20}\u{20}\u{20}kind: \"conversation\"\n\
\u{20}\u{20}\u{20}\u{20}locator_pattern: \"{locator}\"\n\
\u{20}\u{20}\u{20}\u{20}platforms: {platforms}\n\
\u{20}\u{20}\u{20}\u{20}parser_hint: \"ai_conversation_{provider_id}\"\n\
\u{20}\u{20}\u{20}\u{20}default_tags: [\"{provider_id}\", \"agent\", \"conversation\"]\n\
\u{20}\u{20}\u{20}\u{20}description: \"{provider_label} conversation transcript\"\n\
\u{20}\u{20}\u{20}\u{20}version_constraint: null\n\
\u{20}\u{20}\u{20}\u{20}confidence: \"agent_discovered\"\n\
\u{20}\u{20}}}\n\
\n\
The default_tags list MUST include \"{provider_id}\" — daemon8's first-run check uses that tag to recognize the template on subsequent discovery scans."
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use daemon8_providers::{ALL_PROVIDERS, AiProvider, Provider};
    use daemon8_store::{LibrarianNode, LibrarianStore, SurrealStore};
    use daemon8_types::{LocatorKind, Platform, TemplateConfidence};

    use super::*;

    fn provider_dyn(p: Provider) -> &'static dyn AiProvider {
        p.as_provider()
    }

    fn template_node(data: SourceTemplateData, label: &str) -> LibrarianNode {
        LibrarianNode {
            id: None,
            kind: LibrarianNodeKind::SourceTemplate,
            label: label.into(),
            locator_kind: LocatorKind::File,
            locator: data.locator_pattern.clone(),
            tags: vec![],
            project_slug: String::new(),
            version: String::new(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            last_read_at: None,
            deprecated_at: None,
            canonicalized_at: None,
            data: Some(serde_json::to_value(&data).unwrap()),
        }
    }

    fn conversation_template_for(provider_id: &str) -> SourceTemplateData {
        SourceTemplateData {
            project_types: vec!["any".into()],
            kind: SourceKind::Conversation,
            locator_pattern: format!("~/.{provider_id}/projects/**/*.jsonl"),
            platforms: vec![Platform::Macos, Platform::Linux, Platform::Windows],
            parser_hint: Some(format!("ai_conversation_{provider_id}")),
            default_tags: vec![provider_id.into(), "agent".into(), "conversation".into()],
            description: format!("{provider_id} conversation transcript"),
            version_constraint: None,
            discovered_by_session: None,
            discovered_by_provider: Some(provider_id.into()),
            discovered_at_ns: 0,
            verified_count: 0,
            last_verified_at_ns: 0,
            confidence: TemplateConfidence::AgentDiscovered,
        }
    }

    #[test]
    fn resolve_active_session_from_env_var() {
        // Use the Claude provider — its session_id_env_vars are
        // documented and stable, and we can write a transcript file
        // under a fake home to drive the env-var branch.
        let home = tempfile::tempdir().unwrap();
        let claude = provider_dyn(Provider::ClaudeCode);
        let dir = claude.conversation_dir(home.path()).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        // File whose basename contains the session id from env.
        let session_id = "test-session-abc";
        let proj_dir = dir.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let session_file = proj_dir.join(format!("{session_id}.jsonl"));
        std::fs::write(&session_file, b"{}\n").unwrap();

        let var = claude.session_id_env_vars()[0];
        // SAFETY: process-wide env, but the var is provider-specific
        // and the test owns it. Same pattern used in scanner tests.
        unsafe { std::env::set_var(var, session_id) };
        let resolved = resolve_session_for_provider(claude, home.path());
        unsafe { std::env::remove_var(var) };

        let resolved = resolved.expect("env-var resolution should succeed");
        assert_eq!(resolved.provider_id, claude.id());
        assert_eq!(resolved.session_id.as_deref(), Some(session_id));
        assert_eq!(resolved.session_file, session_file);
    }

    #[test]
    fn resolve_active_session_falls_back_to_mtime() {
        let home = tempfile::tempdir().unwrap();
        let claude = provider_dyn(Provider::ClaudeCode);
        let dir = claude.conversation_dir(home.path()).unwrap();
        let proj_dir = dir.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();

        let older = proj_dir.join("older.jsonl");
        let newer = proj_dir.join("newer.jsonl");
        std::fs::write(&older, b"x").unwrap();
        // Make sure mtime ordering is unambiguous on filesystems with
        // 1-second resolution (HFS+).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&newer, b"y").unwrap();

        // No env var set.
        for var in claude.session_id_env_vars() {
            unsafe { std::env::remove_var(var) };
        }

        let resolved =
            resolve_session_for_provider(claude, home.path()).expect("mtime fallback should win");
        assert_eq!(resolved.session_file, newer);
    }

    #[test]
    fn resolve_active_session_returns_none_when_no_provider_resolves() {
        let home = tempfile::tempdir().unwrap();
        // No transcripts written, no env vars set.
        for &p in ALL_PROVIDERS {
            for var in p.as_provider().session_id_env_vars() {
                unsafe { std::env::remove_var(var) };
            }
        }
        assert!(resolve_active_session(home.path()).is_none());
    }

    #[tokio::test]
    async fn is_first_run_true_when_no_template_present() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());
        assert!(is_first_run_for_provider(&*lib, "claude").await.unwrap());
        assert!(is_first_run_for_provider(&*lib, "codex").await.unwrap());
    }

    #[tokio::test]
    async fn is_first_run_false_when_template_tagged_with_provider() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());
        lib.index_node(template_node(
            conversation_template_for("claude"),
            "claude convo",
        ))
        .await
        .unwrap();
        assert!(!is_first_run_for_provider(&*lib, "claude").await.unwrap());
        // Untagged providers stay first-run.
        assert!(is_first_run_for_provider(&*lib, "codex").await.unwrap());
    }

    #[tokio::test]
    async fn is_first_run_ignores_non_conversation_templates() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let lib: Arc<dyn LibrarianStore> = Arc::new(store.librarian_store());
        let mut log_tpl = conversation_template_for("claude");
        log_tpl.kind = SourceKind::Log;
        lib.index_node(template_node(log_tpl, "claude log"))
            .await
            .unwrap();
        assert!(is_first_run_for_provider(&*lib, "claude").await.unwrap());
    }

    #[test]
    fn build_first_run_payload_includes_filesystem_layout() {
        let home = tempfile::tempdir().unwrap();
        let claude = provider_dyn(Provider::ClaudeCode);
        let payload = build_first_run_payload(claude, home.path());
        assert_eq!(payload.provider_id, claude.id());
        assert_eq!(payload.provider_label, claude.label());
        assert!(
            payload.conversation_dir_hint.starts_with("~/"),
            "dir hint should be home-relative: {}",
            payload.conversation_dir_hint
        );
        assert_eq!(
            payload.conversation_file_glob_hint,
            claude.conversation_file_glob().unwrap()
        );
        assert!(!payload.session_id_env_vars.is_empty());
        assert!(payload.instruction_text.contains(claude.id()));
        assert!(payload.instruction_text.contains("source_template"));
        // The instruction must name the provider id so the agent tags
        // the template correctly — the first-run check keys off that tag.
        assert!(
            payload
                .instruction_text
                .contains(&format!("\"{}\"", claude.id())),
            "instruction must surface provider id as a literal tag value"
        );
    }

    #[test]
    fn display_with_home_strips_home_prefix() {
        let home = PathBuf::from("/Users/alice");
        let path = home.join(".claude/projects");
        assert_eq!(display_with_home(&path, &home), "~/.claude/projects");
    }

    #[test]
    fn display_with_home_passes_through_when_outside_home() {
        let home = PathBuf::from("/Users/alice");
        let path = PathBuf::from("/var/lib/something");
        assert_eq!(display_with_home(&path, &home), "/var/lib/something");
    }
}
