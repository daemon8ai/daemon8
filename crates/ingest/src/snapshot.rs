// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daemon8_parse::ConversationEvent;

pub const VALID_FACETS: &[&str] = &[
    "user_messages",
    "assistant_messages",
    "tool_activity",
    "file_changes",
    "log_activity",
    "summary",
];

pub const DEFAULT_SNAPSHOT_LOOKBACK_MINUTES: u64 = 24 * 60;

#[derive(Debug, Clone)]
pub enum SnapshotSince {
    ConversationStart,
    Checkpoint { timestamp_ns: u64 },
    Duration { minutes: u64 },
}

impl Default for SnapshotSince {
    fn default() -> Self {
        Self::Duration {
            minutes: DEFAULT_SNAPSHOT_LOOKBACK_MINUTES,
        }
    }
}

impl SnapshotSince {
    pub fn cutoff_at(&self, now: SystemTime) -> SnapshotCutoff {
        SnapshotCutoff::from_since_at(self, now)
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotSource {
    pub provider: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct SnapshotRequest {
    pub since: SnapshotSince,
    pub facets: Vec<String>,
    pub sources: Vec<SnapshotSource>,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct SnapshotCutoff {
    pub ns: Option<u64>,
    pub ms: Option<u64>,
}

impl SnapshotCutoff {
    pub fn from_since_at(since: &SnapshotSince, now: SystemTime) -> Self {
        match since {
            SnapshotSince::ConversationStart => Self { ns: None, ms: None },
            SnapshotSince::Checkpoint { timestamp_ns } => Self {
                ns: Some(*timestamp_ns),
                ms: Some(timestamp_ns / 1_000_000),
            },
            SnapshotSince::Duration { minutes } => {
                let now_ns = now
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let window_ns = minutes.saturating_mul(60).saturating_mul(1_000_000_000);
                let cutoff_ns = now_ns.saturating_sub(window_ns);
                Self {
                    ns: Some(cutoff_ns),
                    ms: Some(cutoff_ns / 1_000_000),
                }
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FacetResult {
    pub path: String,
    pub bytes: u64,
    pub entry_count: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct SnapshotResult {
    pub snapshot_path: String,
    pub facets: BTreeMap<String, FacetResult>,
    pub sources_read: Vec<String>,
    pub time_range: SnapshotTimeRange,
}

#[derive(Debug, serde::Serialize)]
pub struct SnapshotTimeRange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("no transcript sources available")]
    NoSources,
    #[error("output directory creation failed: {0}")]
    OutputDir(std::io::Error),
    #[error("write failed for facet '{facet}': {source}")]
    WriteFacet {
        facet: String,
        source: std::io::Error,
    },
}

pub fn build_snapshot(request: &SnapshotRequest) -> Result<SnapshotResult, SnapshotError> {
    build_snapshot_at(request, SystemTime::now())
}

pub fn build_snapshot_at(
    request: &SnapshotRequest,
    now: SystemTime,
) -> Result<SnapshotResult, SnapshotError> {
    let cutoff = request.since.cutoff_at(now);

    if request.sources.is_empty() {
        return Err(SnapshotError::NoSources);
    }

    if let Some(parent) = request.output_dir.parent() {
        std::fs::create_dir_all(parent).map_err(SnapshotError::OutputDir)?;
    }
    std::fs::create_dir(&request.output_dir).map_err(SnapshotError::OutputDir)?;

    let mut all_events: Vec<ConversationEvent> = Vec::new();
    let mut sources_read: Vec<String> = Vec::new();

    for source in &request.sources {
        let events = parse_transcript_events(source, cutoff.ns);
        if !events.is_empty() {
            sources_read.push(source.path.display().to_string());
        }
        all_events.extend(events);
    }

    all_events.sort_by(|a, b| {
        event_timestamp_ns(a)
            .unwrap_or(0)
            .cmp(&event_timestamp_ns(b).unwrap_or(0))
    });

    let time_range = extract_time_range(&all_events);

    let active_facets: Vec<&str> = if request.facets.is_empty() {
        VALID_FACETS.to_vec()
    } else {
        request.facets.iter().map(|s| s.as_str()).collect()
    };

    let mut facets = BTreeMap::new();
    for facet_name in &active_facets {
        let (content, entry_count) = match *facet_name {
            "user_messages" => build_user_messages_facet(&all_events),
            "assistant_messages" => build_assistant_messages_facet(&all_events),
            "tool_activity" => build_tool_activity_facet(&all_events),
            "file_changes" => build_file_changes_facet(&all_events),
            "log_activity" => build_log_activity_facet(&all_events),
            "summary" => build_summary_facet(&all_events),
            _ => continue,
        };

        let file_name = format!("{}.md", facet_name.replace('_', "-"));
        let file_path = request.output_dir.join(&file_name);
        std::fs::write(&file_path, &content).map_err(|e| SnapshotError::WriteFacet {
            facet: facet_name.to_string(),
            source: e,
        })?;

        facets.insert(
            facet_name.to_string(),
            FacetResult {
                path: file_name,
                bytes: content.len() as u64,
                entry_count,
            },
        );
    }

    Ok(SnapshotResult {
        snapshot_path: request.output_dir.display().to_string(),
        facets,
        sources_read,
        time_range,
    })
}

fn parse_transcript_events(
    source: &SnapshotSource,
    since_cutoff_ns: Option<u64>,
) -> Vec<ConversationEvent> {
    let file = match std::fs::File::open(&source.path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let source_modified_ns = file
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| modified.as_nanos() as u64);
    let reader = BufReader::new(file);
    let Some(cutoff) = since_cutoff_ns else {
        return parse_all_transcript_events(source, reader);
    };
    let source_modified_in_window = source_modified_ns.is_some_and(|modified| modified >= cutoff);

    parse_bounded_transcript_events(source, reader, cutoff, source_modified_in_window)
}

fn parse_all_transcript_events(
    source: &SnapshotSource,
    reader: impl BufRead,
) -> Vec<ConversationEvent> {
    let mut events = Vec::new();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        events.extend(parse_line_events(source, &line));
    }

    events
}

fn parse_bounded_transcript_events(
    source: &SnapshotSource,
    reader: impl BufRead,
    cutoff: u64,
    source_modified_in_window: bool,
) -> Vec<ConversationEvent> {
    let mut retained_events = Vec::new();
    let mut has_timestamped_event = false;
    let mut has_in_window_timestamped_event = false;

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let line_timestamp = line_timestamp_ns(&line);
        let line_in_window = line_timestamp.is_some_and(|ts| ts >= cutoff);
        for event in parse_line_events(source, &line) {
            let Some(retained) =
                retain_bounded_event(event, cutoff, line_timestamp.is_some(), line_in_window)
            else {
                continue;
            };
            if retained.is_timestamped() {
                has_timestamped_event = true;
            }
            if retained.is_in_window() {
                has_in_window_timestamped_event = true;
            }
            retained_events.push(retained);
        }
    }

    retained_events
        .into_iter()
        .filter(|event| {
            event.is_in_window()
                || (event.is_source_fresh_untimestamped()
                    && source_modified_in_window
                    && !has_timestamped_event)
                || (event.is_structural_metadata() && has_in_window_timestamped_event)
        })
        .filter_map(BoundedEvent::into_event)
        .collect()
}

fn parse_line_events(source: &SnapshotSource, line: &str) -> Vec<ConversationEvent> {
    daemon8_parse::parse_conversation_line(&source.provider, line)
}

fn line_timestamp_ns(line: &str) -> Option<u64> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let timestamp = value.get("timestamp").and_then(|value| value.as_str())?;
    let ns = daemon8_parse::timestamp::normalize_timestamp_ns(timestamp)?;
    if ns < 0 {
        return None;
    }
    Some(ns as u64)
}

fn retain_bounded_event(
    event: ConversationEvent,
    cutoff: u64,
    line_has_timestamp: bool,
    line_in_window: bool,
) -> Option<BoundedEvent> {
    match event_timestamp_ns(&event) {
        Some(ts) if ts >= cutoff => Some(BoundedEvent::InWindow(event)),
        Some(_) => Some(BoundedEvent::OutOfWindow),
        None if is_structural_metadata(&event) && line_has_timestamp && line_in_window => {
            Some(BoundedEvent::StructuralMetadata(event))
        }
        None if is_structural_metadata(&event) && line_has_timestamp => None,
        None if is_structural_metadata(&event) => Some(BoundedEvent::StructuralMetadata(event)),
        None => Some(BoundedEvent::SourceFreshUntimestamped(event)),
    }
}

enum BoundedEvent {
    InWindow(ConversationEvent),
    OutOfWindow,
    StructuralMetadata(ConversationEvent),
    SourceFreshUntimestamped(ConversationEvent),
}

impl BoundedEvent {
    fn is_timestamped(&self) -> bool {
        matches!(self, Self::InWindow(_) | Self::OutOfWindow)
    }

    fn is_in_window(&self) -> bool {
        matches!(self, Self::InWindow(_))
    }

    fn is_structural_metadata(&self) -> bool {
        matches!(self, Self::StructuralMetadata(_))
    }

    fn is_source_fresh_untimestamped(&self) -> bool {
        matches!(
            self,
            Self::SourceFreshUntimestamped(_) | Self::StructuralMetadata(_)
        )
    }

    fn into_event(self) -> Option<ConversationEvent> {
        match self {
            Self::InWindow(event)
            | Self::StructuralMetadata(event)
            | Self::SourceFreshUntimestamped(event) => Some(event),
            Self::OutOfWindow => None,
        }
    }
}

fn is_structural_metadata(event: &ConversationEvent) -> bool {
    matches!(
        event,
        ConversationEvent::SessionMeta { .. }
            | ConversationEvent::TurnMeta { .. }
            | ConversationEvent::AgentSpawn { .. }
    )
}

fn event_timestamp_str(event: &ConversationEvent) -> Option<&str> {
    match event {
        ConversationEvent::ToolUse { timestamp, .. }
        | ConversationEvent::ToolResult { timestamp, .. }
        | ConversationEvent::UserPrompt { timestamp, .. }
        | ConversationEvent::FileChange { timestamp, .. }
        | ConversationEvent::AssistantMessage { timestamp, .. }
        | ConversationEvent::RawEvent { timestamp, .. } => timestamp.as_deref(),
        ConversationEvent::SessionMeta { .. }
        | ConversationEvent::TurnMeta { .. }
        | ConversationEvent::AgentSpawn { .. } => None,
    }
}

fn event_timestamp_ns(event: &ConversationEvent) -> Option<u64> {
    let ns = daemon8_parse::timestamp::normalize_timestamp_ns(event_timestamp_str(event)?)?;
    if ns < 0 {
        return None;
    }
    Some(ns as u64)
}

fn extract_time_range(events: &[ConversationEvent]) -> SnapshotTimeRange {
    let mut timestamps = events.iter().filter_map(event_timestamp_str);

    let from = timestamps.next().map(str::to_string);
    let to = timestamps
        .next_back()
        .map(str::to_string)
        .or_else(|| from.clone());

    SnapshotTimeRange { from, to }
}

pub fn condense_tool_input(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Read {path}")
        }
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("?");
            let truncated: String = cmd.chars().take(200).collect();
            format!("Bash: {truncated}")
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let first_line = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .and_then(|s| s.lines().next())
                .unwrap_or("...");
            format!("Edit {path}: {first_line}")
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let len = input
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.len())
                .unwrap_or(0);
            format!("Write {path} ({len} bytes)")
        }
        "Agent" => {
            let desc = input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let subagent = input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("Agent({subagent}): {desc}")
        }
        t if t.starts_with("mcp__") => {
            let keys = input_keys(input);
            format!("{t}({keys})")
        }
        other => {
            let keys = input_keys(input);
            format!("{other}({keys})")
        }
    }
}

