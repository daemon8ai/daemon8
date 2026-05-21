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

    if obj.get("$set").is_some() {
        return Vec::new();
    }

    let Some(line_type) = obj.get("type").and_then(|t| t.as_str()) else {
        return Vec::new();
    };

    let timestamp = obj
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);

    let ts = timestamp.as_deref();

    match line_type {
        "gemini" => parse_gemini_line(&obj, ts),
        "user" => parse_user_line(&obj, ts),
        _ => vec![ConversationEvent::RawEvent {
            line_type: line_type.to_string(),
            timestamp,
        }],
    }
}

fn parse_gemini_line(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let mut events = Vec::new();

    let model = obj.get("model").and_then(|v| v.as_str()).map(String::from);
    if model.is_some() {
        events.push(ConversationEvent::TurnMeta {
            model,
            git_branch: None,
            git_sha: None,
            tokens: None,
            duration_ms: None,
            permission_mode: None,
            cli_version: None,
        });
    }

    if let Some(text) = obj.get("content").and_then(|c| c.as_str())
        && !text.trim().is_empty()
    {
        events.push(ConversationEvent::AssistantMessage {
            text: text.to_string(),
            timestamp: timestamp.map(String::from),
        });
    }

    let Some(tool_calls) = obj.get("toolCalls").and_then(|t| t.as_array()) else {
        return events;
    };

    for tc in tool_calls {
        let Some(name) = tc.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let args = tc.get("args").cloned().unwrap_or_default();
        let call_id = tc.get("id").and_then(|v| v.as_str()).map(String::from);
        let tc_timestamp = tc
            .get("timestamp")
            .and_then(|t| t.as_str())
            .or(timestamp)
            .map(String::from);

        events.push(ConversationEvent::ToolUse {
            tool: name.to_string(),
            input: args,
            call_id: call_id.clone(),
            timestamp: tc_timestamp.clone(),
        });

        if let Some(results) = tc.get("result").and_then(|r| r.as_array()) {
            for result in results {
                if let Some(fr) = result.get("functionResponse") {
                    let output = fr.get("response").cloned().unwrap_or_default();
                    let result_id = fr
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| call_id.clone());
                    events.push(ConversationEvent::ToolResult {
                        call_id: result_id,
                        output,
                        exit_code: None,
                        timestamp: tc_timestamp.clone(),
                    });
                }
            }
        }
    }

    events
}

fn parse_user_line(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let Some(content) = obj.get("content").and_then(|c| c.as_array()) else {
        return Vec::new();
    };

    content
        .iter()
        .filter_map(|block| {
            let text = block.get("text").and_then(|t| t.as_str())?;
            if text.trim().is_empty() {
                return None;
            }
            Some(ConversationEvent::UserPrompt {
                text: text.to_string(),
                timestamp: timestamp.map(String::from),
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
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::SessionMeta { session_id, provider, .. }
            if session_id == "abc-123" && provider == "gemini"
        )));
    }

    #[test]
    fn parse_tool_call() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"","model":"gemini-3-flash","toolCalls":[{"id":"run_shell_1","name":"run_shell_command","args":{"command":"ls","description":"listing"},"result":[{"functionResponse":{"id":"run_shell_1","name":"run_shell_command","response":{"output":"file1\nfile2"}}}],"status":"success","timestamp":"2026-01-01T00:00:01Z"}]}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::ToolUse { tool, .. } if tool == "run_shell_command"
        )));
    }

    #[test]
    fn parse_tool_result_from_function_response() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"","model":"gemini-3-flash","toolCalls":[{"id":"tc1","name":"read_file","args":{"path":"a.rs"},"result":[{"functionResponse":{"id":"tc1","name":"read_file","response":{"output":"fn main(){}"}}}],"status":"success","timestamp":"2026-01-01T00:00:01Z"}]}"#;
        let events = parse_line(line);
        let results: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ConversationEvent::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 1);
        assert!(matches!(&results[0],
            ConversationEvent::ToolResult { call_id: Some(id), output, .. }
            if id == "tc1" && output["output"] == "fn main(){}"
        ));
    }

    #[test]
    fn parse_model_into_turn_meta() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"hello","model":"gemini-3-flash"}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { model: Some(m), .. } if m == "gemini-3-flash"
        )));
    }

    #[test]
    fn parse_multiple_tool_calls() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"","toolCalls":[{"id":"tc1","name":"read_file","args":{"path":"a.rs"},"status":"success","timestamp":"2026-01-01T00:00:01Z"},{"id":"tc2","name":"run_shell_command","args":{"command":"ls"},"status":"success","timestamp":"2026-01-01T00:00:02Z"}]}"#;
        let tool_uses: Vec<_> = parse_line(line)
            .into_iter()
            .filter(|e| matches!(e, ConversationEvent::ToolUse { .. }))
            .collect();
        assert_eq!(tool_uses.len(), 2);
    }

    #[test]
    fn parse_user_prompt() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"user","content":[{"text":"fix the bug"}]}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::UserPrompt { text, .. } if text == "fix the bug"
        )));
    }

    #[test]
    fn gemini_text_only_returns_turn_meta_and_message() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"hello world","model":"gemini-3-flash","thoughts":[]}"#;
        let events = parse_line(line);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ConversationEvent::TurnMeta { .. }))
        );
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::AssistantMessage { text, .. } if text == "hello world"
        )));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ConversationEvent::ToolUse { .. }))
        );
    }

    #[test]
    fn gemini_assistant_text_extracted() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"I found the issue in your code."}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::AssistantMessage { text, .. } if text == "I found the issue in your code."
        )));
    }

    #[test]
    fn gemini_mixed_text_and_tools() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"Let me check that file.","model":"gemini-3-flash","toolCalls":[{"id":"tc1","name":"read_file","args":{"path":"a.rs"},"status":"success","timestamp":"2026-01-01T00:00:01Z"}]}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::AssistantMessage { text, .. } if text == "Let me check that file."
        )));
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::ToolUse { tool, .. } if tool == "read_file"
        )));
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { model: Some(m), .. } if m == "gemini-3-flash"
        )));
    }

    #[test]
    fn user_message_returns_prompt() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"user","content":[{"text":"hello"}]}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::UserPrompt { text, .. } if text == "hello"
        )));
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line("").is_empty());
    }

    #[test]
    fn whitespace_only_content_skipped() {
        let line = r#"{"id":"msg1","timestamp":"2026-01-01T00:00:00Z","type":"gemini","content":"   \n  "}"#;
        let events = parse_line(line);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ConversationEvent::AssistantMessage { .. }))
        );
    }

    #[test]
    fn set_directive_returns_empty() {
        let line = r#"{"$set":{"lastUpdated":"2026-01-01T00:00:00Z"}}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn unknown_type_emits_raw() {
        let line = r#"{"type":"system","timestamp":"2026-01-01T00:00:00Z","content":"restarting"}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::RawEvent { line_type, .. } if line_type == "system"
        )));
    }
}
