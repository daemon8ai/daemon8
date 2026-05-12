// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use super::ConversationEvent;

pub fn parse_line(line: &str) -> Vec<ConversationEvent> {
    parse_single(line).into_iter().collect()
}

fn parse_single(line: &str) -> Option<ConversationEvent> {
    let obj: serde_json::Value = serde_json::from_str(line).ok()?;
    let line_type = obj.get("type")?.as_str()?;
    let timestamp = obj
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);
    let payload = obj.get("payload")?;

    match line_type {
        "session_meta" => {
            let session_id = payload.get("id")?.as_str()?.to_string();
            Some(ConversationEvent::SessionMeta {
                session_id,
                cwd: payload
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                provider: "codex".into(),
                model: None,
            })
        }
        "response_item" => {
            let payload_type = payload.get("type")?.as_str()?;
            match payload_type {
                "function_call" => {
                    let name = payload.get("name")?.as_str()?.to_string();
                    let input = payload
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();
                    Some(ConversationEvent::ToolUse {
                        tool: name,
                        input,
                        call_id: payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        timestamp,
                    })
                }
                "custom_tool_call" => {
                    let name = payload.get("name")?.as_str()?.to_string();
                    let input = payload.get("input").cloned().unwrap_or_default();
                    Some(ConversationEvent::ToolUse {
                        tool: name,
                        input,
                        call_id: payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        timestamp,
                    })
                }
                "function_call_output" | "custom_tool_call_output" => {
                    let output_raw = payload.get("output")?.clone();
                    let (output, exit_code) = if let Some(s) = output_raw.as_str() {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                            let code = parsed
                                .get("metadata")
                                .and_then(|m| m.get("exit_code"))
                                .and_then(|c| c.as_i64())
                                .map(|c| c as i32);
                            (parsed, code)
                        } else {
                            (output_raw, None)
                        }
                    } else {
                        (output_raw, None)
                    };
                    Some(ConversationEvent::ToolResult {
                        call_id: payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        output,
                        exit_code,
                        timestamp,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_meta() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"019ba665","timestamp":"2026-01-01T00:00:00Z","cwd":"/project"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::SessionMeta {
                session_id,
                cwd,
                provider,
                ..
            } => {
                assert_eq!(session_id, "019ba665");
                assert_eq!(cwd.as_deref(), Some("/project"));
                assert_eq!(provider, "codex");
            }
            other => panic!("expected SessionMeta, got {other:?}"),
        }
    }

    #[test]
    fn parse_function_call() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"ls\"}","call_id":"call_abc"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolUse {
                tool,
                input,
                call_id,
                ..
            } => {
                assert_eq!(tool, "shell_command");
                assert_eq!(input["command"], "ls");
                assert_eq!(call_id.as_deref(), Some("call_abc"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_custom_tool_call() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"patch content","call_id":"call_xyz","status":"completed"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolUse { tool, call_id, .. } => {
                assert_eq!(tool, "apply_patch");
                assert_eq!(call_id.as_deref(), Some("call_xyz"));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn custom_tool_call_preserves_non_string_input() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"edit","input":{"file":"a.rs","content":"fn main(){}"},"call_id":"call_1"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolUse { input, .. } => {
                assert_eq!(input["file"], "a.rs");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_function_call_output() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_abc","output":"Exit code: 0\nOutput: hello"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolResult {
                call_id, output, ..
            } => {
                assert_eq!(call_id.as_deref(), Some("call_abc"));
                assert!(output.as_str().unwrap().contains("hello"));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_custom_tool_call_output_with_exit_code() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_xyz","output":"{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ConversationEvent::ToolResult {
                exit_code, call_id, ..
            } => {
                assert_eq!(exit_code, &Some(0));
                assert_eq!(call_id.as_deref(), Some("call_xyz"));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn unknown_response_item_returns_empty() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":"hello"}}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line("").is_empty());
    }
}
