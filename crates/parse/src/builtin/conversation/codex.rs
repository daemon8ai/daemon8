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

    let ts = timestamp.as_deref();

    match line_type {
        "session_meta" => parse_session_meta(&obj),
        "response_item" => parse_response_item(&obj, ts),
        "user_message" => parse_user_message(&obj, ts),
        _ => vec![ConversationEvent::RawEvent {
            line_type: line_type.to_string(),
            timestamp,
        }],
    }
}

fn parse_session_meta(obj: &serde_json::Value) -> Vec<ConversationEvent> {
    let Some(payload) = obj.get("payload") else {
        return Vec::new();
    };
    let Some(session_id) = payload.get("id").and_then(|v| v.as_str()) else {
        return Vec::new();
    };

    let mut events = vec![ConversationEvent::SessionMeta {
        session_id: session_id.to_string(),
        cwd: payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(String::from),
        provider: "codex".into(),
        model: None,
    }];

    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);
    if model.is_some() {
        events.push(ConversationEvent::TurnMeta {
            model,
            git_branch: None,
            git_sha: None,
            tokens: None,
            duration_ms: None,
            permission_mode: payload
                .get("approval_mode")
                .and_then(|v| v.as_str())
                .map(String::from),
            cli_version: payload
                .get("cli_version")
                .and_then(|v| v.as_str())
                .map(String::from),
        });
    }

    events
}

