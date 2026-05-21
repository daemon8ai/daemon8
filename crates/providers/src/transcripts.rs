// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ALL_PROVIDERS, CONVERSATION_RECENCY_MS, Provider};

const MAX_METADATA_LINES: usize = 50;
const AMBIGUOUS_CANDIDATE_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCandidate {
    pub provider: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptResolution {
    Bound(TranscriptCandidate),
    NotFound,
    Ambiguous(Vec<TranscriptCandidate>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptResolutionError {
    pub code: TranscriptResolutionErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptResolutionErrorCode {
    InvalidProvider,
    TranscriptScopeMismatch,
    TranscriptUnreadable,
    TranscriptProviderMismatch,
}

impl TranscriptResolutionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidProvider => "invalid_provider",
            Self::TranscriptScopeMismatch => "transcript_scope_mismatch",
            Self::TranscriptUnreadable => "transcript_unreadable",
            Self::TranscriptProviderMismatch => "transcript_provider_mismatch",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TranscriptResolver<'a> {
    pub provider: &'a str,
    pub scope_root: &'a Path,
    pub home: &'a Path,
    pub transcript_path: Option<&'a Path>,
}

pub fn resolve_transcript(
    input: TranscriptResolver<'_>,
) -> Result<TranscriptResolution, TranscriptResolutionError> {
    let provider =
        Provider::from_id_or_alias(input.provider).ok_or_else(|| TranscriptResolutionError {
            code: TranscriptResolutionErrorCode::InvalidProvider,
            message: format!("unknown provider '{}'", input.provider),
        })?;
    let provider_id = provider.as_provider().id();

    if let Some(path) = input.transcript_path {
        return explicit_transcript(provider, input.home, input.scope_root, path)
            .map(TranscriptResolution::Bound);
    }

    let scope_root =
        fs::canonicalize(input.scope_root).unwrap_or_else(|_| input.scope_root.to_path_buf());

    let conversation_root = provider.as_provider().conversation_dir(input.home);
    let files = provider.as_provider().project_conversation_files(
        input.home,
        &scope_root,
        CONVERSATION_RECENCY_MS,
    );

    let files = if files.is_empty() {
        fallback_conversation_files(conversation_root.as_deref(), CONVERSATION_RECENCY_MS)
    } else {
        files
    };

    let mut candidates: Vec<TranscriptCandidate> = files
        .iter()
        .filter_map(|path| {
            let metadata = fs::metadata(path).ok()?;
            let canonical = fs::canonicalize(path).ok()?;
            candidate_from_path(
                provider_id,
                conversation_root.as_deref(),
                &canonical,
                &metadata,
            )
            .ok()
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.modified_at_ms
            .cmp(&a.modified_at_ms)
            .then_with(|| a.path.cmp(&b.path))
    });

    let session_id = provider.as_provider().session_id_from_env();
    if let Some(session_id) = session_id.as_deref() {
        let session_matches = matching_session_candidates(&candidates, session_id, &scope_root);
        if session_matches.len() == 1 {
            return Ok(TranscriptResolution::Bound(session_matches[0].clone()));
        }
        if session_matches.len() > 1 {
            return ambiguous(session_matches);
        }
    }

    let scope_matches = matching_scope_candidates(&candidates, &scope_root);
    if scope_matches.len() == 1 {
        return Ok(TranscriptResolution::Bound(scope_matches[0].clone()));
    }
    if scope_matches.len() > 1 {
        return ambiguous(scope_matches);
    }

    if candidates.len() == 1 {
        let candidate = candidates.remove(0);
        if candidate
            .cwd
            .as_deref()
            .is_none_or(|cwd| cwd_matches_scope(cwd, &scope_root))
        {
            return Ok(TranscriptResolution::Bound(candidate));
        }
    }

    Ok(TranscriptResolution::NotFound)
}

pub fn discover_project_conversations(
    scope_root: &Path,
    home: &Path,
    exclude_path: Option<&str>,
) -> Vec<TranscriptCandidate> {
    let scope_root = fs::canonicalize(scope_root).unwrap_or_else(|_| scope_root.to_path_buf());
    let exclude_canonical =
        exclude_path.and_then(|e| fs::canonicalize(e).ok().map(|p| p.display().to_string()));

    let mut candidates = Vec::new();
    for provider in ALL_PROVIDERS {
        let ai = provider.as_provider();
        let provider_id = ai.id();
        let conversation_root = ai.conversation_dir(home);
        let files = ai.project_conversation_files(home, &scope_root, CONVERSATION_RECENCY_MS);

        for path in &files {
            let Some(metadata) = fs::metadata(path).ok() else {
                continue;
            };
            let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if exclude_canonical
                .as_ref()
                .is_some_and(|e| canonical.display().to_string() == *e)
            {
                continue;
            }
            if let Ok(candidate) = candidate_from_path(
                provider_id,
                conversation_root.as_deref(),
                &canonical,
                &metadata,
            ) {
                candidates.push(candidate);
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.modified_at_ms
            .cmp(&a.modified_at_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    candidates
}

const FALLBACK_MAX_FILES: usize = 50;

fn fallback_conversation_files(root: Option<&Path>, since_ms: u64) -> Vec<PathBuf> {
    let Some(root) = root else {
        return Vec::new();
    };
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    walk_jsonl(root, &mut files, since_ms);
    files
}

fn walk_jsonl(dir: &Path, files: &mut Vec<PathBuf>, since_ms: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= FALLBACK_MAX_FILES {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl(&path, files, since_ms);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            let dominated = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .is_some_and(|d| (d.as_millis() as u64) < since_ms);
            if !dominated {
                files.push(path);
            }
        }
    }
}

pub fn normalize_provider_id(raw: &str) -> Result<&'static str, TranscriptResolutionError> {
    Provider::from_id_or_alias(raw)
        .map(|provider| provider.as_provider().id())
        .ok_or_else(|| TranscriptResolutionError {
            code: TranscriptResolutionErrorCode::InvalidProvider,
            message: format!("unknown provider '{raw}'"),
        })
}

fn explicit_transcript(
    provider: Provider,
    home: &Path,
    scope_root: &Path,
    path: &Path,
) -> Result<TranscriptCandidate, TranscriptResolutionError> {
    let provider_id = provider.as_provider().id();
    let path = fs::canonicalize(path).map_err(|err| TranscriptResolutionError {
        code: TranscriptResolutionErrorCode::TranscriptUnreadable,
        message: format!("failed to read transcript path {}: {err}", path.display()),
    })?;
    let metadata = fs::metadata(&path).map_err(|err| TranscriptResolutionError {
        code: TranscriptResolutionErrorCode::TranscriptUnreadable,
        message: format!(
            "failed to inspect transcript path {}: {err}",
            path.display()
        ),
    })?;
    if !metadata.is_file() {
        return Err(TranscriptResolutionError {
            code: TranscriptResolutionErrorCode::TranscriptUnreadable,
            message: format!("transcript path {} is not a file", path.display()),
        });
    }
    if path.extension().is_none_or(|ext| ext != "jsonl") {
        return Err(TranscriptResolutionError {
            code: TranscriptResolutionErrorCode::TranscriptUnreadable,
            message: format!("transcript path {} is not a JSONL file", path.display()),
        });
    }

    let root = provider
        .as_provider()
        .conversation_dir(home)
        .and_then(|root| fs::canonicalize(root).ok())
        .ok_or_else(|| TranscriptResolutionError {
            code: TranscriptResolutionErrorCode::TranscriptUnreadable,
            message: format!("{provider_id} conversation root is not readable"),
        })?;

    if !provider_path_matches(provider_id, &root, &path) {
        let code = if known_provider_root(home, &path).is_some() {
            TranscriptResolutionErrorCode::TranscriptProviderMismatch
        } else {
            TranscriptResolutionErrorCode::TranscriptUnreadable
        };
        return Err(TranscriptResolutionError {
            code,
            message: format!(
                "transcript path {} is not a {} transcript",
                path.display(),
                provider_id
            ),
        });
    }

    match sniff_provider(&path) {
        Some(owner) if owner == provider_id => {}
        Some(owner) => {
            return Err(TranscriptResolutionError {
                code: TranscriptResolutionErrorCode::TranscriptProviderMismatch,
                message: format!(
                    "transcript path {} contains {owner} events, not {provider_id}",
                    path.display()
                ),
            });
        }
        None => {
            return Err(TranscriptResolutionError {
                code: TranscriptResolutionErrorCode::TranscriptUnreadable,
                message: format!(
                    "transcript path {} does not contain recognizable {provider_id} events",
                    path.display()
                ),
            });
        }
    }
    let scope_root = fs::canonicalize(scope_root).unwrap_or_else(|_| scope_root.to_path_buf());
    let candidate = candidate_from_path(provider_id, Some(&root), &path, &metadata)?;
    if candidate
        .cwd
        .as_deref()
        .is_some_and(|cwd| !cwd_matches_scope(cwd, &scope_root))
    {
        return Err(TranscriptResolutionError {
            code: TranscriptResolutionErrorCode::TranscriptScopeMismatch,
            message: format!(
                "transcript path {} belongs to a different project scope",
                path.display()
            ),
        });
    }
    Ok(candidate)
}

fn candidate_from_path(
    provider: &str,
    root: Option<&Path>,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<TranscriptCandidate, TranscriptResolutionError> {
    let mut candidate = TranscriptCandidate {
        provider: provider.into(),
        path: path.display().to_string(),
        provider_session_id: None,
        cwd: None,
        modified_at_ms: metadata.modified().ok().and_then(system_time_ms),
        size_bytes: Some(metadata.len()),
    };

    if let Ok(metadata) = sniff_metadata(provider, path) {
        candidate.provider_session_id = metadata.session_id;
        candidate.cwd = metadata.cwd;
    }
    if let Some(root) = root {
        enrich_claude_project_cwd(&mut candidate, root, path);
    }
    Ok(candidate)
}

fn ambiguous(
    mut candidates: Vec<TranscriptCandidate>,
) -> Result<TranscriptResolution, TranscriptResolutionError> {
    candidates.sort_by(|a, b| {
        b.modified_at_ms
            .cmp(&a.modified_at_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    candidates.truncate(AMBIGUOUS_CANDIDATE_LIMIT);
    Ok(TranscriptResolution::Ambiguous(candidates))
}

fn matching_session_candidates(
    candidates: &[TranscriptCandidate],
    session_id: &str,
    scope_root: &Path,
) -> Vec<TranscriptCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.provider_session_id.as_deref() == Some(session_id)
                && candidate
                    .cwd
                    .as_deref()
                    .is_none_or(|cwd| cwd_matches_scope(cwd, scope_root))
        })
        .cloned()
        .collect()
}

fn matching_scope_candidates(
    candidates: &[TranscriptCandidate],
    scope_root: &Path,
) -> Vec<TranscriptCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate
                .cwd
                .as_deref()
                .is_some_and(|cwd| cwd_matches_scope(cwd, scope_root))
        })
        .cloned()
        .collect()
}

fn provider_path_matches(provider: &str, root: &Path, path: &Path) -> bool {
    if !path.starts_with(root) || path.extension().is_none_or(|ext| ext != "jsonl") {
        return false;
    }
    if provider == "gemini" {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("session-"))
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some("chats");
    }
    matches!(provider, "claude" | "codex")
}

fn known_provider_root(home: &Path, path: &Path) -> Option<&'static str> {
    for provider in ALL_PROVIDERS {
        let Some(root) = provider.as_provider().conversation_dir(home) else {
            continue;
        };
        if let Ok(root) = fs::canonicalize(root)
            && path.starts_with(root)
        {
            return Some(provider.as_provider().id());
        }
    }
    None
}

#[derive(Default)]
struct TranscriptMetadata {
    session_id: Option<String>,
    cwd: Option<String>,
}

fn sniff_metadata(provider: &str, path: &Path) -> std::io::Result<TranscriptMetadata> {
    let file = fs::File::open(path)?;
    let lines = BufReader::new(file).lines();
    let mut metadata = TranscriptMetadata::default();
    for line in lines.take(MAX_METADATA_LINES).flatten() {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match provider {
            "claude" => sniff_claude(&value, &mut metadata),
            "codex" => sniff_codex(&value, &mut metadata),
            "gemini" => sniff_gemini(&value, &mut metadata),
            _ => {}
        }
        if metadata.session_id.is_some() && metadata.cwd.is_some() {
            break;
        }
    }
    Ok(metadata)
}

fn sniff_provider(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let lines = BufReader::new(file).lines();
    for line in lines.take(MAX_METADATA_LINES).flatten() {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("session_meta" | "response_item" | "user_message")
        ) {
            return Some("codex".into());
        }
        if value.get("sessionId").is_some() && value.get("startTime").is_some() {
            return Some("gemini".into());
        }
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some(
                "permission-mode" | "assistant" | "system" | "file-history-snapshot" | "attachment"
            )
        ) {
            return Some("claude".into());
        }
        if value.get("type").and_then(Value::as_str) == Some("user") {
            if value.get("message").is_some() {
                return Some("claude".into());
            }
            if value.get("content").is_some() {
                return Some("gemini".into());
            }
        }
        if matches!(value.get("type").and_then(Value::as_str), Some("gemini"))
            && (value.get("model").is_some() || value.get("toolCalls").is_some())
        {
            return Some("gemini".into());
        }
    }
    None
}