fn input_keys(input: &serde_json::Value) -> String {
    input
        .as_object()
        .map(|obj| {
            obj.keys()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn build_user_messages_facet(events: &[ConversationEvent]) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;

    for event in events {
        if let ConversationEvent::UserPrompt { text, timestamp } = event {
            count += 1;
            let label = timestamp.as_deref().unwrap_or("prompt");
            out.push_str(&format!("## [{label}]\n\n{text}\n\n"));
        }
    }

    (out, count)
}

fn build_assistant_messages_facet(events: &[ConversationEvent]) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;

    for event in events {
        if let ConversationEvent::AssistantMessage { text, timestamp } = event {
            count += 1;
            let label = timestamp.as_deref().unwrap_or("response");
            out.push_str(&format!("## [{label}]\n\n{text}\n\n"));
        }
    }

    (out, count)
}

fn build_tool_activity_facet(events: &[ConversationEvent]) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;

    for event in events {
        if let ConversationEvent::ToolUse {
            tool,
            input,
            timestamp,
            ..
        } = event
        {
            count += 1;
            let label = timestamp.as_deref().unwrap_or("?");
            let condensed = condense_tool_input(tool, input);
            out.push_str(&format!("- [{label}] {condensed}\n"));
        }
    }

    (out, count)
}

fn build_file_changes_facet(events: &[ConversationEvent]) -> (String, usize) {
    let mut out = String::new();
    let mut seen = BTreeSet::new();

    for event in events {
        if let ConversationEvent::FileChange { path, .. } = event
            && seen.insert(path.clone())
        {
            out.push_str(&format!("- {path}\n"));
        }
    }

    let count = seen.len();
    (out, count)
}

