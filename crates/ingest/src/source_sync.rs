// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{VecDeque, hash_map::DefaultHasher};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use daemon8_core::project_config::{
    ConversationSource, FileSource, ProjectConfig, ProjectSource, parse_project_config_file,
    resolve_project_source_path,
};
use daemon8_parse::{
    ConversationEvent, ParsedLine, parse_conversation_line, timestamp::normalize_timestamp_ns,
};
use daemon8_store::{CursorState, CursorStore, StoreError};
use daemon8_types::{
    AppName, Observation, ObservationKind, Origin, SYSTEM_TAG, Severity, SourceLocation,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const CONFIG_PATH: &str = ".daemon8/config.md";
const MAX_LINES_PER_TRIGGER: usize = 500;
const MAX_BYTES_PER_TRIGGER: u64 = 256 * 1024;
const CURSOR_MARKER_BYTES: u64 = 256;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSyncReport {
    pub sources_considered: u64,
    pub instances_considered: u64,
    pub observations_written: u64,
    pub observations_deduped: u64,
    pub cursors_updated: u64,
    pub failures: Vec<SourceSyncFailure>,
}

impl SourceSyncReport {
    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }

    fn failure(
        &mut self,
        source: impl Into<String>,
        source_instance: Option<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.failures.push(SourceSyncFailure {
            source: source.into(),
            source_instance,
            code: code.into(),
            message: message.into(),
        });
    }

    fn absorb(&mut self, other: SourceSyncReport) {
        self.sources_considered += other.sources_considered;
        self.instances_considered += other.instances_considered;
        self.observations_written += other.observations_written;
        self.observations_deduped += other.observations_deduped;
        self.cursors_updated += other.cursors_updated;
        self.failures.extend(other.failures);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSyncFailure {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_instance: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct SourceTriggerRequest {
    pub scope_root: PathBuf,
    pub active_transcript: Option<ActiveTranscriptSource>,
}

impl SourceTriggerRequest {
    pub fn project(scope_root: PathBuf) -> Self {
        Self {
            scope_root,
            active_transcript: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTranscriptSource {
    pub provider: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationWriteStatus {
    Inserted,
    Deduped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationWriteResult {
    pub status: ObservationWriteStatus,
    pub id: Option<u64>,
}

#[async_trait]
pub trait ObservationWriter: Send + Sync {
    async fn write_observation(&self, obs: Observation) -> Result<ObservationWriteResult, String>;
}

#[async_trait]
pub trait SourceTrigger: Send + Sync {
    async fn trigger_sources(&self, request: SourceTriggerRequest) -> SourceSyncReport;
}

pub struct ConfiguredSourceTrigger {
    cursors: Arc<dyn CursorStore>,
    writer: Arc<dyn ObservationWriter>,
}

impl ConfiguredSourceTrigger {
    pub fn new(cursors: Arc<dyn CursorStore>, writer: Arc<dyn ObservationWriter>) -> Self {
        Self { cursors, writer }
    }
}

#[async_trait]
impl SourceTrigger for ConfiguredSourceTrigger {
    async fn trigger_sources(&self, request: SourceTriggerRequest) -> SourceSyncReport {
        let scope_root = canonical_source_path(&request.scope_root).unwrap_or(request.scope_root);
        let config_path = scope_root.join(CONFIG_PATH);
        let config = match parse_project_config_file(&config_path) {
            Ok(config) => config,
            Err(err) => {
                let mut report = SourceSyncReport::default();
                report.failure(
                    "project.config",
                    Some(config_path.display().to_string()),
                    "invalid_config",
                    err.to_string(),
                );
                return report;
            }
        };

        let active_transcript = request.active_transcript;
        let active_transcript_path = active_transcript
            .as_ref()
            .and_then(|active| canonical_source_path(&active.path).ok());
        let mut report = SourceSyncReport::default();
        for source in &config.sources {
            let source_report = match source {
                ProjectSource::File(file) => {
                    self.ingest_file_source(&scope_root, &config, file).await
                }
                ProjectSource::Conversation(conversation) => {
                    if let Some(active) = active_transcript.as_ref()
                        && let Some(path) = active_transcript_path.as_ref()
                        && active.provider == conversation.provider
                        && conversation_source_covers_transcript(&config, conversation, path)
                    {
                        self.ingest_selected_conversation_source(&scope_root, conversation, path)
                            .await
                    } else {
                        self.ingest_conversation_source(&scope_root, &config, conversation)
                            .await
                    }
                }
            };
            report.absorb(source_report);
        }
        if let Some(active_transcript) = active_transcript {
            let source_report = self
                .ingest_active_transcript(&scope_root, &config, &active_transcript)
                .await;
            report.absorb(source_report);
        }
        report
    }
}

impl ConfiguredSourceTrigger {
    async fn ingest_file_source(
        &self,
        scope_root: &Path,
        config: &ProjectConfig,
        source: &FileSource,
    ) -> SourceSyncReport {
        let mut report = SourceSyncReport {
            sources_considered: 1,
            ..Default::default()
        };
        let project_source = ProjectSource::File(source.clone());
        let resolved_path = match resolve_project_source_path(config, &project_source) {
            Ok(path) => path,
            Err(err) => {
                report.failure(&source.id, None, "invalid_source_path", err.to_string());
                return report;
            }
        };
        let source_instance = resolved_path.display().to_string();
        report.instances_considered += 1;

        let parser = match daemon8_parse::resolve_parser_with_pattern(
            &source.parser,
            source.parser_pattern.as_deref(),
        ) {
            Ok(parser) => parser,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(source_instance),
                    "invalid_parser",
                    err.to_string(),
                );
                return report;
            }
        };

        let path = match canonical_source_path(&resolved_path) {
            Ok(path) => path,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(source_instance),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };
        let source_instance = path.display().to_string();
        let fingerprint = match source_file_fingerprint(&path) {
            Ok(fingerprint) => fingerprint,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(source_instance),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };
        let cursor_position = match self
            .cursor_position(scope_root, &source.id, &path, &fingerprint)
            .await
        {
            Ok(position) => position,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(source_instance),
                    "cursor_lookup_failed",
                    err.to_string(),
                );
                return report;
            }
        };

        let window = match read_complete_window(&path, cursor_position) {
            Ok(window) => window,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(path.display().to_string()),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };

        for line in &window.lines {
            let Some(parsed) = parser.parse(&line.text) else {
                continue;
            };
            let obs = file_observation(source, &path, parsed);
            match self.writer.write_observation(obs).await {
                Ok(result) => match result.status {
                    ObservationWriteStatus::Inserted => report.observations_written += 1,
                    ObservationWriteStatus::Deduped => report.observations_deduped += 1,
                },
                Err(err) => {
                    report.failure(
                        &source.id,
                        Some(path.display().to_string()),
                        "write_failed",
                        err,
                    );
                    return report;
                }
            }
        }

        if let Err(err) = self
            .upsert_cursor(
                scope_root,
                &source.id,
                &path,
                window.next_position,
                &fingerprint,
            )
            .await
        {
            report.failure(
                &source.id,
                Some(path.display().to_string()),
                "cursor_update_failed",
                err.to_string(),
            );
            return report;
        }
        report.cursors_updated += 1;
        report
    }

    async fn ingest_conversation_source(
        &self,
        scope_root: &Path,
        config: &ProjectConfig,
        source: &ConversationSource,
    ) -> SourceSyncReport {
        let mut report = SourceSyncReport {
            sources_considered: 1,
            ..Default::default()
        };
        if !matches!(source.provider.as_str(), "claude" | "codex" | "gemini") {
            report.failure(
                &source.id,
                None,
                "invalid_provider",
                format!("unsupported conversation provider '{}'", source.provider),
            );
            return report;
        }

        let project_source = ProjectSource::Conversation(source.clone());
        let resolved_path = match resolve_project_source_path(config, &project_source) {
            Ok(path) => path,
            Err(err) => {
                report.failure(&source.id, None, "invalid_source_path", err.to_string());
                return report;
            }
        };
        let path = match canonical_source_path(&resolved_path) {
            Ok(path) => path,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(resolved_path.display().to_string()),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };

        let instances = match conversation_instances(&path) {
            Ok(instances) => instances,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(path.display().to_string()),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };

        for instance in instances {
            report.instances_considered += 1;
            let mut instance_report = self
                .ingest_conversation_instance(scope_root, source, &instance)
                .await;
            report.observations_written += instance_report.observations_written;
            report.observations_deduped += instance_report.observations_deduped;
            report.cursors_updated += instance_report.cursors_updated;
            report.failures.append(&mut instance_report.failures);
        }
        report
    }

    async fn ingest_selected_conversation_source(
        &self,
        scope_root: &Path,
        source: &ConversationSource,
        path: &Path,
    ) -> SourceSyncReport {
        let mut report = SourceSyncReport {
            sources_considered: 1,
            instances_considered: 1,
            ..Default::default()
        };
        let mut instance_report = self
            .ingest_conversation_instance(scope_root, source, path)
            .await;
        report.observations_written += instance_report.observations_written;
        report.observations_deduped += instance_report.observations_deduped;
        report.cursors_updated += instance_report.cursors_updated;
        report.failures.append(&mut instance_report.failures);
        report
    }

    async fn ingest_active_transcript(
        &self,
        scope_root: &Path,
        config: &ProjectConfig,
        active: &ActiveTranscriptSource,
    ) -> SourceSyncReport {
        let mut report = SourceSyncReport {
            sources_considered: 1,
            ..Default::default()
        };
        let source_id = format!("runtime.transcript.{}", active.provider);
        if !matches!(active.provider.as_str(), "claude" | "codex" | "gemini") {
            report.failure(
                &source_id,
                Some(active.path.display().to_string()),
                "invalid_provider",
                format!("unsupported conversation provider '{}'", active.provider),
            );
            return report;
        }

        let path = match canonical_source_path(&active.path) {
            Ok(path) => path,
            Err(err) => {
                report.failure(
                    &source_id,
                    Some(active.path.display().to_string()),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };
        if configured_source_covers_transcript(config, &active.provider, &path) {
            return report;
        }

        report.instances_considered += 1;
        let source = ConversationSource {
            id: source_id,
            service: active.provider.clone(),
            path: path.display().to_string(),
            provider: active.provider.clone(),
            tags: vec!["active_transcript".into()],
        };
        let mut instance_report = self
            .ingest_conversation_instance(scope_root, &source, &path)
            .await;
        report.observations_written += instance_report.observations_written;
        report.observations_deduped += instance_report.observations_deduped;
        report.cursors_updated += instance_report.cursors_updated;
        report.failures.append(&mut instance_report.failures);
        report
    }

    async fn ingest_conversation_instance(
        &self,
        scope_root: &Path,
        source: &ConversationSource,
        path: &Path,
    ) -> SourceSyncReport {
        let mut report = SourceSyncReport::default();
        let fingerprint = match source_file_fingerprint(path) {
            Ok(fingerprint) => fingerprint,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(path.display().to_string()),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };
        let cursor_position = match self
            .cursor_position(scope_root, &source.id, path, &fingerprint)
            .await
        {
            Ok(position) => position,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(path.display().to_string()),
                    "cursor_lookup_failed",
                    err.to_string(),
                );
                return report;
            }
        };

        let window = match read_complete_window(path, cursor_position) {
            Ok(window) => window,
            Err(err) => {
                report.failure(
                    &source.id,
                    Some(path.display().to_string()),
                    "read_failed",
                    err.to_string(),
                );
                return report;
            }
        };

        for line in &window.lines {
            for event in parse_conversation_line(&source.provider, &line.text) {
                let obs = conversation_observation(source, path, event);
                match self.writer.write_observation(obs).await {
                    Ok(result) => match result.status {
                        ObservationWriteStatus::Inserted => report.observations_written += 1,
                        ObservationWriteStatus::Deduped => report.observations_deduped += 1,
                    },
                    Err(err) => {
                        report.failure(
                            &source.id,
                            Some(path.display().to_string()),
                            "write_failed",
                            err,
                        );
                        return report;
                    }
                }
            }
        }

        if let Err(err) = self
            .upsert_cursor(
                scope_root,
                &source.id,
                path,
                window.next_position,
                &fingerprint,
            )
            .await
        {
            report.failure(
                &source.id,
                Some(path.display().to_string()),
                "cursor_update_failed",
                err.to_string(),
            );
            return report;
        }
        report.cursors_updated += 1;
        report
    }

    async fn cursor_position(
        &self,
        scope_root: &Path,
        source: &str,
        path: &Path,
        fingerprint: &SourceFileFingerprint,
    ) -> Result<Option<u64>, StoreError> {
        let scope_root = scope_root.display().to_string();
        let source_instance = path.display().to_string();
        let cursor = self
            .cursors
            .get_cursor(&scope_root, source, &source_instance)
            .await?;
        Ok(cursor.and_then(|cursor| valid_cursor_position(cursor, path, fingerprint)))
    }

    async fn upsert_cursor(
        &self,
        scope_root: &Path,
        source: &str,
        path: &Path,
        position: u64,
        fingerprint: &SourceFileFingerprint,
    ) -> Result<(), StoreError> {
        let marker = source_cursor_marker(path, position).ok().flatten();
        self.cursors
            .upsert_cursor(CursorState {
                id: None,
                scope_root: scope_root.display().to_string(),
                source: source.to_string(),
                source_instance: path.display().to_string(),
                position,
                updated_at: current_ns(),
                metadata: Some(json!({
                    "reader": "triggered",
                    "file": fingerprint,
                    "marker": marker,
                    "max_lines": MAX_LINES_PER_TRIGGER,
                    "max_bytes": MAX_BYTES_PER_TRIGGER
                })),
            })
            .await
    }
}

fn file_observation(source: &FileSource, path: &Path, parsed: ParsedLine) -> Observation {
    let mut data = Value::Object(parsed.fields);
    data["message"] = Value::String(parsed.message);
    if let Some(timestamp) = parsed.timestamp {
        data["timestamp"] = Value::String(timestamp);
    }
    data["parser"] = Value::String(source.parser.clone());
    let kind = parsed
        .channel
        .map(|channel| ObservationKind::Custom { channel })
        .unwrap_or(ObservationKind::Log);
    let source_timestamp = timestamp_from_data(&data);
    stamped_observation(ObservationStamp {
        service: &source.service,
        source: &source.id,
        path,
        tags: source.tags.clone(),
        kind,
        data,
        severity: parsed.severity.unwrap_or(Severity::Info),
        source_timestamp: source_timestamp.as_deref(),
    })
}

fn conversation_observation(
    source: &ConversationSource,
    path: &Path,
    event: ConversationEvent,
) -> Observation {
    let (kind, data) = match event {
        ConversationEvent::ToolUse {
            tool,
            input,
            call_id,
            timestamp,
        } => (
            ObservationKind::ToolCall {
                tool,
                input,
                output: None,
                exit_code: None,
                duration_ms: None,
            },
            json!({
                "event": "tool_use",
                "provider": source.provider,
                "call_id": call_id,
                "timestamp": timestamp
            }),
        ),
        ConversationEvent::ToolResult {
            call_id,
            output,
            exit_code,
            timestamp,
        } => (
            ObservationKind::ToolCall {
                tool: "tool_result".into(),
                input: Value::Null,
                output: Some(output),
                exit_code,
                duration_ms: None,
            },
            json!({
                "event": "tool_result",
                "provider": source.provider,
                "call_id": call_id,
                "timestamp": timestamp
            }),
        ),
        ConversationEvent::SessionMeta {
            session_id,
            cwd,
            provider,
            model,
        } => (
            ObservationKind::StateSnapshot {
                label: "conversation.session_meta".into(),
            },
            json!({
                "event": "session_meta",
                "provider": provider,
                "session_id": session_id,
                "cwd": cwd,
                "model": model
            }),
        ),
        ConversationEvent::UserPrompt { text, timestamp } => (
            ObservationKind::Custom {
                channel: "conversation.user_prompt".into(),
            },
            json!({
                "event": "user_prompt",
                "provider": source.provider,
                "text": text,
                "timestamp": timestamp
            }),
        ),
        ConversationEvent::TurnMeta {
            model,
            git_branch,
            git_sha,
            tokens,
            duration_ms,
            permission_mode,
            cli_version,
        } => (
            ObservationKind::StateSnapshot {
                label: "conversation.turn_meta".into(),
            },
            json!({
                "event": "turn_meta",
                "provider": source.provider,
                "model": model,
                "git_branch": git_branch,
                "git_sha": git_sha,
                "tokens": tokens,
                "duration_ms": duration_ms,
                "permission_mode": permission_mode,
                "cli_version": cli_version
            }),
        ),
        ConversationEvent::AgentSpawn {
            parent_session,
            child_session,
            role,
            nickname,
            status,
        } => (
            ObservationKind::Custom {
                channel: "conversation.agent_spawn".into(),
            },
            json!({
                "event": "agent_spawn",
                "provider": source.provider,
                "parent_session": parent_session,
                "child_session": child_session,
                "role": role,
                "nickname": nickname,
                "status": status
            }),
        ),
        ConversationEvent::FileChange { path, timestamp } => (
            ObservationKind::Custom {
                channel: "conversation.file_change".into(),
            },
            json!({
                "event": "file_change",
                "provider": source.provider,
                "path": path,
                "timestamp": timestamp
            }),
        ),
        ConversationEvent::RawEvent {
            line_type,
            timestamp,
        } => (
            ObservationKind::Custom {
                channel: "conversation.raw_event".into(),
            },
            json!({
                "event": "raw_event",
                "provider": source.provider,
                "line_type": line_type,
                "timestamp": timestamp
            }),
        ),
    };

    let source_timestamp = timestamp_from_data(&data);
    stamped_observation(ObservationStamp {
        service: &source.service,
        source: &source.id,
        path,
        tags: source.tags.clone(),
        kind,
        data,
        severity: Severity::Info,
        source_timestamp: source_timestamp.as_deref(),
    })
}

struct ObservationStamp<'a> {
    service: &'a str,
    source: &'a str,
    path: &'a Path,
    tags: Vec<String>,
    kind: ObservationKind,
    data: Value,
    severity: Severity,
    source_timestamp: Option<&'a str>,
}

fn stamped_observation(input: ObservationStamp<'_>) -> Observation {
    let ObservationStamp {
        service,
        source,
        path,
        mut tags,
        kind,
        data,
        severity,
        source_timestamp,
    } = input;
    tags.retain(|tag| tag != SYSTEM_TAG);
    let mut obs = Observation::new(
        Origin::Application {
            name: AppName::from(service),
        },
        kind,
        data,
        severity,
        Some(SourceLocation {
            file: path.display().to_string(),
            line: 0,
            function: None,
        }),
    );
    if let Some(timestamp_ns) = source_timestamp.and_then(parsed_timestamp_ns) {
        obs.timestamp_ns = timestamp_ns;
    }
    obs.service = Some(Arc::from(service));
    obs.source = Some(Arc::from(source));
    obs.source_instance = Some(Arc::from(path.display().to_string()));
    if !tags.is_empty() {
        obs.tags = Some(tags);
    }
    obs
}

fn timestamp_from_data(data: &Value) -> Option<String> {
    data.get("timestamp")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parsed_timestamp_ns(raw: &str) -> Option<u64> {
    normalize_timestamp_ns(raw).and_then(|ns| u64::try_from(ns).ok())
}

#[derive(Debug)]
struct ReadWindow {
    lines: Vec<CompleteLine>,
    next_position: u64,
}

#[derive(Debug)]
struct CompleteLine {
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceFileFingerprint {
    len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_ns: Option<u64>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceCursorMarker {
    position: u64,
    window_start: u64,
    hash: String,
}

fn canonical_source_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

fn source_file_fingerprint(path: &Path) -> std::io::Result<SourceFileFingerprint> {
    let metadata = std::fs::metadata(path)?;
    Ok(SourceFileFingerprint {
        len: metadata.len(),
        modified_ns: metadata.modified().ok().and_then(system_time_ns),
        #[cfg(unix)]
        dev: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
        #[cfg(unix)]
        ino: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
    })
}

fn system_time_ns(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos() as u64)
}

fn source_cursor_marker(path: &Path, position: u64) -> std::io::Result<Option<SourceCursorMarker>> {
    if position == 0 {
        return Ok(None);
    }

    let window_start = position.saturating_sub(CURSOR_MARKER_BYTES);
    let window_len = position - window_start;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(window_start))?;
    let mut bytes = Vec::with_capacity(window_len as usize);
    file.take(window_len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != window_len {
        return Ok(None);
    }

    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(Some(SourceCursorMarker {
        position,
        window_start,
        hash: format!("{:016x}", hasher.finish()),
    }))
}

fn valid_cursor_position(
    cursor: CursorState,
    path: &Path,
    fingerprint: &SourceFileFingerprint,
) -> Option<u64> {
    if cursor.position > fingerprint.len {
        return None;
    }
    let metadata = cursor.metadata.as_ref()?;
    let stored = metadata
        .get("file")
        .and_then(|value| serde_json::from_value::<SourceFileFingerprint>(value.clone()).ok())?;
    if !same_source_file(&stored, fingerprint) {
        return None;
    }
    if cursor.position == 0 {
        return Some(0);
    }

    let stored_marker = metadata
        .get("marker")
        .and_then(|value| serde_json::from_value::<SourceCursorMarker>(value.clone()).ok())?;
    if stored_marker.position != cursor.position {
        return None;
    }
    let current_marker = source_cursor_marker(path, cursor.position).ok().flatten()?;
    if stored_marker == current_marker {
        Some(cursor.position)
    } else {
        None
    }
}

#[cfg(unix)]
fn same_source_file(stored: &SourceFileFingerprint, current: &SourceFileFingerprint) -> bool {
    stored.dev == current.dev && stored.ino == current.ino
}

#[cfg(not(unix))]
fn same_source_file(stored: &SourceFileFingerprint, current: &SourceFileFingerprint) -> bool {
    stored.modified_ns == current.modified_ns
}

fn read_complete_window(path: &Path, cursor_position: Option<u64>) -> std::io::Result<ReadWindow> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let cursor_is_valid = cursor_position.is_some_and(|position| position <= file_len);
    let start = if cursor_is_valid {
        cursor_position.unwrap()
    } else {
        file_len.saturating_sub(MAX_BYTES_PER_TRIGGER)
    };
    let end = (start + MAX_BYTES_PER_TRIGGER).min(file_len);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    file.take(end - start).read_to_end(&mut bytes)?;
    Ok(collect_complete_lines(
        &bytes,
        start,
        !cursor_is_valid && start > 0,
    ))
}

fn collect_complete_lines(
    bytes: &[u8],
    base_offset: u64,
    drop_leading_partial: bool,
) -> ReadWindow {
    let mut cursor = 0usize;
    if drop_leading_partial && !bytes.is_empty() {
        if let Some(pos) = bytes.iter().position(|byte| *byte == b'\n') {
            cursor = pos + 1;
        } else {
            return ReadWindow {
                lines: Vec::new(),
                next_position: base_offset,
            };
        }
    }

    let mut lines = VecDeque::new();
    let mut line_start = cursor;
    let mut next_position = base_offset + cursor as u64;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\n' {
            let mut line = &bytes[line_start..cursor];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len().saturating_sub(1)];
            }
            if lines.len() == MAX_LINES_PER_TRIGGER {
                lines.pop_front();
            }
            lines.push_back(CompleteLine {
                text: String::from_utf8_lossy(line).into_owned(),
            });
            cursor += 1;
            next_position = base_offset + cursor as u64;
            line_start = cursor;
            continue;
        }
        cursor += 1;
    }

    ReadWindow {
        lines: lines.into_iter().collect(),
        next_position,
    }
}

