// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use super::ConversationEvent;

pub fn parse_line(line: &str) -> Vec<ConversationEvent> {
    let Some(obj) = serde_json::from_str::<serde_json::Value>(line).ok() else {
        return Vec::new();
    };
    let Some(line_type) = obj.get("type").and_then(|t| t.as_str()) else {
        return Vec::new();
    };
    let timestamp = obj
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);

    match line_type {
        "permission-mode" => {
            let Some(session_id) = obj.get("sessionId").and_then(|v| v.as_str()) else {
                return Vec::new();
            };
            vec![ConversationEvent::SessionMeta {
                session_id: session_id.to_string(),
                cwd: obj.get("cwd").and_then(|v| v.as_str()).map(String::from),
                provider: "claude".into(),
                model: None,
            }]
        }
        "assistant" => {
            let Some(content) = obj
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                return Vec::new();
            };
            content
                .iter()
                .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .filter_map(|block| {
                    Some(ConversationEvent::ToolUse {
                        tool: block.get("name")?.as_str()?.to_string(),
                        input: block.get("input").cloned().unwrap_or_default(),
                        call_id: block.get("id").and_then(|v| v.as_str()).map(String::from),
                        timestamp: timestamp.clone(),
                    })
                })
                .collect()
        }
        "user" => {
            let Some(content) = obj
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                return Vec::new();
            };
            content
                .iter()
                .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                .map(|block| ConversationEvent::ToolResult {
                    call_id: block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    output: block.get("content").cloned().unwrap_or_default(),
                    exit_code: None,
                    timestamp: timestamp.clone(),
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_meta() {
        let line = r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"abc-123"}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::SessionMeta {
                session_id,
                provider,
                ..
            } => {
                assert_eq!(session_id, "abc-123");
                assert_eq!(provider, "claude");
            }
            other => panic!("expected SessionMeta, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Read","input":{"file_path":"/tmp/test.rs"}}]}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolUse {
                tool,
                input,
                call_id,
                timestamp,
            } => {
                assert_eq!(tool, "Read");
                assert_eq!(input["file_path"], "/tmp/test.rs");
                assert_eq!(call_id.as_deref(), Some("toolu_abc"));
                assert_eq!(timestamp.as_deref(), Some("2026-01-01T00:00:00Z"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_parallel_tool_calls() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.rs"}},{"type":"text","text":"reading files"},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"ls"}}]}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 2);
        match &events[0] {
            ConversationEvent::ToolUse { tool, call_id, .. } => {
                assert_eq!(tool, "Read");
                assert_eq!(call_id.as_deref(), Some("toolu_1"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match &events[1] {
            ConversationEvent::ToolUse { tool, call_id, .. } => {
                assert_eq!(tool, "Bash");
                assert_eq!(call_id.as_deref(), Some("toolu_2"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_result() {
        let line = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_abc","content":"file contents here"}]}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolResult {
                call_id, output, ..
            } => {
                assert_eq!(call_id.as_deref(), Some("toolu_abc"));
                assert_eq!(output.as_str(), Some("file contents here"));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_multiple_tool_results() {
        let line = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"result 1"},{"type":"tool_result","tool_use_id":"toolu_2","content":"result 2"}]}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn unknown_type_returns_empty() {
        let line = r#"{"type":"file-history-snapshot","messageId":"x","snapshot":{}}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json at all").is_empty());
        assert!(parse_line("").is_empty());
    }

    #[test]
    fn assistant_without_tool_use_returns_empty() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#;
        assert!(parse_line(line).is_empty());
    }
}
