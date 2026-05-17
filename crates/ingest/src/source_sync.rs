// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use daemon8_core::project_config::{
    ConversationSource, FileSource, ProjectConfig, ProjectSource, parse_project_config_file,
    resolve_project_source_path,
};
use daemon8_parse::{ConversationEvent, ParsedLine, parse_conversation_line};
use daemon8_store::{CursorState, CursorStore};
use daemon8_types::{
    AppName, Observation, ObservationKind, Origin, SYSTEM_TAG, Severity, SourceLocation,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const CONFIG_PATH: &str = ".daemon8/config.md";
const MAX_LINES_PER_TRIGGER: usize = 500;
const MAX_BYTES_PER_TRIGGER: u64 = 256 * 1024;

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
        let config_path = request.scope_root.join(CONFIG_PATH);
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

        let mut report = SourceSyncReport::default();
        for source in &config.sources {
            let source_report = match source {
                ProjectSource::File(file) => {
                    self.ingest_file_source(&request.scope_root, &config, file)
                        .await
                }
                ProjectSource::Conversation(conversation) => {
                    self.ingest_conversation_source(&request.scope_root, &config, conversation)
                        .await
                }
            };
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
        let path = match resolve_project_source_path(config, &project_source) {
            Ok(path) => path,
            Err(err) => {
                report.failure(&source.id, None, "invalid_source_path", err.to_string());
                return report;
            }
        };
        let source_instance = path.display().to_string();
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

        let window = match read_complete_window(
            &path,
            self.cursor_position(scope_root, &source.id, &path).await,
        ) {
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
            .upsert_cursor(scope_root, &source.id, &path, window.next_position)
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
        let path = match resolve_project_source_path(config, &project_source) {
            Ok(path) => path,
            Err(err) => {
                report.failure(&source.id, None, "invalid_source_path", err.to_string());
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

    async fn ingest_conversation_instance(
        &self,
        scope_root: &Path,
        source: &ConversationSource,
        path: &Path,
    ) -> SourceSyncReport {
        let mut report = SourceSyncReport::default();
        let window = match read_complete_window(
            path,
            self.cursor_position(scope_root, &source.id, path).await,
        ) {
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
            .upsert_cursor(scope_root, &source.id, path, window.next_position)
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

    async fn cursor_position(&self, scope_root: &Path, source: &str, path: &Path) -> Option<u64> {
        let scope_root = scope_root.display().to_string();
        let source_instance = path.display().to_string();
        match self
            .cursors
            .get_cursor(&scope_root, source, &source_instance)
            .await
        {
            Ok(cursor) => cursor.map(|cursor| cursor.position),
            Err(err) => {
                tracing::warn!(source, source_instance, error = %err, "cursor lookup failed");
                None
            }
        }
    }

    async fn upsert_cursor(
        &self,
        scope_root: &Path,
        source: &str,
        path: &Path,
        position: u64,
    ) -> Result<(), daemon8_store::StoreError> {
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
    stamped_observation(
        &source.service,
        &source.id,
        path,
        source.tags.clone(),
        kind,
        data,
        parsed.severity.unwrap_or(Severity::Info),
    )
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

    stamped_observation(
        &source.service,
        &source.id,
        path,
        source.tags.clone(),
        kind,
        data,
        Severity::Info,
    )
}

fn stamped_observation(
    service: &str,
    source: &str,
    path: &Path,
    mut tags: Vec<String>,
    kind: ObservationKind,
    data: Value,
    severity: Severity,
) -> Observation {
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
    obs.service = Some(Arc::from(service));
    obs.source = Some(Arc::from(source));
    obs.source_instance = Some(Arc::from(path.display().to_string()));
    if !tags.is_empty() {
        obs.tags = Some(tags);
    }
    obs
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

fn read_complete_window(path: &Path, cursor_position: Option<u64>) -> std::io::Result<ReadWindow> {
    let bytes = std::fs::read(path)?;
    let file_len = bytes.len() as u64;
    let cursor_is_valid = cursor_position.is_some_and(|position| position <= file_len);
    let start = if cursor_is_valid {
        cursor_position.unwrap()
    } else {
        file_len.saturating_sub(MAX_BYTES_PER_TRIGGER)
    };
    let end = (start + MAX_BYTES_PER_TRIGGER).min(file_len);
    let window = &bytes[start as usize..end as usize];
    Ok(collect_complete_lines(
        window,
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
            .trigger_sources(SourceTriggerRequest {
                scope_root: root.to_path_buf(),
            })
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
        assert_eq!(
            observations[0].source_instance.as_deref(),
            Some(tmp.path().join("app.log").display().to_string().as_str())
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
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
            })
            .await;

        assert_eq!(report.observations_written, 2);
        let cursors = store
            .cursor_store()
            .list_cursors_for_scope(&tmp.path().display().to_string())
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
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
            })
            .await;
        std::fs::write(&log, "first\nsecond\n").unwrap();
        let report = trigger
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
            })
            .await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 2);
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
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
            })
            .await;
        std::fs::write(&log, "new\n").unwrap();
        let report = trigger
            .trigger_sources(SourceTriggerRequest {
                scope_root: tmp.path().to_path_buf(),
            })
            .await;

        assert_eq!(report.observations_written, 1);
        assert_eq!(writer.observations().len(), 3);
    }
}