fn build_log_activity_facet(events: &[ConversationEvent]) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;

    for event in events {
        match event {
            ConversationEvent::RawEvent {
                line_type,
                timestamp,
            } => {
                count += 1;
                let label = timestamp.as_deref().unwrap_or("?");
                out.push_str(&format!("- [{label}] {line_type}\n"));
            }
            ConversationEvent::TurnMeta {
                model,
                tokens,
                duration_ms,
                ..
            } => {
                count += 1;
                let model_str = model.as_deref().unwrap_or("unknown");
                let tokens_str = tokens.map(|t| format!("{t} tokens")).unwrap_or_default();
                let duration_str = duration_ms.map(|d| format!("{d}ms")).unwrap_or_default();
                let parts: Vec<&str> = [model_str, &tokens_str, &duration_str]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect();
                out.push_str(&format!("- turn: {}\n", parts.join(", ")));
            }
            _ => {}
        }
    }

    (out, count)
}

fn build_summary_facet(events: &[ConversationEvent]) -> (String, usize) {
    let mut out = String::new();
    let mut turns: Vec<(String, usize, usize)> = Vec::new();
    let mut current_prompt: Option<String> = None;
    let mut tool_count: usize = 0;
    let mut file_count: usize = 0;

    for event in events {
        match event {
            ConversationEvent::UserPrompt { text, .. } => {
                if let Some(prompt) = current_prompt.take() {
                    turns.push((prompt, tool_count, file_count));
                }
                let truncated: String = if text.chars().count() > 50 {
                    let mut t: String = text.chars().take(50).collect();
                    t.push_str("...");
                    t
                } else {
                    text.clone()
                };
                current_prompt = Some(truncated);
                tool_count = 0;
                file_count = 0;
            }
            ConversationEvent::ToolUse { .. } => {
                tool_count += 1;
            }
            ConversationEvent::FileChange { .. } => {
                file_count += 1;
            }
            _ => {}
        }
    }

    if let Some(prompt) = current_prompt.take() {
        turns.push((prompt, tool_count, file_count));
    }

    for (prompt, tools, files) in &turns {
        let tool_word = if *tools == 1 { "call" } else { "calls" };
        let file_word = if *files == 1 { "change" } else { "changes" };
        out.push_str(&format!(
            "- User: {prompt} -> {tools} tool {tool_word}, {files} file {file_word}\n"
        ));
    }

    let count = turns.len();
    (out, count)
}