fn conversation_instances(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut paths = Vec::new();
    collect_jsonl_files(path, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn configured_source_covers_transcript(
    config: &ProjectConfig,
    provider: &str,
    transcript_path: &Path,
) -> bool {
    config.sources.iter().any(|source| {
        let ProjectSource::Conversation(conversation) = source else {
            return false;
        };
        if conversation.provider != provider {
            return false;
        }
        conversation_source_covers_transcript(config, conversation, transcript_path)
    })
}

fn conversation_source_covers_transcript(
    config: &ProjectConfig,
    source: &ConversationSource,
    transcript_path: &Path,
) -> bool {
    let project_source = ProjectSource::Conversation(source.clone());
    let Ok(path) = resolve_project_source_path(config, &project_source) else {
        return false;
    };
    let Ok(path) = canonical_source_path(&path) else {
        return false;
    };
    let Ok(instances) = conversation_instances(&path) else {
        return false;
    };
    instances.iter().any(|instance| {
        canonical_source_path(instance)
            .map(|instance| transcript_path == instance)
            .unwrap_or(false)
    })
}

fn collect_jsonl_files(path: &Path, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_jsonl_files(&path, paths)?;
            continue;
        }
        if file_type.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            paths.push(path);
        }
    }
    Ok(())
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use daemon8_store::SurrealStore;
    use daemon8_types::ObservationKindTag;

    #[derive(Default)]
    struct VecWriter {
        observations: Mutex<Vec<Observation>>,
    }

    #[async_trait]
    impl ObservationWriter for VecWriter {
        async fn write_observation(
            &self,
            obs: Observation,
        ) -> Result<ObservationWriteResult, String> {
            self.observations.lock().unwrap().push(obs);
            Ok(ObservationWriteResult {
                status: ObservationWriteStatus::Inserted,
                id: Some(1),
            })
        }
    }

    impl VecWriter {
        fn observations(&self) -> Vec<Observation> {
            self.observations.lock().unwrap().clone()
        }
    }

    struct FailingCursorStore;

    #[async_trait]
    impl CursorStore for FailingCursorStore {
        async fn upsert_cursor(&self, _cursor: CursorState) -> Result<(), StoreError> {
            Ok(())
        }

        async fn get_cursor(
            &self,
            _scope_root: &str,
            _source: &str,
            _source_instance: &str,
        ) -> Result<Option<CursorState>, StoreError> {
            Err(StoreError::Other("cursor store offline".into()))
        }

        async fn list_cursors_for_scope(
            &self,
            _scope_root: &str,
        ) -> Result<Vec<CursorState>, StoreError> {
            Ok(Vec::new())
        }
    }

    fn config(root: &Path, source: &str) -> String {
        format!(
            r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: source-sync-test
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "{}"
sources:
{source}
---
# daemon8
"#,
            root.display()
        )
    }

    fn write_config(root: &Path, source: &str) {
        let dir = root.join(".daemon8");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.md"), config(root, source)).unwrap();
    }

    async fn trigger(root: &Path, writer: Arc<VecWriter>) -> SourceSyncReport {
        let store = SurrealStore::memory().await.unwrap();
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer);
        trigger
            .trigger_sources(SourceTriggerRequest::project(root.to_path_buf()))
            .await
    }

    #[test]
    fn complete_line_window_leaves_unterminated_tail_pending() {
        let window = collect_complete_lines(b"one\ntwo\nthree", 0, false);
        assert_eq!(window.lines.len(), 2);
        assert_eq!(window.next_position, 8);
        assert_eq!(window.lines[1].text, "two");
    }

    #[test]
    fn complete_line_window_keeps_last_500_lines() {
        let input = (0..600)
            .map(|n| format!("{n}\n"))
            .collect::<Vec<_>>()
            .join("");
        let window = collect_complete_lines(input.as_bytes(), 0, false);
        assert_eq!(window.lines.len(), 500);
        assert_eq!(window.lines[0].text, "100");
        assert_eq!(window.lines[499].text, "599");
    }

    #[tokio::test]
    async fn file_source_ingests_project_configured_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "first\nsecond\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
    tags: [runtime]
