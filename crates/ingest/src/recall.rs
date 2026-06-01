// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

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
}