pub fn cleanup_stale_snapshots(
    snapshots_dir: &Path,
    now: SystemTime,
    retention: Duration,
) -> std::io::Result<u64> {
    let session_dirs = match std::fs::read_dir(snapshots_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };

    let mut deleted = 0;
    for session in session_dirs {
        let session = session?;
        let session_path = session.path();
        if !session.file_type()?.is_dir() {
            continue;
        }

        if is_stale_snapshot_dir(&session_path, now, retention)? {
            std::fs::remove_dir_all(&session_path)?;
            deleted += 1;
            continue;
        }

        let mut session_empty = true;
        for run in std::fs::read_dir(&session_path)? {
            let run = run?;
            let run_path = run.path();
            session_empty = false;
            if run.file_type()?.is_dir() && is_stale_dir(&run_path, now, retention)? {
                std::fs::remove_dir_all(&run_path)?;
                deleted += 1;
            }
        }

        if !session_empty && session_path.read_dir()?.next().is_none() {
            std::fs::remove_dir(&session_path)?;
        }
    }

    Ok(deleted)
}

fn is_stale_snapshot_dir(
    path: &Path,
    now: SystemTime,
    retention: Duration,
) -> std::io::Result<bool> {
    if contains_facet_files(path)? && !contains_child_dirs(path)? {
        return is_stale_dir(path, now, retention);
    }
    Ok(false)
}

