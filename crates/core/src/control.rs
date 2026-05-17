// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::{Path, PathBuf};

use daemon8_providers::transcripts::{
    TranscriptCandidate, TranscriptResolution, TranscriptResolver, normalize_provider_id,
    resolve_transcript,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::project_config::{ProjectConfigError, parse_project_config_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaStatus {
    Success,
    Error,
    ConnectRequired,
    SetupRequired,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeMode {
    Project,
    General,
    Invalid,
}

impl ScopeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::General => "general",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextAction {
    pub tool: String,
    pub reason: String,
    #[serde(default)]
    pub params: Value,
}

impl NextAction {
    pub fn new(tool: impl Into<String>, reason: impl Into<String>, params: Value) -> Self {
        Self {
            tool: tool.into(),
            reason: reason.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlphaEnvelope {
    pub status: AlphaStatus,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_actions: Vec<NextAction>,
}

impl AlphaEnvelope {
    pub fn success(code: impl Into<String>, message: impl Into<String>, data: Value) -> Self {
        Self {
            status: AlphaStatus::Success,
            code: code.into(),
            message: message.into(),
            why: None,
            data: Some(data),
            hints: Vec::new(),
            next_actions: Vec::new(),
        }
    }

    pub fn non_success(
        status: AlphaStatus,
        code: impl Into<String>,
        message: impl Into<String>,
        why: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            why: Some(why.into()),
            data: None,
            hints: Vec::new(),
            next_actions: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    pub fn with_next_action(mut self, action: NextAction) -> Self {
        self.next_actions.push(action);
        self
    }

    pub fn render(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|err| {
            format!(
                "{{\"status\":\"error\",\"code\":\"serialization_failed\",\"message\":\"envelope serialization failed\",\"why\":\"{err}\"}}"
            )
        })
    }
}

pub fn status_envelope(data: Value) -> AlphaEnvelope {
    AlphaEnvelope::success("status", "daemon status", data)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConnection {
    pub session_id: String,
    pub mode: ScopeMode,
    pub requested_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_root: Option<String>,
    pub provider: String,
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub session_id: String,
    pub provider: String,
    pub project_path: PathBuf,
    pub agent_name: Option<String>,
    pub transcript_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectOutcome {
    pub envelope: AlphaEnvelope,
    pub connection: Option<SessionConnection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeCandidate {
    Project(PathBuf),
    General(PathBuf),
}

pub fn normalize_provider_for_connect(
    session_id: &str,
    provider: &str,
    requested_path: &str,
) -> Result<String, Box<AlphaEnvelope>> {
    normalize_provider_id(provider)
        .map(ToString::to_string)
        .map_err(|err| {
            Box::new(
                AlphaEnvelope::non_success(
                    AlphaStatus::Error,
                    err.code.as_str(),
                    "provider is not supported",
                    err.message,
                )
                .with_data(json!({
                    "session_id": session_id,
                    "mode": ScopeMode::Invalid,
                    "requested_path": requested_path,
                })),
            )
        })
}

pub fn connect(request: ConnectRequest) -> ConnectOutcome {
    let requested_path = request.project_path.display().to_string();
    let provider = request.provider.trim().to_string();
    if provider.is_empty() {
        return ConnectOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::Error,
                "invalid_provider",
                "provider is required",
                "daemon8_connect needs the calling agent provider to bind this session explicitly",
            )
            .with_data(json!({
                "session_id": request.session_id,
                "mode": ScopeMode::Invalid,
                "requested_path": requested_path,
            })),
            connection: None,
        };
    }

    let canonical = match canonical_project_dir(&request.project_path) {
        Ok(path) => path,
        Err(reason) => {
            return ConnectOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Error,
                    "invalid_scope",
                    "path cannot be used as a daemon8 scope",
                    reason,
                )
                .with_data(json!({
                    "session_id": request.session_id,
                    "mode": ScopeMode::Invalid,
                    "requested_path": requested_path,
                })),
                connection: None,
            };
        }
    };

    match classify_scope(&canonical) {
        ScopeCandidate::General(scope) => {
            let connection = SessionConnection {
                session_id: request.session_id,
                mode: ScopeMode::General,
                requested_path,
                scope_root: Some(scope.display().to_string()),
                provider: provider.clone(),
                agent_name: request
                    .agent_name
                    .unwrap_or_else(|| format!("{provider}-agent")),
                transcript_path: None,
            };
            ConnectOutcome {
                envelope: AlphaEnvelope::success(
                    "connected",
                    "connected in general mode",
                    serde_json::to_value(&connection).unwrap_or(Value::Null),
                )
                .with_hint("general mode is connected; project-only tools require reconnecting with a project path"),
                connection: Some(connection),
            }
        }
        ScopeCandidate::Project(scope) => connect_project(request, provider, requested_path, scope),
    }
}

fn connect_project(
    request: ConnectRequest,
    provider: String,
    requested_path: String,
    scope_root: PathBuf,
) -> ConnectOutcome {
    let config_path = scope_root.join(".daemon8").join("config.md");
    let common_data = json!({
        "session_id": request.session_id,
        "mode": ScopeMode::Project,
        "requested_path": requested_path,
        "scope_root": scope_root.display().to_string(),
        "config_path": config_path.display().to_string(),
    });

    if !config_path.exists() {
        return ConnectOutcome {
            envelope: AlphaEnvelope::non_success(
                AlphaStatus::SetupRequired,
                "missing_config",
                "project config is missing",
                "daemon8 project mode requires .daemon8/config.md before project-scoped tools can run",
            )
            .with_data(common_data.clone())
            .with_next_action(NextAction::new(
                "daemon8_init",
                "write .daemon8/config.md for this project, then retry daemon8_connect",
                json!({"project_path": scope_root.display().to_string()}),
            )),
            connection: None,
        };
    }

    let config = match parse_project_config_file(&config_path) {
        Ok(config) => config,
        Err(ProjectConfigError::Read { path, source }) => {
            return ConnectOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "config_unreadable",
                    "project config cannot be read",
                    format!("daemon8 cannot safely repair {}: {source}", path.display()),
                )
                .with_data(common_data),
                connection: None,
            };
        }
        Err(err) => {
            return ConnectOutcome {
                envelope: AlphaEnvelope::non_success(
                    AlphaStatus::SetupRequired,
                    "invalid_config",
                    "project config is invalid",
                    format!("daemon8 can reconnect after .daemon8/config.md is regenerated: {err}"),
                )
                .with_data(common_data)
                .with_next_action(NextAction::new(
                    "daemon8_init",
                    "overwrite the invalid project config, then retry daemon8_connect",
                    json!({
                        "project_path": scope_root.display().to_string(),
                        "overwrite": true,
                    }),
                )),
                connection: None,
            };
        }
    };

    let connection = SessionConnection {
        session_id: request.session_id,
        mode: ScopeMode::Project,
        requested_path,
        scope_root: Some(scope_root.display().to_string()),
        provider: provider.clone(),
        agent_name: request
            .agent_name
            .unwrap_or_else(|| format!("{provider}-agent")),
        transcript_path: request
            .transcript_path
            .map(|path| path.display().to_string()),
    };
    let mut data = serde_json::to_value(&connection).unwrap_or(Value::Null);
    data["project_name"] = json!(config.project.name);
    data["source_count"] = json!(config.sources.len());
    data["config_path"] = json!(config_path.display().to_string());

    ConnectOutcome {
        envelope: AlphaEnvelope::success("connected", "connected to project", data),
        connection: Some(connection),
    }
}

pub fn resolve_connect_transcript(
    outcome: ConnectOutcome,
    transcript_path: Option<&Path>,
    home: &Path,
) -> ConnectOutcome {
    if outcome.envelope.status != AlphaStatus::Success {
        return outcome;
    }
    let Some(connection) = &outcome.connection else {
        return outcome;
    };
    if connection.mode != ScopeMode::Project {
        return outcome;
    }
    let Some(scope_root) = connection.scope_root.as_deref() else {
        return outcome;
    };

    let provider = connection.provider.clone();
    let scope_root = PathBuf::from(scope_root);
    match resolve_transcript(TranscriptResolver {
        provider: &provider,
        scope_root: &scope_root,
        home,
        transcript_path,
    }) {
        Ok(TranscriptResolution::Bound(candidate)) => {
            connect_outcome_with_transcript(outcome, "bound", Some(candidate))
        }
        Ok(TranscriptResolution::NotFound) => {
            connect_outcome_with_transcript(outcome, "not_found", None)
        }
        Ok(TranscriptResolution::Ambiguous(candidates)) => transcript_blocked_outcome(
            outcome,
            "transcript_ambiguous",
            "multiple provider transcripts match this session",
            "daemon8 found more than one provider transcript for this project and will not choose implicitly",
            json!({
                "status": "ambiguous",
                "provider": provider,
                "candidates": candidates,
            }),
            None,
        ),
        Err(err) => transcript_error_outcome(
            outcome,
            err.code.as_str(),
            "provider transcript cannot be bound",
            err.message,
        ),
    }
}

fn connect_outcome_with_transcript(
    mut outcome: ConnectOutcome,
    status: &str,
    candidate: Option<TranscriptCandidate>,
) -> ConnectOutcome {
    let Some(connection) = outcome.connection.as_mut() else {
        return outcome;
    };
    let transcript = match candidate {
        Some(candidate) => {
            connection.transcript_path = Some(candidate.path.clone());
            json!({
                "status": status,
                "provider": candidate.provider,
                "path": candidate.path,
                "provider_session_id": candidate.provider_session_id,
                "cwd": candidate.cwd,
                "modified_at_ms": candidate.modified_at_ms,
                "size_bytes": candidate.size_bytes,
            })
        }
        None => json!({
            "status": status,
            "provider": connection.provider,
        }),
    };

    let mut data = outcome.envelope.data.take().unwrap_or_else(|| json!({}));
    if let Some(path) = &connection.transcript_path {
        data["transcript_path"] = json!(path);
    }
    data["transcript"] = transcript;
    outcome.envelope.data = Some(data);
    outcome
}

fn transcript_blocked_outcome(
    outcome: ConnectOutcome,
    code: &str,
    message: &str,
    why: &str,
    transcript: Value,
    retry_path: Option<String>,
) -> ConnectOutcome {
    let data = transcript_outcome_data(&outcome, transcript);
    let params = transcript_retry_params(&outcome, retry_path);
    ConnectOutcome {
        envelope: AlphaEnvelope::non_success(AlphaStatus::Blocked, code, message, why)
            .with_data(data)
            .with_hint("retry daemon8_connect with transcript_path set to one candidate path")
            .with_next_action(NextAction::new(
                "daemon8_connect",
                "bind an explicit transcript path",
                params,
            )),
        connection: None,
    }
}

fn transcript_retry_params(outcome: &ConnectOutcome, retry_path: Option<String>) -> Value {
    let Some(connection) = &outcome.connection else {
        return json!({});
    };
    json!({
        "provider": connection.provider,
        "project_path": connection.scope_root.as_deref().unwrap_or(&connection.requested_path),
        "agent_name": connection.agent_name,
        "transcript_path": retry_path.unwrap_or_else(|| "<candidate path>".into()),
    })
}

fn transcript_error_outcome(
    outcome: ConnectOutcome,
    code: &str,
    message: &str,
    why: String,
) -> ConnectOutcome {
    let transcript = json!({
        "status": "error",
        "code": code,
    });
    ConnectOutcome {
        envelope: AlphaEnvelope::non_success(AlphaStatus::Error, code, message, why)
            .with_data(transcript_outcome_data(&outcome, transcript)),
        connection: None,
    }
}

fn transcript_outcome_data(outcome: &ConnectOutcome, transcript: Value) -> Value {
    let Some(connection) = &outcome.connection else {
        return json!({ "transcript": transcript });
    };
    let mut data = serde_json::to_value(connection).unwrap_or_default();
    data["transcript"] = transcript;
    if let Some(existing) = &outcome.envelope.data
        && let Some(config_path) = existing.get("config_path")
    {
        data["config_path"] = config_path.clone();
    }
    data
}

fn canonical_project_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    path.canonicalize()
        .map_err(|err| format!("failed to canonicalize {}: {err}", path.display()))
}

pub fn classify_scope(path: &Path) -> ScopeCandidate {
    let mut current = path.to_path_buf();
    loop {
        if is_project_marker_dir(&current) {
            return ScopeCandidate::Project(current);
        }
        if !current.pop() {
            break;
        }
    }

    ScopeCandidate::General(path.to_path_buf())
}

fn is_project_marker_dir(path: &Path) -> bool {
    path.join(".daemon8").exists()
        || path.join(".git").exists()
        || path.join("Cargo.toml").exists()
        || path.join("package.json").exists()
        || path.join("composer.json").exists()
        || path.join("pyproject.toml").exists()
        || path.join("go.mod").exists()
        || path.join("artisan").exists()
        || path.join("bin/console").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{InitRequest, init_project};

    fn request(path: &Path) -> ConnectRequest {
        ConnectRequest {
            session_id: "mcp-1".into(),
            provider: "codex".into(),
            project_path: path.to_path_buf(),
            agent_name: None,
            transcript_path: None,
        }
    }

    fn write_project_config(root: &Path) {
        let outcome = init_project(InitRequest {
            project_path: root.to_path_buf(),
            name: None,
            overwrite: false,
        });
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
    }

    fn write_codex_session(path: &Path, session_id: &str, cwd: &Path) {
        std::fs::write(
            path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\"}}}}\n",
                cwd.display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn envelope_has_alpha_shape() {
        let envelope = AlphaEnvelope::success("connected", "connected", json!({"x": 1}))
            .with_next_action(NextAction::new("daemon8_status", "inspect", json!({})));
        let value: Value = serde_json::from_str(&envelope.render()).unwrap();
        assert_eq!(value["status"], "success");
        assert_eq!(value["code"], "connected");
        assert_eq!(value["data"]["x"], 1);
        assert_eq!(value["next_actions"][0]["tool"], "daemon8_status");
        assert!(value.get("result").is_none());
        assert!(value.get("daemon8").is_none());
        assert!(value.get("error").is_none());
    }

    #[test]
    fn missing_project_config_returns_setup_required() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let outcome = connect(request(tmp.path()));
        assert_eq!(outcome.envelope.status, AlphaStatus::SetupRequired);
        assert_eq!(outcome.envelope.code, "missing_config");
        assert!(outcome.connection.is_none());
    }

    #[test]
    fn unreadable_project_config_returns_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let config_path = tmp.path().join(".daemon8").join("config.md");
        std::fs::create_dir_all(&config_path).unwrap();

        let outcome = connect(request(tmp.path()));
        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "config_unreadable");
        assert!(outcome.envelope.next_actions.is_empty());
        assert!(outcome.connection.is_none());
    }

    #[test]
    fn general_directory_connects() {
        let tmp = tempfile::tempdir().unwrap();

        let outcome = connect(request(tmp.path()));
        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        assert_eq!(outcome.connection.unwrap().mode, ScopeMode::General);
    }

    #[test]
    fn file_path_is_invalid_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, "").unwrap();

        let outcome = connect(request(&file));
        assert_eq!(outcome.envelope.status, AlphaStatus::Error);
        assert_eq!(outcome.envelope.code, "invalid_scope");
    }

    #[test]
    fn resolve_connect_transcript_binds_explicit_provider_file() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        write_project_config(&project);
        let transcript = sessions.join("one.jsonl");
        write_codex_session(&transcript, "s1", &project);

        let mut req = request(&project);
        req.transcript_path = Some(transcript.clone());
        let outcome = resolve_connect_transcript(connect(req), Some(&transcript), &home);

        assert_eq!(outcome.envelope.status, AlphaStatus::Success);
        let connection = outcome.connection.unwrap();
        let expected = transcript.canonicalize().unwrap().display().to_string();
        assert_eq!(
            connection.transcript_path.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            outcome.envelope.data.unwrap()["transcript"]["status"],
            "bound"
        );
    }

    #[test]
    fn resolve_connect_transcript_rejects_different_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let other_project = tmp.path().join("other-project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other_project).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        write_project_config(&project);
        let transcript = sessions.join("other.jsonl");
        write_codex_session(&transcript, "s1", &other_project);

        let mut req = request(&project);
        req.transcript_path = Some(transcript.clone());
        let outcome = resolve_connect_transcript(connect(req), Some(&transcript), &home);

        assert_eq!(outcome.envelope.status, AlphaStatus::Error);
        assert_eq!(outcome.envelope.code, "transcript_scope_mismatch");
        assert!(outcome.connection.is_none());
    }

    #[test]
    fn resolve_connect_transcript_blocks_ambiguous_scope_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        let sessions = home.join(".codex/sessions");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        write_project_config(&project);
        write_codex_session(&sessions.join("one.jsonl"), "s1", &project);
        write_codex_session(&sessions.join("two.jsonl"), "s2", &project);

        let outcome = resolve_connect_transcript(connect(request(&project)), None, &home);

        assert_eq!(outcome.envelope.status, AlphaStatus::Blocked);
        assert_eq!(outcome.envelope.code, "transcript_ambiguous");
        assert!(outcome.connection.is_none());
        let data = outcome.envelope.data.unwrap();
        assert_eq!(data["transcript"]["status"], "ambiguous");
        assert_eq!(
            data["transcript"]["candidates"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            outcome.envelope.next_actions[0].params["transcript_path"],
            "<candidate path>"
        );
    }
}
