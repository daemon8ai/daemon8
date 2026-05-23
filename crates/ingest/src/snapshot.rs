// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use daemon8_parse::ConversationEvent;

pub const VALID_FACETS: &[&str] = &[
    "user_messages",
    "assistant_messages",
    "tool_activity",
    "file_changes",
    "log_activity",
    "summary",
];

#[derive(Debug, Clone)]
pub enum SnapshotSince {
    ConversationStart,
    Checkpoint { timestamp_ns: u64 },
    Duration { minutes: u64 },
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
    if request.sources.is_empty() {
        return Err(SnapshotError::NoSources);
    }

    if request.output_dir.exists() {
        std::fs::remove_dir_all(&request.output_dir).map_err(SnapshotError::OutputDir)?;
    }
    std::fs::create_dir_all(&request.output_dir).map_err(SnapshotError::OutputDir)?;

    let cutoff = resolve_since_cutoff_ns(&request.since);
    let mut all_events: Vec<ConversationEvent> = Vec::new();
    let mut sources_read: Vec<String> = Vec::new();

    for source in &request.sources {
        let events = parse_transcript_events(source, cutoff);
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

    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let parsed = daemon8_parse::parse_conversation_line(&source.provider, &line);
        for event in parsed {
            if let Some(cutoff) = since_cutoff_ns
                && let Some(ts) = event_timestamp_ns(&event)
                && ts < cutoff
            {
                continue;
            }
            events.push(event);
        }
    }

    events
}

fn resolve_since_cutoff_ns(since: &SnapshotSince) -> Option<u64> {
    match since {
        SnapshotSince::ConversationStart => None,
        SnapshotSince::Checkpoint { timestamp_ns } => Some(*timestamp_ns),
        SnapshotSince::Duration { minutes } => {
            let now_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            Some(now_ns.saturating_sub(minutes * 60 * 1_000_000_000))
        }
    }
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

pub fn cleanup_session_snapshots(snapshot_dir: &Path) -> std::io::Result<()> {
    if snapshot_dir.exists() {
        std::fs::remove_dir_all(snapshot_dir)?;
    }
    Ok(())
}

pub fn cleanup_all_snapshots(daemon8_dir: &Path) -> std::io::Result<()> {
    let snapshots_dir = daemon8_dir.join("snapshots");
    if snapshots_dir.exists() {
        std::fs::remove_dir_all(&snapshots_dir)?;
    }
    Ok(())
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
    fn build_snapshot_replaces_previous() {
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

        let _second = build_snapshot(&request).unwrap();
        assert!(
            !stale.exists(),
            "previous snapshot dir should be wiped clean"
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
    fn cleanup_session_snapshots_removes_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let snap_dir = tmp.path().join("snapshots").join("sess1");
        std::fs::create_dir_all(&snap_dir).unwrap();
        std::fs::write(snap_dir.join("test.md"), "data").unwrap();
        cleanup_session_snapshots(&snap_dir).unwrap();
        assert!(!snap_dir.exists());
    }

    #[test]
    fn cleanup_all_snapshots_removes_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let daemon8_dir = tmp.path().join(".daemon8");
        let snapshots = daemon8_dir.join("snapshots");
        std::fs::create_dir_all(snapshots.join("sess1")).unwrap();
        cleanup_all_snapshots(&daemon8_dir).unwrap();
        assert!(!snapshots.exists());
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