fn contains_child_dirs(path: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(path)? {
        if entry?.file_type()?.is_dir() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn contains_facet_files(path: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(|ext| ext.to_str()) == Some("md")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_stale_dir(path: &Path, now: SystemTime, retention: Duration) -> std::io::Result<bool> {
    let modified = path.metadata()?.modified()?;
    Ok(now.duration_since(modified).unwrap_or_default() > retention)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn condense_read_tool() {
        let result = condense_tool_input("Read", &json!({"file_path": "/src/main.rs"}));
        assert_eq!(result, "Read /src/main.rs");
    }

    #[test]
    fn condense_bash_tool() {
        let result = condense_tool_input("Bash", &json!({"command": "cargo test"}));
        assert_eq!(result, "Bash: cargo test");
    }

    #[test]
    fn condense_edit_tool() {
        let result = condense_tool_input(
            "Edit",
            &json!({
                "file_path": "/src/lib.rs",
                "old_string": "fn old_name()\nsecond line",
                "new_string": "fn new_name()"
            }),
        );
        assert_eq!(result, "Edit /src/lib.rs: fn old_name()");
    }

    #[test]
    fn condense_write_tool() {
        let result = condense_tool_input(
            "Write",
            &json!({"file_path": "/src/new.rs", "content": "hello world"}),
        );
        assert_eq!(result, "Write /src/new.rs (11 bytes)");
    }

    #[test]
    fn condense_agent_tool() {
        let result = condense_tool_input(
            "Agent",
            &json!({"description": "review code", "subagent_type": "code-review"}),
        );
        assert_eq!(result, "Agent(code-review): review code");
    }

    #[test]
    fn condense_unknown_tool() {
        let result = condense_tool_input("CustomTool", &json!({"x": 1, "y": 2}));
        assert_eq!(result, "CustomTool(x, y)");
    }

    #[test]
    fn build_user_messages_renders_with_timestamps() {
        let events = vec![
            ConversationEvent::UserPrompt {
                text: "fix the bug".into(),
                timestamp: Some("2026-05-22T10:00:00Z".into()),
            },
            ConversationEvent::UserPrompt {
                text: "now add tests".into(),
                timestamp: None,
            },
        ];
        let (content, count) = build_user_messages_facet(&events);
        assert_eq!(count, 2);
        assert!(content.contains("## [2026-05-22T10:00:00Z]"));
        assert!(content.contains("## [prompt]"));
        assert!(content.contains("fix the bug"));
        assert!(content.contains("now add tests"));
    }

    #[test]
    fn build_tool_activity_condenses_inputs() {
        let events = vec![
            ConversationEvent::ToolUse {
                tool: "Read".into(),
                input: json!({"file_path": "/src/main.rs"}),
                call_id: None,
                timestamp: Some("2026-05-22T10:00:01Z".into()),
            },
            ConversationEvent::ToolUse {
                tool: "Bash".into(),
                input: json!({"command": "cargo test"}),
                call_id: None,
                timestamp: Some("2026-05-22T10:00:02Z".into()),
            },
        ];
        let (content, count) = build_tool_activity_facet(&events);
        assert_eq!(count, 2);
        assert!(content.contains("Read /src/main.rs"));
        assert!(content.contains("Bash: cargo test"));
    }

    #[test]
    fn build_file_changes_deduplicates() {
        let events = vec![
            ConversationEvent::FileChange {
                path: "/src/main.rs".into(),
                timestamp: None,
            },
            ConversationEvent::FileChange {
                path: "/src/main.rs".into(),
                timestamp: None,
            },
            ConversationEvent::FileChange {
                path: "/src/lib.rs".into(),
                timestamp: None,
            },
        ];
        let (content, count) = build_file_changes_facet(&events);
        assert_eq!(count, 2);
        assert_eq!(content.matches("/src/main.rs").count(), 1);
    }

    #[test]
    fn build_summary_groups_by_turn() {
        let events = vec![
            ConversationEvent::UserPrompt {
                text: "fix the login bug".into(),
                timestamp: None,
            },
            ConversationEvent::ToolUse {
                tool: "Read".into(),
                input: json!({}),
                call_id: None,
                timestamp: None,
            },
            ConversationEvent::ToolUse {
                tool: "Edit".into(),
                input: json!({}),
                call_id: None,
                timestamp: None,
            },
            ConversationEvent::FileChange {
                path: "/src/auth.rs".into(),
                timestamp: None,
            },
            ConversationEvent::UserPrompt {
                text: "now add tests".into(),
                timestamp: None,
            },
            ConversationEvent::ToolUse {
                tool: "Write".into(),
                input: json!({}),
                call_id: None,
                timestamp: None,
            },
        ];
        let (content, count) = build_summary_facet(&events);
        assert_eq!(count, 2);
        assert!(content.contains("2 tool calls, 1 file change"));
        assert!(content.contains("1 tool call, 0 file changes"));
    }

    #[test]
    fn build_snapshot_writes_all_facets() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"/project"}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"fix the login bug"}]},"timestamp":"2026-05-22T10:00:00.000Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"Read","input":{"file_path":"/src/auth.rs"}}]},"timestamp":"2026-05-22T10:00:01.000Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I found the bug in the auth module."}]},"timestamp":"2026-05-22T10:00:05.000Z"}
"#,
        )
        .unwrap();

        let output_dir = tmp.path().join("snapshots").join("test-session");
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: Vec::new(),
            sources: vec![SnapshotSource {
                provider: "claude".into(),
                path: transcript,
            }],
            output_dir: output_dir.clone(),
        };

        let result = build_snapshot(&request).unwrap();
        assert_eq!(result.facets.len(), 6);
        assert!(!result.sources_read.is_empty());
        for facet_name in VALID_FACETS {
            let file_name = format!("{}.md", facet_name.replace('_', "-"));
            assert!(
                output_dir.join(&file_name).exists(),
                "missing facet file: {file_name}"
            );
        }
    }

    #[test]
    fn build_snapshot_subset_facets() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]},"timestamp":"2026-05-22T10:00:00.000Z"}
