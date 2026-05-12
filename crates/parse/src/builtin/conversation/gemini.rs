// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use super::ConversationEvent;

pub fn parse_line(line: &str) -> Vec<ConversationEvent> {
    let Some(obj) = serde_json::from_str::<serde_json::Value>(line).ok() else {
        return Vec::new();
    };

    if obj.get("type").is_none()
        && obj.get("startTime").is_some()
        && let Some(session_id) = obj.get("sessionId").and_then(|v| v.as_str())
    {
        return vec![ConversationEvent::SessionMeta {
            session_id: session_id.to_string(),
            cwd: None,
            provider: "gemini".into(),
            model: None,
        }];
    }

    let Some(line_type) = obj.get("type").and_then(|t| t.as_str()) else {
        return Vec::new();
    };
    if line_type != "gemini" {
        return Vec::new();
    }

    let msg_timestamp = obj
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);
    let Some(tool_calls) = obj.get("toolCalls").and_then(|t| t.as_array()) else {
        return Vec::new();
    };

    tool_calls
        .iter()
        .filter_map(|tc| {
            let name = tc.get("name")?.as_str()?.to_string();
            let args = tc.get("args").cloned().unwrap_or_default();
            let call_id = tc.get("id").and_then(|v| v.as_str()).map(String::from);
            let timestamp = tc
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(String::from)
                .or_else(|| msg_timestamp.clone());
            Some(ConversationEvent::ToolUse {
                tool: name,
                input: args,
                call_id,
                timestamp,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_meta() {
        let line = r#"{"sessionId":"abc-123","projectHash":"xyz","startTime":"2026-01-01T00:00:00Z","lastUpdated":"2026-01-01T00:00:00Z","kind":"main"}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::SessionMeta {
                session_id,
                provider,
                ..
            } => {
                assert_eq!(session_id, "abc-123");
                assert_eq!(provider, "gemini");
            }
            other => panic!("expected SessionMeta, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"","model":"gemini-3-flash","toolCalls":[{"id":"run_shell_1","name":"run_shell_command","args":{"command":"ls","description":"listing"},"result":[{"functionResponse":{"id":"run_shell_1","name":"run_shell_command","response":{"output":"file1\nfile2"}}}],"status":"success","timestamp":"2026-01-01T00:00:01Z"}]}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolUse {
                tool,
                input,
                call_id,
                timestamp,
            } => {
                assert_eq!(tool, "run_shell_command");
                assert_eq!(input["command"], "ls");
                assert_eq!(call_id.as_deref(), Some("run_shell_1"));
                assert_eq!(timestamp.as_deref(), Some("2026-01-01T00:00:01Z"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_multiple_tool_calls() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"","toolCalls":[{"id":"tc1","name":"read_file","args":{"path":"a.rs"},"status":"success","timestamp":"2026-01-01T00:00:01Z"},{"id":"tc2","name":"run_shell_command","args":{"command":"ls"},"status":"success","timestamp":"2026-01-01T00:00:02Z"}]}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 2);
        match &events[0] {
            ConversationEvent::ToolUse { tool, call_id, .. } => {
                assert_eq!(tool, "read_file");
                assert_eq!(call_id.as_deref(), Some("tc1"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match &events[1] {
            ConversationEvent::ToolUse { tool, call_id, .. } => {
                assert_eq!(tool, "run_shell_command");
                assert_eq!(call_id.as_deref(), Some("tc2"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn gemini_text_only_returns_empty() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"hello world","thoughts":[]}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn user_message_returns_empty() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"user","content":[{"text":"hello"}]}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line("").is_empty());
    }

    #[test]
    fn set_directive_returns_empty() {
        let line = r#"{"$set":{"lastUpdated":"2026-01-01T00:00:00Z"}}"#;
        assert!(parse_line(line).is_empty());
    }
}
