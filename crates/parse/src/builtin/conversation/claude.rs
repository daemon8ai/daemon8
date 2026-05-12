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
        "permission-mode" => parse_permission_mode(&obj),
        "assistant" => parse_assistant(&obj, ts),
        "user" => parse_user(&obj, ts),
        "system" => parse_system(&obj, ts),
        "file-history-snapshot" => parse_file_history(&obj),
        "attachment" => parse_attachment(&obj, ts),
        _ => vec![ConversationEvent::RawEvent {
            line_type: line_type.to_string(),
            timestamp,
        }],
    }
}

fn parse_permission_mode(obj: &serde_json::Value) -> Vec<ConversationEvent> {
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

fn parse_assistant(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let mut events = Vec::new();

    let message = obj.get("message");
    let model = message
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let git_branch = obj
        .get("gitBranch")
        .and_then(|v| v.as_str())
        .map(String::from);
    let cli_version = obj
        .get("version")
        .and_then(|v| v.as_str())
        .map(String::from);

    if model.is_some() || git_branch.is_some() || cli_version.is_some() {
        events.push(ConversationEvent::TurnMeta {
            model,
            git_branch,
            git_sha: None,
            tokens: None,
            duration_ms: None,
            permission_mode: None,
            cli_version,
        });
    }

    let Some(content) = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return events;
    };

    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && let Some(event) = parse_tool_use_block(block, timestamp)
        {
            events.push(event);
        }
    }

    events
}

fn parse_user(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let mut events = Vec::new();

    let git_branch = obj
        .get("gitBranch")
        .and_then(|v| v.as_str())
        .map(String::from);
    let cli_version = obj
        .get("version")
        .and_then(|v| v.as_str())
        .map(String::from);
    let permission_mode = obj
        .get("permissionMode")
        .and_then(|v| v.as_str())
        .map(String::from);

    if git_branch.is_some() || cli_version.is_some() || permission_mode.is_some() {
        events.push(ConversationEvent::TurnMeta {
            model: None,
            git_branch,
            git_sha: None,
            tokens: None,
            duration_ms: None,
            permission_mode,
            cli_version,
        });
    }

    let Some(content) = obj
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return events;
    };

    for block in content {
        let block_type = block.get("type").and_then(|t| t.as_str());
        match block_type {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|t| t.as_str())
                    && !text.is_empty()
                {
                    events.push(ConversationEvent::UserPrompt {
                        text: text.to_string(),
                        timestamp: timestamp.map(String::from),
                    });
                }
            }
            Some("tool_result") => {
                events.push(ConversationEvent::ToolResult {
                    call_id: block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    output: block.get("content").cloned().unwrap_or_default(),
                    exit_code: None,
                    timestamp: timestamp.map(String::from),
                });
            }
            _ => {}
        }
    }

    events
}

fn parse_system(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let subtype = obj.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
    match subtype {
        "turn_duration" => {
            let duration_ms = obj.get("durationMs").and_then(|v| v.as_u64());
            vec![ConversationEvent::TurnMeta {
                model: None,
                git_branch: None,
                git_sha: None,
                tokens: None,
                duration_ms,
                permission_mode: None,
                cli_version: None,
            }]
        }
        _ => vec![ConversationEvent::RawEvent {
            line_type: format!("system.{subtype}"),
            timestamp: timestamp.map(String::from),
        }],
    }
}

fn parse_file_history(obj: &serde_json::Value) -> Vec<ConversationEvent> {
    let Some(snapshot) = obj.get("snapshot") else {
        return Vec::new();
    };
    let Some(backups) = snapshot
        .get("trackedFileBackups")
        .and_then(|v| v.as_object())
    else {
        return Vec::new();
    };
    let ts = snapshot
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(String::from);

    backups
        .keys()
        .map(|path| ConversationEvent::FileChange {
            path: path.clone(),
            timestamp: ts.clone(),
        })
        .collect()
}

fn parse_attachment(obj: &serde_json::Value, timestamp: Option<&str>) -> Vec<ConversationEvent> {
    let Some(attachment) = obj.get("attachment") else {
        return Vec::new();
    };
    let subtype = attachment
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match subtype {
        "edited_text_file" => {
            let path = attachment
                .get("filePath")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if path.is_empty() {
                return Vec::new();
            }
            vec![ConversationEvent::FileChange {
                path,
                timestamp: timestamp.map(String::from),
            }]
        }
        _ => vec![ConversationEvent::RawEvent {
            line_type: format!("attachment.{subtype}"),
            timestamp: timestamp.map(String::from),
        }],
    }
}

