// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::BTreeSet;
use std::path::PathBuf;

use daemon8_parse::ConversationEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Recall,
    Diagnostic,
    Hidden,
}

#[derive(Debug)]
pub struct RecallEntry {
    pub event: ConversationEvent,
    pub visibility: Visibility,
    pub timestamp_ns: Option<u64>,
}

#[derive(Debug)]
pub struct SourceMeta {
    pub provider: String,
    pub path: PathBuf,
    pub modified_at_ns: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RecallPolicy {
    pub max_bytes_per_entry: usize,
    pub max_entries_per_facet: usize,
    pub max_bytes_per_facet: usize,
    pub derive_file_changes: bool,
}

impl Default for RecallPolicy {
    fn default() -> Self {
        Self {
            max_bytes_per_entry: 8192,
            max_entries_per_facet: 200,
            max_bytes_per_facet: 131_072,
            derive_file_changes: true,
        }
    }
}

pub(crate) fn is_structural_metadata(event: &ConversationEvent) -> bool {
    matches!(
        event,
        ConversationEvent::SessionMeta { .. }
            | ConversationEvent::TurnMeta { .. }
            | ConversationEvent::AgentSpawn { .. }
    )
}

pub(crate) fn event_timestamp_str(event: &ConversationEvent) -> Option<&str> {
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

pub(crate) fn event_timestamp_ns(event: &ConversationEvent) -> Option<u64> {
    let ns = daemon8_parse::timestamp::normalize_timestamp_ns(event_timestamp_str(event)?)?;

    if ns < 0 {
        return None;
    }

    Some(ns as u64)
}

pub(crate) fn classify(event: &ConversationEvent) -> Visibility {
    match event {
        ConversationEvent::UserPrompt { text, .. } => {
            if is_instruction_block(text) {
                Visibility::Hidden
            } else {
                Visibility::Recall
            }
        }

        ConversationEvent::AssistantMessage { .. }
        | ConversationEvent::ToolUse { .. }
        | ConversationEvent::FileChange { .. } => Visibility::Recall,

        ConversationEvent::ToolResult { .. }
        | ConversationEvent::TurnMeta { .. }
        | ConversationEvent::AgentSpawn { .. }
        | ConversationEvent::RawEvent { .. } => Visibility::Diagnostic,

        ConversationEvent::SessionMeta { .. } => Visibility::Hidden,
    }
}

fn is_instruction_block(text: &str) -> bool {
    let t = text.trim();

    if t.starts_with("<system-reminder>") {
        return true;
    }

    t.len() > 4000
        && (t.contains("<system-reminder>")
            || t.contains("# System")
            || (t.contains("Contents of") && t.contains("CLAUDE.md")))
}

pub type TimestampedEvents = Vec<(ConversationEvent, Option<u64>)>;

pub fn recall_pipeline(
    sources: Vec<(SourceMeta, TimestampedEvents)>,
    cutoff: &crate::snapshot::SnapshotCutoff,
    policy: &RecallPolicy,
) -> Vec<RecallEntry> {
    let mut entries = Vec::new();

    for (source, events) in sources {
        match cutoff.ns {
            None => {
                for (event, _line_ts) in events {
                    let vis = classify(&event);

                    if vis == Visibility::Hidden {
                        continue;
                    }

                    let ts = event_timestamp_ns(&event);

                    entries.push(RecallEntry {
                        event,
                        visibility: vis,
                        timestamp_ns: ts,
                    });
                }
            }

            Some(cutoff_ns) => {
                let has_timestamped = events
                    .iter()
                    .any(|(ev, _)| event_timestamp_ns(ev).is_some());

                let has_in_window_timestamped = events
                    .iter()
                    .any(|(ev, _)| event_timestamp_ns(ev).is_some_and(|ts| ts >= cutoff_ns));

                let source_modified_in_window =
                    source.modified_at_ns.is_some_and(|m| m >= cutoff_ns);

                for (event, line_ts) in events {
                    let vis = classify(&event);

                    if vis == Visibility::Hidden {
                        continue;
                    }

                    let event_ts = event_timestamp_ns(&event);

                    if !is_in_scope(
                        &event,
                        event_ts,
                        cutoff_ns,
                        line_ts,
                        source_modified_in_window,
                        has_timestamped,
                        has_in_window_timestamped,
                    ) {
                        continue;
                    }

                    entries.push(RecallEntry {
                        event,
                        visibility: vis,
                        timestamp_ns: event_ts,
                    });
                }
            }
        }
    }

    if policy.derive_file_changes {
        derive_file_mutations(&mut entries);
    }

    entries.sort_by_key(|e| e.timestamp_ns.unwrap_or(0));

    truncate_text(&mut entries, policy.max_bytes_per_entry);

    entries
}

fn is_in_scope(
    event: &ConversationEvent,
    event_ts: Option<u64>,
    cutoff_ns: u64,
    line_ts: Option<u64>,
    source_modified_in_window: bool,
    has_timestamped: bool,
    has_in_window_timestamped: bool,
) -> bool {
    match event_ts {
        Some(ts) if ts >= cutoff_ns => true,
        Some(_) => false,
        None if is_structural_metadata(event) => {
            if line_ts.is_some_and(|lt| lt < cutoff_ns) {
                return false;
            }

            has_in_window_timestamped
        }
        None => source_modified_in_window && !has_timestamped,
    }
}

fn derive_file_mutations(entries: &mut Vec<RecallEntry>) {
    let existing: BTreeSet<String> = entries
        .iter()
        .filter_map(|e| match &e.event {
            ConversationEvent::FileChange { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();

    let mut derived = Vec::new();

    for entry in entries.iter() {
        let (tool, input, ts) = match &entry.event {
            ConversationEvent::ToolUse {
                tool,
                input,
                timestamp,
                ..
            } => (tool.as_str(), input, timestamp.clone()),
            _ => continue,
        };

        let Some(path) = extract_mutated_path(tool, input) else {
            continue;
        };

        if existing.contains(path) {
            continue;
        }

        derived.push(RecallEntry {
            event: ConversationEvent::FileChange {
                path: path.to_owned(),
                timestamp: ts,
            },
            visibility: Visibility::Recall,
            timestamp_ns: entry.timestamp_ns,
        });
    }

    entries.extend(derived);
}

fn extract_mutated_path<'a>(tool: &str, input: &'a serde_json::Value) -> Option<&'a str> {
    match tool {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            input.get("file_path").and_then(|v| v.as_str())
        }
        "apply_patch" | "apply_diff" => input.get("path").and_then(|v| v.as_str()),
        _ => None,
    }
}

fn truncate_text(entries: &mut [RecallEntry], max_bytes: usize) {
    for entry in entries.iter_mut() {
        let text = match &mut entry.event {
            ConversationEvent::UserPrompt { text, .. }
            | ConversationEvent::AssistantMessage { text, .. } => text,
            _ => continue,
        };

        if text.len() <= max_bytes {
            continue;
        }

        let mut end = max_bytes;

        while !text.is_char_boundary(end) {
            end -= 1;
        }

        text.truncate(end);
        text.push_str("\n\n[truncated]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_user_prompt_is_recall() {
        let event = ConversationEvent::UserPrompt {
            text: "fix the login bug".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        assert_eq!(classify(&event), Visibility::Recall);
    }

    #[test]
    fn classify_instruction_block_is_hidden() {
        let event = ConversationEvent::UserPrompt {
            text: "<system-reminder>\nYou are Claude...\n</system-reminder>".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        assert_eq!(classify(&event), Visibility::Hidden);
    }

    #[test]
    fn classify_long_instruction_with_markers_is_hidden() {
        let mut text = "Here is your instruction context.\n".to_string();
        text.push_str(&"x".repeat(4000));
        text.push_str("\nContents of /home/user/.claude/CLAUDE.md\n");

        let event = ConversationEvent::UserPrompt {
            text,
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Hidden);
    }

    #[test]
    fn classify_long_normal_text_is_recall() {
        let text = "a]".repeat(3000);

        let event = ConversationEvent::UserPrompt {
            text,
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Recall);
    }

    #[test]
    fn classify_assistant_message_is_recall() {
        let event = ConversationEvent::AssistantMessage {
            text: "I'll fix that for you.".into(),
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Recall);
    }

    #[test]
    fn classify_tool_use_is_recall() {
        let event = ConversationEvent::ToolUse {
            tool: "Read".into(),
            input: serde_json::json!({"file_path": "/src/main.rs"}),
            call_id: None,
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Recall);
    }

    #[test]
    fn classify_tool_result_is_diagnostic() {
        let event = ConversationEvent::ToolResult {
            call_id: None,
            output: serde_json::json!("file contents here"),
            exit_code: None,
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Diagnostic);
    }

    #[test]
    fn classify_session_meta_is_hidden() {
        let event = ConversationEvent::SessionMeta {
            session_id: "abc-123".into(),
            cwd: Some("/project".into()),
            provider: "claude".into(),
            model: None,
        };

        assert_eq!(classify(&event), Visibility::Hidden);
    }

    #[test]
    fn classify_turn_meta_is_diagnostic() {
        let event = ConversationEvent::TurnMeta {
            model: Some("claude-opus-4-6".into()),
            git_branch: None,
            git_sha: None,
            tokens: Some(1500),
            duration_ms: Some(3200),
            permission_mode: None,
            cli_version: None,
        };

        assert_eq!(classify(&event), Visibility::Diagnostic);
    }

    #[test]
    fn classify_file_change_is_recall() {
        let event = ConversationEvent::FileChange {
            path: "/src/auth.rs".into(),
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Recall);
    }

    #[test]
    fn classify_raw_event_is_diagnostic() {
        let event = ConversationEvent::RawEvent {
            line_type: "unknown_type".into(),
            timestamp: None,
        };

        assert_eq!(classify(&event), Visibility::Diagnostic);
    }

    #[test]
    fn recall_policy_default_values() {
        let policy = RecallPolicy::default();

        assert_eq!(policy.max_bytes_per_entry, 8192);
        assert_eq!(policy.max_entries_per_facet, 200);
        assert_eq!(policy.max_bytes_per_facet, 131_072);
        assert!(policy.derive_file_changes);
    }

    #[test]
    fn event_timestamp_ns_some_for_timestamped() {
        let event = ConversationEvent::UserPrompt {
            text: "hello".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        assert!(event_timestamp_ns(&event).is_some());
    }

    #[test]
    fn event_timestamp_ns_none_for_structural() {
        let event = ConversationEvent::SessionMeta {
            session_id: "s1".into(),
            cwd: None,
            provider: "claude".into(),
            model: None,
        };

        assert!(event_timestamp_ns(&event).is_none());
    }

    #[test]
    fn is_structural_metadata_variants() {
        assert!(is_structural_metadata(&ConversationEvent::SessionMeta {
            session_id: "s1".into(),
            cwd: None,
            provider: "claude".into(),
            model: None,
        }));

        assert!(is_structural_metadata(&ConversationEvent::TurnMeta {
            model: None,
            git_branch: None,
            git_sha: None,
            tokens: None,
            duration_ms: None,
            permission_mode: None,
            cli_version: None,
        }));

        assert!(is_structural_metadata(&ConversationEvent::AgentSpawn {
            parent_session: "p".into(),
            child_session: "c".into(),
            role: None,
            nickname: None,
            status: None,
        }));

        assert!(!is_structural_metadata(&ConversationEvent::UserPrompt {
            text: "hi".into(),
            timestamp: None,
        }));

        assert!(!is_structural_metadata(&ConversationEvent::ToolUse {
            tool: "Read".into(),
            input: serde_json::Value::Null,
            call_id: None,
            timestamp: None,
        }));
    }

    // --- Pipeline scoping tests ---

    fn cutoff_with_ns(ns: u64) -> crate::snapshot::SnapshotCutoff {
        crate::snapshot::SnapshotCutoff {
            ns: Some(ns),
            ms: Some(ns / 1_000_000),
        }
    }

    fn cutoff_none() -> crate::snapshot::SnapshotCutoff {
        crate::snapshot::SnapshotCutoff { ns: None, ms: None }
    }

    fn source_meta(modified_ns: Option<u64>) -> SourceMeta {
        SourceMeta {
            provider: "claude".into(),
            path: PathBuf::from("/tmp/test.jsonl"),
            modified_at_ns: modified_ns,
        }
    }

    #[test]
    fn scope_drops_old_timestamped_events() {
        let event = ConversationEvent::UserPrompt {
            text: "old message".into(),
            timestamp: Some("2023-01-01T00:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();
        let sources = vec![(source_meta(None), vec![(event, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert!(result.is_empty());
    }

    #[test]
    fn scope_keeps_in_window_events() {
        let event = ConversationEvent::UserPrompt {
            text: "recent message".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();
        let sources = vec![(source_meta(None), vec![(event, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].visibility, Visibility::Recall);
    }

    #[test]
    fn scope_drops_timestampless_from_old_source() {
        let event = ConversationEvent::UserPrompt {
            text: "no timestamp".into(),
            timestamp: None,
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();

        let old_modified = Some(1_600_000_000_000_000_000);
        let sources = vec![(source_meta(old_modified), vec![(event, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert!(result.is_empty());
    }

    #[test]
    fn scope_keeps_timestampless_from_fresh_source() {
        let event = ConversationEvent::UserPrompt {
            text: "no timestamp but fresh source".into(),
            timestamp: None,
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();

        let fresh_modified = Some(1_800_000_000_000_000_000);
        let sources = vec![(source_meta(fresh_modified), vec![(event, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 1);
    }

    #[test]
    fn scope_no_cutoff_keeps_all() {
        let events = vec![
            (
                ConversationEvent::UserPrompt {
                    text: "first".into(),
                    timestamp: Some("2023-01-01T00:00:00Z".into()),
                },
                None,
            ),
            (
                ConversationEvent::AssistantMessage {
                    text: "second".into(),
                    timestamp: None,
                },
                None,
            ),
        ];

        let cutoff = cutoff_none();
        let policy = RecallPolicy {
            derive_file_changes: false,
            ..RecallPolicy::default()
        };
        let sources = vec![(source_meta(None), events)];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn scope_drops_structural_from_old_timestamped_line() {
        let event = ConversationEvent::TurnMeta {
            model: Some("claude-opus-4-6".into()),
            git_branch: None,
            git_sha: None,
            tokens: None,
            duration_ms: None,
            permission_mode: None,
            cli_version: None,
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy {
            derive_file_changes: false,
            ..RecallPolicy::default()
        };

        let old_line_ts = Some(1_600_000_000_000_000_000);
        let sources = vec![(source_meta(None), vec![(event, old_line_ts)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert!(result.is_empty());
    }

    // --- Pipeline composition tests ---

    #[test]
    fn pipeline_filters_hidden_events() {
        let session_meta = ConversationEvent::SessionMeta {
            session_id: "s1".into(),
            cwd: None,
            provider: "claude".into(),
            model: None,
        };

        let instruction = ConversationEvent::UserPrompt {
            text: "<system-reminder>\nlots of instructions\n</system-reminder>".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let normal = ConversationEvent::UserPrompt {
            text: "fix the bug".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy {
            derive_file_changes: false,
            ..RecallPolicy::default()
        };
        let sources = vec![(
            source_meta(None),
            vec![(session_meta, None), (instruction, None), (normal, None)],
        )];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 1);

        match &result[0].event {
            ConversationEvent::UserPrompt { text, .. } => assert_eq!(text, "fix the bug"),
            other => panic!("expected UserPrompt, got {:?}", other),
        }
    }

    #[test]
    fn pipeline_sorts_by_timestamp() {
        let later = ConversationEvent::UserPrompt {
            text: "later".into(),
            timestamp: Some("2026-05-31T12:00:00Z".into()),
        };

        let earlier = ConversationEvent::AssistantMessage {
            text: "earlier".into(),
            timestamp: Some("2026-05-31T08:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy {
            derive_file_changes: false,
            ..RecallPolicy::default()
        };

        let sources = vec![
            (source_meta(None), vec![(later, None)]),
            (source_meta(None), vec![(earlier, None)]),
        ];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 2);
        assert!(result[0].timestamp_ns.unwrap() < result[1].timestamp_ns.unwrap());
    }

    // --- File mutation derivation tests ---

    #[test]
    fn derive_from_edit_tool() {
        let tool_use = ConversationEvent::ToolUse {
            tool: "Edit".into(),
            input: serde_json::json!({"file_path": "/src/lib.rs", "old_string": "a", "new_string": "b"}),
            call_id: None,
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();
        let sources = vec![(source_meta(None), vec![(tool_use, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        let file_changes: Vec<_> = result
            .iter()
            .filter(|e| matches!(&e.event, ConversationEvent::FileChange { .. }))
            .collect();

        assert_eq!(file_changes.len(), 1);

        match &file_changes[0].event {
            ConversationEvent::FileChange { path, .. } => assert_eq!(path, "/src/lib.rs"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn derive_dedup_against_existing() {
        let file_change = ConversationEvent::FileChange {
            path: "/src/lib.rs".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let tool_use = ConversationEvent::ToolUse {
            tool: "Edit".into(),
            input: serde_json::json!({"file_path": "/src/lib.rs"}),
            call_id: None,
            timestamp: Some("2026-05-31T10:01:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();
        let sources = vec![(
            source_meta(None),
            vec![(file_change, None), (tool_use, None)],
        )];

        let result = recall_pipeline(sources, &cutoff, &policy);

        let file_changes: Vec<_> = result
            .iter()
            .filter(|e| matches!(&e.event, ConversationEvent::FileChange { .. }))
            .collect();

        assert_eq!(file_changes.len(), 1);
    }

    #[test]
    fn derive_skipped_when_disabled() {
        let tool_use = ConversationEvent::ToolUse {
            tool: "Write".into(),
            input: serde_json::json!({"file_path": "/src/new.rs", "content": "fn main() {}"}),
            call_id: None,
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy {
            derive_file_changes: false,
            ..RecallPolicy::default()
        };
        let sources = vec![(source_meta(None), vec![(tool_use, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        let file_changes: Vec<_> = result
            .iter()
            .filter(|e| matches!(&e.event, ConversationEvent::FileChange { .. }))
            .collect();

        assert!(file_changes.is_empty());
    }

    #[test]
    fn extract_mutated_path_unknown_returns_none() {
        let input = serde_json::json!({"file_path": "/src/main.rs"});

        assert!(extract_mutated_path("Read", &input).is_none());
    }

    // --- Truncation tests ---

    #[test]
    fn truncate_long_prompt() {
        let long_text = "a".repeat(20_000);

        let event = ConversationEvent::UserPrompt {
            text: long_text,
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();
        let sources = vec![(source_meta(None), vec![(event, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 1);

        match &result[0].event {
            ConversationEvent::UserPrompt { text, .. } => {
                assert!(text.ends_with("\n\n[truncated]"));
                assert!(text.len() < 20_000);
                assert!(text.len() <= 8192 + "\n\n[truncated]".len());
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn truncate_preserves_short_text() {
        let event = ConversationEvent::UserPrompt {
            text: "short message".into(),
            timestamp: Some("2026-05-31T10:00:00Z".into()),
        };

        let cutoff = cutoff_with_ns(1_700_000_000_000_000_000);
        let policy = RecallPolicy::default();
        let sources = vec![(source_meta(None), vec![(event, None)])];

        let result = recall_pipeline(sources, &cutoff, &policy);

        assert_eq!(result.len(), 1);

        match &result[0].event {
            ConversationEvent::UserPrompt { text, .. } => assert_eq!(text, "short message"),
            _ => unreachable!(),
        }
    }
}