"#,
        )
        .unwrap();

        let output_dir = tmp.path().join("snapshots").join("subset");
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: vec!["user_messages".into(), "tool_activity".into()],
            sources: vec![SnapshotSource {
                provider: "claude".into(),
                path: transcript,
            }],
            output_dir: output_dir.clone(),
        };

        let result = build_snapshot(&request).unwrap();
        assert_eq!(result.facets.len(), 2);
        assert!(result.facets.contains_key("user_messages"));
        assert!(result.facets.contains_key("tool_activity"));
        assert!(!output_dir.join("summary.md").exists());
    }

    #[test]
    fn build_snapshot_rejects_existing_output_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]},"timestamp":"2026-05-22T10:00:00.000Z"}
"#,
        )
        .unwrap();

        let output_dir = tmp.path().join("snapshots").join("replace");
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: Vec::new(),
            sources: vec![SnapshotSource {
                provider: "claude".into(),
                path: transcript.clone(),
            }],
            output_dir: output_dir.clone(),
        };

        let _first = build_snapshot(&request).unwrap();
        let stale = output_dir.join("stale-marker.txt");
        std::fs::write(&stale, "leftover").unwrap();

        assert!(matches!(
            build_snapshot(&request),
            Err(SnapshotError::OutputDir(_))
        ));
        assert!(
            stale.exists(),
            "existing snapshot dir must not be deleted as normal build behavior"
        );
    }

    #[test]
    fn build_snapshot_empty_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("empty.jsonl");
        std::fs::write(&transcript, "").unwrap();

        let output_dir = tmp.path().join("snapshots").join("empty");
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: Vec::new(),
            sources: vec![SnapshotSource {
                provider: "claude".into(),
                path: transcript,
            }],
            output_dir: output_dir.clone(),
        };

        let result = build_snapshot(&request).unwrap();
        assert_eq!(result.facets.len(), 6);
        for facet in result.facets.values() {
            assert_eq!(facet.entry_count, 0);
        }
    }

    #[test]
    fn parse_transcript_claude() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("claude.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"/project"}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"fix the login bug"}]},"timestamp":"2026-05-22T10:00:00.000Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"Read","input":{"file_path":"/src/auth.rs"}}]},"timestamp":"2026-05-22T10:00:01.000Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I found the bug."}]},"timestamp":"2026-05-22T10:00:05.000Z"}