fn parse_tool_use_block(
    block: &serde_json::Value,
    timestamp: Option<&str>,
) -> Option<ConversationEvent> {
    Some(ConversationEvent::ToolUse {
        tool: block.get("name")?.as_str()?.to_string(),
        input: block.get("input").cloned().unwrap_or_default(),
        call_id: block.get("id").and_then(|v| v.as_str()).map(String::from),
        timestamp: timestamp.map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_meta() {
        let line = r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"abc-123"}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::SessionMeta { session_id, provider, .. }
            if session_id == "abc-123" && provider == "claude"
        )));
    }

    #[test]
    fn parse_tool_use() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"tool_use","id":"toolu_abc","name":"Read","input":{"file_path":"/tmp/test.rs"}}]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::ToolUse { tool, .. } if tool == "Read"
        )));
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { model: Some(m), .. } if m == "claude-opus-4-6"
        )));
    }

    #[test]
    fn parse_parallel_tool_calls() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.rs"}},{"type":"text","text":"reading files"},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"ls"}}]}}"#;
        let tool_uses: Vec<_> = parse_line(line)
            .into_iter()
            .filter(|e| matches!(e, ConversationEvent::ToolUse { .. }))
            .collect();
        assert_eq!(tool_uses.len(), 2);
    }

    #[test]
    fn parse_tool_result() {
        let line = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_abc","content":"file contents here"}]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::ToolResult { call_id: Some(id), .. } if id == "toolu_abc"
        )));
    }

    #[test]
    fn parse_multiple_tool_results() {
        let line = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"result 1"},{"type":"tool_result","tool_use_id":"toolu_2","content":"result 2"}]}}"#;
        let results: Vec<_> = parse_line(line)
            .into_iter()
            .filter(|e| matches!(e, ConversationEvent::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn parse_user_prompt() {
        let line = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"text","text":"fix the bug in main.rs"}]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::UserPrompt { text, .. } if text == "fix the bug in main.rs"
        )));
    }

    #[test]
    fn parse_assistant_turn_meta() {
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","version":"2.1.139","gitBranch":"main","message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"hello"}]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { model: Some(m), git_branch: Some(b), cli_version: Some(v), .. }
            if m == "claude-opus-4-6" && b == "main" && v == "2.1.139"
        )));
    }

    #[test]
    fn parse_user_turn_meta() {
        let line = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","version":"2.1.139","gitBranch":"feat","permissionMode":"auto","message":{"role":"user","content":[]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { permission_mode: Some(p), git_branch: Some(b), .. }
            if p == "auto" && b == "feat"
        )));
    }

    #[test]
    fn parse_turn_duration() {
        let line = r#"{"type":"system","subtype":"turn_duration","durationMs":210452,"timestamp":"2026-01-01T00:00:00Z"}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(
            e,
            ConversationEvent::TurnMeta {
                duration_ms: Some(210452),
                ..
            }
        )));
    }

    #[test]
    fn parse_file_history_snapshot() {
        let line = r#"{"type":"file-history-snapshot","snapshot":{"trackedFileBackups":{"/tmp/a.rs":{"content":"old"}},  "timestamp":"2026-01-01T00:00:00Z"}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::FileChange { path, .. } if path == "/tmp/a.rs"
        )));
    }

    #[test]
    fn parse_attachment_edited_file() {
        let line = r#"{"type":"attachment","timestamp":"2026-01-01T00:00:00Z","attachment":{"type":"edited_text_file","filePath":"/tmp/main.rs"}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::FileChange { path, .. } if path == "/tmp/main.rs"
        )));
    }

    #[test]
    fn parse_attachment_other() {
        let line = r#"{"type":"attachment","timestamp":"2026-01-01T00:00:00Z","attachment":{"type":"deferred_tools_delta","addedNames":["Bash"]}}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::RawEvent { line_type, .. } if line_type == "attachment.deferred_tools_delta"
        )));
    }

    #[test]
    fn unknown_type_emits_raw_event() {
        let line = r#"{"type":"ai-title","title":"test session"}"#;
        let events = parse_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0],
            ConversationEvent::RawEvent { line_type, .. } if line_type == "ai-title"
        ));
    }

    #[test]
    fn malformed_json_returns_empty() {
        assert!(parse_line("not json at all").is_empty());
        assert!(parse_line("").is_empty());
    }

    #[test]
    fn assistant_without_tool_use_returns_turn_meta_only() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"hello"}]}}"#;
        let events = parse_line(line);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ConversationEvent::TurnMeta { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ConversationEvent::ToolUse { .. }))
        );
    }

    #[test]
    fn system_stop_hook_emits_raw() {
        let line = r#"{"type":"system","subtype":"stop_hook_summary","hookCount":1,"timestamp":"2026-01-01T00:00:00Z"}"#;
        let events = parse_line(line);
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::RawEvent { line_type, .. } if line_type == "system.stop_hook_summary"
        )));
    }
}