"#,
        );
        let writer = Arc::new(VecWriter::default());

        let report = trigger(tmp.path(), writer.clone()).await;

        assert_eq!(report.sources_considered, 1);
        assert_eq!(report.instances_considered, 1);
        assert_eq!(report.observations_written, 2);
        assert_eq!(report.cursors_updated, 1);
        let observations = writer.observations();
        assert_eq!(observations[0].service.as_deref(), Some("app"));
        assert_eq!(observations[0].source.as_deref(), Some("app.logs"));
        let canonical_log = std::fs::canonicalize(tmp.path().join("app.log")).unwrap();
        assert_eq!(
            observations[0].source_instance.as_deref(),
            Some(canonical_log.display().to_string().as_str())
        );
        assert!(
            observations[0]
                .tags
                .as_ref()
                .unwrap()
                .contains(&"runtime".into())
        );
    }

    #[tokio::test]
    async fn file_source_applies_parsed_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("app.log"),
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":\"boot\"}\n",
        )
        .unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: json
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let writer = Arc::new(VecWriter::default());

        let report = trigger(tmp.path(), writer.clone()).await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(
            writer.observations()[0].timestamp_ns,
            1_767_225_600_000_000_000
        );
    }

    #[tokio::test]
    async fn missing_file_reports_failure() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/missing.log"