fn parse_response_item(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let Some(payload) = obj.get("payload") else {
        return Vec::new();
    };
    let Some(payload_type) = payload.get("type").and_then(|t| t.as_str()) else {
        return Vec::new();
    };

    match payload_type {
        "function_call" => {
            let Some(name) = payload.get("name").and_then(|n| n.as_str()) else {
                return Vec::new();
            };
            let input = payload
                .get("arguments")
                .and_then(|a| a.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            vec![ConversationEvent::ToolUse {
                tool: name.to_string(),
                input,
                call_id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                timestamp: timestamp.map(String::from),
            }]
        }
        "custom_tool_call" => {
            let Some(name) = payload.get("name").and_then(|n| n.as_str()) else {
                return Vec::new();
            };
            let input = payload.get("input").cloned().unwrap_or_default();
            vec![ConversationEvent::ToolUse {
                tool: name.to_string(),
                input,
                call_id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                timestamp: timestamp.map(String::from),
            }]
        }
        "function_call_output" | "custom_tool_call_output" => {
            let Some(output_raw) = payload.get("output").cloned() else {
                return Vec::new();
            };
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
            vec![ConversationEvent::ToolResult {
                call_id: payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                output,
                exit_code,
                timestamp: timestamp.map(String::from),
            }]
        }
        "message" => {
            let role = payload.get("role").and_then(|r| r.as_str());
            let content_blocks = payload.get("content").and_then(|c| c.as_array());
            match role {
                Some("user") => {
                    if let Some(text) = content_blocks.and_then(|arr| {
                        arr.iter()
                            .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("input_text"))
                            .and_then(|b| b.get("text").and_then(|t| t.as_str()))
                    }) && !text.trim().is_empty()
                    {
                        vec![ConversationEvent::UserPrompt {
                            text: text.to_string(),
                            timestamp: timestamp.map(String::from),
                        }]
                    } else {
                        Vec::new()
                    }
                }
                Some("assistant") => {
                    let Some(blocks) = content_blocks else {
                        return Vec::new();
                    };
                    blocks
                        .iter()
                        .filter_map(|b| {
                            if b.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                                let text = b.get("text").and_then(|t| t.as_str())?;
                                if text.trim().is_empty() {
                                    return None;
                                }
                                Some(ConversationEvent::AssistantMessage {
                                    text: text.to_string(),
                                    timestamp: timestamp.map(String::from),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                }
                _ => Vec::new(),
            }
        }
        _ => vec![ConversationEvent::RawEvent {
            line_type: format!("response_item.{payload_type}"),
            timestamp: timestamp.map(String::from),
        }],
    }
}

fn parse_user_message(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let text = obj
        .get("payload")
        .and_then(|p| p.get("text"))
        .or_else(|| obj.get("text"))
        .and_then(|v| v.as_str());

    match text {
        Some(t) if !t.trim().is_empty() => vec![ConversationEvent::UserPrompt {
            text: t.to_string(),
            timestamp: timestamp.map(String::from),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_meta() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"019ba665","timestamp":"2026-01-01T00:00:00Z","cwd":"/project"}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::SessionMeta { session_id, cwd: Some(c), provider, .. }
            if session_id == "019ba665" && c == "/project" && provider == "codex"
        )));
    }

    #[test]
    fn parse_session_meta_with_model() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"019ba665","cwd":"/project","model":"o3","approval_mode":"suggest","cli_version":"0.1.2"}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { model: Some(m), permission_mode: Some(p), cli_version: Some(v), .. }
            if m == "o3" && p == "suggest" && v == "0.1.2"
        )));
    }

    #[test]
    fn parse_function_call() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"ls\"}","call_id":"call_abc"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::ToolUse { tool, input, call_id: Some(id), .. }
            if tool == "shell_command" && input["command"] == "ls" && id == "call_abc"
        ));
    }

    #[test]
    fn parse_custom_tool_call() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"patch content","call_id":"call_xyz","status":"completed"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::ToolUse { tool, call_id: Some(id), .. }
            if tool == "apply_patch" && id == "call_xyz"
        ));
    }

    #[test]
    fn custom_tool_call_preserves_non_string_input() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"custom_tool_call","name":"edit","input":{"file":"a.rs","content":"fn main(){}"},"call_id":"call_1"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::ToolUse { input, .. } if input["file"] == "a.rs"
        ));
    }

    #[test]
    fn parse_function_call_output() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_abc","output":"Exit code: 0\nOutput: hello"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::ToolResult { call_id: Some(id), output, .. }
            if id == "call_abc" && output.as_str().unwrap().contains("hello")
        ));
    }

    #[test]
    fn parse_custom_tool_call_output_with_exit_code() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_xyz","output":"{\"output\":\"Success\",\"metadata\":{\"exit_code\":0}}"}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::ToolResult { exit_code: Some(0), call_id: Some(id), .. }
            if id == "call_xyz"
        ));
    }

    #[test]
    fn parse_user_message_from_message_payload() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the bug"}]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::UserPrompt { text, .. } if text == "fix the bug"
        )));
    }

    #[test]
    fn parse_user_message_type() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"user_message","payload":{"text":"hello codex"}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::UserPrompt { text, .. } if text == "hello codex"
        )));
    }

    #[test]
    fn parse_assistant_message() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Here is my analysis."}]}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::AssistantMessage { text, .. } if text == "Here is my analysis."
        ));
    }

    #[test]
    fn parse_assistant_message_multiple_blocks() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"First part."},{"type":"refusal","refusal":"no"},{"type":"output_text","text":"Second part."}]}}"#;
        let events = parse_line(line);
        let messages: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::AssistantMessage { .. }))
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(
            matches!(&messages[0], ConversationEvent::AssistantMessage { text, .. } if text == "First part.")
        );
        assert!(
            matches!(&messages[1], ConversationEvent::AssistantMessage { text, .. } if text == "Second part.")
        );
    }

    #[test]
    fn unknown_response_item_emits_raw() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"reasoning","content":"thinking..."}}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::RawEvent { line_type, .. } if line_type == "response_item.reasoning"
        ));
    }

    #[test]
    fn unknown_top_level_type_emits_raw() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_end","payload":{}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::RawEvent { line_type, .. } if line_type == "session_end"
        )));
    }

    #[test]
    fn whitespace_only_assistant_text_skipped() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"  \n  "}]}}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line("").is_empty());
    }
}