"#,
        )
        .unwrap();

        let source = SnapshotSource {
            provider: "claude".into(),
            path: transcript,
        };
        let events = parse_transcript_events(&source, None);

        let user_prompts = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::UserPrompt { .. }))
            .count();
        let tool_uses = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::ToolUse { .. }))
            .count();
        let assistant_msgs = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::AssistantMessage { .. }))
            .count();

        assert_eq!(user_prompts, 1);
        assert_eq!(tool_uses, 1);
        assert_eq!(assistant_msgs, 1);
    }

    #[test]
    fn parse_transcript_codex() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("codex.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"session_meta","payload":{"id":"sess_001","cwd":"/project","model":"o3"}}
{"type":"user_message","payload":{"text":"add error handling"}}
{"type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"fc_1","arguments":"{\"command\":\"cargo test\"}"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Tests pass now."}]}}
"#,
        )
        .unwrap();
        set_file_modified(&transcript, SystemTime::UNIX_EPOCH + Duration::from_secs(1));

        let source = SnapshotSource {
            provider: "codex".into(),
            path: transcript,
        };
        let events = parse_transcript_events(&source, None);

        let session_metas = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::SessionMeta { .. }))
            .count();
        let user_prompts = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::UserPrompt { .. }))
            .count();

        assert!(session_metas >= 1);
        assert_eq!(user_prompts, 1);
    }

    #[test]
    fn bounded_parse_drops_timestampless_content_without_in_window_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("codex.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"session_meta","payload":{"id":"sess_001","cwd":"/project","model":"o3"}}
{"type":"user_message","payload":{"text":"old context without timestamp"}}
"#,
        )
        .unwrap();
        let now = SystemTime::now();
        let cutoff = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .saturating_sub(24_u128 * 60 * 60 * 1_000_000_000) as u64;
        set_file_modified(&transcript, now - Duration::from_secs(48 * 60 * 60));

        let source = SnapshotSource {
            provider: "codex".into(),
            path: transcript,
        };
        let events = parse_transcript_events(&source, Some(cutoff));

        assert!(
            events.is_empty(),
            "bounded snapshots must not pull timestampless content from old transcripts"
        );
    }

    #[test]
    fn bounded_parse_keeps_timestampless_content_when_source_is_recent_and_untimestamped() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("codex.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"session_meta","payload":{"id":"sess_001","cwd":"/project","model":"o3"}}
{"type":"user_message","payload":{"text":"recent context without timestamp"}}
"#,
        )
        .unwrap();
        set_file_modified(&transcript, SystemTime::now());

        let source = SnapshotSource {
            provider: "codex".into(),
            path: transcript,
        };
        let events = parse_transcript_events(&source, Some(1_700_000_000_000_000_000));

        assert!(events.iter().any(|e| matches!(
            e,
            ConversationEvent::UserPrompt { text, .. } if text == "recent context without timestamp"
        )));
    }

    #[test]
    fn bounded_parse_keeps_metadata_when_source_has_in_window_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("claude.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"/project"}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"recent prompt"}]},"timestamp":"2024-01-01T00:00:00.000Z"}
"#,
        )
        .unwrap();

        let source = SnapshotSource {
            provider: "claude".into(),
            path: transcript,
        };
        let events = parse_transcript_events(&source, Some(1_700_000_000_000_000_000));

        assert!(
            events
                .iter()
                .any(|e| matches!(e, ConversationEvent::SessionMeta { .. }))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ConversationEvent::UserPrompt { text, .. } if text == "recent prompt"
        )));
    }

    #[test]
    fn bounded_parse_drops_structural_metadata_from_old_timestamped_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("claude.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"assistant","message":{"role":"assistant","model":"old-model","content":[]},"timestamp":"2023-01-01T00:00:00.000Z"}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"recent prompt"}]},"timestamp":"2024-01-01T00:00:00.000Z"}
"#,
        )
        .unwrap();

        let source = SnapshotSource {
            provider: "claude".into(),
            path: transcript,
        };
        let events = parse_transcript_events(&source, Some(1_700_000_000_000_000_000));

        assert!(events.iter().any(|e| matches!(
            e,
            ConversationEvent::UserPrompt { text, .. } if text == "recent prompt"
        )));
        assert!(!events.iter().any(|e| matches!(
            e,
            ConversationEvent::TurnMeta { model: Some(model), .. } if model == "old-model"
        )));
    }

    #[test]
    fn build_snapshot_no_sources() {
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: Vec::new(),
            sources: Vec::new(),
            output_dir: PathBuf::from("/tmp/should-not-exist"),
        };
        assert!(matches!(
            build_snapshot(&request),
            Err(SnapshotError::NoSources)
        ));
    }

    #[test]
    fn build_snapshot_missing_file_graceful() {
        let tmp = tempfile::tempdir().unwrap();
        let output_dir = tmp.path().join("snapshots").join("missing");
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: Vec::new(),
            sources: vec![SnapshotSource {
                provider: "claude".into(),
                path: PathBuf::from("/nonexistent/transcript.jsonl"),
            }],
            output_dir,
        };
        let result = build_snapshot(&request).unwrap();
        assert!(result.sources_read.is_empty());
    }

    #[test]
    fn cleanup_stale_snapshots_removes_old_run_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshots = tmp.path().join(".daemon8/snapshots");
        let run_dir = snapshots.join("sess1/run1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("summary.md"), "data").unwrap();
        set_dir_modified(
            &run_dir,
            SystemTime::now() - Duration::from_secs(25 * 60 * 60),
        );

        let deleted = cleanup_stale_snapshots(
            &snapshots,
            SystemTime::now(),
            Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();

        assert_eq!(deleted, 1);
        assert!(!run_dir.exists());
        assert!(!snapshots.join("sess1").exists());
    }

    #[test]
    fn cleanup_stale_snapshots_keeps_fresh_run_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshots = tmp.path().join(".daemon8/snapshots");
        let run_dir = snapshots.join("sess1/run1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("summary.md"), "data").unwrap();

        let deleted = cleanup_stale_snapshots(
            &snapshots,
            SystemTime::now(),
            Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();

        assert_eq!(deleted, 0);
        assert!(run_dir.exists());
    }

    #[test]
    fn cleanup_stale_snapshots_removes_stale_flat_session_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshots = tmp.path().join(".daemon8/snapshots");
        let flat_dir = snapshots.join("sess1");
        std::fs::create_dir_all(&flat_dir).unwrap();
        std::fs::write(flat_dir.join("summary.md"), "data").unwrap();
        set_dir_modified(
            &flat_dir,
            SystemTime::now() - Duration::from_secs(25 * 60 * 60),
        );

        let deleted = cleanup_stale_snapshots(
            &snapshots,
            SystemTime::now(),
            Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();

        assert_eq!(deleted, 1);
        assert!(!flat_dir.exists());
    }

    #[test]
    fn cleanup_stale_snapshots_leaves_daemon8_sibling_files() {
        let tmp = tempfile::tempdir().unwrap();
        let daemon8_dir = tmp.path().join(".daemon8");
        let snapshots = daemon8_dir.join("snapshots");
        let config = daemon8_dir.join("config.md");
        let other = daemon8_dir.join("notes.txt");
        std::fs::create_dir_all(&snapshots).unwrap();
        std::fs::write(&config, "config").unwrap();
        std::fs::write(&other, "notes").unwrap();

        let deleted = cleanup_stale_snapshots(
            &snapshots,
            SystemTime::now() + Duration::from_secs(25 * 60 * 60),
            Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();

        assert_eq!(deleted, 0);
        assert!(config.exists());
        assert!(other.exists());
    }

    #[test]
    fn cleanup_stale_snapshots_surfaces_unreadable_snapshot_path() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshots = tmp.path().join(".daemon8/snapshots");
        std::fs::create_dir_all(snapshots.parent().unwrap()).unwrap();
        std::fs::write(&snapshots, "not a directory").unwrap();

        let err = cleanup_stale_snapshots(
            &snapshots,
            SystemTime::now(),
            Duration::from_secs(24 * 60 * 60),
        )
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
    }

    #[test]
    fn duration_since_filters_old_events() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("session.jsonl");
        let cutoff_ns: u64 = 1_700_000_000_000_000_000; // ~2023-11-14
        let old_ts = "2023-01-01T00:00:00.000Z";
        let recent_ts = "2024-01-01T00:00:00.000Z";

        std::fs::write(
            &transcript,
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"old prompt"}}]}},"timestamp":"{old_ts}"}}
{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"recent prompt"}}]}},"timestamp":"{recent_ts}"}}
"#
            ),
        )
        .unwrap();

        let output_dir = tmp.path().join("snapshots").join("dur");
        let request = SnapshotRequest {
            since: SnapshotSince::Checkpoint {
                timestamp_ns: cutoff_ns,
            },
            facets: vec!["user_messages".into()],
            sources: vec![SnapshotSource {
                provider: "claude".into(),
                path: transcript,
            }],
            output_dir: output_dir.clone(),
        };

        let result = build_snapshot(&request).unwrap();
        let content = std::fs::read_to_string(output_dir.join("user-messages.md")).unwrap();
        assert!(content.contains("recent prompt"));
        assert!(!content.contains("old prompt"));
        assert_eq!(result.facets["user_messages"].entry_count, 1);
    }

    fn set_dir_modified(path: &Path, modified: SystemTime) {
        let times = std::fs::FileTimes::new().set_modified(modified);
        std::fs::File::options()
            .read(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    fn set_file_modified(path: &Path, modified: SystemTime) {
        let times = std::fs::FileTimes::new().set_modified(modified);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    #[test]
    fn multi_source_snapshot_sorts_by_timestamp() {
        let tmp = tempfile::tempdir().unwrap();

        let transcript_a = tmp.path().join("a.jsonl");
        std::fs::write(
            &transcript_a,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"second prompt"}]},"timestamp":"2026-05-22T10:00:02.000Z"}
"#,
        )
        .unwrap();

        let transcript_b = tmp.path().join("b.jsonl");
        std::fs::write(
            &transcript_b,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"first prompt"}]},"timestamp":"2026-05-22T10:00:01.000Z"}