"#,
        );

        let report = trigger(tmp.path(), Arc::new(VecWriter::default())).await;

        assert_eq!(report.failures[0].code, "read_failed");
    }

    #[tokio::test]
    async fn invalid_file_parser_reports_failure() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "first\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: nope
    path: "$PRJ_ROOT/app.log"
"#,
        );

        let report = trigger(tmp.path(), Arc::new(VecWriter::default())).await;

        assert_eq!(report.failures[0].code, "invalid_parser");
    }

    #[tokio::test]
    async fn conversation_directory_ingests_jsonl_files() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("one.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"cwd\":\"/tmp/project\",\"model\":\"gpt\"}}\n",
        )
        .unwrap();
        std::fs::write(
            sessions.join("two.txt"),
            r#"{"type":"session_meta","session_id":"ignored"}"#,
        )
        .unwrap();
        write_config(
            tmp.path(),
            r#"  - id: codex.sessions
    service: codex
    kind: conversation
    provider: codex
    path: "$PRJ_ROOT/sessions"
"#,
        );
        let writer = Arc::new(VecWriter::default());

        let report = trigger(tmp.path(), writer.clone()).await;

        assert_eq!(report.instances_considered, 1);
        assert_eq!(report.observations_written, 2);
        let observations = writer.observations();
        assert_eq!(
            observations[0].kind.tag(),
            ObservationKindTag::StateSnapshot
        );
        assert_eq!(observations[0].source.as_deref(), Some("codex.sessions"));
    }

    #[tokio::test]
    async fn conversation_source_applies_event_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("one.jsonl"),
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello\"}]}}\n",
        )
        .unwrap();
        write_config(
            tmp.path(),
            r#"  - id: codex.sessions
    service: codex
    kind: conversation
    provider: codex
    path: "$PRJ_ROOT/sessions"
"#,
        );
        let writer = Arc::new(VecWriter::default());

        let report = trigger(tmp.path(), writer.clone()).await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(
            writer.observations()[0].timestamp_ns,
            1_767_225_600_000_000_000
        );
    }

    #[tokio::test]
    async fn active_transcript_overlay_ingests_runtime_source_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("outside-sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("active.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"cwd\":\"/tmp/project\"}}\n",
        )
        .unwrap();
        write_config(tmp.path(), "  []");
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        let report = trigger
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
                active_transcript: Some(ActiveTranscriptSource {
                    provider: "codex".into(),
                    path: transcript.clone(),
                }),
            })
            .await;

        assert_eq!(report.sources_considered, 1);
        assert_eq!(report.instances_considered, 1);
        assert_eq!(report.observations_written, 1);
        assert_eq!(report.cursors_updated, 1);
        let observations = writer.observations();
        let canonical = std::fs::canonicalize(transcript).unwrap();
        assert_eq!(
            observations[0].source.as_deref(),
            Some("runtime.transcript.codex")
        );
        assert_eq!(
            observations[0].source_instance.as_deref(),
            Some(canonical.display().to_string().as_str())
        );
        assert!(
            observations[0]
                .tags
                .as_ref()
                .unwrap()
                .contains(&"active_transcript".into())
        );
    }

    #[tokio::test]
    async fn active_transcript_overlay_uses_cursor_on_second_trigger() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("outside-sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("active.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"cwd\":\"/tmp/project\"}}\n",
        )
        .unwrap();
        write_config(tmp.path(), "  []");
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());
        let request = || SourceTriggerRequest {
            scope_root: tmp.path().to_path_buf(),
            active_transcript: Some(ActiveTranscriptSource {
                provider: "codex".into(),
                path: transcript.clone(),
            }),
        };

        trigger.trigger_sources(request()).await;
        std::fs::write(
            &transcript,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"cwd\":\"/tmp/project\"}}\n{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello\"}]}}\n",
        )
        .unwrap();
        let report = trigger.trigger_sources(request()).await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 2);
    }

    #[tokio::test]
    async fn active_transcript_overlay_skips_configured_duplicate_path() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("active.jsonl");
        std::fs::write(
            &transcript,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"shell\",\"call_id\":\"c1\",\"arguments\":\"{}\"}}\n",
        )
        .unwrap();
        std::fs::write(
            sessions.join("other.jsonl"),
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"ignored\",\"call_id\":\"c2\",\"arguments\":\"{}\"}}\n",
        )
        .unwrap();
        write_config(
            tmp.path(),
            r#"  - id: codex.sessions
    service: codex
    kind: conversation
    provider: codex
    path: "$PRJ_ROOT/sessions"
"#,
        );
        let writer = Arc::new(VecWriter::default());
        let store = SurrealStore::memory().await.unwrap();
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        let report = trigger
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
                active_transcript: Some(ActiveTranscriptSource {
                    provider: "codex".into(),
                    path: transcript,
                }),
            })
            .await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 1);
        assert_eq!(
            writer.observations()[0].source.as_deref(),
            Some("codex.sessions")
        );
    }

    #[tokio::test]
    async fn active_transcript_overlay_skips_configured_duplicate_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let transcript = sessions.join("active.jsonl");
        std::fs::write(
            &transcript,
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"shell\",\"call_id\":\"c1\",\"arguments\":\"{}\"}}\n",
        )
        .unwrap();
        write_config(
            tmp.path(),
            r#"  - id: codex.active
    service: codex
    kind: conversation
    provider: codex
    path: "$PRJ_ROOT/sessions/active.jsonl"
"#,
        );
        let writer = Arc::new(VecWriter::default());
        let store = SurrealStore::memory().await.unwrap();
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        let report = trigger
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
                active_transcript: Some(ActiveTranscriptSource {
                    provider: "codex".into(),
                    path: sessions.join("../sessions/active.jsonl"),
                }),
            })
            .await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 1);
        assert_eq!(
            writer.observations()[0].source.as_deref(),
            Some("codex.active")
        );
    }

    #[tokio::test]
    async fn cursor_lookup_failure_reports_failure() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "first\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(FailingCursorStore), writer.clone());

        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.failures[0].code, "cursor_lookup_failed");
        assert!(writer.observations().is_empty());
    }

    #[tokio::test]
    async fn distinct_source_ids_share_file_with_distinct_cursors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("shared.log"), "first\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.one
    service: app
    kind: file
    path: "$PRJ_ROOT/shared.log"
  - id: app.two
    service: app
    kind: file
    path: "$PRJ_ROOT/shared.log"