fn sniff_claude(value: &Value, metadata: &mut TranscriptMetadata) {
    if metadata.session_id.is_none() {
        metadata.session_id = value
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    if metadata.cwd.is_none() {
        metadata.cwd = value
            .get("cwd")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
}

fn sniff_codex(value: &Value, metadata: &mut TranscriptMetadata) {
    let Some(payload) = value.get("payload") else {
        return;
    };
    if metadata.session_id.is_none() {
        metadata.session_id = payload
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    if metadata.cwd.is_none() {
        metadata.cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
}

fn sniff_gemini(value: &Value, metadata: &mut TranscriptMetadata) {
    if metadata.session_id.is_none() {
        metadata.session_id = value
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
}

fn enrich_claude_project_cwd(candidate: &mut TranscriptCandidate, root: &Path, path: &Path) {
    if candidate.provider != "claude" || candidate.cwd.is_some() {
        return;
    }
    let Ok(relative) = path.strip_prefix(root) else {
        return;
    };
    let Some(project_slug) = relative.components().next() else {
        return;
    };
    let slug = project_slug.as_os_str().to_string_lossy();
    if !slug.starts_with('-') {
        return;
    }
    let decoded = slug.replacen('-', "/", 1).replace('-', "/");
    candidate.cwd = Some(decoded);
}

fn cwd_matches_scope(cwd: &str, scope_root: &Path) -> bool {
    let path = PathBuf::from(cwd);
    if let Ok(canonical) = fs::canonicalize(&path) {
        return canonical == scope_root;
    }
    path == scope_root
}

fn system_time_ms(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_codex_session(path: &Path, session_id: &str, cwd: &Path) {
        fs::write(
            path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\"}}}}\n",
                cwd.display()
            ),
        )
        .unwrap();
    }

    fn transcript_candidate(session_id: &str, cwd: Option<&Path>) -> TranscriptCandidate {
        TranscriptCandidate {
            provider: "codex".into(),
            path: "session.jsonl".into(),
            provider_session_id: Some(session_id.into()),
            cwd: cwd.map(|path| path.display().to_string()),
            modified_at_ms: None,
            size_bytes: None,
        }
    }

    #[test]
    fn provider_id_normalizes_aliases() {
        assert_eq!(normalize_provider_id("codex-cli").unwrap(), "codex");
        assert_eq!(normalize_provider_id("claude-code").unwrap(), "claude");
        assert_eq!(normalize_provider_id("gemini-cli").unwrap(), "gemini");
        assert!(normalize_provider_id("nope").is_err());
    }

    #[test]
    fn explicit_transcript_binds_provider_file() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("one.jsonl");
        write_codex_session(&transcript, "s1", &project);

        let resolution = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: Some(&transcript),
        })
        .unwrap();

        let TranscriptResolution::Bound(candidate) = resolution else {
            panic!("expected bound transcript");
        };
        assert_eq!(candidate.provider, "codex");
        assert_eq!(candidate.provider_session_id.as_deref(), Some("s1"));
        assert_eq!(
            candidate.cwd.as_deref(),
            Some(project.display().to_string().as_str())
        );
    }

    #[test]
    fn explicit_transcript_rejects_wrong_provider_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let claude = home.join(".claude/projects/-tmp-project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&claude).unwrap();
        let transcript = claude.join("one.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"permission-mode\",\"sessionId\":\"c1\",\"cwd\":\"/tmp/project\"}\n",
        )
        .unwrap();
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();

        let err = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: Some(&transcript),
        })
        .unwrap_err();

        assert_eq!(
            err.code,
            TranscriptResolutionErrorCode::TranscriptProviderMismatch
        );
    }

    #[test]
    fn explicit_transcript_rejects_matching_jsonl_outside_provider_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        let transcript = tmp.path().join("outside.jsonl");
        write_codex_session(&transcript, "s1", &project);

        let err = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: Some(&transcript),
        })
        .unwrap_err();

        assert_eq!(
            err.code,
            TranscriptResolutionErrorCode::TranscriptUnreadable
        );
    }

    #[test]
    fn explicit_transcript_rejects_wrong_provider_content_in_provider_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("claude-shaped.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"permission-mode\",\"sessionId\":\"c1\",\"cwd\":\"/tmp/project\"}\n",
        )
        .unwrap();

        let err = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: Some(&transcript),
        })
        .unwrap_err();

        assert_eq!(
            err.code,
            TranscriptResolutionErrorCode::TranscriptProviderMismatch
        );
    }

    #[test]
    fn explicit_transcript_rejects_different_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let other_project = tmp.path().join("other-project");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&other_project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("other.jsonl");
        write_codex_session(&transcript, "s1", &other_project);

        let err = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: Some(&transcript),
        })
        .unwrap_err();

        assert_eq!(
            err.code,
            TranscriptResolutionErrorCode::TranscriptScopeMismatch
        );
    }

    #[test]
    fn single_candidate_binds_without_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        write_codex_session(&sessions.join("one.jsonl"), "s1", &project);

        let resolution = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: None,
        })
        .unwrap();

        let TranscriptResolution::Bound(candidate) = resolution else {
            panic!("expected bound transcript");
        };
        assert_eq!(candidate.provider_session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn single_candidate_for_different_scope_does_not_bind() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let other_project = tmp.path().join("other-project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&other_project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        write_codex_session(&sessions.join("one.jsonl"), "s1", &other_project);

        let resolution = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: None,
        })
        .unwrap();

        assert_eq!(resolution, TranscriptResolution::NotFound);
    }

    #[test]
    fn scope_match_selects_matching_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let other_project = tmp.path().join("other-project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&other_project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        write_codex_session(&sessions.join("one.jsonl"), "s1", &project);
        write_codex_session(&sessions.join("two.jsonl"), "s2", &other_project);

        let resolution = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: None,
        })
        .unwrap();

        let TranscriptResolution::Bound(candidate) = resolution else {
            panic!("expected bound transcript");
        };
        assert_eq!(candidate.provider_session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn session_match_ignores_different_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let other_project = tmp.path().join("other-project");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&other_project).unwrap();
        let candidates = vec![transcript_candidate("s1", Some(&other_project))];

        let matches = matching_session_candidates(&candidates, "s1", &project);

        assert!(matches.is_empty());
    }

    #[test]
    fn session_match_keeps_unknown_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let candidates = vec![transcript_candidate("s1", None)];

        let matches = matching_session_candidates(&candidates, "s1", &project);

        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn multiple_scope_matches_are_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        write_codex_session(&sessions.join("one.jsonl"), "s1", &project);
        write_codex_session(&sessions.join("two.jsonl"), "s2", &project);

        let resolution = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: None,
        })
        .unwrap();

        let TranscriptResolution::Ambiguous(candidates) = resolution else {
            panic!("expected ambiguous transcript candidates");
        };
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn no_candidates_succeeds_unbound() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();

        let resolution = resolve_transcript(TranscriptResolver {
            provider: "codex",
            scope_root: &project,
            home: &home,
            transcript_path: None,
        })
        .unwrap();

        assert_eq!(resolution, TranscriptResolution::NotFound);
    }

    fn write_claude_transcript(home: &Path, scope_root: &Path, filename: &str) -> PathBuf {
        let canonical = fs::canonicalize(scope_root).unwrap_or_else(|_| scope_root.to_path_buf());
        let slug = canonical.to_string_lossy().replace('/', "-");
        let dir = home.join(".claude/projects").join(&slug);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(filename);
        fs::write(
            &path,
            r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"test-session"}"#,
        )
        .unwrap();
        path
    }

    #[test]
    fn discovery_returns_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("myproject");
        let home = tmp.path().join("home");
        fs::create_dir_all(&project).unwrap();

        write_claude_transcript(&home, &project, "session-a.jsonl");
        write_claude_transcript(&home, &project, "session-b.jsonl");

        let candidates = discover_project_conversations(&project, &home, None);
        assert_eq!(candidates.len(), 2, "expected exactly 2 candidates");
        assert!(candidates.iter().all(|c| c.provider == "claude"));
    }

    #[test]
    fn discovery_excludes_bound_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("myproject");
        let home = tmp.path().join("home");
        fs::create_dir_all(&project).unwrap();

        let bound = write_claude_transcript(&home, &project, "bound.jsonl");
        write_claude_transcript(&home, &project, "other.jsonl");

        let bound_canonical = fs::canonicalize(&bound).unwrap();
        let candidates = discover_project_conversations(
            &project,
            &home,
            Some(&bound_canonical.display().to_string()),
        );
        assert!(
            !candidates
                .iter()
                .any(|c| c.path == bound_canonical.display().to_string()),
            "bound transcript should be excluded"
        );
        assert!(
            !candidates.is_empty(),
            "other transcript should still be present"
        );
    }

    #[test]
    fn discovery_returns_candidates_from_multiple_providers() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("myproject");
        let home = tmp.path().join("home");
        fs::create_dir_all(&project).unwrap();

        write_claude_transcript(&home, &project, "claude-session.jsonl");

        let canonical = fs::canonicalize(&project).unwrap();
        let scope_str = canonical.to_string_lossy();
        let gemini_slug = "test-slug";
        let projects_json = format!(r#"{{"projects":{{"{}":"{}"}}}}"#, scope_str, gemini_slug);
        let gemini_projects = home.join(".gemini");
        fs::create_dir_all(&gemini_projects).unwrap();
        fs::write(gemini_projects.join("projects.json"), projects_json).unwrap();
        let chats_dir = home.join(".gemini/tmp").join(gemini_slug).join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        fs::write(
            chats_dir.join("gemini-session.jsonl"),
            r#"{"sessionId":"g1","startTime":"2026-01-01T00:00:00Z","lastUpdated":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let candidates = discover_project_conversations(&project, &home, None);
        let providers: Vec<&str> = candidates.iter().map(|c| c.provider.as_str()).collect();
        assert!(providers.contains(&"claude"), "expected claude candidate");
        assert!(providers.contains(&"gemini"), "expected gemini candidate");
    }

    #[test]
    fn discovery_returns_empty_for_unknown_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("nonexistent");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let candidates = discover_project_conversations(&project, &home, None);
        assert!(candidates.is_empty());
    }
}