"#,
        )
        .unwrap();

        let output_dir = tmp.path().join("snapshots").join("multi");
        let request = SnapshotRequest {
            since: SnapshotSince::ConversationStart,
            facets: vec!["user_messages".into()],
            sources: vec![
                SnapshotSource {
                    provider: "claude".into(),
                    path: transcript_a,
                },
                SnapshotSource {
                    provider: "claude".into(),
                    path: transcript_b,
                },
            ],
            output_dir: output_dir.clone(),
        };

        let result = build_snapshot(&request).unwrap();
        assert_eq!(result.sources_read.len(), 2);
        assert_eq!(result.facets["user_messages"].entry_count, 2);

        let content = std::fs::read_to_string(output_dir.join("user-messages.md")).unwrap();
        let first_pos = content.find("first prompt").unwrap();
        let second_pos = content.find("second prompt").unwrap();
        assert!(
            first_pos < second_pos,
            "events from multiple sources should be sorted by timestamp"
        );

        assert!(result.time_range.from.is_some());
        assert!(result.time_range.to.is_some());
    }

    #[test]
    fn condense_bash_utf8_safety() {
        let long_cjk: String = "aaaa"
            .chars()
            .cycle()
            .take(198)
            .chain("日本語".chars())
            .collect();
        let result = condense_tool_input("Bash", &json!({"command": long_cjk}));
        let content = result.strip_prefix("Bash: ").unwrap();
        assert_eq!(content.chars().count(), 200);
    }
}