"#,
        );
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer);

        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.observations_written, 2);
        let cursors = store
            .cursor_store()
            .list_cursors_for_scope(
                &std::fs::canonicalize(tmp.path())
                    .unwrap()
                    .display()
                    .to_string(),
            )
            .await
            .unwrap();
        assert_eq!(cursors.len(), 2);
    }

    #[tokio::test]
    async fn append_only_ingest_uses_cursor_on_second_trigger() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("app.log");
        std::fs::write(&log, "first\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;
        std::fs::write(&log, "first\nsecond\n").unwrap();
        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 2);
    }

    #[tokio::test]
    async fn lexical_path_variants_share_canonical_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("logs")).unwrap();
        let log = tmp.path().join("app.log");
        std::fs::write(&log, "first\n").unwrap();
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/logs/../app.log"
"#,
        );
        trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        std::fs::write(&log, "first\nsecond\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.observations_written, 1);
        let cursors = store
            .cursor_store()
            .list_cursors_for_scope(
                &std::fs::canonicalize(tmp.path())
                    .unwrap()
                    .display()
                    .to_string(),
            )
            .await
            .unwrap();
        assert_eq!(cursors.len(), 1);
    }

    #[tokio::test]
    async fn replaced_larger_file_resets_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("app.log");
        std::fs::write(&log, "first\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;
        std::fs::remove_file(&log).unwrap();
        std::fs::write(&log, "fresh\nnew\n").unwrap();
        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.observations_written, 2);
        assert_eq!(writer.observations().len(), 3);
    }

    #[tokio::test]
    async fn same_inode_overwrite_resets_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("app.log");
        std::fs::write(&log, "first\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;
        std::fs::write(&log, "fresh\nnew\n").unwrap();
        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.observations_written, 2);
        assert_eq!(writer.observations().len(), 3);
    }

    #[tokio::test]
    async fn truncated_file_is_tailed_again() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("app.log");
        std::fs::write(&log, "first\nsecond\n").unwrap();
        write_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let store = SurrealStore::memory().await.unwrap();
        let writer = Arc::new(VecWriter::default());
        let trigger = ConfiguredSourceTrigger::new(Arc::new(store.cursor_store()), writer.clone());

        trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;
        std::fs::write(&log, "new\n").unwrap();
        let report = trigger
            .trigger_sources(SourceTriggerRequest::project(tmp.path().to_path_buf()))
            .await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 3);
    }
}
